//! The application: state, layout, keyboard, and the one place anything changes.
//!
//! # Why everything goes through [`Action`]
//!
//! A pane's breadcrumb can close a tab in another pane; the sidebar navigates
//! whichever pane has focus; dropping a tab restructures the tree the drawing loop
//! is currently walking. None of that can be done with a `&mut` in hand — so the
//! drawing code never mutates structure. It pushes an [`Action`], and
//! [`App::apply`] performs it after the frame is drawn, when nothing is borrowed.
//!
//! That is not ceremony: it is what makes "drag this tab out of the pane it is
//! being drawn from, into a split that does not exist yet" expressible at all.

use std::path::{Path, PathBuf};

use azur_egui_theme as azur;
use egui::{pos2, vec2, Rect, Ui};

use crate::config::Config;
use crate::dock::{self, Splitter};
use crate::fs::places::Place;
use crate::fs::time::LocalZone;
use crate::fs::{self, Column};
use crate::loader::{Loader, Volumes};
use crate::pane::{Pane, PaneId, Side, Tab};
use crate::theme::Theme;
use crate::ui::breadcrumb::{self, CrumbMenu, PathComplete};
use crate::ui::chrome::{self, TabDrag};
use crate::ui::sidebar::{self, Sections};
use crate::ui::{filelist, GUTTER};

mod action;
mod capture;
mod cloud;
mod connect;
mod diff;
mod frame;
mod git;
mod keyboard;
mod load;
mod menu;
mod perform;
mod preview;
mod transfer;

#[cfg(test)]
mod click_tests;
#[cfg(test)]
mod tests;

// One `impl App` per concern, in a file each. The struct and the structure-changing methods stay
// here; everything a frame *does* with it lives next door, and the split follows the banners this
// file used to carry rather than inventing new lines.
pub use action::{Action, WindowAction};
use action::Asking;
use capture::{process_memory, Scrolling, Settling, Shape, Walk};
use git::GIT_WRITE_SETTLE;

/// How many closed tabs are remembered, for `Ctrl+Shift+T`.
///
/// Ten, because the gesture is for undoing a click that went wrong — one or two deep, in the
/// half-minute after it happened. Nobody reaches for it to go archaeology.
const CLOSED_TABS: usize = 10;

/// A drag this window started, and what the window has to know about it while it runs.
///
/// The handle is the OLE drag itself — see [`crate::shell::dnd::Drag`], which is off on a thread of
/// its own. The rest is what the window draws the gesture from while it runs, and none of it can be
/// worked out again later: the **pane** so a move can re-read the folder the files left, the
/// **items** because the drag is drawn at both ends — the ghost under the pointer is these files,
/// and so are the rows marked as the place they came from — and the [`Dragging::ghost`] it is drawn
/// from.
struct Dragging {
    pane: PaneId,
    /// What was picked up, in the order the listing had them.
    items: Vec<PathBuf>,
    /// What the ghost under the pointer is made of: the type of each of the first few items, as
    /// `(extension, is a folder)`.
    ///
    /// Settled when the drag starts rather than looked up while it runs, because `is_dir` is a
    /// syscall and the ghost is redrawn on every frame of the gesture. The *type* rather than the
    /// item, deliberately: [`crate::shell::icons::Icons`] holds one entry per type and a dozen or
    /// so per-path entries for the sidebar's places — asking it per dragged path would put an entry
    /// in that map for every file anybody ever drags.
    ghost: Vec<(String, bool)>,
    drag: crate::shell::dnd::Drag,
}

impl Dragging {
    /// Pick these items up, and settle what the ghost under the pointer is made of.
    fn new(pane: PaneId, items: Vec<PathBuf>, drag: crate::shell::dnd::Drag) -> Self {
        let ghost = items
            .iter()
            .take(crate::ui::GHOST_STACK)
            .map(|path| {
                let ext = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or_default()
                    .to_owned();
                (ext, path.is_dir())
            })
            .collect();
        Self {
            pane,
            items,
            ghost,
            drag,
        }
    }
}

/// What came back from a thread that was pulling a file out of an archive.
///
/// Deliberately not an [`Action`]: the failure arm has nowhere to go but the status line, and
/// widening the action enum with a variant only this can produce would put a case into
/// [`App::perform`]'s match that nothing else could ever reach.
enum Extracted {
    /// On a disk now, at this path. Handed straight back to [`Action::Open`], which is how a
    /// `.zip` inside a `.zip` comes to be browsable: what arrives here is an ordinary file.
    Ready(std::path::PathBuf),
    /// Extracted so they can be copied out — `Ctrl+C` inside an archive. The clipboard is written
    /// when these land, and not before: a `CF_HDROP` naming files that do not exist yet is a paste
    /// that fails in another program.
    Copied(Vec<std::path::PathBuf>),
    /// Extracted so a **drop** can be finished, the same drop carried back with real paths in it.
    /// Handed to [`App::land`] again, which is where the two passes are explained.
    Landed(Box<crate::shell::dnd::Dropped>),
    /// Why not, in words for the status line.
    Failed(String),
}

/// What to do with the entries once they are on a disk.
///
/// Three callers, one worker — see [`App::out_of_archive`]. It is an enum rather than the flag it
/// grew out of because the third arm carries something: a drop has a destination and an effect and a
/// button that was held, and all of it has to survive the extraction to be acted on afterwards.
enum Then {
    /// Open the one file that was asked for.
    Open,
    /// Put them all on the clipboard.
    Clipboard,
    /// Finish the drop they were dragged into.
    ///
    /// Boxed because it is much the largest of the three and this enum is moved into the worker: the
    /// other two arms should not each cost a `Dropped`'s worth of stack.
    Land(Box<crate::shell::dnd::Dropped>),
}

/// Where a request to close the window stands while a fast copy is running. See
/// [`App::mind_the_close`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Closing {
    No,
    /// Asked, and waiting for an answer on the transfer panel.
    Asking,
    /// Answered: close as soon as no copy is running — after they finish, or after they have
    /// cleaned up from being cancelled.
    WhenDone,
}

pub struct App {
    theme: Theme,
    /// Set once per theme change; installing a style every frame would throw away
    /// egui's own caches.
    installed: bool,
    zone: LocalZone,
    loader: Loader,

    layout: dock::Node,
    panes: Vec<Pane>,
    next_pane: PaneId,
    focused: PaneId,
    /// Where the tabs that have been closed were pointing, oldest first, capped at
    /// [`CLOSED_TABS`].
    ///
    /// The path and nothing else. A closed tab's own Back and Forward trail is deliberately not
    /// kept: reopening is "put that folder back", not "restore that tab as it was", and ten
    /// tabs' worth of navigation history is a great deal of state to carry around for a gesture
    /// that exists to undo a misplaced click. A reopened tab therefore starts with the one
    /// place in its history, exactly as a tab opened any other way does.
    ///
    /// Lives for the session. Closing the window closes it with everything in it, and the last
    /// tab of the last pane is never recorded — that close *is* the window closing.
    closed: Vec<PathBuf>,

    drag: Option<TabDrag>,
    /// Events for egui that no platform event will bring — see [`App::release_buttons`].
    injected: Vec<egui::Event>,
    /// Tells us when a folder on screen changes underneath us.
    watch: crate::watch::Watch,
    crumbs: CrumbMenu,
    /// What the path field is offering to complete.
    ///
    /// One for the window rather than one per pane, like `crumbs` above and for the same reason:
    /// only one field can hold the keyboard, so only one of these can be in use. Which pane it
    /// belongs to is on it — see [`PathComplete`].
    complete: PathComplete,
    /// The shell icons, resolved once per file type and held for the session.
    icons: crate::shell::icons::Icons,
    /// The shell's thumbnails, for the tiles in the large-icon view. See [`crate::shell::thumbs`].
    thumbs: crate::shell::thumbs::Thumbs,
    /// What each shortcut row points at, asked once per row per view.
    links: crate::shell::links::Links,
    /// Counts what is inside each folder on show, for the panes whose measure button is on.
    ///
    /// The totals live on the tabs — [`crate::pane::Tab::deep_size`] — and die with the listing they
    /// describe. Nothing here is keyed by path, for the reason [`crate::git`] gives about a status
    /// cache: a byte count is only true of the moment it was taken. See [`crate::sizes`].
    sizes: crate::sizes::Sizes,
    /// The measurements handed to it last frame, so the set is only handed over when it changes.
    ///
    /// See [`App::collect_sizes`]: telling the service the same thing sixty times a second means
    /// taking the lock its workers need in order to make any progress at all.
    measuring: Vec<u64>,
    /// Asks git about the folder each pane is showing, once per view of it.
    ///
    /// The answers live on the tabs — [`Tab::git`] — and die with them. There is no map keyed by
    /// path here on purpose; see [`crate::git`] for why a status cache is the thing to avoid.
    git: crate::git::Git,
    /// Questions asked of git and not yet answered.
    ///
    /// Only [`App::git_pending`] reads it, and only a capture run asks that — but it has to be
    /// counted here rather than inferred from the tabs, because "answered with nothing" and "not
    /// answered yet" leave a tab looking identical, which is exactly the state a capture must not
    /// wait forever in.
    git_waiting: usize,
    /// Asks the sync provider about the entries of a synced folder, once per view of it. The
    /// answers live on the tabs — [`Tab::cloud`] — like git's. See [`crate::shell::cloud`].
    cloud: crate::shell::cloud::Cloud,
    /// Where every pane's preview panel goes and how much room it takes: the window's
    /// preference, one of it. Whether one is *showing* is the tab's — see `Tab::preview`.
    preview: crate::ui::preview::Layout,
    /// The view each unrecognised extension turned out to preview as, once somebody picked one that
    /// worked. For as long as the window is open and deliberately no longer — see
    /// [`crate::ui::preview::Remembered`], and not in [`Self::settings`].
    preview_as: crate::ui::preview::Remembered,
    /// How much of a pane the console panel takes, as a fraction. The window's preference for the
    /// same reason the preview's share is: one number, whichever pane you drag it in.
    console_share: f32,
    /// The shell a console opens on: whichever one was last chosen, in any pane.
    console_shell: crate::console::Kind,
    /// Which way the flatten button shows a tree: as one list, or as the hierarchy.
    ///
    /// The window's preference for the same reason the preview's position is — which of the two
    /// you want is a habit, not a fact about the folder you are in — and the tabs each keep the
    /// mode their order is currently built in. See [`crate::pane::FlatMode`].
    flat_mode: crate::pane::FlatMode,
    /// Which rows a folder diff opens showing: the last choice made on any diff's button. The
    /// window's preference, kept like [`App::flat_mode`]; each diff's halves keep the copy they are
    /// filtered by. See [`crate::diff::Show`].
    diff_show: crate::diff::Show,
    /// Whether a tree merges a chain of folders with nothing in them but each other into one row.
    ///
    /// The window's preference beside [`App::flat_mode`], kept the same way and for the same reason.
    /// See [`crate::fs::sort::build_tree_order`], which is what it does.
    regroup: bool,
    /// Whether every listing shows the files Windows marks hidden. `Ctrl+H`.
    ///
    /// The window's preference beside [`App::regroup`], kept the same way — and **one answer for the
    /// window rather than one per pane**, which is the change that let it into the settings file at
    /// all: two panes free to disagree have no single state for one line to remember, and Explorer's
    /// own Hidden items box is one box for every window it opens. The tabs each keep the copy their
    /// order is built from; see [`crate::pane::Tab::show_hidden`].
    show_hidden: bool,
    /// Which column a tab opens sorted by, and which way: the last header clicked, in any pane.
    ///
    /// Beside [`App::show_hidden`] and handed to [`Tab::showing`] with it, since the first order a
    /// tab builds is built with it. Unlike that one, a change is *not* pushed into the tabs already
    /// open: sorting one pane by size is not a request to re-sort the other. See
    /// [`crate::fs::Sort`].
    sort: crate::fs::Sort,
    /// Whether the path field writes `/` between the parts of a path rather than `\`.
    ///
    /// The window's preference again — which slash you want is a habit, and it is about where the
    /// path is going after it leaves here rather than about the folder in front of you. Ticked in
    /// the field's own context menu. See [`crate::config::Config::forward_slashes`].
    ///
    /// Read by [`Action::CopyPaths`] as well as by the field, since a path being copied out is that
    /// "somewhere else" more literally than the bar is.
    forward_slashes: bool,
    /// Whether a folder that is opened becomes tiles on its own, and at what share of pictures.
    ///
    /// The window's preference again, and read in exactly one place — [`crate::pane::Tab::choose_view`],
    /// on the frame a folder's listing lands. Nothing else consults it, which is what keeps "only when
    /// opening a folder" a property of the code rather than of a comment. See
    /// [`crate::pane::AutoTiles`].
    auto_tiles: crate::pane::AutoTiles,
    /// Which context-menu entries the user has moved between a collapsed group and the main menu.
    ///
    /// A mirrored setting like the four above rather than something read off [`App::config`], and it
    /// has to be: `config` is *what this window last wrote* and [`App::settings`] rebuilds the file
    /// from these fields, so a preference that lived only in `config` would be compared against a
    /// freshly built `Config` that did not have it — and the save that followed would write it back
    /// out empty. See [`App::save_settings`].
    ///
    /// Read in one place, [`crate::shell::menu::regroup`], through
    /// `crate::ui::menu::Open::arrange`.
    menu_moves: crate::shell::menu::Moves,
    /// Which file types this machine can draw a picture of, asked once per type and kept for the
    /// session. Only read when [`App::auto_tiles`]' probe is on — see [`crate::shell::providers`].
    providers: crate::shell::providers::Providers,
    /// Reads whatever the preview panels are pointed at, off the UI thread.
    previews: crate::preview::Previews,
    /// The window the shell parents its own dialogs to.
    owner: crate::shell::Owner,
    /// Sign-ins to shares that would not open, and which paths have already been asked about.
    /// See [`connect`].
    connecting: connect::Connecting,
    /// Shell file operations in flight.
    ops: crate::shell::ops::Operations,
    /// Whether the window was asked to close while a fast copy was running. See
    /// [`App::mind_the_close`].
    closing: Closing,
    /// The ones that have finished, so Ctrl+Z can take them back.
    ///
    /// Lives for the session and is not written to the settings file — see
    /// [`crate::shell::ops::history`], which sets out both what is undoable and why none of it
    /// outlives the window.
    history: crate::shell::ops::history::History,
    /// Items cut but not yet pasted, shown ghosted the way Explorer shows them.
    cut: Vec<PathBuf>,
    /// The last thing that went wrong, for the status line.
    notice: Option<String>,
    /// How far an archive extraction has got, in words — empty when none is running.
    ///
    /// Rebuilt from [`crate::archive::extract::doing`] once a frame rather than pushed from the
    /// worker, because the worker counts in 8 KB blocks: a message per block would be thousands of
    /// wake-ups for a readout that can only change sixty times a second. The buffer is kept and
    /// rewritten for the reason [`crate::fs::fmt`] gives about the listing's own figures.
    extracting: String,
    /// Where a drag from outside is hovering, in points, for the pane highlight.
    drop_hover: Option<(i32, i32)>,
    /// What a drop where it is hovering would do, in words — see
    /// [`crate::shell::dnd::Shared::telling`], which is where the sentence is decided, and
    /// [`crate::shell::dnd::Told`] for why it arrives in pieces rather than as a string.
    drop_telling: Option<crate::shell::dnd::Told>,
    /// Whether that sentence is one to keep quiet about — see
    /// [`crate::shell::dnd::Shared::silent`], where the one case is set out. Mirrored beside
    /// [`App::drop_telling`] because the two are decided together and have to be drawn together.
    drop_silent: bool,
    /// Whether the clipboard is offering files. Sampled once a frame rather than once
    /// per menu entry, since it is a syscall.
    clipboard_has_files: bool,
    /// Files being dragged in from elsewhere, and the drops that land.
    drops: crate::shell::dnd::Zone,
    /// The two shell calls that take over the pointer, run off the UI thread.
    modal: crate::shell::Modal,
    /// The context menu, while one is open.
    menu: Option<crate::ui::menu::Open>,
    /// A context menu that has been asked for and is not on screen yet.
    asking: Option<Asking>,
    /// Builds the shell's half of the context menu, off this thread — half a second of
    /// `QueryContextMenu` on a file, which used to be half a second of frozen window.
    menu_builder: crate::shell::menu::Builder,
    /// Compares the two halves of every folder diff, off this thread. See [`crate::diff`].
    differ: crate::diff::Differ,
    /// Where the sidebar's Bookmarks group was drawn, so a drag can be dropped on it.
    /// Read a frame later than it is written, which is a frame the sidebar has not moved in.
    bookmarks_rect: Option<Rect>,
    /// Where each bookmark group's row was drawn, and which group it is, so a folder dragged in
    /// can land in the group it is dropped on rather than at the end of the list.
    ///
    /// The same arrangement a listing's folder rows have — see [`crate::pane::Pane::drop_rows`]:
    /// a zone of its own, published after the section it sits in so that it wins.
    bookmark_rows: Vec<(Rect, usize)>,
    /// The bookmark list mid-gesture: a row being dragged, and a group being named.
    bookmark_edit: crate::ui::sidebar::Editing,
    /// The OLE drag in flight, if there is one. One at a time: the pointer is only holding one
    /// thing, and a second drag would be following the same button as the first.
    file_drag: Option<Dragging>,

    volumes: Volumes,
    places: Vec<Place>,
    bookmarks: crate::ui::sidebar::Bookmarks,
    sections: Sections,
    sidebar_width: f32,
    /// Whether the panel down the left is on screen at all.
    ///
    /// The window's preference — there is one panel, so there is one answer — and remembered, like
    /// the width beside it. See [`Action::ToggleSidebar`]. Its own flag rather than a width of zero
    /// because the two are different questions: hiding the panel and bringing it back has to find the
    /// width it was dragged to, which a zero would have thrown away.
    sidebar_shown: bool,

    maximized: bool,
    /// What `Win+E` opens, for the tick in the application menu.
    ///
    /// Not a setting of this program's in the way the rest of that menu is — the answer lives in
    /// the registry, and this is a copy kept only so a menu that redraws every frame is not asking
    /// the registry sixty times a second. Read once at startup and **re-read after each toggle
    /// rather than assumed**, because a refused write has to leave the tick where it was: see
    /// [`crate::shell::winkey::set`], and [`crate::shell::winkey`] on why there is no copy of this
    /// in the settings file.
    win_key: crate::shell::winkey::State,
    /// Whose video is filling the screen, if one is.
    ///
    /// **The one state in this program that suppresses the rest of the window.** While it is set the
    /// frame draws the player and nothing else — no panes, no sidebar, no title bar — and the
    /// keyboard's shortcuts stand down but for the one that gets out. See [`App::theatre`], which is
    /// also where every way of leaving is listed.
    ///
    /// Not in the settings file. Where the preview panel goes is a habit worth keeping; a video
    /// filling the screen is a thing you were doing a minute ago.
    fullscreen_video: Option<PaneId>,
    /// The window's inner size, tracked so it can be restored next launch.
    window_size: Option<[f32; 2]>,
    /// And where it is: the outer top-left corner in *physical pixels*. See
    /// [`crate::config::Config::position`] for why that unit and not points.
    window_position: Option<[f32; 2]>,
    /// Collected during drawing, drained after.
    actions: Vec<Action>,
    /// Files pulled out of an archive, arriving from the thread that pulled them.
    ///
    /// Opening a file inside a `.zip` is the one gesture in this program that needs real work done
    /// before it can be acted on — the bytes have to be on a disk before anything can open them,
    /// and for a solid `.7z` that means decompressing several entries. Doing it where the click
    /// happens would freeze the window for as long as it took, so it happens on a thread and the
    /// answer comes back here. See [`Self::open_from_archive`].
    extracted: std::sync::mpsc::Receiver<Extracted>,
    /// The other end, cloned into each of those threads.
    extractions: std::sync::mpsc::Sender<Extracted>,
    /// One buffer every formatted cell in the window is written through.
    scratch: String,
    /// Where each pane was drawn, for the docking gesture and for keyboard focus.
    pane_rects: Vec<(PaneId, Rect)>,
    /// Every tab drawn this frame, wherever its strip was. The drag resolution needs all
    /// of them at once, and they are not all known until the panes have been drawn.
    ///
    /// A drag of *files* reads them too, to bring the tab under the pointer to the front — see
    /// [`App::reveal_hovered_tab`].
    tab_slots: Vec<chrome::Slot>,
    /// And each pane's strip, whole: the tabs, the `+`, and the room left over.
    ///
    /// The zone a folder is dropped on to be opened in a tab — see [`crate::shell::dnd::Onto::Tabs`]
    /// — so it is the strip and not the tabs: the empty end of a strip is the part of it that is
    /// obviously not a tab, and it has to take the drop as much as the tabs do. Read a frame late,
    /// like every other published zone: see [`App::publish_drop_targets`].
    tab_strips: Vec<(PaneId, Rect)>,
    splitters: Vec<Splitter>,
    pane_order: Vec<PaneId>,

    /// Settings worth writing back, and whether they have changed.
    config: Config,
    config_dirty: bool,
    /// When the settings were last written, so a gesture that marks them dirty on every frame
    /// costs one write rather than one per frame. See [`Self::apply`].
    config_saved_at: f64,
    /// When the cut list was last checked against the disk, for the same reason: `exists()` is a
    /// syscall per item. See [`Self::collect_operations`].
    cut_checked_at: f64,

    /// Keeps frames coming while the window is being resized, rescaled or restored.
    settling: Settling,
    /// `--walk=<dir>`: browse by itself, for measuring a real session's memory.
    walk: Option<Walk>,
    /// `--scroll`: run the listing up and down, for the same reason.
    scrolling: Scrolling,

    /// Names of the actions performed, when something is watching.
    ///
    /// Off unless a test turns it on. Click targets are the one part of this program
    /// that cannot be checked by reading — an interaction rect covered by another one
    /// looks perfectly correct in the source and simply does not respond — so the
    /// tests drive real pointer events through real frames, and this is how they see
    /// what landed.
    journal: Option<Vec<&'static str>>,
}

/// Ask the loader for `tab`'s listing, the right way round for the view the tab is in.
///
/// **The one place that choice is made**, and it is a function rather than two branches because
/// getting it wrong is silent. A flattened tab asked with [`Loader::request`] is answered with the
/// folder's own children, and `Tab::apply` puts them on screen without complaint: the listing goes
/// back to one folder deep while [`Tab::flat`] stays set and the button stays lit. Nothing looks
/// broken. The view has simply switched itself off.
///
/// Which is what [`App::folder_changed`] did, and it read as the flatten "sometimes resetting on
/// its own" precisely because the trigger is invisible: [`crate::watch`] re-reads a folder when it
/// changes on disk, so saving a file into a flattened folder — or a build touching one — undid the
/// flatten from underneath. Non-recursively, too, so a change deeper in the tree did nothing and
/// only the root folder's own files could do it. `F5` had been fixed for this once already
/// (`Tab::refresh` drops the listing and lets [`App::start_scans`] re-ask, which does branch); the
/// watcher's path was the one that had not.
fn ask_for(tab: &Tab, loader: &mut Loader) -> u64 {
    if tab.flat {
        loader.request_deep(&tab.path)
    } else {
        loader.request(&tab.path)
    }
}

/// One pane as the settings file remembers it: where its tabs point, and which was in front.
///
/// **Less its folder diffs.** A diff is two folders and a question, and a settings line is one path:
/// written down, it would come back as an ordinary tab on its left half, which is not what was open.
/// A pane of nothing but diffs keeps the first one's left folder, because a pane has to have a tab.
fn pane_tabs(pane: &Pane) -> crate::config::PaneTabs {
    let kept: Vec<usize> = (0..pane.tabs.len())
        .filter(|&at| pane.tabs[at].diff.is_none())
        .collect();
    if kept.is_empty() {
        return crate::config::PaneTabs {
            paths: vec![pane.tab().path.clone()],
            active: 0,
        };
    }
    let active = pane.active.min(pane.tabs.len().saturating_sub(1));
    crate::config::PaneTabs {
        paths: kept.iter().map(|&at| pane.tabs[at].path.clone()).collect(),
        // The tab in front if it is kept, and otherwise the one that was beside it.
        active: kept
            .iter()
            .position(|&at| at >= active)
            .unwrap_or(kept.len() - 1),
    }
}

impl App {
    /// `open` is what the command line asked for: one pane per path, so
    /// `yafe --open=A --open=B` comes up side by side. Empty means restore the
    /// remembered tabs instead.
    /// `side` is where each extra `--open=` path lands.
    ///
    /// Only the command line passes anything but `Right`: `--stack` is how a capture run
    /// gets a stacked layout on screen, which is the one arrangement whose tab strips do
    /// not live in the title bar, and so the one a screenshot has to be able to show.
    pub fn opening(ctx: &egui::Context, config: Config, open: Vec<PathBuf>, side: Side) -> Self {
        let theme = Theme::of(config.palette);
        let loader = Loader::new(ctx);
        // Letters now, labels and free space in the background. Asking for a volume
        // label on the startup path is what makes a file manager take twenty seconds
        // to open on a machine with a mapped share.
        let volumes = Volumes::new(ctx);
        // The known folders are a handful of `SHGetKnownFolderPath` calls -- single
        // digit milliseconds, and they do not change while the window is open.
        let places = fs::places::standard();

        let first = 1u32;
        let mut panes = Vec::new();
        let mut layout = dock::Node::Leaf(first);
        let mut next_pane = 2;
        let mut focused = first;
        // Every tab below is made showing what the settings file says the window shows, because the
        // first listing to land is ordered with it and there is no frame in which to correct it. See
        // [`Tab::showing`], which is why these are the two preferences threaded through here.
        let (hidden, sort) = (config.show_hidden, config.sort);

        if open.is_empty() {
            // The panes that were open last time, and the tree that arranged them. Both, or
            // neither: a layout is a tree over exactly these panes, so if it does not read
            // back as one — a file from before it was written, or one edited into something
            // else — every tab goes into a single pane, which is what this used to do always.
            let groups: Vec<&crate::config::PaneTabs> = config
                .panes
                .iter()
                // A pane with no tabs is not a pane. Dropping them here is also what keeps
                // the numbering honest: `decode` insists on one leaf per pane, so a dropped
                // one is caught as a count that no longer matches rather than shifting every
                // pane along by one.
                .filter(|group| !group.paths.is_empty())
                .collect();
            let ids: Vec<PaneId> = (0..groups.len()).map(|i| first + i as u32).collect();
            let tree = config
                .layout
                .as_deref()
                .filter(|_| groups.len() > 1)
                .and_then(|text| dock::Node::decode(text, &ids));

            match tree {
                Some(tree) => {
                    for (group, &id) in groups.iter().zip(&ids) {
                        let mut tabs = group
                            .paths
                            .iter()
                            .cloned()
                            .map(|path| Tab::showing(path, hidden, sort));
                        let mut pane =
                            Pane::new(id, tabs.next().expect("a group with no tabs was filtered"));
                        pane.tabs.extend(tabs);
                        pane.active = group.active.min(pane.tabs.len() - 1);
                        panes.push(pane);
                    }
                    next_pane = first + ids.len() as u32;
                    focused = ids.get(config.focus).copied().unwrap_or(first);
                    layout = tree;
                }
                None => {
                    let mut tabs: Vec<Tab> = groups
                        .iter()
                        .flat_map(|group| group.paths.iter())
                        .cloned()
                        .map(|path| Tab::showing(path, hidden, sort))
                        .collect();
                    if tabs.is_empty() {
                        tabs.push(Tab::showing(fs::places::default_start(), hidden, sort));
                    }
                    let mut pane = Pane::new(first, tabs.remove(0));
                    pane.tabs.extend(tabs);
                    panes.push(pane);
                }
            }
        } else {
            let mut paths = open.into_iter();
            panes.push(Pane::new(
                first,
                Tab::showing(paths.next().unwrap_or_default(), hidden, sort),
            ));
            for path in paths {
                let id = next_pane;
                next_pane += 1;
                panes.push(Pane::new(id, Tab::showing(path, hidden, sort)));
                layout.split(id - 1, side, id);
            }
        }

        // Both ends kept: the sender is cloned into each extraction thread, and holding one here
        // means the receiver never sees the channel close just because no extraction is running.
        let (extractions, extracted) = std::sync::mpsc::channel();

        Self {
            theme,
            installed: false,
            zone: LocalZone::current(),
            loader,
            layout,
            panes,
            next_pane,
            focused,
            closed: Vec::new(),
            drag: None,
            injected: Vec::new(),
            watch: crate::watch::Watch::new(ctx),
            crumbs: CrumbMenu::default(),
            complete: PathComplete::default(),
            icons: crate::shell::icons::Icons::new(ctx),
            thumbs: crate::shell::thumbs::Thumbs::new(ctx),
            links: crate::shell::links::Links::new(ctx),
            sizes: crate::sizes::Sizes::new(ctx),
            measuring: Vec::new(),
            git: crate::git::Git::new(ctx),
            git_waiting: 0,
            cloud: crate::shell::cloud::Cloud::new(ctx),
            previews: crate::preview::Previews::new(ctx),
            owner: crate::shell::Owner::default(),
            connecting: connect::Connecting::default(),
            ops: {
                let mut ops = crate::shell::ops::Operations::new();
                ops.set_fast(config.fast_copy);
                ops
            },
            closing: Closing::No,
            history: crate::shell::ops::history::History::default(),
            cut: Vec::new(),
            notice: None,
            extracting: String::new(),
            drop_hover: None,
            drop_telling: None,
            drop_silent: false,
            clipboard_has_files: false,
            drops: crate::shell::dnd::Zone::new(),
            modal: crate::shell::Modal::new(ctx),
            menu: None,
            asking: None,
            menu_builder: crate::shell::menu::Builder::new(ctx),
            differ: crate::diff::Differ::new(ctx),
            bookmarks_rect: None,
            bookmark_rows: Vec::new(),
            bookmark_edit: crate::ui::sidebar::Editing::default(),
            file_drag: None,
            volumes,
            places,
            bookmarks: config.bookmarks.clone(),
            sections: config.sections,
            sidebar_width: config.sidebar_width,
            sidebar_shown: config.sidebar_shown,
            preview: config.preview,
            preview_as: Default::default(),
            console_share: config.console_share,
            console_shell: config.console_shell,
            flat_mode: config.flat_mode,
            diff_show: config.diff_show,
            regroup: config.regroup,
            show_hidden: config.show_hidden,
            sort: config.sort,
            forward_slashes: config.forward_slashes,
            auto_tiles: config.auto_tiles,
            menu_moves: config.menu_moves.clone(),
            providers: crate::shell::providers::Providers::new(),
            // The window as the settings file describes it, and not as this program is about to
            // find it: a maximised window never writes the other two — see `Self::frame` — so
            // starting them empty threw away the size and the place the window would go back to
            // the moment a session was left maximised, and the next launch had nothing to restore
            // to but the default. What the platform actually did with them is
            // `main::open_maximized`'s business; this is only what is remembered.
            maximized: config.maximized,
            // `heal` and not `state`: a registration this program made and then broke — the build it
            // named having been cleaned, moved or deployed elsewhere — leaves `Win+E` putting up
            // *Application not found*, and the switch that would turn it off is in the executable
            // that went missing. Startup is the one moment a running copy can put that right. See
            // `crate::shell::winkey`, which is emphatic about how narrow the repair is.
            win_key: crate::shell::winkey::heal(),
            fullscreen_video: None,
            window_size: config.window,
            window_position: config.position,
            actions: Vec::new(),
            extracted,
            extractions,
            scratch: String::with_capacity(64),
            pane_rects: Vec::new(),
            tab_slots: Vec::new(),
            tab_strips: Vec::new(),
            splitters: Vec::new(),
            pane_order: vec![first],
            config,
            config_dirty: false,
            config_saved_at: f64::NEG_INFINITY,
            cut_checked_at: f64::NEG_INFINITY,
            settling: Settling::default(),
            walk: None,
            scrolling: Scrolling::default(),
            journal: None,
        }
    }

    /// Tell the shell which window to parent its dialogs to.
    ///
    /// Set once, from eframe, which is the only thing that knows the handle. Without
    /// it a progress or conflict dialog appears behind this window and looks hung.
    pub fn set_owner(&mut self, owner: crate::shell::Owner) {
        self.owner = owner;
    }

    /// The colour eframe clears the window to, so a resize does not flash.
    pub fn clear_color(&self) -> [f32; 4] {
        self.theme.bg.canvas.to_normalized_gamma_f32()
    }

    /// Write the settings out, but only if *this window* has changed any of them.
    ///
    /// **Because a second window is not a second copy of the settings.** They are written whole, so
    /// a save is this window's entire idea of them replacing whatever is on disk — including the
    /// parts another window put there. Open one window in the morning, add a bookmark in a second
    /// one during the day, and closing the first wrote its morning state back over the new bookmark:
    /// the exit save ran unconditionally, whether or not the window closing had touched anything.
    /// One `.bak` deep is not a fix for that.
    ///
    /// So the test is what this window last persisted, held on [`Self::config`] — set when the file
    /// was read at startup and again on every save. Comparing the *text* rather than the fields
    /// covers every setting at once, including the ones that reach the file through an encoding of
    /// their own, and cannot be forgotten by a field added later. A window that has genuinely
    /// changed something still writes all of it; this makes the untouched window stop overwriting.
    ///
    /// It does not make two windows *merge* — the one that changed something still wins for
    /// everything. That needs re-reading the file and reconciling it, which is a bigger decision
    /// than a guard.
    pub fn save_settings(&mut self) {
        let settings = self.settings();
        // Nothing this window would write differs from what it already wrote. Which also quietly
        // removes the no-op saves a gesture can ask for — a splitter dragged back to where it
        // started marks the settings dirty and has changed nothing.
        if settings.to_text() == self.config.to_text() {
            return;
        }
        self.config = settings;
        self.config.save();
    }

    /// Settings to write back on the way out.
    pub fn settings(&self) -> Config {
        let mut config = Config {
            bookmarks: self.bookmarks.clone(),
            sidebar_width: self.sidebar_width,
            sidebar_shown: self.sidebar_shown,
            preview: self.preview,
            console_share: self.console_share,
            console_shell: self.console_shell,
            flat_mode: self.flat_mode,
            diff_show: self.diff_show,
            regroup: self.regroup,
            show_hidden: self.show_hidden,
            sort: self.sort,
            forward_slashes: self.forward_slashes,
            fast_copy: self.ops.fast(),
            auto_tiles: self.auto_tiles,
            menu_moves: self.menu_moves.clone(),
            sections: self.sections,
            panes: Vec::new(),
            layout: None,
            focus: 0,
            window: self.window_size,
            position: self.window_position,
            maximized: self.maximized,
            palette: self.theme.palette,
        };

        // In layout order, because that is the order [`dock::Node::encode`] numbers the panes
        // in and the two have to agree — the `pane` lines are what the `layout` line's numbers
        // point at.
        //
        // Nothing is deduplicated. It used to be, because every tab was going into one pane
        // and the same folder twice would have been two tabs on it; now the same folder open
        // in two panes is a thing somebody arranged deliberately, and dropping the second copy
        // would take a pane's only tab away from it.
        let mut order = Vec::new();
        self.layout.panes(&mut order);
        for (slot, id) in order.iter().enumerate() {
            let Some(pane) = self.panes.iter().find(|p| p.id == *id) else {
                // A leaf with no pane behind it cannot happen, and if it did the layout would
                // no longer describe what follows it — so the tree is dropped rather than
                // written wrong. The tabs are still all here.
                config.layout = None;
                config.panes.clear();
                for pane in &self.panes {
                    config.panes.push(pane_tabs(pane));
                }
                return config;
            };
            if *id == self.outer(self.focused) {
                config.focus = slot;
            }
            config.panes.push(pane_tabs(pane));
        }
        config.layout = Some(self.layout.encode());
        config
    }

    // ---------------------------------------------------------------------
    // The frame
    // ---------------------------------------------------------------------

    // ---------------------------------------------------------------------
    // Loading
    // ---------------------------------------------------------------------

    // ---------------------------------------------------------------------
    // Git
    // ---------------------------------------------------------------------

    // ---------------------------------------------------------------------
    // Previews
    // ---------------------------------------------------------------------

    /// Say something in the status line: the one place this window has to tell the user
    /// anything, and where a failed file operation already goes.
    ///
    /// Used for the graphics device, which is not this program's to fix but very much its job
    /// to admit to: a window that cannot present a frame goes on taking input and showing the
    /// last thing it drew, and without a word from it that is indistinguishable from a hang.
    pub fn report(&mut self, what: String) {
        self.notice = Some(what);
    }

    /// Put a file that is inside an archive onto a disk, and open it when it is there.
    ///
    /// The answer comes back as [`Extracted::Ready`] and is handed to [`Action::Open`] *again*, on
    /// the next frame. That second pass is what makes the feature compose rather than special-case:
    /// the extracted path is an ordinary file, so a document opens with its application and a
    /// **nested archive is browsable**, because by then it is a real archive on a real disk. See
    /// [`crate::archive::split`], which explains why nesting could not be done any other way.
    fn open_from_archive(&mut self, ctx: &egui::Context, path: std::path::PathBuf) {
        self.out_of_archive(ctx, vec![path], Then::Open);
    }

    /// Copy files out of an archive onto the clipboard — `Ctrl+C` on a selection inside one.
    ///
    /// The only way out of an archive that this program offers, and enough of one: what lands on the
    /// clipboard is real files, so pasting them works in Explorer, in a save dialog, in another copy
    /// of this program, and in anything else that takes a `CF_HDROP`.
    fn copy_out_of_archive(&mut self, ctx: &egui::Context, paths: Vec<std::path::PathBuf>) {
        self.report("Copying out of the archive…".to_owned());
        self.out_of_archive(ctx, paths, Then::Clipboard);
    }

    /// Put files that are inside an archive onto a disk, and then do [`Then`] with them.
    ///
    /// **The one worker every route out of an archive goes through** — opening, `Ctrl+C`, and a drop
    /// — which is what keeps the decompression off the UI thread in all three. Each has its own way
    /// of picking the work back up; see the three arms of [`Then`], and [`App::land`] for the one
    /// whose answer re-enters where it left.
    ///
    /// Detached, like every other worker in this program: there is nothing useful to do with a
    /// half-extracted file when the window has closed, and joining would hold the process open for
    /// as long as the decompression had left to run — the argument [`crate::loader::Loader`]'s
    /// `Drop` sets out at length.
    fn out_of_archive(&mut self, ctx: &egui::Context, paths: Vec<std::path::PathBuf>, then: Then) {
        let answers = self.extractions.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("extract".to_owned())
            .spawn(move || {
                crate::fs::scan::silence_device_dialogs();
                // **One call for the whole selection**, not one per file: a solid `.7z` is
                // decompressed from the start of its block for each separate read, so asking file by
                // file turns a copy of 300 entries into 300 walks of the archive. See
                // [`crate::archive::extract::all`].
                //
                // It also fails as a unit, which is what a clipboard needs: half a selection on it
                // would be a paste that quietly loses files.
                let answer = match crate::archive::extract::all(&paths) {
                    Ok(done) => match then {
                        Then::Clipboard => Extracted::Copied(done),
                        Then::Open => match done.into_iter().next() {
                            Some(one) => Extracted::Ready(one),
                            None => Extracted::Failed("Nothing to open".to_owned()),
                        },
                        // The same drop, now naming files that exist. **In the order asked**, which
                        // is what [`crate::archive::extract::all`] guarantees and what lets the list
                        // be swapped whole: a drop is not positional but a mixed selection would
                        // otherwise have its real paths shuffled among its extracted ones.
                        Then::Land(mut dropped) => {
                            dropped.items = done;
                            Extracted::Landed(dropped)
                        }
                    },
                    Err(why) => Extracted::Failed(why),
                };
                if answers.send(answer).is_ok() {
                    ctx.request_repaint();
                }
            });
        // A machine that will not give us a thread means the double click did nothing, which the
        // next one can try again. Nothing else in the window is affected, and inventing a second
        // failure path for a case nobody has seen would be worse than this line.
        if spawned.is_err() {
            self.report("Could not start reading this archive".to_owned());
        }
    }

    // ---------------------------------------------------------------------
    // Keyboard
    // ---------------------------------------------------------------------

    // ---------------------------------------------------------------------
    // Applying
    // ---------------------------------------------------------------------

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    fn spawn_pane(&mut self, tab: Tab) -> PaneId {
        let id = self.next_pane;
        self.next_pane += 1;
        self.panes.push(Pane::new(id, tab));
        id
    }

    /// Take a tab out of a pane, dropping the pane if that was its last.
    fn take_tab(&mut self, pane: PaneId, index: usize) -> Option<Tab> {
        let position = self.panes.iter().position(|p| p.id == pane)?;
        if index >= self.panes[position].tabs.len() {
            return None;
        }
        let tab = self.panes[position].tabs.remove(index);
        let p = &mut self.panes[position];
        if p.tabs.is_empty() {
            // Only close the pane if the tree can absorb it; a lone root pane has
            // nowhere to collapse into, and is refilled by the caller.
            if self.layout.remove(pane) {
                self.panes.remove(position);
                if self.focused == pane {
                    self.focused = self.panes.first().map(|p| p.id).unwrap_or(pane);
                }
            }
        } else if p.active >= p.tabs.len() {
            p.show_tab(p.tabs.len() - 1);
        }
        Some(tab)
    }

    fn move_tab(&mut self, from: PaneId, index: usize, to: PaneId, at: usize) {
        if from == to {
            // A reorder inside one strip. The insertion point was measured with the
            // tab still in place, so removing it shifts everything after it.
            let Some(p) = self.pane_mut(from) else { return };
            if index >= p.tabs.len() {
                return;
            }
            let target = at.min(p.tabs.len());
            let tab = p.tabs.remove(index);
            let target = if target > index { target - 1 } else { target };
            let target = target.min(p.tabs.len());
            p.tabs.insert(target, tab);
            p.show_tab(target);
            self.focused = from;
            self.config_dirty = true;
            return;
        }

        let Some(tab) = self.take_tab(from, index) else {
            return;
        };
        let Some(p) = self.pane_mut(to) else {
            // The destination went away; keep the tab by putting it back somewhere.
            let anchor = self.focused;
            if let Some(p) = self.pane_mut(anchor) {
                p.tabs.push(tab);
                p.show_tab(p.tabs.len() - 1);
            }
            return;
        };
        let target = at.min(p.tabs.len());
        p.tabs.insert(target, tab);
        p.show_tab(target);
        self.focused = to;
        self.config_dirty = true;
    }

    fn close_tab(&mut self, ctx: &egui::Context, pane: PaneId, index: usize) {
        let Some(position) = self.panes.iter().position(|p| p.id == pane) else {
            return;
        };
        // Counted in the layout: a diff's right half is a pane, but not one the window is made of.
        let last_pane = self.panes.iter().filter(|p| !p.twin).count() == 1;
        let last_tab = self.panes[position].tabs.len() == 1;

        if last_tab && last_pane {
            // The last tab of the last pane: the window is what is being closed.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        // Remembered before it goes, so `Ctrl+Shift+T` has something to put back. Guarded by
        // `get` for the same reason `Pane::close_tab` guards: an index past the end closes
        // nothing, and a history of tabs that were never closed would hand back folders that
        // are still open.
        // Not a diff, which `Ctrl+Shift+T` could only put back as half of itself.
        if let Some(path) = self.panes[position]
            .tabs
            .get(index)
            .filter(|t| t.diff.is_none())
            .map(|t| t.path.clone())
        {
            self.closed.push(path);
            if self.closed.len() > CLOSED_TABS {
                self.closed.remove(0);
            }
        }
        if !self.panes[position].close_tab(index) {
            // The pane is empty now, so it goes too.
            if self.layout.remove(pane) {
                self.panes.remove(position);
                if self.focused == pane {
                    self.focused = self.panes.first().map(|p| p.id).unwrap_or(pane);
                }
            }
        }
        self.config_dirty = true;
    }

    /// Whether a path is bookmarked, wherever in the list it sits — a group is somewhere a
    /// bookmark can be, so `Ctrl+D` on a folder pinned inside one has to find it there.
    pub fn is_bookmarked(&self, path: &Path) -> bool {
        self.bookmarks.contains(path)
    }
}
