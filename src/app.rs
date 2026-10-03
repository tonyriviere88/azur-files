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

/// What the title bar asks of the platform.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowAction {
    Minimize,
    ToggleMaximize,
    /// Back to [`crate::config::WINDOW_SIZE`] — the way out of a window dragged to a shape you
    /// did not mean, and the counterpart of double-clicking the sidebar splitter.
    ResetSize,
    Close,
    Drag,
}

/// Everything the interface can ask for. Performed after the frame, never during.
pub enum Action {
    Focus(PaneId),

    // Tabs
    ActivateTab { pane: PaneId, tab: usize },
    NewTab { pane: PaneId },
    /// The same, for the global menu, which has no pane in hand.
    NewTabFocused,
    /// Split whichever pane has focus, showing the folder it is already on.
    SplitFocused { side: Side },
    CloseTab { pane: PaneId, tab: usize },
    /// Put the last closed tab back, in whichever pane has focus. `Ctrl+Shift+T`.
    ReopenTab,
    NextTab { pane: PaneId, delta: isize },
    BeginTabDrag { pane: PaneId, tab: usize, grab_dx: f32 },
    /// Move a tab into another pane's strip. `index == usize::MAX` means append.
    MoveTab { from: PaneId, tab: usize, to: PaneId, index: usize },
    /// Pull a tab out into a new pane beside `target`.
    SplitTab { from: PaneId, tab: usize, target: PaneId, side: Side },
    /// Open a path in a new pane beside `pane`.
    OpenInSplit { pane: PaneId, path: PathBuf, side: Side },

    // Navigation
    Navigate { pane: PaneId, path: PathBuf },
    NavigateNewTab { pane: PaneId, path: PathBuf },
    Back(PaneId),
    Forward(PaneId),
    Up(PaneId),
    Refresh(PaneId),
    EditPath(PaneId),

    // The view
    Sort { pane: PaneId, column: Column },
    SelectAll(PaneId),
    ToggleHidden(PaneId),
    /// Show this folder's whole tree instead of its own children, or stop.
    ToggleFlat(PaneId),
    /// Show a flattened tree as a list or as a tree — the window's preference, so every pane
    /// showing one follows. See [`crate::pane::FlatMode`].
    SetFlatMode(crate::pane::FlatMode),
    /// Rows or tiles, for the tab in front of this pane — the switch at the left of its status line.
    /// See [`crate::pane::ViewMode`].
    ///
    /// The mode rather than a toggle, so that `--tiles` and the switch are the same operation: a flag
    /// that set `Tab::view_mode` by hand would quietly miss whatever switching has to do besides, which
    /// it already did — the scroll fix-up below. `SetFlatMode` beside it is the same shape for the same
    /// reason.
    SetView { pane: PaneId, mode: crate::pane::ViewMode },
    /// Show a chain of folders with nothing in them but each other as one row, or stop — the
    /// window's preference again, so every pane showing a tree follows.
    SetRegroup(bool),
    /// Write `/` rather than `\` between the parts of a path in the path field, or stop. The
    /// window's preference again, and the path field's own context menu is where it is ticked —
    /// see [`crate::ui::breadcrumb::slash_menu`].
    SetForwardSlashes(bool),
    /// Everything under this folder that git has something to say about: the flatten on, and the
    /// filter set to [`crate::fs::sort::CHANGED`]. What the status line's `N changed` asks for.
    ShowChanges(PaneId),
    /// Open or shut a folder in a flattened tree, by its position in the display order.
    ToggleCollapsed { pane: PaneId, position: usize },
    /// Open this folder's preview panel on whatever the keyboard is on, or shut it.
    TogglePreview(PaneId),
    /// Show or hide this pane's console.
    ToggleConsole(PaneId),
    /// Shut it — the panel's own close button.
    ClosePreview(PaneId),
    /// The preview panel's position or size changed, so the settings file is out of date.
    RememberLayout,

    // Files
    /// Put the selection on the clipboard as a move.
    Cut(PaneId),
    /// Put the selection on the clipboard as a copy.
    Copy(PaneId),
    /// Act on whatever is on the clipboard, into this pane's folder.
    Paste(PaneId),
    /// The same three, on paths named outright rather than on whatever a pane has selected.
    ///
    /// What the shell's own `cut`, `copy` and `paste` verbs are redirected into — see
    /// [`App::ours_rather_than_the_shell_s`]. They cannot go through [`Action::Cut`] and its two
    /// siblings, because those read the pane: the menu was raised over a *particular* selection
    /// and, for `paste`, over a particular folder that is not the one the pane is showing.
    CutItems(Vec<PathBuf>),
    CopyItems(Vec<PathBuf>),
    PasteIntoFolder(PathBuf),
    /// `permanent` is Shift+Delete; otherwise it goes to the Recycle Bin.
    Delete { pane: PaneId, permanent: bool },
    /// Start renaming the row under the cursor.
    BeginRename(PaneId),
    /// Finish a rename, or do nothing if the name is unchanged or empty.
    CommitRename { pane: PaneId, name: String },
    CancelRename(PaneId),
    NewFolder(PaneId),
    /// Pick the selection up and hand it to OLE.
    DragOut { pane: PaneId, items: Vec<PathBuf> },
    /// What a right-button drag was asked about, once it has been answered.
    DropHere {
        pane: PaneId,
        items: Vec<PathBuf>,
        into: PathBuf,
        moving: bool,
    },
    /// Show the shell context menu for these items at this screen position.
    ///
    /// `items` empty means the folder's *background* menu — the one you get by right-clicking
    /// below the files, and the only one New is on. It is a different shell object from a
    /// selection's and not the same one with the items left out; `shell::menu`'s `context_of`
    /// has the difference. Paste, Refresh, Sort by and the view settings are *not* on it:
    /// Explorer synthesises those around it, and this program has them on their shortcuts.
    ShellMenu {
        pane: PaneId,
        items: Vec<PathBuf>,
        at: (i32, i32),
    },

    // The outside world
    Open(PathBuf),
    /// Open a shortcut in a tab of its own, if it leads somewhere a tab can go.
    OpenNewTab(PathBuf),
    Reveal(PathBuf),
    OpenTerminal(PathBuf),
    CopyPaths(Vec<PathBuf>),
    AddBookmark(PathBuf),
    /// Reorder the bookmarks: take the one at `from` and put it before `to`.
    MoveBookmark { from: usize, to: usize },
    RemoveBookmark(PathBuf),
    ToggleBookmark(PathBuf),
    SetTheme { dark: bool },
    Window(WindowAction),
}

impl Action {
    /// A stable name, for the journal the click tests read.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Focus(_) => "Focus",
            Self::ActivateTab { .. } => "ActivateTab",
            Self::NewTab { .. } => "NewTab",
            Self::NewTabFocused => "NewTabFocused",
            Self::SplitFocused { .. } => "SplitFocused",
            Self::CloseTab { .. } => "CloseTab",
            Self::ReopenTab => "ReopenTab",
            Self::NextTab { .. } => "NextTab",
            Self::BeginTabDrag { .. } => "BeginTabDrag",
            Self::MoveTab { .. } => "MoveTab",
            Self::SplitTab { .. } => "SplitTab",
            Self::OpenInSplit { .. } => "OpenInSplit",
            Self::Navigate { .. } => "Navigate",
            Self::NavigateNewTab { .. } => "NavigateNewTab",
            Self::Back(_) => "Back",
            Self::Forward(_) => "Forward",
            Self::Up(_) => "Up",
            Self::Refresh(_) => "Refresh",
            Self::EditPath(_) => "EditPath",
            Self::Sort { .. } => "Sort",
            Self::SelectAll(_) => "SelectAll",
            Self::ToggleHidden(_) => "ToggleHidden",
            Self::ToggleFlat(_) => "ToggleFlat",
            Self::SetFlatMode(_) => "SetFlatMode",
            Self::SetView { .. } => "SetView",
            Self::SetRegroup(_) => "SetRegroup",
            Self::SetForwardSlashes(_) => "SetForwardSlashes",
            Self::ShowChanges(_) => "ShowChanges",
            Self::ToggleCollapsed { .. } => "ToggleCollapsed",
            Self::TogglePreview(_) => "TogglePreview",
            Self::ToggleConsole(_) => "ToggleConsole",
            Self::ClosePreview(_) => "ClosePreview",
            Self::RememberLayout => "RememberLayout",
            Self::Cut(_) => "Cut",
            Self::Copy(_) => "Copy",
            Self::Paste(_) => "Paste",
            Self::CutItems(_) => "CutItems",
            Self::CopyItems(_) => "CopyItems",
            Self::PasteIntoFolder(_) => "PasteIntoFolder",
            Self::Delete { .. } => "Delete",
            Self::BeginRename(_) => "BeginRename",
            Self::CommitRename { .. } => "CommitRename",
            Self::CancelRename(_) => "CancelRename",
            Self::NewFolder(_) => "NewFolder",
            Self::ShellMenu { .. } => "ShellMenu",
            Self::DragOut { .. } => "DragOut",
            Self::DropHere { .. } => "DropHere",
            Self::Open(_) => "Open",
            Self::OpenNewTab(_) => "OpenNewTab",
            Self::Reveal(_) => "Reveal",
            Self::OpenTerminal(_) => "OpenTerminal",
            Self::CopyPaths(_) => "CopyPaths",
            Self::AddBookmark(_) => "AddBookmark",
            Self::MoveBookmark { .. } => "MoveBookmark",
            Self::RemoveBookmark(_) => "RemoveBookmark",
            Self::ToggleBookmark(_) => "ToggleBookmark",
            Self::SetTheme { .. } => "SetTheme",
            Self::Window(_) => "Window",
        }
    }
}

/// A context menu the shell is still being asked about.
    ///
/// Everything needed to open it, held for the tenth-of-a-second-or-so the shell takes, so
/// that the menu can appear complete rather than grow while somebody is reading it. The
/// pointer may well have moved by the time it arrives; `at` is where the click was, which is
/// where a menu belongs.
struct Asking {
    token: u64,
    pane: PaneId,
    at: egui::Pos2,
    items: Vec<PathBuf>,
    folder: PathBuf,
    /// How much of a menu was asked for, so that a slow one can be asked for again with less.
    depth: crate::shell::menu::Depth,
    /// The pass this was asked for in, so the click that asked for it is not also the click
    /// that cancels it — see [`App::pump_asking`].
    since: u64,
    /// When it was asked for, for the deadline in [`App::pump_asking`].
    asked: std::time::Instant,
}

/// How many closed tabs are remembered, for `Ctrl+Shift+T`.
///
/// Ten, because the gesture is for undoing a click that went wrong — one or two deep, in the
/// half-minute after it happened. Nobody reaches for it to go archaeology.
const CLOSED_TABS: usize = 10;

/// How long a change under a watched folder or its `.git` is presumed to be our own git write
/// echoing back, rather than something worth asking git about again.
///
/// Generous against how fast the echo actually arrives: the write happens inside the same
/// `git::read` call whose answer just landed, so the two are always close, typically well under a
/// second. See [`App::collect_changes`] for the loop this exists to break, and
/// [`crate::pane::Tab::git_settled_at`] for the timestamp it is measured against.
const GIT_WRITE_SETTLE: f64 = 2.0;

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
    /// Where every pane's preview panel goes and how much room it takes: the window's
    /// preference, one of it. Whether one is *showing* is the tab's — see `Tab::preview`.
    preview: crate::ui::preview::Layout,
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
    /// Whether a tree merges a chain of folders with nothing in them but each other into one row.
    ///
    /// The window's preference beside [`App::flat_mode`], kept the same way and for the same reason.
    /// See [`crate::fs::sort::build_tree_order`], which is what it does.
    regroup: bool,
    /// Whether the path field writes `/` between the parts of a path rather than `\`.
    ///
    /// The window's preference again — which slash you want is a habit, and it is about where the
    /// path is going after it leaves here rather than about the folder in front of you. Ticked in
    /// the field's own context menu. See [`crate::config::Config::forward_slashes`].
    forward_slashes: bool,
    /// Reads whatever the preview panels are pointed at, off the UI thread.
    previews: crate::preview::Previews,
    /// The window the shell parents its own dialogs to.
    owner: crate::shell::Owner,
    /// Shell file operations in flight.
    ops: crate::shell::ops::Operations,
    /// Items cut but not yet pasted, shown ghosted the way Explorer shows them.
    cut: Vec<PathBuf>,
    /// The last thing that went wrong, for the status line.
    notice: Option<String>,
    /// Where a drag from outside is hovering, in points, for the pane highlight.
    drop_hover: Option<(i32, i32)>,
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
    /// Where the sidebar's Bookmarks group was drawn, so a drag can be dropped on it.
    /// Read a frame later than it is written, which is a frame the sidebar has not moved in.
    bookmarks_rect: Option<Rect>,
    /// A bookmark being dragged to a new position in the list.
    bookmark_drag: Option<usize>,
    /// The OLE drag in flight, if there is one, and the pane the files were picked up in so
    /// a move can re-read the folder they left. One at a time: the pointer is only holding
    /// one thing, and a second drag would be following the same button as the first.
    file_drag: Option<(PaneId, crate::shell::dnd::Drag)>,

    volumes: Volumes,
    places: Vec<Place>,
    bookmarks: Vec<PathBuf>,
    sections: Sections,
    sidebar_width: f32,

    maximized: bool,
    /// The window's inner size, tracked so it can be restored next launch.
    window_size: Option<[f32; 2]>,
    /// And where it is: the outer top-left corner in *physical pixels*. See
    /// [`crate::config::Config::position`] for why that unit and not points.
    window_position: Option<[f32; 2]>,
    /// Collected during drawing, drained after.
    actions: Vec<Action>,
    /// One buffer every formatted cell in the window is written through.
    scratch: String,
    /// Where each pane was drawn, for the docking gesture and for keyboard focus.
    pane_rects: Vec<(PaneId, Rect)>,
    /// Every tab drawn this frame, wherever its strip was. The drag resolution needs all
    /// of them at once, and they are not all known until the panes have been drawn.
    tab_slots: Vec<chrome::Slot>,
    splitters: Vec<Splitter>,
    pane_order: Vec<PaneId>,

    /// Settings worth writing back, and whether they have changed.
    config: Config,
    config_dirty: bool,

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

/// What the window looks like to the platform, as far as rendering crisply is concerned.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
struct Shape {
    /// In *physical pixels*, which is what the framebuffer is sized in.
    pixels: (u32, u32),
    /// Rounded, because a scale factor arrives as a float and comparing floats for
    /// equality every frame is a way to request repaints for ever.
    scale: u32,
    /// What the *platform* says the scale is, against `scale`, which is what egui rasterised
    /// the glyph atlas for.
    ///
    /// Tracked separately because the two disagreeing is the one thing that would make text
    /// soft without anything else moving: an atlas built for one scale, drawn at another.
    /// Zero when the platform has not said.
    native_scale: u32,
    focused: bool,
    minimized: bool,
}

impl Shape {
    fn of(ctx: &egui::Context) -> Self {
        let scale = ctx.pixels_per_point();
        ctx.input(|i| {
            let size = i.viewport_rect().size() * scale;
            Self {
                pixels: (size.x.round() as u32, size.y.round() as u32),
                scale: (scale * 1000.0).round() as u32,
                native_scale: i
                    .viewport()
                    .native_pixels_per_point
                    .map_or(0, |ppp| (ppp * 1000.0).round() as u32),
                focused: i.viewport().focused.unwrap_or(true),
                minimized: i.viewport().minimized.unwrap_or(false),
            }
        })
    }
}

/// This process's private bytes, and its GDI and USER handle counts.
///
/// The three numbers a memory report is actually about. Private bytes is Task Manager's
/// "Memory" column, and it counts COM's allocator and every loaded shell extension's own
/// heap as well as Rust's. The handle counts are here because a leaked `HICON`, `HBITMAP` or
/// `HDC` costs memory without a single Rust allocation, and this program asks the shell for
/// an icon per file type it meets.
#[cfg(windows)]
fn process_memory() -> (usize, u32, u32) {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS, GR_USEROBJECTS,
    };

    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct is told its own size, and the handle is this process's
    // pseudo-handle, which needs no closing.
    unsafe {
        let me = GetCurrentProcess();
        let _ = GetProcessMemoryInfo(
            me,
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        );
        (
            counters.PrivateUsage,
            GetGuiResources(me, GR_GDIOBJECTS),
            GetGuiResources(me, GR_USEROBJECTS),
        )
    }
}

#[cfg(not(windows))]
fn process_memory() -> (usize, u32, u32) {
    (0, 0, 0)
}

/// `--walk=<dir>`: browse subfolder after subfolder, on a timer, for as long as the window
/// is open.
///
/// The measurement in `app::click_tests` runs without a renderer, so it can only see the
/// Rust heap and the handles — not the GL driver, the texture the glyph atlas lives in, or
/// the shell extensions a real session loads. This drives the *real* window through the same
/// walk with `--trace` reporting beside it, which is the only way to watch the number a user
/// is watching.
struct Walk {
    dirs: Vec<PathBuf>,
    at: usize,
    stepped_at: f64,
}

impl Walk {
    /// A step every this often. Faster than a person browses, slow enough that each folder
    /// is actually read and drawn rather than superseded before its scan lands.
    const STEP: f64 = 0.25;

    fn collect(from: &Path) -> Self {
        let mut dirs = Vec::new();
        let mut queue = std::collections::VecDeque::from([from.to_path_buf()]);
        while let Some(dir) = queue.pop_front() {
            if dirs.len() >= 2000 {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    queue.push_back(entry.path());
                    dirs.push(entry.path());
                }
            }
        }
        println!("walking {} folders under {}", dirs.len(), from.display());
        Self {
            dirs,
            at: 0,
            stepped_at: 0.0,
        }
    }

    /// The next folder, if it is time for one.
    fn step(&mut self, time: f64) -> Option<PathBuf> {
        if self.dirs.is_empty() || time - self.stepped_at < Self::STEP {
            return None;
        }
        self.stepped_at = time;
        self.at = (self.at + 1) % self.dirs.len();
        Some(self.dirs[self.at].clone())
    }
}

/// `--scroll`: run the listing up and down for ever, which is the one thing a harness with no
/// renderer cannot measure.
///
/// Scrolling redraws rows that have already been drawn — no new names, no new icons, no new
/// column widths — so anything that grows under this grows *per frame*, in the painting, and
/// leaving the folder would not give it back.
#[derive(Default)]
struct Scrolling {
    on: bool,
    /// Where in the sweep, 0..1 and back.
    phase: f32,
    up: bool,
}

impl Scrolling {
    /// A step every frame, as fast as the window will paint.
    ///
    /// `span` is how tall the listing is — which is not `rows × ROW_HEIGHT` in the large-icon view, so
    /// the caller works it out from whichever view is showing. Handed the row figure, a grid of tiles
    /// would sweep to an offset several times its own height and spend nearly the whole cycle clamped
    /// against the bottom, which measures the one position that is not moving.
    fn step(&mut self, span: f32) -> Option<f32> {
        if !self.on || span <= 0.0 {
            return None;
        }
        // Two seconds top to bottom at 60fps, which is faster than a wheel and slower than a
        // dragged scrollbar.
        let step = 1.0 / 120.0;
        if self.up {
            self.phase -= step;
            if self.phase <= 0.0 {
                self.up = false;
            }
        } else {
            self.phase += step;
            if self.phase >= 1.0 {
                self.up = true;
            }
        }
        Some(self.phase.clamp(0.0, 1.0) * span)
    }
}

/// Asks for frames for a moment after the window's shape changes.
///
/// This program paints on demand, which is the right default — a file listing that repaints
/// sixty times a second to show the same rows is a laptop fan. But it means that between
/// events the screen holds whatever was painted last, and a *stale* frame is only as good
/// as the assumption that the window still looks the way it did: restore it from the
/// taskbar, drag it to a monitor at another scale, or let a maximised window resize when
/// the taskbar hides, and the compositor has a surface of the wrong size to show. It
/// stretches it, which arrives as text going soft for as long as nothing asks for a new
/// frame — and then coming back sharp the moment something does.
///
/// So: notice the shape changing, and keep asking for frames until it has been still for
/// [`Self::QUIET`]. Costs a handful of frames per window gesture and nothing at rest.
#[derive(Default)]
struct Settling {
    was: Shape,
    /// When the shape last changed, in `InputState::time`.
    changed_at: Option<f64>,
    /// `--trace`: report each change on stdout.
    trace: bool,
    /// When `--trace` last reported the memory figures.
    reported_at: f64,
}

impl Settling {
    /// How long after a change to keep painting. Long enough to cover a restore animation
    /// and a scale change, short enough that nobody notices the frames.
    const QUIET: f64 = 0.75;

    /// How often `--trace` reports the memory figures. Often enough to see a curve over a
    /// minute of browsing, rare enough that the log stays readable.
    const REPORT: f64 = 3.0;

    /// Whether a frame should be asked for.
    fn observe(&mut self, now: Shape, time: f64) -> bool {
        if now != self.was {
            if self.trace {
                println!("{time:8.3}  {:?} -> {now:?}", self.was);
            }
            self.was = now;
            self.changed_at = Some(time);
        }
        match self.changed_at {
            Some(at) if time - at < Self::QUIET => true,
            Some(_) => {
                self.changed_at = None;
                false
            }
            None => false,
        }
    }
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
fn pane_tabs(pane: &Pane) -> crate::config::PaneTabs {
    crate::config::PaneTabs {
        paths: pane.tabs.iter().map(|tab| tab.path.clone()).collect(),
        active: pane.active.min(pane.tabs.len().saturating_sub(1)),
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
        let theme = if config.dark {
            Theme::dark()
        } else {
            Theme::light()
        };
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
                        let mut tabs = group.paths.iter().cloned().map(Tab::new);
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
                        .map(Tab::new)
                        .collect();
                    if tabs.is_empty() {
                        tabs.push(Tab::new(fs::places::default_start()));
                    }
                    let mut pane = Pane::new(first, tabs.remove(0));
                    pane.tabs.extend(tabs);
                    panes.push(pane);
                }
            }
        } else {
            let mut paths = open.into_iter();
            panes.push(Pane::new(first, Tab::new(paths.next().unwrap_or_default())));
            for path in paths {
                let id = next_pane;
                next_pane += 1;
                panes.push(Pane::new(id, Tab::new(path)));
                layout.split(id - 1, side, id);
            }
        }

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
            icons: crate::shell::icons::Icons::new(),
            thumbs: crate::shell::thumbs::Thumbs::new(ctx),
            links: crate::shell::links::Links::new(ctx),
            git: crate::git::Git::new(ctx),
            git_waiting: 0,
            previews: crate::preview::Previews::new(ctx),
            owner: crate::shell::Owner::default(),
            ops: crate::shell::ops::Operations::new(),
            cut: Vec::new(),
            notice: None,
            drop_hover: None,
            clipboard_has_files: false,
            drops: crate::shell::dnd::Zone::new(),
            modal: crate::shell::Modal::new(ctx),
            menu: None,
            asking: None,
            menu_builder: crate::shell::menu::Builder::new(ctx),
            bookmarks_rect: None,
            bookmark_drag: None,
            file_drag: None,
            volumes,
            places,
            bookmarks: config.bookmarks.clone(),
            sections: config.sections,
            sidebar_width: config.sidebar_width,
            preview: config.preview,
            console_share: config.console_share,
            console_shell: config.console_shell,
            flat_mode: config.flat_mode,
            regroup: config.regroup,
            forward_slashes: config.forward_slashes,
            // The window as the settings file describes it, and not as this program is about to
            // find it: a maximised window never writes the other two — see `Self::frame` — so
            // starting them empty threw away the size and the place the window would go back to
            // the moment a session was left maximised, and the next launch had nothing to restore
            // to but the default. What the platform actually did with them is
            // `main::open_maximized`'s business; this is only what is remembered.
            maximized: config.maximized,
            window_size: config.window,
            window_position: config.position,
            actions: Vec::new(),
            scratch: String::with_capacity(64),
            pane_rects: Vec::new(),
            tab_slots: Vec::new(),
            splitters: Vec::new(),
            pane_order: vec![first],
            config,
            config_dirty: false,
            settling: Settling::default(),
            walk: None,
            scrolling: Scrolling::default(),
            journal: None,
        }
    }

    /// `--walk=<dir>`: browse subfolder after subfolder by itself. See [`Walk`].
    pub fn walking(mut self, from: Option<PathBuf>) -> Self {
        self.walk = from.as_deref().map(Walk::collect);
        self
    }

    /// `--flat`: open with every pane flattened, for looking at that view without having to
    /// press the button first. The same family as `--menu` and `--rename`.
    pub fn flattened(mut self, on: bool) -> Self {
        if on {
            for pane in &mut self.panes {
                for tab in &mut pane.tabs {
                    tab.flat = true;
                    // In whichever mode the settings ask for, because that is what pressing the
                    // button would have done — a flag that opened the list while the settings said
                    // tree would be a capture of a view nobody can reach.
                    tab.flat_mode = self.flat_mode;
                    tab.regroup = self.regroup;
                }
            }
        }
        self
    }

    /// `--scroll`: run the listing up and down for ever. See [`Scrolling`].
    pub fn scrolling(mut self, on: bool) -> Self {
        self.scrolling.on = on;
        self
    }

    /// `--trace`: report every change to the window's shape on stdout.
    ///
    /// For the one class of bug this program cannot see from the inside — text that goes
    /// soft for a few seconds after a window gesture. One line per change says whether it
    /// was the size, the scale, the focus or the minimised flag that moved, which is the
    /// difference between a fix and a guess.
    pub fn tracing(mut self, on: bool) -> Self {
        self.settling.trace = on;
        self
    }

    /// Tell the shell which window to parent its dialogs to.
    ///
    /// Set once, from eframe, which is the only thing that knows the handle. Without
    /// it a progress or conflict dialog appears behind this window and looks hung.
    pub fn set_owner(&mut self, owner: crate::shell::Owner) {
        self.owner = owner;
    }

    /// Select and scroll to an entry in the first pane once its listing arrives.
    ///
    /// Deferred rather than applied here because the listing is not read yet — the
    /// tab carries the name and [`Tab::apply`] acts on it, which is the same path
    /// going Up uses to highlight the folder you came out of.
    pub fn revealing(mut self, name: Option<String>) -> Self {
        if let Some(name) = name {
            if let Some(pane) = self.panes.first_mut() {
                pane.tab_mut().reveal = Some(name);
            }
        }
        self
    }

    /// `--filter=<text>`: put a line in the first pane's filter box before anything is drawn.
    ///
    /// The same reason `--reveal=` and `--console=` exist: a capture run has no keyboard, and the
    /// filter is a box you type into. It is set on the tab rather than pushed through the box so that
    /// it is already in force on the frame the listing lands — `Tab::apply` rebuilds the order from the
    /// filter as it stands, so a line typed afterwards would cost a second pass to no purpose.
    pub fn filtering(mut self, text: Option<String>) -> Self {
        if let Some(text) = text {
            if let Some(pane) = self.panes.first_mut() {
                pane.tab_mut().filter = text;
            }
        }
        self
    }

    /// The colour eframe clears the window to, so a resize does not flash.
    pub fn clear_color(&self) -> [f32; 4] {
        self.theme.bg.canvas.to_normalized_gamma_f32()
    }

    /// Settings to write back on the way out.
    pub fn settings(&self) -> Config {
        let mut config = Config {
            bookmarks: self.bookmarks.clone(),
            sidebar_width: self.sidebar_width,
            preview: self.preview,
            console_share: self.console_share,
            console_shell: self.console_shell,
            flat_mode: self.flat_mode,
            regroup: self.regroup,
            forward_slashes: self.forward_slashes,
            sections: self.sections,
            panes: Vec::new(),
            layout: None,
            focus: 0,
            window: self.window_size,
            position: self.window_position,
            maximized: self.maximized,
            dark: self.theme.dark,
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
            if *id == self.focused {
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

    pub fn frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if !self.installed {
            azur::install(
                &ctx,
                self.theme.azur(),
                &azur::StyleOptions {
                    // A dense, table-heavy window, which is exactly the case the
                    // design system documents these two for.
                    control_height: azur::tokens::control::SMALL,
                    selectable_labels: false,
                    ..Default::default()
                },
            );
            self.installed = true;
        }
        self.maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        self.close_on_blur(&ctx);
        // Keep painting while the window is being restored, resized or rescaled, so the
        // compositor never has to stretch a frame that was drawn for a different shape.
        let now = ctx.input(|i| i.time);
        if self.settling.observe(Shape::of(&ctx), now) {
            ctx.request_repaint();
        }
        // `--walk`: step to the next folder by itself, so a real window can be measured
        // browsing rather than sitting still.
        if let Some(next) = self.walk.as_mut().and_then(|walk| walk.step(now)) {
            let pane = self.focused;
            self.perform(&ctx, Action::Navigate { pane, path: next });
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(Walk::STEP));
        }
        // `--scroll`: keep the listing moving, so the painting is measured and not just the
        // reading.
        if self.scrolling.on {
            let span = self
                .panes
                .iter()
                .find(|p| p.id == self.focused)
                .map_or(0.0, |p| {
                    let tab = p.tab();
                    if tab.view_mode.is_icons() {
                        tab.grid.height()
                    } else {
                        tab.order.len() as f32 * crate::pane::ROW_HEIGHT
                    }
                });
            if let Some(to) = self.scrolling.step(span) {
                if let Some(pane) = self.panes.iter_mut().find(|p| p.id == self.focused) {
                    pane.tab_mut().scroll_to = Some(to);
                }
                ctx.request_repaint();
            }
        }
        // `--trace`: the memory figures, every few seconds, beside what the cache is holding.
        // Together they say whether growth is the cache filling up or something being kept.
        //
        // A repaint has to be booked for it, because this program is idle between events and
        // a report that only fires when something else asks for a frame is a report that
        // never fires while you sit and watch the number climb.
        if self.settling.trace {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(Settling::REPORT));
            if now - self.settling.reported_at > Settling::REPORT {
                self.settling.reported_at = now;
                let (private, gdi, user) = process_memory();
                let (dirs, entries) = self.loader.held();
                let (kinds, paths, textures) = self.icons.held();
                let (known, pictures, pages, spare) = self.thumbs.held();
                let (gaveup, queued) = self.thumbs.stuck();
                println!(
                    "{now:8.1}  private {:>7} KB   gdi {gdi:>5}   user {user:>4}   \
                     cache {dirs:>3} folders / {entries:>8} entries   \
                     icons {kinds:>4} kinds / {paths:>5} paths / {textures:>4} textures /                      {} bitmaps / {} uploads   \
                     thumbs {known:>4} known / {pictures:>4} drawn / {gaveup:>4} gaveup / {queued:>3} queued / {:>4} asks / {pages} pages / {spare:>4} spare / {} fetched / {} uploads",
                    private / 1024,
                    self.icons.bitmaps,
                    self.icons.uploads,
                    self.thumbs.asks,
                    self.thumbs.fetched,
                    self.thumbs.uploads,
                );
            }
        }
        // A drive may have been plugged in or ejected while the window was in the
        // background, and regaining focus is the one moment it is worth re-probing.
        if ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::WindowFocused(true)))
        }) {
            self.volumes.refresh(&ctx);
        }
        self.volumes.poll();
        self.icons.poll(&ctx);
        // Which listings are on screen, so the icon worker can drop the questions it still has
        // about folders nobody is looking at any more.
        let views: Vec<u64> = self
            .panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .map(|tab| tab.view)
            .collect();
        self.icons.only(&views);
        // Which folders the tiles' pictures may still be wanted for, for the same reason: one that has
        // been scrolled away from must not hold up the one in front of it.
        //
        // **Delivery is at the *end* of the frame** rather than here beside the icons' — see
        // [`crate::shell::thumbs::Thumbs::poll`], which is where that has to happen and why.
        self.thumbs.only(&views);
        self.deliver_icons();
        self.deliver_links();
        self.collect_previews(&ctx, now);
        // Both of these give memory back once nothing has wanted it for two minutes, so a
        // session that browsed a hundred folders and then settled does not hold the
        // high-water mark until the window closes.
        self.loader.sweep();
        self.clipboard_has_files = crate::shell::clipboard::has_files();
        self.collect_operations();
        // Registered on the window rather than at startup, because the handle does not
        // exist until the platform has made one.
        self.drops.attach(self.owner);
        self.publish_drop_targets(&ctx);
        self.collect_drops(&ctx);
        self.pump_drag(&ctx);
        self.pump_asking(&ctx);
        self.collect_changes(&ctx);
        self.collect_modal();
        if !self.maximized {
            // Only a restored window's size is worth remembering; a maximised one is
            // described by the flag, and saving the screen size would pin the window
            // to this monitor.
            let size = ctx.viewport_rect().size();
            if size.x >= 640.0 && size.y >= 400.0 {
                self.window_size = Some([size.x, size.y]);
            }
            // Where it is, in physical pixels: the *outer* rect, because that is what the
            // platform places and what it will be given back. egui reports it in points, so
            // this multiplies by the same scale factor the restore divides by — which is what
            // makes the round trip exact whatever the monitor's DPI turns out to be.
            //
            // `None` while the window is minimised, when there is no position to have; the
            // last real one stays, which is the one worth reopening at.
            if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
                let scale = ctx.pixels_per_point();
                self.window_position = Some([outer.min.x * scale, outer.min.y * scale]);
            }
        }

        self.collect_scans();
        self.start_scans(&ctx, now);
        // After the scans, because a listing that landed this frame is a folder to ask about this
        // frame — the branch and the marks then arrive one answer later rather than one navigation
        // later.
        self.collect_git(now);

        // Swapped out rather than borrowed, because the drawing code needs `&Theme`
        // and `&mut self` at the same time. The placeholder is the same side of the
        // palette, so nothing can read the wrong one.
        let placeholder = if self.theme.dark {
            Theme::dark()
        } else {
            Theme::light()
        };
        let theme = std::mem::replace(&mut self.theme, placeholder);

        // ---- Geometry, before anything is drawn -------------------------
        //
        // A pane's tabs sit directly above that pane, in the title bar or on a band of
        // their own, so where the panes are has to be known before the bar can be drawn —
        // and the bands take room off the top of the panes under them, so the panes cannot
        // be drawn until the bands are decided either. Everything here paints at explicit
        // rects rather than through egui's panels, which is what makes that ordering free
        // to choose: the title bar is drawn *last*, over canvas nothing else wanted.
        let screen = ctx.viewport_rect();
        let bar = chrome::bar_rect(screen);
        let body = Rect::from_min_max(pos2(screen.left(), bar.bottom()), screen.max);
        let plan = self.plan_layout(body, bar);

        self.tab_slots.clear();
        self.body(ui, &theme, body, &plan);
        let in_bar = chrome::title_bar(
            ui,
            &theme,
            &self.panes,
            &plan.in_bar,
            self.focused,
            self.maximized,
            &self.drag,
            &mut self.icons,
            &mut self.actions,
        );
        self.tab_slots.extend(in_bar);

        // Resolved after the panes, so the rects the pointer is tested against are the
        // ones drawn this frame rather than last frame's.
        if self.drag.is_some() {
            let slots = std::mem::take(&mut self.tab_slots);
            let mut drag = self.drag.take();
            chrome::resolve_drag(ui, &theme, &self.panes, &slots, &mut drag, &mut self.actions);
            self.drag = drag;
            self.tab_slots = slots;
        }

        // Over everything, and in the root `Ui` so its coordinates are the screen's.
        self.draw_menu(ui, &theme);

        // Last, so they are on top of everything — though they are sized to sit in
        // canvas the panels do not reach, so there is nothing to be on top of.
        chrome::resize_borders(ui, self.maximized);

        // An undecorated window has no frame of its own, and on a dark desktop its
        // edge would be invisible. `stroke-default` is the line Azur gives a panel.
        let edge = ctx.viewport_rect();
        ui.painter().rect_stroke(
            edge,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, self.theme.stroke.default),
            egui::StrokeKind::Inside,
        );

        self.keyboard(&ctx);
        self.thumb_buttons(&ctx);
        self.apply(&ctx);

        // **The tiles' pictures are taken delivery of here, after the panes have been drawn**, and the
        // position in this function is the whole of what makes the atlas's eviction rule exact: a cell
        // is reusable precisely when no tile drew it in the frame that has just finished, and that is
        // only knowable once the frame has finished. Polled at the top of the frame, as everything else
        // here is, the newest information would be a frame old and "on screen" would have to be guessed
        // at. See [`crate::shell::thumbs::Thumbs::poll`].
        self.thumbs.poll();
    }

    /// Divide the window up and lay the panes out, without drawing anything.
    ///
    /// Split out from [`App::body`] because the title bar needs the answer before it can
    /// place its tab strips, and because it is arithmetic worth being able to check on its
    /// own. Returns where every pane's tabs go, with the panes already reduced by the room
    /// their strip bands take.
    fn plan_layout(&mut self, body: Rect, bar: Rect) -> chrome::StripPlan {
        let (_, panes_area) = Self::split_body(body, self.sidebar_width);
        self.layout
            .layout(panes_area, &mut self.pane_rects, &mut self.splitters);
        self.pane_order = self.pane_rects.iter().map(|(id, _)| *id).collect();
        chrome::plan_strips(&mut self.pane_rects, bar)
    }

    /// The sidebar and the area the panes divide between them.
    ///
    /// The body, whole: the panels reach the window's edges, and the only thing between a panel
    /// and the outside is the window's own one-pixel border painted over the top of it. They used
    /// to stop [`GUTTER`] short of it on every side, which put a band of canvas round the block
    /// and made it read as a tray of cards.
    ///
    /// [`crate::ui::SEAM`] between the two, and that is the only division in here.
    fn split_body(body: Rect, sidebar_width: f32) -> (Rect, Rect) {
        let inner = body;
        let width = sidebar_width.clamp(140.0, (inner.width() - 240.0).max(140.0));
        let sidebar = Rect::from_min_max(inner.min, pos2(inner.left() + width, inner.bottom()));
        let panes_area =
            Rect::from_min_max(pos2(sidebar.right() + crate::ui::SEAM, inner.top()), inner.max);
        (sidebar, panes_area)
    }

    /// The sidebar, the splitter between it and the panes, the tab bands, and the panes.
    fn body(&mut self, ui: &mut Ui, t: &Theme, full: Rect, plan: &chrome::StripPlan) {
        if full.width() < 80.0 || full.height() < 40.0 {
            return;
        }

        let inner = full;
        let (sidebar, panes_area) = Self::split_body(full, self.sidebar_width);

        // ---- What shows through the seams ---------------------------------
        //
        // One fill behind the whole block, so every boundary between two panels is whatever
        // this leaves showing — and no panel has to know where its neighbours are. That
        // matters more than it sounds: panes come from a tree of splits, so "where the seams
        // are" is the layout's answer and it changes with every drag, whereas "the panels do
        // not quite cover this" is true for free.
        ui.painter()
            .rect_filled(inner, egui::CornerRadius::ZERO, crate::ui::seam(t));

        // ---- The sidebar --------------------------------------------------
        //
        // Its content fills it. There was a point of inset here, left from when the panel wore a
        // one-pixel ring and the content had to start inside it; with the ring gone it was a
        // point of nothing, on all four sides of both panels.
        //
        // `background-layer-alt`, not `self.surface`'s `background-layer`: the sidebar reads
        // as the same surface as the title bar and the status bar, not as a pane.
        ui.painter()
            .rect_filled(sidebar, egui::CornerRadius::ZERO, t.bg.layer_alt);
        {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(sidebar)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            child.set_clip_rect(sidebar.intersect(ui.clip_rect()));
            let current = self
                .panes
                .iter()
                .find(|p| p.id == self.focused)
                .map(|p| p.tab().path.clone())
                .unwrap_or_default();
            self.bookmarks_rect = sidebar::show(
                &mut child,
                t,
                self.volumes.all(),
                &self.bookmarks,
                &self.places,
                &current,
                self.focused,
                &mut self.sections,
                &mut self.icons,
                &mut self.bookmark_drag,
                &mut self.scratch,
                &mut self.actions,
            );
            // A drag hovering over Bookmarks. Same highlight the listing gets, so pinning
            // reads as a drop rather than as nothing happening — and drawn here, after the
            // rows, for the same reason it is in the pane.
            if let Some(area) = self.bookmarks_preview(ui.ctx().pixels_per_point()) {
                crate::ui::drop_target(child.painter(), area, t);
            }
        }

        // ---- The splitter beside it ---------------------------------------
        let grip = Rect::from_min_max(
            pos2(sidebar.right() - 3.0, inner.top()),
            pos2(panes_area.left() + 3.0, inner.bottom()),
        );
        let response = ui.interact(
            grip,
            egui::Id::new("sidebar-grip"),
            egui::Sense::click_and_drag(),
        );
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if response.dragged() {
            self.sidebar_width = (self.sidebar_width + response.drag_delta().x).clamp(140.0, 520.0);
            self.config_dirty = true;
        }
        // Double-clicking a splitter puts it back where it started, which is the same gesture
        // the column edges in the listing already answer to.
        if response.double_clicked() {
            self.sidebar_width = crate::config::SIDEBAR_WIDTH;
            self.config_dirty = true;
        }

        // ---- The panes ----------------------------------------------------
        //
        // Already laid out by `plan_layout`, which had to run before the title bar.

        // ---- The tab bands, for the rows the title bar cannot reach --------
        //
        // Painted like the title bar, because that is what they are for the row below
        // them: `background-layer-alt` and a hairline along the bottom.
        for row in &plan.rows {
            ui.painter()
                .rect_filled(row.band, egui::CornerRadius::ZERO, t.bg.layer_alt);
            crate::ui::rule_below(ui.painter(), row.band, t);
        }

        let rects = self.pane_rects.clone();
        for (id, rect) in rects {
            self.pane(ui, t, id, rect);
        }

        // ---- The splitters, between the panes and the tabs ------------------
        //
        // **The order here is the priority, and it has to be exactly this.** Within a layer egui
        // gives a click to the *last* widget that asked for it ("in tie, pick last = topmost" in its
        // `hit_test`), so registering these decides what wins where they overlap — and they overlap
        // two things, in opposite directions.
        //
        // *After the panes*, because [`dock::GRAB`] reaches four points into the pane on each side and
        // the listing's vertical scrollbar is exactly those four points. Registered before the panes,
        // the scrollbar won and **two panes side by side could not be resized at all** — the pointer
        // over the divider was the scrollbar's. A stacked pair was never affected: there the grab
        // reaches into the pane's *bottom* edge, and a `ScrollArea::vertical` has nothing there.
        //
        // *Before the tab strips*, because the grab spans the split's full height, tab bands included.
        // Last would mean a splitter taking clicks off the tabs nearest a pane boundary.
        //
        // Nothing here paints, so none of this changes what is drawn.
        let mut ratios: Vec<(Vec<u8>, f32)> = Vec::new();
        for splitter in &self.splitters {
            let response = ui.interact(
                splitter.rect,
                egui::Id::new(("splitter", &splitter.route)),
                egui::Sense::click_and_drag(),
            );
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(if splitter.horizontal {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
            }
            if response.dragged() {
                let delta = if splitter.horizontal {
                    response.drag_delta().x / panes_area.width().max(1.0)
                } else {
                    response.drag_delta().y / panes_area.height().max(1.0)
                };
                ratios.push((splitter.route.clone(), delta));
            }
            // A double click evens the split up again.
            if response.double_clicked() {
                if let Some(ratio) = self.layout.ratio_at(&splitter.route) {
                    *ratio = 0.5;
                }
            }
        }
        for (route, delta) in ratios {
            if let Some(ratio) = self.layout.ratio_at(&route) {
                *ratio = (*ratio + delta).clamp(dock::RATIO_MIN, dock::RATIO_MAX);
            }
        }

        // The strips themselves after the panes, so a tab is never under a pane's card.
        for row in &plan.rows {
            for (id, strip) in &row.strips {
                let Some(index) = self.panes.iter().position(|p| p.id == *id) else {
                    continue;
                };
                let inner = Rect::from_min_max(
                    pos2(strip.left() + GUTTER, strip.top()),
                    pos2(strip.right() - GUTTER, strip.bottom()),
                );
                let focused = *id == self.focused;
                let slots = chrome::tab_strip(
                    ui,
                    t,
                    inner,
                    &self.panes[index],
                    focused,
                    &self.drag,
                    &mut self.icons,
                    &mut self.actions,
                );
                self.tab_slots.extend(slots);
            }
        }
    }

    /// One pane: its surface, its path bar, its listing.
    fn pane(&mut self, ui: &mut Ui, t: &Theme, id: PaneId, rect: Rect) {
        let Some(index) = self.panes.iter().position(|p| p.id == id) else {
            return;
        };
        self.panes[index].rect = rect;
        // The card is drawn whatever the size, so a pane squeezed past the point of
        // usefulness still reads as a pane you can drag wider rather than as a hole
        // in the window.
        self.surface(ui, t, rect);
        if rect.width() < 96.0 || rect.height() < 48.0 {
            return;
        }

        // Which pane the keyboard is in is said by the accent under its active tab, and
        // only there. A ring around the whole card says the same thing ten times as
        // loudly, and in a two-pane window it turns every click into a visible change of
        // frame — so the pane itself stays quiet.
        // **Whether the *listing* has the keyboard**, which is not the same as whether the pane does:
        // this pane's console may be holding it, and then the arrow keys are the console's and the
        // rows have to say so. Asked of the panel — [`console::State::keeps_keys`] is the one
        // definition, and the same call the panel makes before it reads a key, so a row cannot draw
        // itself focused in a frame the keys were going somewhere else.
        let console_open = self.panes[index].console_open;
        let console_has_keys =
            console_open && self.panes[index].console_state.keeps_keys(ui.ctx(), id);
        let focused = id == self.focused && !console_has_keys;

        // The whole pane, with nothing held back: the path bar reaches the seams on both sides
        // and the listing reaches the bottom edge. The point of inset that used to be here was
        // room for the ring the panel no longer wears.
        let inside = rect;
        let bar = Rect::from_min_size(inside.min, vec2(inside.width(), breadcrumb::HEIGHT));
        // **The status line is the floor of the pane**, across its whole width, and everything else is
        // stacked on top of it. It is taken off *first* for that reason: it is the one piece of
        // furniture that is about the pane rather than about a panel inside it, and a pane whose bottom
        // edge is a status line under one half and a preview panel under the other has two floors.
        //
        // Which is also why it is not the listing's to place any more. It was, and it looked right for
        // as long as the preview panel went along the bottom — where it lands *above* the line either
        // way — and wrong the moment the panel was docked to the right, where it ran down past the
        // line's left end to the pane's own edge.
        let floor = Rect::from_min_max(
            pos2(inside.left(), inside.bottom() - crate::ui::filelist::STATUS_HEIGHT),
            inside.max,
        );
        // Everything between the path bar and the floor, which the listing and this folder's preview
        // panel divide between them. The panel is *inside* the pane — see [`crate::ui::preview`] — so
        // it is this shape that `Where::Auto` reads, and both rects come out of one split.
        let body = Rect::from_min_max(bar.left_bottom(), pos2(inside.right(), floor.top()));
        let open = self.panes[index].tab().preview.open;
        let (list, panel) = crate::ui::preview::split(body, open, self.preview);

        // **The console is in the stack, not over it.** It takes its band off the bottom of the
        // *listing's* region, so the order down a pane is rows, console, status — and nothing about
        // where the preview panel goes changes.
        //
        // The listing still gets the whole of `list` to draw in. What it gets told is how much of the
        // bottom to leave alone, because a rect that reaches under the console is one whose rows are
        // laid out and hit-tested there — the console painting over them is what makes it look like an
        // overlay.
        let (rows, console) =
            crate::ui::console::split(list, self.panes[index].console_open, self.console_share);
        let reserved = (list.bottom() - rows.bottom()).max(0.0);

        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inside)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(inside.intersect(ui.clip_rect()));

        // A click anywhere in the pane moves the keyboard here.
        let claim = child.interact(rect, egui::Id::new(("pane-claim", id)), egui::Sense::click());
        if claim.clicked() || claim.secondary_clicked() {
            self.actions.push(Action::Focus(id));
        }

        // The path bar paints its own surface, in `breadcrumb::show`. It used to be
        // `background-layer` like the rest of the pane with a `stroke-subtle` hairline under it
        // to say where it ended; now it is [`crate::ui::seam`], the fill says it, and a hairline
        // between two fills that already differ is a third line nobody asked for.

        let Self {
            panes,
            crumbs,
            complete,
            loader,
            actions,
            scratch,
            zone,
            icons,
            thumbs,
            links,
            cut,
            notice,
            ops,
            preview,
            flat_mode,
            regroup,
            forward_slashes,
            ..
        } = self;
        let (flat_mode, regroup, slashes) = (*flat_mode, *regroup, *forward_slashes);
        // A copy in progress, or the last thing that went wrong: whichever there is,
        // the pane's status line says so instead of counting files.
        let status = ops.in_progress().or(notice.as_deref());
        let pane = &mut panes[index];
        let tab = pane.tab_mut();

        breadcrumb::show(
            &mut child, t, bar, id, tab, crumbs, complete, loader, icons, preview, flat_mode,
            regroup, slashes, actions,
        );
        let outcome = filelist::show(
            &mut child, t, zone, list, floor, id, tab, focused, icons, links, thumbs, cut, status,
            console_open, reserved, scratch, actions,
        );
        // After the listing, so the panel's surface is over it rather than under: the listing
        // reaches for the whole body when it measures its own columns, and a panel drawn first
        // would have a row's fill painted across it.
        if let Some(panel) = panel {
            // Whether git has a different version of what the panel is about, which is what decides
            // whether a *picture* is offered the diff toggle. A text file answers that from its own
            // payload — the diff came back with it — and a picture cannot: the comparison is what the
            // toggle asks for, so with it off there is nothing in the panel that knows.
            let changed = Self::changed_here(tab);
            crate::ui::preview::show(
                &mut child,
                t,
                panel,
                id,
                &mut tab.preview,
                preview,
                body,
                changed,
                scratch,
                actions,
            );
        }

        // Last of the three, and after the listing for the same reason the preview panel is: the
        // listing paints a row's fill across the whole width of its body, so anything drawn under
        // it comes out with a highlight through it.
        if let Some(console) = console {
            self.console_panel(&mut child, t, id, index, console, list);
        }

        self.panes[index].drop_rows = outcome.drop_rows;
        self.panes[index].drop_area = outcome.drop_area;
        if let Some(path) = outcome.prefetch {
            self.loader.prefetch(&path);
        }

        // Where a drag hovering over this pane would land. Painted *after* the listing: a
        // selected row's fill is an accent surface drawn over anything underneath it, so a
        // highlight drawn first disappeared under the one row most likely to be dropped on.
        if let Some(area) = self.preview_rect(id, ui.ctx().pixels_per_point()) {
            crate::ui::drop_target(ui.painter(), area, t);
        }
    }

    /// Draw a pane's console, and do what it asked for.
    ///
    /// **The shell is started here rather than when the panel is toggled**, and lazily: starting one
    /// needs somewhere to start it, and the folder is the pane's. It is also where a `Shift+Tab`
    /// lands — a different kind means a different process, so the old one is ended and the new one
    /// **keeps the log**, because throwing away what a session printed is not what changing shell
    /// was asked to do.
    #[allow(clippy::too_many_arguments)]
    fn console_panel(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        id: PaneId,
        index: usize,
        rect: Rect,
        body: Rect,
    ) {
        let ctx = ui.ctx().clone();
        let here = self.panes[index].tab().path.clone();
        let want = self.panes[index].console_state.kind();

        let stale = match &self.panes[index].console {
            Some(session) => session.kind != want,
            None => self.panes[index].console_failed != Some(want),
        };
        if stale {
            // What the old session printed, minus anything still running — a block whose sentinel
            // is coming from a shell that no longer exists would never close.
            let kept = self
                .panes[index]
                .console
                .take()
                .map_or_else(Vec::new, |mut old| {
                    let mut blocks = std::mem::take(&mut old.blocks);
                    blocks.retain(|block| !block.running());
                    blocks
                });
            match crate::console::Session::start(want, &here, &ctx) {
                Ok(mut session) => {
                    session.blocks = kept;
                    self.panes[index].console = Some(session);
                    self.panes[index].console_failed = None;
                }
                Err(what) => {
                    self.panes[index].console_failed = Some(want);
                    self.notice = Some(what);
                }
            }
        }
        if let Some(session) = &mut self.panes[index].console {
            session.poll();
            // Whatever `--console` asked for, now that there is something to ask.
            for command in std::mem::take(&mut self.panes[index].console_queue) {
                if let Some(session) = &mut self.panes[index].console {
                    session.send(Some(&here), &command);
                }
            }
        }

        let share = self.console_share;
        let out = {
            let pane = &mut self.panes[index];
            crate::ui::console::show(
                ui,
                t,
                rect,
                id,
                &mut pane.console_state,
                pane.console.as_mut(),
                Some(here.as_path()),
                body,
                share,
            )
        };

        let mut orphan = None;
        if let Some(session) = &mut self.panes[index].console {
            if let Some(command) = &out.send {
                session.send(Some(&here), command);
            }
            if out.stop && session.running() {
                session.stop(&ctx);
            }
            if out.clear {
                session.clear();
            }
        } else {
            // Nothing to run it in, which is only reachable while a shell is refusing to start.
            orphan = out.send.clone();
        }
        if let Some(command) = orphan {
            self.notice = Some(format!("no {} to run `{command}` in", want.label()));
        }
        if out.claimed {
            self.actions.push(Action::Focus(id));
        }
        if let Some(text) = out.copy {
            ctx.copy_text(text);
        }
        if let Some(share) = out.share {
            self.console_share = share;
            self.config_dirty = true;
        }
        // Whichever shell was just picked is the one the next console opens on, in this pane or any
        // other, this session or the next.
        if let Some(kind) = out.swap {
            self.console_shell = kind;
            self.config_dirty = true;
        }
        // A command moved the shell, so the pane goes with it. Through the action queue like every
        // other navigation, which is what gives it the history entry and the scan.
        if let Some(path) = out.cwd {
            self.actions.push(Action::Navigate { pane: id, path });
        }
    }

    /// The surface every panel sits on: a plain square fill, and nothing else.
    ///
    /// It was a card — `radius-medium` and a `stroke-subtle` ring — which is the right
    /// treatment for a card floating on canvas and the wrong one for a panel that is part of
    /// the window's structure. Two of them side by side gave every boundary two rings and a
    /// channel of canvas between them, so the sidebar and the panes read as separate windows
    /// that happened to be next to each other.
    ///
    /// The ring is gone rather than kept-and-thinned because the seam does its job: panels are
    /// [`crate::ui::SEAM`] apart, and [`crate::ui::seam`] is already painted behind them, so
    /// the one line between two panels is a line neither of them draws.
    fn surface(&self, ui: &Ui, t: &Theme, rect: Rect) {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::ZERO, t.bg.layer);
    }

    // ---------------------------------------------------------------------
    // Loading
    // ---------------------------------------------------------------------

    /// Hand every finished scan to the tab that asked for it.
    fn collect_scans(&mut self) {
        // Collected first so the loader is not borrowed while the panes are.
        let arrived: Vec<_> = self.loader.drain().collect();
        for loaded in arrived {
            for pane in &mut self.panes {
                for tab in &mut pane.tabs {
                    if tab.awaiting == Some(loaded.token) {
                        tab.apply(loaded.dir.clone());
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------------
    // Git
    // ---------------------------------------------------------------------

    /// Ask git about whatever each pane is showing, and hand the answers back.
    ///
    /// **Only the active tab of each pane**, because only that one is on screen: a window with twelve
    /// tabs open would otherwise start twelve `git status` runs on every navigation, eleven of them
    /// for folders nobody is looking at. A background tab asks when it is switched to, which is the
    /// frame its listing is drawn in.
    ///
    /// Once per view of a folder. The answer is dropped if the tab has moved on — see
    /// [`crate::pane::Tab::view`] — which is the same discipline the scans and the icon lookups use,
    /// and the reason none of this needs cancelling.
    fn collect_git(&mut self, now: f64) {
        for pane in &mut self.panes {
            let tab = pane.tab_mut();
            // A listing has to exist first: git is asked about the folder that is *on screen*, and
            // "This PC" — the synthetic listing with an empty path — is not a folder at all.
            if tab.git_asked || tab.dir.is_none() || tab.path.as_os_str().is_empty() {
                continue;
            }
            tab.git_asked = true;
            let (view, path) = (tab.view, tab.path.clone());
            self.git.request(view, &path);
            self.git_waiting += 1;
        }

        let arrived: Vec<_> = self.git.drain().collect();
        self.git_waiting = self.git_waiting.saturating_sub(arrived.len());
        for answer in arrived {
            for pane in &mut self.panes {
                for tab in &mut pane.tabs {
                    if tab.view == answer.view {
                        tab.git = answer.repo.clone();
                        tab.git_answered = true;
                        // See [`crate::pane::Tab::git_settled_at`]: the read that produced this
                        // answer may have just written the repository's own index, and that write
                        // is what `collect_changes` is about to see arrive under `.git`.
                        tab.git_settled_at = Some(now);
                        // **A filter that asked about git has been waiting for this.** `@git`
                        // cannot be evaluated until the answer is here — see
                        // [`crate::pane::Tab::git_answered`] — so until now the listing has been
                        // showing every row, and this is the frame it narrows in. Only when the filter
                        // asks: nothing else about the order depends on git, and rebuilding it for
                        // every folder in a repository would be a sort per navigation for nothing.
                        if tab.filters_on_git() {
                            tab.rebuild_order();
                        }
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------------
    // Previews
    // ---------------------------------------------------------------------

    /// Hand finished reads to the panels that asked, and keep each one pointed at its own
    /// keyboard.
    ///
    /// A panel **follows the selection** rather than being told once: you open it, then arrow
    /// down the folder and look at each file in turn without touching the shortcut again. That is
    /// only bearable because it waits — see [`crate::ui::preview::FOLLOW_DELAY`] — so holding the
    /// arrow key down decodes the file you stop on and not the thirty on the way past.
    ///
    /// **Every open panel is followed, not only the focused one.** Two panes side by side each
    /// showing a build of the same DLL is the case the panel is inside the pane *for*, and a
    /// preview that only tracked the pane with the keyboard would go stale in the other one the
    /// moment you clicked across to compare them.
    fn collect_previews(&mut self, ctx: &egui::Context, now: f64) {
        // Collected first, so the reader is not borrowed while the panels are written to. The
        // payload goes to the one panel waiting for that token and nowhere else: it is a decoded
        // picture or a walked graph, so there is one of it and no copy to hand round.
        for loaded in self.previews.drain().collect::<Vec<_>>() {
            let wanted = self
                .panes
                .iter_mut()
                .flat_map(|pane| pane.tabs.iter_mut())
                .find(|tab| tab.preview.wants(loaded.token));
            if let Some(tab) = wanted {
                tab.preview.arrived(loaded.token, loaded.payload, ctx);
            }
        }

        // What each open panel should be looking at, worked out before anything is borrowed
        // mutably: `selected_preview` reads the tab, and asking for a read writes to it.
        type Wanted = (PaneId, usize, Option<crate::preview::Ask>);
        let wanted: Vec<Wanted> = self
            .panes
            .iter()
            .flat_map(|pane| {
                pane.tabs
                    .iter()
                    .enumerate()
                    .filter(|(at, tab)| tab.preview.open && *at == pane.active)
                    .map(|(at, tab)| (pane.id, at, Self::selected_preview(tab, self.preview.diff)))
                    .collect::<Vec<_>>()
            })
            .collect();

        let mut soonest: Option<f64> = None;
        for (id, at, what) in wanted {
            let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) else {
                continue;
            };
            let Some(tab) = pane.tabs.get_mut(at) else {
                continue;
            };
            tab.preview.follow(what, now);
            let (ready, left) = tab.preview.settle(now);
            if let Some(ask) = ready {
                let token = self.previews.request(&ask);
                if let Some(tab) = self
                    .panes
                    .iter_mut()
                    .find(|p| p.id == id)
                    .and_then(|p| p.tabs.get_mut(at))
                {
                    tab.preview.asked(ask, token);
                }
            }
            if let Some(left) = left {
                soonest = Some(soonest.map_or(left, |soonest: f64| soonest.min(left)));
            }
        }
        // And the frame that would notice the wait is up. Nothing else would ask for it: this
        // program is idle between events, so a deadline nobody books a frame for is a deadline
        // that arrives the next time something unrelated happens to want one. The *soonest* of
        // them, since one frame serves every panel that is waiting.
        if let Some(left) = soonest {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
    }

    /// What this tab's preview panel should be showing, if anything.
    ///
    /// **Two pictures selected at once is a comparison**, and that is the one case where the
    /// *selection* rather than the cursor decides: picking a second image is a deliberate act with
    /// an obvious meaning, and no other pair of files has one. Everything else is the cursor's
    /// answer — the cursor is where the keyboard is, it follows a click as well, and a preview is
    /// about one file, so a selection of thirty has nothing to show.
    ///
    /// **And a picture git has a different version of is a comparison too**, when the panel's diff
    /// toggle is on: a `.png` has no lines to put a red band behind, so what "show me what changed"
    /// means for one is the two versions and the difference between them — which is the view two
    /// selected pictures already get. `diffing` is that toggle, and it is read here rather than in the
    /// panel because it changes *what is read*: turning it off has to be a different [`Ask`], or the
    /// panel would go on showing the comparison it is holding.
    fn selected_preview(tab: &Tab, diffing: bool) -> Option<crate::preview::Ask> {
        use crate::preview::{kind_of, Ask, Kind};

        let dir = tab.dir.as_ref()?;
        let kind_at = |row: usize| -> Option<Kind> {
            let entry = tab.entry_at(row)?;
            // From the name alone, which costs nothing to ask about a row. A file that passes and
            // turns out to be something else says so in the panel rather than being refused here.
            kind_of(
                dir.leaf(entry),
                dir.ext(entry),
                dir.entries[entry].is_dir(),
            )
        };

        if tab.selected_count == 2 {
            // In display order, so the left-hand or upper view is the upper row — the pair reads
            // the way the listing above it does rather than the way the clicks happened to land.
            let two: Vec<usize> = (0..tab.order.len())
                .filter(|&row| tab.is_selected(row))
                .take(3)
                .collect();
            if let [a, b] = two[..] {
                if kind_at(a) == Some(Kind::Picture) && kind_at(b) == Some(Kind::Picture) {
                    return Some(Ask::Pair(tab.target_at(a)?, tab.target_at(b)?));
                }
            }
        }
        let at = tab.cursor?;
        let kind = kind_at(at)?;
        let path = tab.target_at(at)?;
        if diffing && kind == Kind::Picture && Self::changed_here(tab) {
            return Some(Ask::AgainstHead(path));
        }
        Some(Ask::One(path, kind))
    }

    /// Whether git says the row the keyboard is on has a version in the last commit that is not the
    /// one on disk.
    ///
    /// Read off the marks the listing already has — the same answer the row's own badge is drawn from,
    /// so the badge and the panel's diff button can never disagree — and so it costs a hash lookup
    /// rather than a git process. `false` for a folder outside a repository, for a listing that has not
    /// landed, and while the answer from git is still on its way.
    fn changed_here(tab: &Tab) -> bool {
        let (Some(repo), Some(dir), Some(at)) = (tab.git.as_ref(), tab.dir.as_ref(), tab.cursor)
        else {
            return false;
        };
        tab.entry_at(at)
            .and_then(|entry| repo.state(dir.name(entry)))
            .is_some_and(crate::git::State::differs_from_head)
    }

    /// Put the selection on the clipboard, as a cut or as a copy.
    fn put_on_clipboard(&mut self, pane: PaneId, cutting: bool) {
        let paths = self
            .pane_mut(pane)
            .map(|p| p.tab().selection_paths())
            .unwrap_or_default();
        self.put_these_on_clipboard(paths, cutting);
    }

    /// The same, on paths named outright.
    ///
    /// Split out from [`App::put_on_clipboard`] because the context menu's `cut` and `copy` are
    /// redirected into it — see [`App::ours_rather_than_the_shell_s`] — and the menu carries the
    /// items it was raised over rather than reading them back off the pane. One implementation, so
    /// that Ctrl+X and the menu's Couper cannot come to mean two different things.
    fn put_these_on_clipboard(&mut self, paths: Vec<PathBuf>, cutting: bool) {
        use crate::shell::clipboard::{put, Effect};

        if paths.is_empty() {
            self.notice = Some("Nothing selected".to_owned());
            return;
        }
        let effect = if cutting { Effect::Move } else { Effect::Copy };
        match put(&paths, effect) {
            // A cut marks its sources rather than moving them — nothing moves until
            // something pastes — so until then they have to *look* pending.
            Ok(()) => {
                self.cut = if cutting { paths } else { Vec::new() };
                self.notice = None;
            }
            Err(why) => self.notice = Some(why),
        }
    }

    /// Act on whatever is on the clipboard, into this pane's folder.
    fn paste_into(&mut self, pane: PaneId, ctx: &egui::Context) {
        let Some(into) = self.pane_mut(pane).map(|p| p.tab().path.clone()) else {
            return;
        };
        self.paste_into_folder(into, ctx);
    }

    /// The same, into a folder named outright.
    ///
    /// Split out from [`App::paste_into`] because the context menu's `paste` is redirected into it
    /// — see [`App::ours_rather_than_the_shell_s`] — and that entry means "into the folder the
    /// menu was raised over", which is a *selected* folder and not the one the pane is showing.
    fn paste_into_folder(&mut self, into: PathBuf, ctx: &egui::Context) {
        use crate::shell::clipboard::{self, Effect};
        use crate::shell::ops::Job;

        if into.as_os_str().is_empty() {
            self.notice = Some("This PC is not a folder to paste into".to_owned());
            return;
        }
        let Some(pasteable) = clipboard::get() else {
            self.notice = Some("There are no files on the clipboard".to_owned());
            return;
        };

        let moving = pasteable.effect == Effect::Move;
        let items = pasteable.items;
        // A cut is finished when the move is: `After::FinishCut` tells the clipboard's owner
        // it worked and then empties it, once the shell says it did. Emptying it here instead
        // -- which is what this used to do -- loses the cut for anyone who answers the
        // conflict dialog with Cancel, and leaves the sources still faded with nothing on the
        // clipboard to paste them from.
        let (job, after) = if moving {
            (
                Job::Move { items, into },
                crate::shell::ops::After::FinishCut(clipboard::sequence()),
            )
        } else {
            (Job::Copy { items, into }, crate::shell::ops::After::Nothing)
        };
        self.notice = None;
        self.ops.start_then(job, after, self.owner, ctx);
    }

    /// Raise the context menu: ask the shell for it, and open it when the answer comes.
    ///
    /// The shell's entries take 130 ms for a folder and up to most of a second for a file --
    /// every time, not just the first -- so they are fetched by
    /// [`crate::shell::menu::Builder`] on a thread of its own. Nothing is drawn in the
    /// meantime: the menu appears once, whole, at its final size. The window keeps running
    /// frames throughout, which is the part that used to be missing.
    fn shell_menu(
        &mut self,
        pane: PaneId,
        items: Vec<PathBuf>,
        at: (i32, i32),
        ctx: &egui::Context,
    ) {
        let Some(p) = self.panes.iter().find(|p| p.id == pane) else {
            return;
        };
        let folder = p.tab().path.clone();
        if folder.as_os_str().is_empty() {
            // This PC is a list of volumes, not a directory; the shell has no menu for it
            // that would mean anything here.
            return;
        }

        // Whatever was open, or on its way, is not what was asked for.
        self.close_menu();
        let scale = ctx.pixels_per_point();
        let at = egui::pos2(at.0 as f32 / scale, at.1 as f32 / scale);
        let depth = Self::menu_depth(&folder, &items);
        self.asking = Some(Asking {
            token: self.menu_builder.build(&folder, &items, depth),
            pane,
            at,
            items,
            folder,
            depth,
            since: ctx.cumulative_pass_nr(),
            asked: std::time::Instant::now(),
        });
    }

    /// How much of a menu to ask the shell for.
    ///
    /// The whole cost of a context menu is inside one `QueryContextMenu`, which lets every
    /// installed extension contribute — so there is no such thing as skipping the slow entries
    /// once they exist. They cost what they cost before this program sees any of them. The only
    /// choice is how much to ask for, and [`crate::shell::menu::Depth`] has the measurements.
    ///
    /// So: an executable image on a network drive gets the reduced menu, and everything else
    /// gets everything. That is a narrow rule and it is the one the evidence supports. What is
    /// slow is not "the network" — a 41 MB `.lib` on the same share builds a full menu in 1.8 s,
    /// and the folder itself in 0.2 s — it is an executable on a share, where the time is linear
    /// in the file's size at about 570 kB/s because something reads all of it. A blanket rule for
    /// network paths would throw away 7-Zip and Send To on every file on the share to fix a
    /// problem that only executables have.
    ///
    /// It is not the last word either: [`App::pump_asking`] downgrades anything on a network
    /// drive that turns out to be slow regardless of what it is called, which is what covers the
    /// file types this list has not heard of.
    fn menu_depth(folder: &Path, items: &[PathBuf]) -> crate::shell::menu::Depth {
        use crate::shell::menu::Depth;
        // What Windows loads as an executable image, which is the set something is entitled to
        // inspect byte by byte before deciding what to offer.
        const IMAGES: [&str; 8] = ["exe", "com", "scr", "dll", "ocx", "sys", "cpl", "drv"];

        let any_image = items.iter().any(|item| {
            item.extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .is_some_and(|ext| IMAGES.contains(&ext.as_str()))
        });
        if any_image && crate::shell::over_network(folder) {
            Depth::Fast
        } else {
            Depth::Full
        }
    }

    /// While the shell is still being asked, say so — and take Escape or a click as "never mind".
    ///
    /// Usually there is nothing to say: a folder's menu comes back in a tenth of a second and
    /// this is over before a frame has been drawn. The case it is here for is the one measured
    /// in [`crate::shell::menu::Builder`] — most of half a minute for an executable on a share
    /// — where a window that shows nothing at all is indistinguishable from a window that has
    /// stopped working, and where a menu that finally appears long after the click has been
    /// forgotten is worse than no menu.
    ///
    /// Cancelling does not stop `QueryContextMenu`, because nothing stops `QueryContextMenu`.
    /// It stops *waiting* for it: the worker is abandoned and its answer will be thrown away.
    fn pump_asking(&mut self, ctx: &egui::Context) {
        let Some(asking) = &self.asking else { return };
        // Not on the pass that asked. The right click that opens a menu is a press, and this
        // would take it as the cancellation of the menu it just asked for.
        let settled = ctx.cumulative_pass_nr() > asking.since;
        let quit = settled
            && ctx.input(|i| {
                i.key_pressed(egui::Key::Escape) || i.pointer.any_pressed() || i.pointer.any_click()
            });
        if quit {
            self.close_menu();
            return;
        }

        // A full menu on a network drive that has not arrived by now is not going to arrive
        // soon: the fast measurement on that share was 1.8 s for a 41 MB file, so anything past
        // this is an extension inspecting the file rather than the share being busy. Ask again
        // for less. Network only — a local menu is 0.13-0.69 s and should never be quietly
        // reduced because the machine happened to be busy for a moment.
        //
        // A selection only. The empty-selection menu is the folder's *background* menu, and
        // `CMF_DEFAULTONLY` there asks for the default verb of a thing that has none: what comes
        // back is a couple of entries with New — the only reason to open that menu — not among
        // them. There is nothing to reduce anyway, since the slow case this covers is an
        // extension reading a selected file.
        const PATIENCE: std::time::Duration = std::time::Duration::from_millis(2_500);
        if asking.depth == crate::shell::menu::Depth::Full
            && !asking.items.is_empty()
            && asking.asked.elapsed() > PATIENCE
            && crate::shell::over_network(&asking.folder)
        {
            let (pane, at, items, folder) = (
                asking.pane,
                asking.at,
                asking.items.clone(),
                asking.folder.clone(),
            );
            self.close_menu();
            self.asking = Some(Asking {
                token: self
                    .menu_builder
                    .build(&folder, &items, crate::shell::menu::Depth::Fast),
                pane,
                at,
                items,
                folder,
                depth: crate::shell::menu::Depth::Fast,
                since: ctx.cumulative_pass_nr(),
                asked: std::time::Instant::now(),
            });
        }

        ctx.set_cursor_icon(egui::CursorIcon::Progress);
        // The answer arrives on a channel and the worker asks for a repaint when it has one, so
        // this is not how the menu gets drawn. It is here so that the cursor is re-asserted and
        // the window demonstrably keeps drawing while the shell takes its time — at ten frames
        // a second rather than as fast as possible, since there is nothing to animate.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// Take delivery of whatever the menu builder has finished, and pass on what the menu
    /// on screen has since asked for.
    fn pump_menu(&mut self) {
        use crate::shell::menu::Said;

        while let Some(said) = self.menu_builder.poll() {
            // A menu the user has dismissed, or replaced with a second right click, still
            // has an answer coming. The token is how it is told apart from the live one, and
            // an answer that does not match is dropped.
            match said {
                Said::Built {
                    token,
                    entries,
                    depth,
                } => {
                    let Some(asking) = self.asking.take_if(|a| a.token == token) else {
                        continue;
                    };
                    // A short menu is worth admitting to. Somebody who right-clicks an
                    // executable on a share and finds 7-Zip missing should be told why rather
                    // than left to wonder whether the program is broken.
                    // Short enough for the status line to show all of it — the first version of
                    // this was cut off at "and thi…", which tells nobody anything.
                    if depth == crate::shell::menu::Depth::Fast {
                        self.notice = Some(
                            "Short menu: Windows' extras read the whole file over the network"
                                .to_owned(),
                        );
                    }
                    // An empty selection is the folder's *background* menu, and that is the one
                    // menu the shell hands over with a gap in it: it carries no Paste, and
                    // Explorer's own is synthesised by its view rather than read out of the
                    // shell. So this program's goes in. See `crate::shell::menu::Own::Paste`.
                    let entries = if asking.items.is_empty() {
                        crate::shell::menu::with_our_paste(
                            entries,
                            crate::shell::clipboard::has_files(),
                        )
                    } else {
                        entries
                    };
                    self.menu = Some(crate::ui::menu::Open::new(
                        asking.pane,
                        asking.at,
                        asking.items,
                        asking.folder,
                        entries,
                        depth,
                        token,
                    ));
                }
                Said::Filled { token, id, children } => {
                    if let Some(menu) = self.menu.as_mut().filter(|m| m.token == token) {
                        menu.filled(id, children);
                    }
                }
            }
        }

        if let Some(menu) = self.menu.as_mut() {
            let token = menu.token;
            for id in std::mem::take(&mut menu.fills) {
                self.menu_builder.fill(token, id);
            }
        }
    }

    /// Whether a menu has been asked for and has not appeared yet.
    ///
    /// For `--shot --menu`, which would otherwise photograph the window without one.
    pub fn menu_pending(&self) -> bool {
        self.asking.is_some()
    }

    /// Let go of the menu that is closing, and of the one on its way if there is one.
    pub fn close_menu(&mut self) {
        if let Some(menu) = self.menu.take() {
            self.menu_builder.close(menu.token);
        }
        if self.asking.take().is_some() {
            // Nothing to close — the shell has not finished making it. The worker is let go of
            // instead, so that the next menu is built on a thread that is not inside a call
            // that may have twenty seconds left to run.
            self.menu_builder.abandon();
        }
    }

    /// Put every popup away when the window stops being the one you are using.
    ///
    /// A menu belongs to a moment. Alt-tab to something else and come back ten minutes later and
    /// a context menu still standing over the listing is not where you left off — it is a menu
    /// about a file you have stopped thinking about, over a window you have to click twice to get
    /// back into. Windows itself dismisses a menu when its owner loses activation, and this window
    /// has three kinds of its own to dismiss: the context menu, the popups egui tracks in its
    /// memory — the application menu under the mark at the top left — and the path bar's
    /// dropdowns with the tracking mode they turn on.
    ///
    /// Every unfocused frame rather than only the one where focus was lost: the transition needs a
    /// frame to be noticed in, an unfocused window is not always given one at the moment it goes,
    /// and asking "is anything open while we are not in front" has the same answer either way. It
    /// is also cheap — the three are already empty every other time this runs.
    ///
    /// `focused` is the *window's*, not egui's: a focused text field is a different thing entirely
    /// and closing a menu because the filter box has the caret would be a bug. `RawInput::focused`
    /// defaults to `true`, so a frame from an integration that does not track focus never trips
    /// this.
    fn close_on_blur(&mut self, ctx: &egui::Context) {
        // The design system puts egui's own popups away and reports whether it fired, so this
        // window's two hand-tracked ones — the application menu and the breadcrumb's dropdown,
        // neither of which is an `egui::Popup` — go with them in the same breath.
        if azur_egui_theme::desktop::close_popups_on_blur(ctx) {
            self.close_menu();
            self.crumbs.close();
        }
    }

    /// Keep a drag in flight painting, and clear up after it once it has landed.
    ///
    /// The drag runs on its own thread — see [`crate::shell::dnd::Drag`] — and while it does,
    /// nothing in egui's own event flow is happening: the pointer belongs to OLE, so no mouse
    /// event reaches winit and nothing would ask for a frame. Without a frame the drop
    /// highlight never appears and the selection the drag just made is never drawn. So the
    /// repaint is asked for unconditionally for the length of the drag, which is the one case
    /// in this program where painting is driven by a state rather than by an event.
    fn pump_drag(&mut self, ctx: &egui::Context) {
        let Some((pane, drag)) = &self.file_drag else {
            return;
        };
        let Some(effect) = drag.finished() else {
            ctx.request_repaint();
            return;
        };
        let pane = *pane;
        self.file_drag = None;
        self.release_buttons(ctx);
        // A move took files out of this folder, and OLE does not say which — so the folder
        // is re-read. A copy changed nothing here.
        if effect == Some(crate::shell::clipboard::Effect::Move) {
            self.perform(ctx, Action::Refresh(pane));
        }
        ctx.request_repaint();
    }

    /// Tell egui the button came up, because nothing else is going to.
    ///
    /// **This is why a second drag did nothing.** `DoDragDrop` takes the mouse capture for the
    /// length of the drag and its own loop consumes the button-up that ends it, so the window
    /// never sees the release: egui goes on believing the button is held, and a press that
    /// arrives while a button is already down starts no new drag. One drag per window, and then
    /// nothing — until some unrelated click happened to put the state right, which is why it
    /// looked intermittent and why every test that ran a single drag in a fresh window passed.
    ///
    /// It cannot be fixed by watching for the release: it is never delivered here. So the
    /// release is stated rather than awaited, for whichever buttons egui still thinks are down,
    /// at the position egui already has — moving it would be inventing a gesture rather than
    /// finishing one.
    fn release_buttons(&mut self, ctx: &egui::Context) {
        let (pos, modifiers, down) = ctx.input(|i| {
            (
                i.pointer.latest_pos(),
                i.modifiers,
                [
                    egui::PointerButton::Primary,
                    egui::PointerButton::Secondary,
                ]
                .into_iter()
                .filter(|button| i.pointer.button_down(*button))
                .collect::<Vec<_>>(),
            )
        });
        let Some(pos) = pos else { return };
        for button in down {
            self.injected.push(egui::Event::PointerButton {
                pos,
                button,
                pressed: false,
                modifiers,
            });
        }
    }

    /// Events egui has to be told about because the platform could not deliver them.
    ///
    /// Drained by the input hook, which is the only place raw input can be added to.
    pub fn take_injected(&mut self) -> Vec<egui::Event> {
        std::mem::take(&mut self.injected)
    }

    /// Hand each per-file icon answer to the view that asked for it, and drop the rest.
    ///
    /// This is where the folder-scoped rule is enforced. An answer names the view it belongs
    /// to; if no tab still holds that view — because it moved on, or was closed, or the
    /// folder was refreshed — the answer is discarded here and nothing anywhere remembers the
    /// file it was about. Nothing is keyed by path, so nothing outlives the folder.
    fn deliver_icons(&mut self) {
        let answers = self.icons.answers();
        if answers.is_empty() {
            return;
        }
        for (view, row, index) in answers {
            let Some(tab) = self
                .panes
                .iter_mut()
                .flat_map(|pane| pane.tabs.iter_mut())
                .find(|tab| tab.view == view)
            else {
                continue;
            };
            if let Some(slot) = tab.file_icons.get_mut(row as usize) {
                *slot = index;
            }
        }
    }

    /// The same, for what each shortcut row points at. See [`crate::shell::links`].
    ///
    /// `None` is stored rather than skipped: it means the shortcut was read and had nothing to
    /// show, and storing it is what stops the row asking about it again on every frame.
    fn deliver_links(&mut self) {
        let answers = self.links.answers();
        if answers.is_empty() {
            return;
        }
        for (view, row, target) in answers {
            let Some(tab) = self
                .panes
                .iter_mut()
                .flat_map(|pane| pane.tabs.iter_mut())
                .find(|tab| tab.view == view)
            else {
                continue;
            };
            // Only if the row is still asking. A refresh clears the map, and an answer that
            // arrives after that would otherwise put back a row's context for a listing the
            // entry indices no longer belong to.
            if let Some(slot) = tab.links.get_mut(&row) {
                *slot = target;
            }
        }
    }

    /// Raise the focused pane's folder menu, over its listing.
    ///
    /// For `--menu`, which is how a capture run gets a menu on screen: it has no pointer
    /// to right-click with, and the menu is the one part of the window a screenshot cannot
    /// otherwise reach. Goes through the same action as a real right click, so what it
    /// captures is the real menu and not a mock-up of one.
    pub fn open_folder_menu(&mut self, ctx: &egui::Context) {
        let Some(pane) = self.panes.iter().find(|p| p.id == self.focused) else {
            return;
        };
        let rect = pane.rect;
        let scale = ctx.pixels_per_point();
        let at = rect.min + egui::vec2(rect.width() * 0.22, rect.height() * 0.30);
        self.actions.push(Action::ShellMenu {
            pane: pane.id,
            items: Vec::new(),
            at: ((at.x * scale) as i32, (at.y * scale) as i32),
        });
    }

    /// Say something in the status line: the one place this window has to tell the user
    /// anything, and where a failed file operation already goes.
    ///
    /// Used for the graphics device, which is not this program's to fix but very much its job
    /// to admit to: a window that cannot present a frame goes on taking input and showing the
    /// last thing it drew, and without a word from it that is indistinguishable from a hang.
    pub fn report(&mut self, what: String) {
        self.notice = Some(what);
    }

    /// Whether the focused pane has a listing with anything in it.
    pub fn has_rows(&self) -> bool {
        self.panes
            .iter()
            .find(|p| p.id == self.focused)
            .is_some_and(|p| !p.tab().order.is_empty())
    }

    /// Select the first file in the focused pane and open its name for editing.
    ///
    /// For `--rename`, which is how a capture run gets the rename field on screen: it has no
    /// keyboard to press F2 with. A file rather than a folder, so there is an extension there
    /// for the caret to leave alone.
    pub fn begin_rename_here(&mut self) {
        let pane = self.focused;
        let Some(p) = self.pane_mut(pane) else { return };
        let tab = p.tab_mut();
        // A file with an extension for preference, since the extension is the part worth
        // photographing: a shot of `Makefile` selected whole says nothing about the rule.
        let named = |at: &usize| {
            tab.entry_at(*at)
                .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
                .is_some_and(|name| std::path::Path::new(&name).extension().is_some())
        };
        let rows = 0..tab.order.len();
        let file = rows
            .clone()
            .find(|at| !tab.is_dir_at(*at) && named(at))
            .or_else(|| rows.clone().find(|at| !tab.is_dir_at(*at)));
        if let Some(at) = file.or_else(|| (!tab.order.is_empty()).then_some(0)) {
            tab.select_only(at);
            tab.begin_rename();
        }
    }

    /// Open the focused pane's console, and queue some commands into it.
    ///
    /// For `--console`, which is how a capture run gets the panel on screen and a log into it: it
    /// has no keyboard to press `Ctrl+²` with and no shell to type at. The commands go in through
    /// the same queue a typed one does — see [`crate::console::Session::send`] — so they take the
    /// pane's folder, close their own blocks and fold themselves if they printed nothing, exactly as
    /// typed ones would.
    pub fn open_console_here(&mut self, commands: &[String]) {
        let pane = self.focused;
        let shell = self.console_shell;
        if let Some(p) = self.pane_mut(pane) {
            p.console_open = true;
            p.console_queue = commands.to_vec();
            p.console_state.take_keys();
            // The remembered shell, the same as `Ctrl+²` gets. Left out at first, which made a
            // capture run the one way of opening this panel that ignored the setting.
            if p.console.is_none() {
                p.console_state.set_kind(shell);
            }
        }
    }

    /// Whether a capture should keep waiting for git: asked, and not yet answered.
    ///
    /// Three processes with git's own startup in each of them, which is far more than the frames
    /// before a capture settles — so without this, a shot of a repository is a shot of a listing with
    /// no branch and no marks on it, which is precisely the thing being photographed. The same
    /// argument as [`App::console_busy`], one feature along.
    ///
    /// A folder that is not a repository answers `None` just as quickly as one that is, so this
    /// stops waiting either way; it is not "wait until there is git".
    pub fn git_pending(&self) -> bool {
        self.git_waiting > 0
    }

    /// Whether a capture should keep waiting for the tiles' pictures.
    ///
    /// The same reason as [`App::git_pending`]: the shell answers in tens of milliseconds and a
    /// capture settles in twelve frames, so without this a screenshot of the large-icon view is a grid
    /// of painted placeholder glyphs — a photograph of the loading state rather than of the view.
    pub fn thumbs_pending(&self) -> bool {
        self.thumbs.pending()
    }

    /// Whether a capture should keep waiting for the console: a shell still to start, or a command
    /// still to answer.
    ///
    /// A shell takes a moment to come up and `git status` takes longer, and neither is anywhere near
    /// the four frames before a capture settles — so without this, `--console` photographs an empty
    /// log every time.
    pub fn console_busy(&self) -> bool {
        self.panes.iter().any(|pane| {
            (pane.console_open && !pane.console_queue.is_empty() && pane.console_failed.is_none())
                || pane
                    .console
                    .as_ref()
                    .is_some_and(crate::console::Session::running)
        })
    }

    /// `--tiles`: show every pane's listing as large icons.
    ///
    /// The same family as `--menu`, `--rename` and `--preview`, and it exists for the same reason each
    /// of those does — **a capture run has no other way to reach this view.** It is behind a click on a
    /// switch, and deliberately not in the settings file: rows or tiles is a question about the folder
    /// in front of you, so there is no `view=` key for a screenshot run to set. See
    /// [`crate::pane::ViewMode`].
    ///
    /// Every pane rather than the focused one, unlike the three above: the flag is for looking at the
    /// view, and a split window with tiles in one half is a capture of the switch rather than of the
    /// grid.
    ///
    /// Through [`Action::SetView`] rather than by assignment, which is what `open_preview_here` does and
    /// for the same reason: switching the view is not only a field, and a flag that wrote the field would
    /// keep missing whatever else it comes to involve.
    pub fn show_tiles_here(&mut self) {
        let panes: Vec<PaneId> = self.panes.iter().map(|pane| pane.id).collect();
        for pane in panes {
            self.actions.push(Action::SetView {
                pane,
                mode: crate::pane::ViewMode::Icons,
            });
        }
    }

    /// Put the keyboard on the first previewable file in the focused pane and open the panel.
    ///
    /// For `--preview`, which is how a capture run gets the panel on screen: it has no keyboard to
    /// press `Ctrl+P` with. Goes through the same action a real key press does, and picks the row
    /// the same way, so what it captures is the real panel over a real read.
    pub fn open_preview_here(&mut self) {
        let pane = self.focused;
        let Some(p) = self.pane_mut(pane) else { return };
        let tab = p.tab_mut();
        let previewable = |tab: &Tab, at: usize| {
            tab.entry_at(at)
                .zip(tab.dir.as_ref())
                .and_then(|(entry, dir)| {
                    crate::preview::kind_of(
                        dir.leaf(entry),
                        dir.ext(entry),
                        dir.entries[entry].is_dir(),
                    )
                })
                .is_some()
        };
        // Whatever `--reveal=` already selected, if that can be previewed — so the two flags
        // compose and a capture can name the file it wants. The first previewable row otherwise.
        let row = tab
            .cursor
            .filter(|&at| previewable(tab, at))
            .or_else(|| (0..tab.order.len()).find(|&at| previewable(tab, at)));
        if let Some(at) = row {
            if tab.selected_count < 2 {
                tab.select_only(at);
            }
            self.actions.push(Action::TogglePreview(pane));
        }
    }

    /// `--find=`: open the text preview's find bar on a query, so a capture can photograph it.
    ///
    /// The same reason [`App::open_preview_here`] and [`App::compare_here`] exist — a screenshot run
    /// has no keyboard to type into it — and it composes with them: `--preview --find=fn` is the
    /// panel, open, with the bar over it and the hits marked.
    /// Open the path field with `text` in it, half-typed.
    ///
    /// For `--path=`, and for the same reason as `--rename`: the completion dropdown is behind
    /// `Ctrl+L` and then a keystroke, and a capture run has no keyboard to press either with. The
    /// text is put in with the *last character typed* rather than merely set, because the dropdown
    /// stays down for a path that has only been shown — see
    /// [`crate::ui::breadcrumb::PathComplete`] — and a capture of the field with nothing under it
    /// would be a capture of what was already there before any of this.
    pub fn type_path_here(&mut self, text: &str) {
        let pane = self.focused;
        let Some(p) = self.pane_mut(pane) else { return };
        let tab = p.tab_mut();
        tab.editing_path = true;
        tab.edit_text = text.to_owned();
        self.complete.type_ahead(pane);
    }

    pub fn find_here(&mut self, text: &str) {
        let pane = self.focused;
        if let Some(p) = self.pane_mut(pane) {
            p.tab_mut().preview.look_for(text);
        }
    }

    /// Select the *second* previewable file as well, so `--shot --preview --compare` photographs a
    /// comparison rather than one picture.
    ///
    /// For the same reason [`App::open_preview_here`] exists: two selected rows is a mouse gesture
    /// and a capture run has no mouse.
    pub fn compare_here(&mut self) {
        let pane = self.focused;
        let Some(p) = self.pane_mut(pane) else { return };
        let tab = p.tab_mut();
        let pictures: Vec<usize> = (0..tab.order.len())
            .filter(|&at| {
                tab.entry_at(at)
                    .zip(tab.dir.as_ref())
                    .and_then(|(entry, dir)| {
                        crate::preview::kind_of(
                            dir.leaf(entry),
                            dir.ext(entry),
                            dir.entries[entry].is_dir(),
                        )
                    })
                    == Some(crate::preview::Kind::Picture)
            })
            .take(2)
            .collect();
        if let [a, b] = pictures[..] {
            tab.select_only(a);
            tab.toggle(b);
        }
    }

    /// Whether any preview has been asked for and has not come back yet.
    ///
    /// For `--shot --preview`, which would otherwise photograph the word `Reading…`: a decode or a
    /// dependency walk takes tens of milliseconds, which is several frames.
    pub fn preview_pending(&self) -> bool {
        self.panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .any(|tab| tab.preview.busy())
    }

    /// Draw the context menu, if one is open, and act on what it says.
    fn draw_menu(&mut self, ui: &mut Ui, theme: &Theme) {
        use crate::shell::menu::Command;
        use crate::ui::menu::Outcome;

        // Unconditionally, and before the early return: this is what *opens* the menu, so a
        // version that skipped it while there was nothing on screen would wait for ever for
        // a menu it never took delivery of. Cheap when there is nothing to collect — one
        // `try_recv` that fails.
        self.pump_menu();

        let outcome = match &mut self.menu {
            Some(menu) => crate::ui::menu::show(ui, theme, menu),
            None => return,
        };
        // A hover during this pass may have asked for a submenu; send it now rather than
        // waiting a frame for the next pump.
        self.pump_menu();

        match outcome {
            Outcome::Open => {}
            Outcome::Closed => self.close_menu(),
            Outcome::Chose(command) => {
                let Some(menu) = self.menu.take() else { return };
                self.menu_builder.close(menu.token);
                match command {
                    Command::Own(which) => {
                        if let Some(action) = self.own_menu_action(&menu, which) {
                            self.actions.push(action);
                        }
                    }
                    // Off to the modal thread: invoking can open anything from a
                    // Properties sheet to an installer, and neither belongs in a frame.
                    // Nothing is remembered about which pane asked: whatever the command does
                    // to the folder, `crate::watch` is what notices. See `collect_modal`.
                    //
                    // Unless it is one of the handful this program answers itself — see
                    // `ours_rather_than_the_shell_s`.
                    Command::Shell { .. } => {
                        let ours = Self::ours_rather_than_the_shell_s(&menu, &command);
                        if !ours.is_empty() {
                            self.actions.extend(ours);
                            return;
                        }
                        // With one exception, and it is written down *here* because here is the
                        // last moment it can be.
                        self.watch_for_a_new_item(menu.pane, &menu.folder, &command);
                        self.modal.send(crate::shell::Request::Invoke {
                            parent: menu.folder.clone(),
                            items: menu.items.clone(),
                            command,
                            depth: menu.depth,
                            owner: self.owner,
                        });
                    }
                }
            }
        }
    }

    /// The entries in Windows' own menu that this program answers itself.
    ///
    /// The context menu is the shell's — every entry in it is there because the shell or an
    /// installed extension put it there, under whatever name this Windows is in. For nearly all of
    /// them, handing the verb straight back is exactly right: that is the whole point of showing
    /// the real menu rather than an imitation of it. A handful are different, because what they are
    /// *for* is the file manager in front of the user, and that one is this one:
    ///
    /// | verb | what Windows would do | what happens instead |
    /// | --- | --- | --- |
    /// | `open` on a folder | opens it in a new **Explorer window** | navigates this pane |
    /// | `cut`, `copy` | fills the clipboard, and nothing here knows | [`App::put_these_on_clipboard`] |
    /// | `paste` on a folder | the shell's own copy, with no notice and no undo of ours | [`App::paste_into_folder`] |
    /// | `pintohome` | Explorer's Quick access | this program's bookmarks — see [`App::pin_is_a_bookmark`] |
    ///
    /// Recognised by verb in every case, never by label. `GetCommandString` gives the canonical
    /// name, which is the same on every Windows; the labels on this machine are `Ouvrir`, `Couper`,
    /// `Copier` and `Coller`, and matching those would be a program that works in French.
    /// `the_verbs_this_program_takes_over_are_still_the_shell_s` is what holds the four names to
    /// what the shell actually offers, because a hook keyed on a verb that has been renamed does
    /// not fail — it silently stops intercepting.
    ///
    /// Empty means "not ours, give it to the shell", which is the answer for all but four verbs
    /// and also for the cases below where a verb *is* one of the four and there is nothing here to
    /// do with it.
    ///
    /// # What is deliberately not redirected
    ///
    /// **`open` on files.** Only folders are taken over. The shell's `open` on a file is the
    /// registered default verb with everything that comes with it, and a file is not a place this
    /// program can show, so there is nothing to gain and a working Open to lose.
    ///
    /// **Paste on empty space.** There is no `paste` verb on a folder's *background* menu to
    /// redirect: that menu is the view object's, and Explorer synthesises its own Paste around it
    /// rather than reading one out of the shell — see `shell::menu::win::context_of`. So pasting
    /// into the folder you are looking at is Ctrl+V, as it already was. The `paste` this hooks is
    /// the one on a **selected folder**, which means "into that folder", and the shell offers it
    /// only while there is something on the clipboard.
    fn ours_rather_than_the_shell_s(
        menu: &crate::ui::menu::Open,
        command: &crate::shell::menu::Command,
    ) -> Vec<Action> {
        let crate::shell::menu::Command::Shell { verb: Some(verb), .. } = command else {
            return Vec::new();
        };
        match verb.as_str() {
            "pintohome" | "unpinfromhome" => Self::pin_is_a_bookmark(menu, command),
            "open" => Self::open_in_this_explorer(menu),
            // The selection the menu was raised over, not the pane's — see
            // [`App::put_these_on_clipboard`]. Empty is the background menu, which has neither
            // entry on it; the guard is here so that a shell that grew one would fall through to
            // it rather than quietly clearing the clipboard.
            "cut" if !menu.items.is_empty() => vec![Action::CutItems(menu.items.clone())],
            "copy" if !menu.items.is_empty() => vec![Action::CopyItems(menu.items.clone())],
            // Into the selected folder. The shell offers this on any selection with a folder
            // somewhere in it, including several at once — where Explorer's own answer is not
            // something to reproduce by accident. The first folder is the one, and the rest of
            // the selection is left alone: pasting into one folder is undone by hand, and
            // pasting into four is not.
            "paste" => menu
                .items
                .iter()
                .find(|item| item.is_dir())
                .map(|into| vec![Action::PasteIntoFolder(into.clone())])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// `Open` on a selection with a folder in it, as this program's own navigation.
    ///
    /// The gesture is one `InvokeCommand` for the whole selection — there is no asking the shell
    /// to open half of it — so this either takes the lot or none of it. It takes the lot as soon as
    /// there is one folder present, because the alternative is a folder opening in a second file
    /// manager over the top of this one, which is the thing being fixed. Any files alongside go
    /// through [`Action::Open`], which is what `Enter` and a double click on a row already do.
    ///
    /// A selection of nothing but files is *not* ours — see the note on
    /// [`App::ours_rather_than_the_shell_s`] — and comes back empty.
    ///
    /// One folder navigates the pane the menu was raised in, which is what double-clicking it
    /// does and what Explorer's own Open does to the window it was invoked from. Several open in
    /// tabs of their own instead, leaving the pane where it is: Explorer answers the same gesture
    /// with one new window each, and a tab is what this program has that a window was for.
    fn open_in_this_explorer(menu: &crate::ui::menu::Open) -> Vec<Action> {
        let places: Vec<(PathBuf, Option<PathBuf>)> = menu
            .items
            .iter()
            .map(|item| (item.clone(), Self::place_of(item)))
            .collect();
        let folders: Vec<PathBuf> = places.iter().filter_map(|(_, place)| place.clone()).collect();
        if folders.is_empty() {
            return Vec::new();
        }

        let mut actions: Vec<Action> = if let [only] = &folders[..] {
            vec![Action::Navigate {
                pane: menu.pane,
                path: only.clone(),
            }]
        } else {
            folders
                .into_iter()
                .map(|path| Action::NavigateNewTab {
                    pane: menu.pane,
                    path,
                })
                .collect()
        };
        actions.extend(
            places
                .into_iter()
                .filter(|(_, place)| place.is_none())
                .map(|(item, _)| Action::Open(item)),
        );
        actions
    }

    /// Where this program could go for an item, if the item is a place at all.
    ///
    /// A directory, or a shortcut to one — the same two things `Enter` on a row treats as somewhere
    /// to go, and for the same reason: a `.lnk` to a folder handed to the shell opens Explorer. A
    /// junction or a directory symlink is a directory here, which is what the listing calls it too.
    fn place_of(item: &Path) -> Option<PathBuf> {
        if item.is_dir() {
            return Some(item.to_path_buf());
        }
        crate::shell::links::folder_target(item)
    }

    /// `Pin to Quick access` means *this* program's bookmarks, not Explorer's Quick access.
    ///
    /// The entry is Windows' own — it is in the menu because the shell put it there, under
    /// whatever name this Windows is in: `Épingler à l'accès rapide` here, `Pin to Quick access`
    /// on an English one. What it is *for* is the sidebar of a file manager, and the sidebar in
    /// front of the user is this one. Handing it to the shell put the folder in Explorer's
    /// Quick access, where nothing in this program can see it, and left this program's own
    /// bookmarks — the same gesture, on Ctrl+D — untouched.
    ///
    /// Recognised by verb, because the label is a translation: `pintohome` is what the shell
    /// calls it on every Windows, and `unpinfromhome` is the other half. Both are folder-only,
    /// which is also what a bookmark is. `pintohomefile` — Windows 11's `Add to Favorites`, for
    /// files — is deliberately *not* here: it is a different list of a different kind of thing,
    /// and a bookmark bar of files is not what this sidebar is.
    ///
    /// Empty means "not ours, give it to the shell".
    fn pin_is_a_bookmark(menu: &crate::ui::menu::Open, command: &crate::shell::menu::Command) -> Vec<Action> {
        let crate::shell::menu::Command::Shell { verb: Some(verb), .. } = command else {
            return Vec::new();
        };
        let add = match verb.as_str() {
            "pintohome" => true,
            "unpinfromhome" => false,
            _ => return Vec::new(),
        };
        // A selection is what is selected; an empty one is the folder the menu was raised in,
        // which is the background menu's answer to "pin what?".
        let mut targets: Vec<PathBuf> = if menu.items.is_empty() {
            vec![menu.folder.clone()]
        } else {
            menu.items.clone()
        };
        // Only folders. The shell offers this on nothing else, but the selection is this
        // program's and a rule that depends on the shell having filtered it is not a rule.
        targets.retain(|path| path.is_dir());
        targets
            .into_iter()
            .map(|path| {
                if add {
                    Action::AddBookmark(path)
                } else {
                    Action::RemoveBookmark(path)
                }
            })
            .collect()
    }

    /// Note the listing before handing over a verb that is about to add to it.
    ///
    /// A `New >` entry creates a file and will not say which, so the row that is in the next
    /// listing and not in this one is the file it made — and that next listing is the watcher's,
    /// arriving on its own a moment later. See [`crate::pane::Tab::name_the_new`] for why the name
    /// cannot simply be asked for, and
    /// [`crate::shell::menu::Command::creates_an_item`] for how the entry is told apart from the
    /// rest of the menu.
    ///
    /// Its own method rather than four lines inside [`App::draw_menu`] so that the test which
    /// drives the whole chain — real verb, real watcher, real re-read — makes the same decision the
    /// menu makes instead of a copy of it that can drift.
    fn watch_for_a_new_item(
        &mut self,
        pane: PaneId,
        folder: &Path,
        command: &crate::shell::menu::Command,
    ) {
        if !command.creates_an_item() {
            return;
        }
        let Some(p) = self.pane_mut(pane) else { return };
        let tab = p.tab_mut();
        // Only while this pane is still showing the folder the menu was raised over. A snapshot of
        // somewhere else would call every row in this folder new.
        if tab.path == *folder {
            tab.name_the_new = Some(tab.names());
        }
    }

    /// What one of this program's own menu entries means.
    ///
    /// Two things ask anything of their own now: a right-button drop, and Paste on empty space.
    /// Everything else that used to be here -- Open, Open in new tab, Open in a pane to the right
    /// or below, Add to bookmarks, Copy path, Refresh, Select all, Show hidden files, New folder,
    /// Open terminal here -- has been taken out of the context menu, which otherwise shows Windows'
    /// menu and nothing else.
    fn own_menu_action(
        &self,
        menu: &crate::ui::menu::Open,
        which: crate::shell::menu::Own,
    ) -> Option<Action> {
        use crate::shell::menu::Own;

        Some(match which {
            // A right-button drag, answered. The menu already carries what was dropped and
            // where, so there is nothing to look up.
            Own::CopyHere | Own::MoveHere => Action::DropHere {
                pane: menu.pane,
                items: menu.items.clone(),
                into: menu.folder.clone(),
                moving: which == Own::MoveHere,
            },
            Own::Cancel => return None,
            // Into the folder the menu was raised in, which for a background menu is the folder
            // being shown. Named outright rather than as [`Action::Paste`], which would read the
            // pane again: the same reasoning as the redirected `paste` verb, and the same action.
            Own::Paste => Action::PasteIntoFolder(menu.folder.clone()),
        })
    }

    /// Apply whatever the modal thread came back with.
    ///
    /// **Nothing, and that is the point.** A shell command can do anything — rename, delete,
    /// extract, commit — and there is no way to be told which, so this used to re-read the folder
    /// on the way out of *every* one of them. Which meant a scan and a rebuilt listing after
    /// `Copy`, after `Properties`, after `Open with`, after `Scan with Defender`: the whole view
    /// thrown away and made again to discover that nothing had changed.
    ///
    /// [`crate::watch`] is what answers this properly, and it is already running. Every folder on
    /// screen has a `ReadDirectoryChangesW` handle on it, so a verb that *did* change something is
    /// noticed within a sixth of a second whoever changed it — this program, Explorer, a terminal,
    /// or the extension the verb belonged to — and a verb that changed nothing costs nothing.
    /// Still drained, because the reply is what tells [`crate::shell::Modal`] the gesture is
    /// over — and while one is in flight this window keeps painting for it.
    fn collect_modal(&mut self) {
        match self.modal.poll() {
            Some(crate::shell::Reply::Invoked) | None => {}
        }
    }

    /// Tell the drop target which pane covers which folder.
    ///
    /// Published every frame because a pane can be split, resized or navigated between
    /// one drag and the next, and the OLE callbacks answer `DragOver` synchronously with
    /// no way to ask.
    fn publish_drop_targets(&self, ctx: &egui::Context) {
        use crate::shell::dnd::{Onto, Targets};

        let scale = ctx.pixels_per_point();
        let physical = |rect: Rect| {
            (
                (rect.left() * scale) as i32,
                (rect.top() * scale) as i32,
                (rect.right() * scale) as i32,
                (rect.bottom() * scale) as i32,
            )
        };

        /// Big enough to drop on and to draw a highlight around.
        fn usable(rect: Rect) -> bool {
            rect.width() >= 1.0 && rect.height() >= 1.0
        }

        let mut zones = Vec::with_capacity(self.panes.len() + 1);
        // The bookmarks group first, so it is *behind* the panes: they cannot overlap, and
        // if a future layout let them, dropping onto a listing should mean the listing.
        if let Some(rect) = self.bookmarks_rect {
            zones.push((physical(rect), Onto::Bookmarks));
        }
        for pane in &self.panes {
            let folder = pane.tab().path.clone();
            // This PC is a list of volumes rather than a directory, so nothing can be
            // dropped into it.
            //
            // The rows' rectangle rather than the pane's: the column header sorts and the
            // status line counts files, and dropping on either of them is not dropping into
            // the folder — so neither should light up saying that it is.
            if folder.as_os_str().is_empty() || !usable(pane.drop_area) {
                continue;
            }
            zones.push((physical(pane.drop_area), Onto::Folder(folder)));
        }
        // The folder rows last, so they win: `Targets::at` takes the last match, and dropping
        // onto a folder has to mean *into that folder*. Dropping anywhere else in the listing
        // still means the folder being shown, which is what the zone above is for.
        for pane in &self.panes {
            for (row, folder) in &pane.drop_rows {
                let row = row.intersect(pane.drop_area);
                if usable(row) {
                    zones.push((physical(row), Onto::Folder(folder.clone())));
                }
            }
        }
        self.drops.publish(Targets { zones });
    }

    /// Which of these items a drop into `into` can actually act on.
    ///
    /// Dropping a folder into itself is meaningless whatever button carried it, and the shell would
    /// refuse it noisily. A file dropped back into the folder it is already in is meaningless too
    /// *for a left drag* — it is a move to where it already is — but not for a right one:
    /// right-dragging a file onto its own folder is how Explorer is asked for a copy of it, and the
    /// answer is `one - Copy.txt`. Filtering those out before the question was asked meant a right
    /// drag inside a folder did nothing at all, which is the most obvious way to try the gesture.
    fn droppable(items: Vec<PathBuf>, into: &Path, asked: bool) -> Vec<PathBuf> {
        items
            .into_iter()
            .filter(|item| item != into)
            .filter(|item| asked || item.parent() != Some(into))
            .collect()
    }

    /// What the drop highlight should cover in this pane, if anything.
    ///
    /// The folder row under the pointer when there is one, and the listing otherwise — which
    /// mirrors where the drop will actually go, since a row is published as a drop zone of its own
    /// and wins over the listing it sits in. A listing-wide highlight over a subfolder would
    /// promise the wrong destination.
    fn preview_rect(&self, pane: PaneId, scale: f32) -> Option<Rect> {
        let (x, y) = self.drop_hover?;
        let pane = self.panes.iter().find(|p| p.id == pane)?;
        if pane.tab().path.as_os_str().is_empty() {
            return None;
        }
        // The hover point arrives in physical pixels, as the drop zones are published.
        let scale = scale.max(0.01);
        let at = egui::pos2(x as f32 / scale, y as f32 / scale);
        if !pane.drop_area.contains(at) {
            return None;
        }
        Some(
            pane.drop_rows
                .iter()
                .find(|(row, _)| row.contains(at))
                .map(|(row, _)| row.intersect(pane.drop_area))
                .unwrap_or(pane.drop_area),
        )
    }

    /// [`App::preview_rect`], for the test that checks which rect it picks.
    #[cfg(test)]
    pub fn preview_rect_for_tests(&self, pane: PaneId, scale: f32) -> Option<Rect> {
        self.preview_rect(pane, scale)
    }

    /// The Bookmarks group, when a drag is over it.
    ///
    /// The whole group and not a row within it: pinning appends, so there is no position to
    /// promise — which is also why the pointer is answered `LINK` rather than copy or move.
    fn bookmarks_preview(&self, scale: f32) -> Option<Rect> {
        let (x, y) = self.drop_hover?;
        let rect = self.bookmarks_rect?;
        let at = egui::pos2(x as f32 / scale.max(0.01), y as f32 / scale.max(0.01));
        rect.contains(at).then_some(rect)
    }

    /// Watch the folders on screen, and re-read any that changed underneath us.
    ///
    /// A listing used to be only as fresh as the last thing *this* program did to it. Anything
    /// anybody else did went unseen: a file dragged out to Explorer stayed on screen, because
    /// Explorer performs the move after our drag has finished and there was nothing to wait on;
    /// a build writing into the folder showed it as it had been; a file deleted from a terminal
    /// left a row behind. And a stale row is worse than wrong — dragging one cannot start a
    /// drag, so the window looked like it had stopped responding.
    fn collect_changes(&mut self, ctx: &egui::Context) {
        let mut folders: Vec<PathBuf> = self
            .panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .map(|tab| tab.path.clone())
            .collect();
        // **And every `.git` behind a folder on screen.** A commit, a pull, a branch switch or a
        // rebase changes what git says about a folder without changing the folder, so watching the
        // folder alone would leave the marks describing the last read and nothing to disprove them.
        // See [`crate::git::Repo::dot_git`].
        folders.extend(
            self.panes
                .iter()
                .flat_map(|pane| pane.tabs.iter())
                .filter_map(|tab| tab.git.as_ref())
                .map(|repo| repo.dot_git.clone()),
        );
        folders.dedup();
        self.watch.keep(&folders);

        // **Our own git write, echoing back as a change to notice.** `git::read` can write the
        // repository's index as a side effect of the very question we just asked it — see its own
        // doc — and that write is a rename of `index.lock` to `index`, directly inside `.git`. Two
        // watches can see it: the one on `.git` itself, which is meant to notice a commit made
        // elsewhere, and — because `ReadDirectoryChangesW` reports a direct child's own metadata
        // changing even without watching subtrees — the one on the *folder* too, since `.git` is a
        // direct child of it. Both read this exactly as they would read a real external change,
        // because the watch does not look at which file changed, by design.
        //
        // The second one is the dangerous one: a folder's own "changed" re-reads its listing, and
        // that re-read is what resets `git_asked` — see [`crate::pane::Tab::apply`]. So the folder's
        // watch answers our own write by asking git again, which writes again, which the watch reads
        // again — a folder that spawns `git status` under itself forever, on its own, whether or not
        // the window is even being looked at. This is the loop [`GIT_WRITE_SETTLE`] exists to break.
        let now = ctx.input(|i| i.time);
        for path in self.watch.changed(now) {
            // A change under `.git` asks git again and leaves the listing alone: the working tree
            // did not move, so re-reading the folder would be a scan for nothing.
            if path.file_name().is_some_and(|name| name == ".git") {
                for tab in self.panes.iter_mut().flat_map(|pane| pane.tabs.iter_mut()) {
                    if tab.git.as_ref().is_some_and(|repo| repo.dot_git == path)
                        && now - tab.git_settled_at.unwrap_or(f64::NEG_INFINITY) > GIT_WRITE_SETTLE
                    {
                        tab.git_asked = false;
                    }
                }
                continue;
            }
            // **Except when a change to this folder is the thing being waited for.** A file the
            // user has just asked the shell to make is not our own git write coming back, and the
            // window the filter below suppresses is two whole seconds — long enough that, in any
            // folder git has something to say about, `New >` never appeared at all until something
            // else happened to touch the folder. Measured: git answered at 1.23 s and the file
            // landed before 2.57 s, so every single one was swallowed.
            //
            // It cannot restart the loop the filter is here to break, because the snapshot is
            // consumed by the very re-read this lets through — one extra read, once, and then the
            // filter applies again as before. See [`crate::pane::Tab::name_the_new`].
            let expected = self
                .panes
                .iter()
                .flat_map(|pane| pane.tabs.iter())
                .any(|tab| tab.path == path && tab.name_the_new.is_some());
            // The folder itself, echoing the same write back through its own watch. A real change to
            // this folder's own contents landing in the same short window is missed rather than
            // acted on immediately — recoverable by `F5`, or by the next thing that touches it — which
            // is the cheaper mistake next to a loop that never stops on its own.
            let echo = !expected
                && self.panes.iter().flat_map(|pane| pane.tabs.iter()).any(|tab| {
                    tab.path == path
                        && now - tab.git_settled_at.unwrap_or(f64::NEG_INFINITY) <= GIT_WRITE_SETTLE
                });
            if echo {
                continue;
            }
            self.folder_changed(&path);
        }
        // A change inside its settle window is a frame that has to come back for it, and there
        // is no input on the way to bring one.
        if self.watch.waiting() {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
    }

    /// Re-read a folder that changed on disk, without blanking what is on screen.
    ///
    /// `Tab::refresh` is deliberately *not* used: it drops the listing, which puts "Reading..."
    /// in the pane until the scan lands. That is right for F5, where somebody asked; here it
    /// would flash on every file written into the folder being watched. So the old listing stays
    /// up and only the request is made — `Tab::apply` then carries the selection across by
    /// name, exactly as it does for a refresh.
    fn folder_changed(&mut self, path: &Path) {
        self.loader.invalidate(path);
        let Self { panes, loader, .. } = self;
        for tab in panes.iter_mut().flat_map(|pane| pane.tabs.iter_mut()) {
            if tab.path == path {
                // Replacing a token that is already out means the older answer is dropped when
                // it arrives, which is what should happen: it read the folder as it was.
                tab.awaiting = Some(ask_for(tab, loader));
            }
        }
    }

    /// Act on files dropped onto a pane, and highlight the one being hovered.
    fn collect_drops(&mut self, ctx: &egui::Context) {
        use crate::shell::clipboard::Effect;
        use crate::shell::ops::Job;

        // The highlight, while a drag is over the window. A repaint is asked for
        // because the OLE callbacks run outside egui's own event flow and nothing else
        // would wake it.
        let hovering = self.drops.hovering();
        if hovering != self.drop_hover {
            self.drop_hover = hovering;
            ctx.request_repaint();
        }

        for dropped in self.drops.take_drops() {
            // Where the drop actually landed, decided when the pointer was there rather than
            // worked out again now. It was worked out again, from the pane under the pointer,
            // and so a drop onto a *folder row* went into the folder being shown instead of into
            // the folder it was dropped on — the one thing dragging onto a folder means.
            let into = match dropped.onto {
                // Onto the sidebar: pin the folders and move nothing. Files are ignored rather
                // than refused, so dragging a mixed selection over pins what can be pinned.
                crate::shell::dnd::Onto::Bookmarks => {
                    for item in dropped.items {
                        if item.is_dir() {
                            self.perform(ctx, Action::AddBookmark(item));
                        }
                    }
                    continue;
                }
                crate::shell::dnd::Onto::Folder(into) => into,
            };
            if into.as_os_str().is_empty() {
                continue;
            }

            let scale = ctx.pixels_per_point();
            let at = egui::pos2(
                dropped.at.0 as f32 / scale,
                dropped.at.1 as f32 / scale,
            );
            // Only to decide which pane the keyboard should follow the drop into; the
            // destination is `into`.
            let Some(pane) = self
                .panes
                .iter()
                .find(|p| p.rect.contains(at))
                .map(|p| p.id)
            else {
                continue;
            };
            // Dropping a folder into itself is meaningless whatever button carried it, and the
            // shell would refuse it noisily. A file dropped back into the folder it is already in
            // is meaningless too *for a left drag* -- it is a move to where it already is -- but
            // not for a right one: right-dragging a file onto its own folder is how Explorer is
            // asked for a copy of it, and the answer is `one - Copy.txt`. Filtering those out
            // before the question was asked meant a right drag inside a folder did nothing at
            // all, which is the most obvious way to try the gesture.
            let items = Self::droppable(dropped.items, &into, dropped.asked);
            if items.is_empty() {
                continue;
            }
            self.focused = pane;
            // A right-button drag asks rather than assumes, which is what Windows does and the
            // whole reason anybody drags with the right button.
            if dropped.asked {
                use crate::shell::menu::{Entry, Own};
                let own = [Own::CopyHere, Own::MoveHere, Own::Cancel]
                    .into_iter()
                    .map(Entry::own)
                    .collect();
                self.close_menu();
                // Built here rather than through the builder: these are this program's own
                // entries and there is nothing to ask the shell about, so the menu is ready now.
                // Token 0 matches no build, which is exactly right -- no answer is coming, and
                // the depth is the same nothing: every entry here is this program's own, so
                // there is no shell menu for one of them ever to be resolved against.
                self.menu = Some(crate::ui::menu::Open::new(
                    pane,
                    at,
                    items,
                    into,
                    own,
                    crate::shell::menu::Depth::Full,
                    0,
                ));
                continue;
            }
            let job = match dropped.effect {
                Effect::Move => Job::Move { items, into },
                Effect::Copy => Job::Copy { items, into },
            };
            self.ops.start(job, self.owner, ctx);
        }
    }

    /// Take delivery of finished file operations and re-read what they changed.
    fn collect_operations(&mut self) {
        for done in self.ops.drain() {
            let worked = done.error.is_none();
            if let Some(why) = done.error.filter(|why| !why.is_empty()) {
                self.notice = Some(why);
            }
            // The documented end of a cut, and only when the move actually happened.
            if let crate::shell::ops::After::FinishCut(was) = done.after {
                if worked {
                    crate::shell::clipboard::cut_pasted(was);
                    self.cut.clear();
                }
            }
            // A folder this program has just made: select it and open the name for editing as
            // soon as the re-read brings it in. `New folder` on its own is only half the
            // gesture; nobody wants a folder called `New folder`.
            if let crate::shell::ops::After::NameIt(pane) = done.after {
                if let (true, Some(name)) = (worked, done.created.clone()) {
                    if let Some(p) = self.pane_mut(pane) {
                        let tab = p.tab_mut();
                        tab.reveal = Some(name);
                        tab.rename_revealed = true;
                    }
                }
            }
            for path in &done.touched {
                self.loader.invalidate(path);
            }
            // Only a tab showing an affected folder re-reads. A tab elsewhere is left
            // alone, which is the point of tracking this by path.
            for pane in &mut self.panes {
                for tab in &mut pane.tabs {
                    if done.touched.contains(&tab.path) {
                        tab.refresh();
                    }
                }
            }
        }
        // A cut whose sources have gone is a cut that has been honoured.
        self.cut.retain(|path| path.exists());
    }

    /// Ask for anything nobody has asked for yet.
    ///
    /// The cache is probed synchronously first, which is what makes Back, Forward
    /// and revisiting a folder appear in the same frame as the click.
    fn start_scans(&mut self, ctx: &egui::Context, now: f64) {
        let Self { panes, loader, .. } = self;
        let mut asked = false;
        for pane in panes.iter_mut() {
            for tab in pane.tabs.iter_mut() {
                if tab.dir.is_some() || tab.awaiting.is_some() {
                    continue;
                }
                // A flattened tab never looks in the cache, in either direction: the
                // cache holds the folder's own children under this very path, and handing
                // those over would put a shallow listing on screen with the button lit.
                // It is not put *in* the cache either — see [`Loader::request_deep`].
                if tab.flat {
                    tab.awaiting = Some(ask_for(tab, loader));
                    tab.asked_at = Some(now);
                    asked = true;
                    continue;
                }
                match loader.cached(&tab.path) {
                    Some(dir) => tab.apply(dir),
                    None => {
                        tab.awaiting = Some(ask_for(tab, loader));
                        // When it was asked for, which is what decides whether the listing
                        // says anything about waiting. See [`crate::pane::SLOW_SCAN`].
                        tab.asked_at = Some(now);
                        asked = true;
                    }
                }
            }
        }
        // And the frame that would notice the half-second has passed. Nothing else would ask
        // for it: this program is idle between events, and the answer arriving is the only
        // other thing that wakes it — so without this, `Reading…` would appear on a slow scan
        // only if something else happened to want a frame in the meantime.
        if asked {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(crate::pane::SLOW_SCAN));
        }
    }

    // ---------------------------------------------------------------------
    // Keyboard
    // ---------------------------------------------------------------------

    /// Back and forward on the mouse's thumb buttons.
    ///
    /// The two extra buttons every mouse past a certain price has. Windows sends them as
    /// `WM_XBUTTONDOWN` with `XBUTTON1` or `XBUTTON2`; winit turns those into `MouseButton::Back`
    /// and `Forward`, and egui into `PointerButton::Extra1` and `Extra2`. That is the whole chain,
    /// and it is worth writing down because "button 4 and 5" appear under four different names on
    /// the way through and `winit::MouseButton::Other` — which is what anything past the fifth
    /// button becomes — is dropped before egui ever sees it.
    fn thumb_buttons(&mut self, ctx: &egui::Context) {
        use egui::PointerButton as B;

        let (back, forward) = ctx.input(|i| {
            (
                i.pointer.button_pressed(B::Extra1),
                i.pointer.button_pressed(B::Extra2),
            )
        });
        let pane = self.focused;
        if back {
            self.actions.push(Action::Back(pane));
        }
        if forward {
            self.actions.push(Action::Forward(pane));
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        use egui::Key as K;

        // A focused text field owns the keyboard. Escape is the one key that still
        // has to get through, or a filter box becomes a trap.
        let renaming = self
            .panes
            .iter()
            .any(|p| p.tabs.iter().any(|t| t.renaming.is_some()));
        // A menu on screen owns the keyboard: its own arrows and Enter are handled where
        // it is drawn, and the listing must not move underneath it at the same time.
        let typing =
            renaming || self.menu.is_some() || ctx.memory(|m| m.focused()).is_some();
        if typing {
            // **`Ctrl+E` and `Ctrl+P` still get through.** Both are questions about the folder you
            // are looking at rather than about the field the caret happens to be in, and both
            // compose with a filter: you type two letters to find something, then want the rest of
            // the tree, or want to see what is inside what you found. The filter survives either,
            // so having to click out of the box first is a step with no reason behind it — and
            // `Ctrl+F` already works the other way round.
            //
            // Not while renaming, and not under a menu: a rename is an edit of one name that
            // re-reading the folder would throw away, and a menu owns the keyboard outright.
            // `consume_key` rather than `key_pressed`, so the field it was typed into does not
            // also see it.
            let allowed = !renaming && self.menu.is_none();
            if allowed {
                for (key, action) in [
                    (K::E, Action::ToggleFlat(self.focused)),
                    (K::P, Action::TogglePreview(self.focused)),
                    // And the console's own key, which is the one that has to get through: the
                    // panel holds the keyboard while you are in it, so without this the only way
                    // out of it is the mouse.
                    (K::Backtick, Action::ToggleConsole(self.focused)),
                ] {
                    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, key)) {
                        self.actions.push(action);
                    }
                }
            }
            if ctx.input(|i| i.key_pressed(K::Escape)) {
                ctx.memory_mut(|m| m.stop_text_input());
            }
            return;
        }

        let pane = self.focused;
        let mut push = |action| self.actions.push(action);

        ctx.input(|i| {
            let m = i.modifiers;

            // ---- Tabs and panes ------------------------------------------
            // `Shift` is checked here rather than left out, because `m.command` alone would
            // fire both of these on `Ctrl+Shift+T` — a new tab *and* the reopened one.
            if m.command && i.key_pressed(K::T) {
                if m.shift {
                    push(Action::ReopenTab);
                } else {
                    push(Action::NewTab { pane });
                }
            }
            if m.command && i.key_pressed(K::W) {
                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
                    push(Action::CloseTab {
                        pane,
                        tab: p.active,
                    });
                }
            }
            if m.command && i.key_pressed(K::Tab) {
                push(Action::NextTab {
                    pane,
                    delta: if m.shift { -1 } else { 1 },
                });
            }
            for (index, key) in [K::Num1, K::Num2, K::Num3, K::Num4, K::Num5, K::Num6, K::Num7, K::Num8, K::Num9]
                .into_iter()
                .enumerate()
            {
                if m.command && i.key_pressed(key) {
                    push(Action::ActivateTab { pane, tab: index });
                }
            }
            // A split of the current folder, so the feature is reachable without
            // knowing that tabs can be dragged.
            if m.command && i.key_pressed(K::Backslash) {
                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
                    push(Action::OpenInSplit {
                        pane,
                        path: p.tab().path.clone(),
                        side: Side::Right,
                    });
                }
            }

            // ---- Navigation ----------------------------------------------
            if (m.alt && i.key_pressed(K::ArrowLeft)) || i.key_pressed(K::Backspace) && !m.alt {
                push(if m.alt {
                    Action::Back(pane)
                } else {
                    Action::Up(pane)
                });
            }
            if m.alt && i.key_pressed(K::ArrowRight) {
                push(Action::Forward(pane));
            }
            if m.alt && i.key_pressed(K::ArrowUp) {
                push(Action::Up(pane));
            }
            if i.key_pressed(K::F5) || (m.command && i.key_pressed(K::R)) {
                push(Action::Refresh(pane));
            }
            if (m.command && i.key_pressed(K::L)) || (m.alt && i.key_pressed(K::D)) {
                push(Action::EditPath(pane));
            }
            if m.command && i.key_pressed(K::A) {
                push(Action::SelectAll(pane));
            }
            if m.command && i.key_pressed(K::H) {
                push(Action::ToggleHidden(pane));
            }
            if m.command && i.key_pressed(K::E) {
                push(Action::ToggleFlat(pane));
            }
            if m.command && i.key_pressed(K::D) && !m.alt {
                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
                    push(Action::ToggleBookmark(p.tab().path.clone()));
                }
            }
            // This folder's preview panel. On the pane the keyboard is in, since that is the
            // folder whose selection it would be showing.
            if m.command && i.key_pressed(K::P) {
                push(Action::TogglePreview(pane));
            }
            // **The leftmost key of the number row**, whatever is printed on it: `²` on an AZERTY
            // board, `` ` `` on a QWERTY one. Bound by position, which falls out of how egui
            // resolves a key rather than from anything asked for here — it takes the logical key
            // when it recognises the character and the physical code when it does not, and `²` is
            // not a character it names, so the physical `Backquote` arrives.
            if m.command && i.key_pressed(K::Backtick) {
                push(Action::ToggleConsole(pane));
            }
            // ---- Files ---------------------------------------------------
            //
            // Read as events rather than as key presses, because that is what arrives.
            // `egui-winit` recognises Ctrl+C, Ctrl+X and Ctrl+V itself and queues `Event::Copy`,
            // `Event::Cut` or `Event::Paste` *in place of* the key, so `key_pressed(K::C)` is
            // never true for a copy — which is why these three shortcuts did nothing at all
            // while looking perfectly well wired. See `paste_keystroke` in `main.rs` for the
            // paste half, which arrives only because this program puts it back.
            for event in &i.events {
                match event {
                    // Shift+Delete is a *permanent delete*, and on Windows `egui-winit`
                    // recognises it as a legacy cut: it queues `Event::Cut` for it, the very same
                    // event Ctrl+X produces, with nothing to tell them apart but the modifiers.
                    // Guarding this arm with `!m.shift` and leaving `key_pressed(Delete)` to
                    // catch the rest was how Shift+Delete came to do nothing at all — the key
                    // does not arrive either.
                    //
                    // `!m.command` is the discriminator and the direction it fails matters. With
                    // Ctrl held this is Ctrl+X, or Ctrl+Shift+X with a thumb resting on Shift,
                    // and reading either of those as "delete this for ever" would be the worst
                    // mistake this program could make. So anything ambiguous is a cut, which
                    // moves nothing until something pastes.
                    egui::Event::Cut if m.shift && !m.command => push(Action::Delete {
                        pane,
                        permanent: true,
                    }),
                    egui::Event::Cut => push(Action::Cut(pane)),
                    egui::Event::Copy => push(Action::Copy(pane)),
                    egui::Event::Paste(_) => push(Action::Paste(pane)),
                    _ => {}
                }
            }
            if i.key_pressed(K::Delete) {
                // Shift is the difference between the Recycle Bin and gone.
                push(Action::Delete {
                    pane,
                    permanent: m.shift,
                });
            }
            if i.key_pressed(K::F2) {
                push(Action::BeginRename(pane));
            }
            if m.command && m.shift && i.key_pressed(K::N) {
                push(Action::NewFolder(pane));
            }

            if m.command && m.shift && i.key_pressed(K::C) {
                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
                    let mut paths = p.tab().selection_paths();
                    if paths.is_empty() {
                        paths.push(p.tab().path.clone());
                    }
                    push(Action::CopyPaths(paths));
                }
            }
        });

        // ---- The listing's own keys --------------------------------------
        let Some(index) = self.panes.iter().position(|p| p.id == pane) else {
            return;
        };
        // **How far one step of the cursor goes, and it is not always one.**
        //
        // In the grid, `Down` means "the tile below this one", which is a whole line of tiles along
        // the display order — so the step is the column count the view last laid out, and `Left` and
        // `Right` become the ±1 that `Down` is in a listing of rows.
        //
        // Except in a **tree**, where the step stays one and the two horizontal keys stay the tree's:
        // `Right` opens a folder and `Left` shuts it or steps out, which is what those keys mean in
        // every tree control on the platform and is worth more than moving one cell. A tree's grid
        // columns are per folder anyway — see [`crate::ui::grid::Layout::columns`], which answers 1
        // there for exactly this reason.
        let (step, page) = self
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| {
                let tab = p.tab();
                let height = (p.rect.height() - 80.0).max(1.0);
                if tab.view_mode.is_icons() {
                    let columns = tab.grid.columns.max(1) as isize;
                    let lines = (height / crate::ui::grid::CELL_H).max(1.0) as isize;
                    (columns, lines * columns)
                } else {
                    (1, (height / crate::pane::ROW_HEIGHT).max(1.0) as isize)
                }
            })
            .unwrap_or((1, 20));

        let mut open: Option<(bool, PathBuf)> = None;
        let mut typed: Vec<char> = Vec::new();

        {
            let tab = self.panes[index].tab_mut();
            ctx.input(|i| {
                let extend = i.modifiers.shift;
                if i.key_pressed(K::ArrowDown) {
                    tab.move_cursor(step, extend);
                }
                if i.key_pressed(K::ArrowUp) {
                    tab.move_cursor(-step, extend);
                }
                if i.key_pressed(K::PageDown) {
                    tab.move_cursor(page, extend);
                }
                if i.key_pressed(K::PageUp) {
                    tab.move_cursor(-page, extend);
                }
                if i.key_pressed(K::Home) {
                    tab.move_cursor_to(0, extend);
                }
                if i.key_pressed(K::End) {
                    tab.move_cursor_to(usize::MAX, extend);
                }
                // **Left and Right work a tree**, which is what those two keys mean in every tree
                // control on the platform: Right opens the folder under the cursor, Left shuts it,
                // and Left on something that is not an open folder goes out to the folder it is in.
                //
                // Free to bind here because a details listing has no use for them — nothing scrolls
                // sideways — and the navigation pair is `Alt+Left` and `Alt+Right`, which is read
                // further up with the modifier. Both do nothing at all in a listing that is not a
                // tree, which is what `Tab::set_collapsed` answers `false` for.
                //
                // No `extend`: opening a branch is not a selection gesture, and `Shift+Left` in a
                // tree that grew four hundred rows would select whatever the arithmetic landed on.
                //
                // In a **grid** that is not a tree they are the neighbouring tile instead, which is
                // the same statement from the other end: the two keys go to whichever axis the view
                // has, and a grid of tiles is the one listing here with two.
                let sideways = tab.view_mode.is_icons() && !tab.is_tree();
                if sideways {
                    if i.key_pressed(K::ArrowRight) {
                        tab.move_cursor(1, extend);
                    }
                    if i.key_pressed(K::ArrowLeft) {
                        tab.move_cursor(-1, extend);
                    }
                } else {
                    if !extend && i.key_pressed(K::ArrowRight) {
                        tab.set_collapsed_at_cursor(false);
                    }
                    if !extend && i.key_pressed(K::ArrowLeft) && !tab.set_collapsed_at_cursor(true) {
                        tab.move_cursor_to_parent();
                    }
                }
                if i.key_pressed(K::Escape) {
                    tab.clear_selection();
                }
                if i.key_pressed(K::Enter) {
                    if let Some(at) = tab.cursor {
                        if let Some(path) = tab.target_at(at) {
                            open = Some((tab.is_dir_at(at), path));
                        }
                    }
                }
                // Type-ahead: anything printable that is not a shortcut.
                if !i.modifiers.command && !i.modifiers.alt {
                    for event in &i.events {
                        if let egui::Event::Text(text) = event {
                            typed.extend(text.chars());
                        }
                    }
                }
            });

            let now = ctx.input(|i| i.time);
            for ch in typed {
                tab.type_ahead(ch, now);
            }
        }

        if let Some((is_dir, path)) = open {
            self.actions.push(if is_dir {
                Action::Navigate { pane, path }
            } else {
                Action::Open(path)
            });
        }
    }

    // ---------------------------------------------------------------------
    // Applying
    // ---------------------------------------------------------------------

    fn apply(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        for action in actions {
            self.perform(ctx, action);
        }
        if self.config_dirty {
            // Written on the way out rather than on every drag frame; the cost of
            // losing the last few points of a sidebar width is nothing, and the cost
            // of a file write per frame is a stutter.
            self.config_dirty = false;
            self.config = self.settings();
            self.config.save();
        }
    }

    fn perform(&mut self, ctx: &egui::Context, action: Action) {
        if let Some(journal) = &mut self.journal {
            journal.push(action.name());
        }
        match action {
            Action::Focus(id) => {
                if self.panes.iter().any(|p| p.id == id) {
                    self.focused = id;
                }
            }

            Action::ActivateTab { pane, tab } => {
                if let Some(p) = self.pane_mut(pane) {
                    if tab < p.tabs.len() {
                        p.show_tab(tab);
                    }
                }
                self.focused = pane;
            }
            Action::NextTab { pane, delta } => {
                if let Some(p) = self.pane_mut(pane) {
                    let count = p.tabs.len() as isize;
                    p.show_tab((((p.active as isize + delta) % count + count) % count) as usize);
                }
            }
            Action::NewTab { pane } => {
                if let Some(p) = self.pane_mut(pane) {
                    let tab = p.tab().duplicate();
                    p.tabs.push(tab);
                    p.show_tab(p.tabs.len() - 1);
                }
                self.focused = pane;
                self.config_dirty = true;
            }
            Action::NewTabFocused => {
                let pane = self.focused;
                self.perform(ctx, Action::NewTab { pane });
            }
            Action::SplitFocused { side } => {
                let pane = self.focused;
                if let Some(path) = self.pane_mut(pane).map(|p| p.tab().path.clone()) {
                    self.perform(ctx, Action::OpenInSplit { pane, path, side });
                }
            }
            Action::CloseTab { pane, tab } => self.close_tab(ctx, pane, tab),
            Action::ReopenTab => {
                // Somewhere to put it, before taking it off the stack: the focused pane, or any
                // pane if focus is stale. Popping first and then finding nowhere to open it
                // would spend the entry and give nothing back.
                let Some(pane) = self
                    .panes
                    .iter()
                    .find(|p| p.id == self.focused)
                    .or_else(|| self.panes.first())
                    .map(|p| p.id)
                else {
                    return;
                };
                let Some(path) = self.closed.pop() else {
                    return;
                };
                self.perform(ctx, Action::NavigateNewTab { pane, path });
            }

            Action::BeginTabDrag { pane, tab, grab_dx } => {
                let title = self
                    .pane_mut(pane)
                    .and_then(|p| p.tabs.get(tab))
                    .map(|t| t.title.clone());
                if let Some(title) = title {
                    self.drag = Some(TabDrag {
                        pane,
                        tab,
                        grab_dx,
                        title,
                        live: false,
                    });
                }
            }

            Action::MoveTab {
                from,
                tab,
                to,
                index,
            } => self.move_tab(from, tab, to, index),

            Action::SplitTab {
                from,
                tab,
                target,
                side,
            } => {
                let Some(moved) = self.take_tab(from, tab) else {
                    return;
                };
                let id = self.spawn_pane(moved);
                if !self.layout.split(target, side, id) {
                    // The target vanished between the drop and now. Put the pane
                    // beside the focused one rather than losing the tab.
                    let anchor = self.focused;
                    self.layout.split(anchor, Side::Right, id);
                }
                self.focused = id;
                self.config_dirty = true;
            }

            Action::OpenInSplit { pane, path, side } => {
                let id = self.spawn_pane(Tab::new(path));
                if !self.layout.split(pane, side, id) {
                    self.panes.retain(|p| p.id != id);
                    return;
                }
                self.focused = id;
                self.config_dirty = true;
            }

            Action::Navigate { pane, path } => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().navigate(path);
                }
                self.focused = pane;
                self.config_dirty = true;
            }
            Action::NavigateNewTab { pane, path } => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tabs.push(Tab::new(path));
                    p.show_tab(p.tabs.len() - 1);
                }
                self.focused = pane;
                self.config_dirty = true;
            }
            Action::Back(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().go_back();
                }
            }
            Action::Forward(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().go_forward();
                }
            }
            Action::Up(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().go_up();
                }
            }
            Action::Refresh(pane) => {
                // Re-probe the volumes too: a full disk or an ejected card is exactly
                // the kind of thing someone presses F5 about. The probes are threads,
                // so this costs nothing here.
                self.volumes.refresh(ctx);
                let path = self.pane_mut(pane).map(|p| p.tab().path.clone());
                if let Some(path) = path {
                    self.loader.invalidate(&path);
                    if let Some(p) = self.pane_mut(pane) {
                        let tab = p.tab_mut();
                        // Keep the cursor where it was: a refresh should not move
                        // what you were looking at.
                        let keep = tab
                            .cursor
                            .and_then(|at| tab.entry_at(at))
                            .and_then(|i| tab.dir.as_ref().map(|d| d.name(i).to_owned()));
                        tab.refresh();
                        tab.reveal = keep;
                    }
                }
            }
            Action::EditPath(pane) => {
                let slashes = self.forward_slashes;
                if let Some(p) = self.pane_mut(pane) {
                    breadcrumb::start_editing(p.tab_mut(), slashes);
                }
            }

            Action::Sort { pane, column } => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().sort_by_column(column);
                }
            }
            Action::SelectAll(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().select_all();
                }
            }
            Action::ToggleHidden(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    let tab = p.tab_mut();
                    tab.show_hidden = !tab.show_hidden;
                    tab.rebuild_order();
                    tab.widths_measured = false;
                }
            }
            // Unlike the other view toggles, this one changes what was *read* rather than
            // what is shown of it, so the listing goes and `start_scans` asks again.
            Action::ToggleFlat(pane) => {
                let (mode, regroup) = (self.flat_mode, self.regroup);
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().toggle_flat(mode, regroup);
                }
            }
            // And this one changes neither: both flatten modes are orders over the one listing the
            // walk already produced, so every tab showing a tree re-sorts and nothing is re-read.
            // See [`crate::pane::FlatMode`].
            //
            // **Every tab, not the pane the menu was opened on.** It is the window's preference,
            // it is written to the settings file, and a preference that applied to one pane would
            // leave the other one disagreeing with the tick in a menu that claims to be about
            // both. Tabs that are not flattened take the mode for the next time their button is
            // pressed, which is what `Tab::set_flat_mode` does for nothing.
            // Rows or tiles, for **this tab and no other** — and nothing is remembered: no window
            // preference, no settings key, and the next folder this tab opens is back in the details
            // view. See [`crate::pane::ViewMode`], which is where that argument lives, and
            // `SetFlatMode` just below for the preference this deliberately is not.
            //
            // The listing is not touched: both views are drawn over the same order, the same
            // selection and the same cursor. What does have to move is the *scroll*, because the
            // offset means a different place in each — row 40 of a listing and line 40 of a grid are
            // hundreds of files apart — so the view opens on the cursor if there is one and at the top
            // if there is not, which is the same rule a changed filter follows.
            Action::SetView { pane, mode } => {
                let Some(p) = self.pane_mut(pane) else { return };
                let tab = p.tab_mut();
                if tab.view_mode == mode {
                    return;
                }
                tab.view_mode = mode;
                if tab.cursor.is_some() {
                    tab.scroll_to_cursor = true;
                } else {
                    tab.scroll_y = 0.0;
                    tab.scroll_to = Some(0.0);
                }
            }
            Action::SetFlatMode(mode) => {
                self.flat_mode = mode;
                for p in &mut self.panes {
                    for tab in p.tabs.iter_mut() {
                        tab.set_flat_mode(mode);
                    }
                }
                self.config_dirty = true;
            }
            // The same again for the other half of a tree's shape, and for the same reasons: every
            // tab, at once, nothing re-read — a merged chain is an order over the listing the walk
            // already produced. See [`crate::fs::sort::build_tree_order`].
            Action::SetRegroup(on) => {
                self.regroup = on;
                for p in &mut self.panes {
                    for tab in p.tabs.iter_mut() {
                        tab.set_regroup(on);
                    }
                }
                self.config_dirty = true;
            }
            // Which slash the path field writes. The window's preference like the two above, and
            // written down for the same reason — but this one has to rewrite what is already in a
            // field that is open, because that field is where the menu was just ticked: a setting
            // whose effect you have to close and reopen the field to see reads as a setting that
            // did nothing.
            //
            // Every tab, and the ones without a field open have nothing to rewrite. `\` and `/` are
            // both one byte and neither can appear in a Windows file name, so the swap is exact and
            // leaves the caret in front of the same character — see
            // [`crate::ui::breadcrumb::with_separator`], and [`PathComplete::rewritten`] for why the
            // completion has to be told the text moved without anybody typing.
            Action::SetForwardSlashes(on) => {
                self.forward_slashes = on;
                for p in &mut self.panes {
                    let id = p.id;
                    for tab in p.tabs.iter_mut() {
                        if !tab.editing_path {
                            continue;
                        }
                        tab.edit_text = breadcrumb::with_separator(&tab.edit_text, on);
                        self.complete.rewritten(id, &tab.edit_text);
                    }
                }
                self.config_dirty = true;
            }
            // The status line's `N changed`, pressed. **Two settings, because either one alone
            // answers half the question**: the filter over a folder's own children finds only what
            // changed in *that* folder, and the flatten without the filter is the whole tree with
            // the changes buried in it. Together they are the listing the count is a count of.
            //
            // The flatten is *set*, not toggled — pressing a button whose label is a fact about the
            // repository should not undo itself when the pane is already showing the tree, and the
            // filter is what is being changed on the second press. `toggle_flat` is still the one
            // that does it, because turning the view on is a re-read and everything that comes with
            // it lives there.
            Action::ShowChanges(pane) => {
                let (mode, regroup) = (self.flat_mode, self.regroup);
                if let Some(p) = self.pane_mut(pane) {
                    let tab = p.tab_mut();
                    if !tab.flat {
                        tab.toggle_flat(mode, regroup);
                    }
                    tab.filter = crate::fs::sort::CHANGED.to_owned();
                    // Straight away rather than through `filter_changed`: the delay there is for
                    // keystrokes, and there are none — the whole line arrived at once. A flatten
                    // that is starting has no listing to rebuild yet, and the walk landing rebuilds
                    // from the filter as it stands.
                    tab.rebuild_order();
                    tab.widths_measured = false;
                }
            }
            Action::ToggleCollapsed { pane, position } => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().toggle_collapsed(position);
                }
            }
            Action::TogglePreview(pane) => {
                let diffing = self.preview.diff;
                let Some(p) = self.pane_mut(pane) else { return };
                let tab = p.tab_mut();
                if tab.preview.open {
                    tab.preview.close();
                } else {
                    // Opened with nothing previewable selected, the panel still opens and says
                    // what it would show. A shortcut that silently does nothing is a shortcut
                    // people conclude is broken.
                    match Self::selected_preview(tab, diffing) {
                        Some(ask) => tab.preview.ask_for(ask),
                        None => tab.preview.open = true,
                    }
                }
                self.config_dirty = true;
            }
            Action::ClosePreview(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().preview.close();
                }
                self.config_dirty = true;
            }
            // The shell is not started here and not stopped here. Opening the panel is the cheap
            // half — `console_panel` starts one on the frame it first has a rect to draw in, and
            // closing leaves the shell running, because a build you hid the panel to get out of the
            // way is a build you still want when you bring it back.
            Action::ToggleConsole(pane) => {
                let id = crate::ui::console::id(pane);
                let open = match self.pane_mut(pane) {
                    Some(p) => {
                        p.console_open = !p.console_open;
                        p.console_open
                    }
                    None => return,
                };
                if open {
                    // Opening one puts the keyboard in it — and takes it off any other pane's,
                    // because two panels both certain they own the keyboard would both read the same
                    // keystroke.
                    for other in &mut self.panes {
                        other.console_state.drop_keys();
                    }
                    let shell = self.console_shell;
                    if let Some(p) = self.pane_mut(pane) {
                        p.console_state.take_keys();
                        // The shell this window was last working in — but only when there is no
                        // session yet, since changing the kind is what replaces one.
                        if p.console.is_none() {
                            p.console_state.set_kind(shell);
                        }
                    }
                    ctx.memory_mut(|m| m.request_focus(id));
                } else {
                    // The panel is gone, so what was holding the keyboard inside it cannot still be,
                    // or the listing stays deaf with nothing on screen to explain why.
                    if let Some(p) = self.pane_mut(pane) {
                        p.console_state.drop_keys();
                    }
                    ctx.memory_mut(|m| m.surrender_focus(id));
                }
                self.config_dirty = true;
            }
            Action::RememberLayout => self.config_dirty = true,

            Action::Cut(pane) => self.put_on_clipboard(pane, true),
            Action::Copy(pane) => self.put_on_clipboard(pane, false),
            Action::Paste(pane) => self.paste_into(pane, ctx),
            // The context menu's Couper, Copier and Coller, which name what they act on rather
            // than reading it off a pane. See `ours_rather_than_the_shell_s`.
            Action::CutItems(items) => self.put_these_on_clipboard(items, true),
            Action::CopyItems(items) => self.put_these_on_clipboard(items, false),
            Action::PasteIntoFolder(into) => self.paste_into_folder(into, ctx),
            Action::Delete { pane, permanent } => {
                let items = self
                    .pane_mut(pane)
                    .map(|p| p.tab().selection_paths())
                    .unwrap_or_default();
                if items.is_empty() {
                    self.notice = Some("Nothing selected".to_owned());
                    return;
                }
                // No confirmation of our own: the shell asks, and being asked twice
                // about the same thing is how a prompt becomes something people click
                // through without reading.
                self.ops.start(
                    crate::shell::ops::Job::Delete {
                        items,
                        to_bin: !permanent,
                    },
                    self.owner,
                    ctx,
                );
            }
            Action::BeginRename(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().begin_rename();
                }
            }
            Action::CommitRename { pane, name } => {
                let owner = self.owner;
                let Some(p) = self.pane_mut(pane) else { return };
                let tab = p.tab_mut();
                let Some((entry, _)) = tab.renaming.take() else {
                    return;
                };
                let Some(dir) = tab.dir.clone() else { return };
                let name = name.trim().to_owned();
                // Against the leaf, which is what the field was seeded with — in a
                // flattened listing the entry's *name* is a relative path, and comparing
                // against that would make every rename look like a change.
                if name.is_empty() || name == dir.leaf(entry) {
                    return;
                }
                let item = dir.target(entry);
                // Selected again once the folder is re-read, so the renamed file is
                // still the thing you were looking at.
                tab.reveal = Some(name.clone());
                self.ops
                    .start(crate::shell::ops::Job::Rename { item, name }, owner, ctx);
            }
            Action::CancelRename(pane) => {
                if let Some(p) = self.pane_mut(pane) {
                    p.tab_mut().renaming = None;
                }
            }
            Action::NewFolder(pane) => {
                let owner = self.owner;
                let Some(parent) = self.pane_mut(pane).map(|p| p.tab().path.clone()) else {
                    return;
                };
                if parent.as_os_str().is_empty() {
                    self.notice = Some("This PC is not a folder to create in".to_owned());
                    return;
                }
                // The shell picks a free name from this one, so "New folder (2)" and the
                // rest come out right without this program having to count — and reports back
                // which it chose, so the row can be named the moment it appears.
                let name = "New folder".to_owned();
                self.ops.start_then(
                    crate::shell::ops::Job::NewFolder { parent, name },
                    crate::shell::ops::After::NameIt(pane),
                    owner,
                    ctx,
                );
            }
            Action::DropHere {
                pane,
                items,
                into,
                moving,
            } => {
                self.focused = pane;
                let job = if moving {
                    crate::shell::ops::Job::Move { items, into }
                } else {
                    crate::shell::ops::Job::Copy { items, into }
                };
                self.ops.start(job, self.owner, ctx);
            }
            Action::DragOut { pane, items } => {
                // Started here and now rather than parked for later: the drag has its own
                // thread, so nothing about it re-enters this pass. One at a time, since the
                // second would be following a button the first is already holding — and only
                // with a window, because the drag joins *this* thread's input queue to find
                // the button it is following and a thread with no window has no gesture to
                // follow. That last one is also what keeps the tests off the real pointer.
                if self.file_drag.is_none() && self.owner.0 != 0 {
                    self.file_drag =
                        crate::shell::dnd::drag_out(items).map(|drag| (pane, drag));
                }
            }
            Action::ShellMenu {
                pane,
                items,
                at,
            } => self.shell_menu(pane, items, at, ctx),
            // **A shortcut to a folder opens here, not in Explorer.** Handing it to the shell is
            // what `.lnk` files get by default, and for a folder that means a second file
            // manager opening over the top of this one — which is not what clicking a row in
            // this window can be allowed to do. Every route into this arm gets it: a double
            // click, `Enter`, and a path typed into the bar.
            //
            // A directory *reparse point* — a junction or a directory symlink — never comes
            // through here at all: the enumeration reports it as a directory, so it is a
            // `Navigate` before this is reached.
            //
            // The pane is the focused one because that is where the gesture was: a click on a
            // row focuses its pane first, and `Enter` acts on the focused pane by definition.
            Action::Open(path) => match crate::shell::links::folder_target(&path) {
                Some(folder) => {
                    let pane = self.focused;
                    self.perform(ctx, Action::Navigate { pane, path: folder });
                }
                None => fs::shell::open(&path),
            },
            // The same, in a tab of its own — a middle click on a folder shortcut. A shortcut to
            // a *file* does nothing here rather than opening it somewhere it cannot be shown: a
            // new tab is a place, and a file is not one.
            Action::OpenNewTab(path) => {
                if let Some(folder) = crate::shell::links::folder_target(&path) {
                    let pane = self.focused;
                    self.perform(ctx, Action::NavigateNewTab { pane, path: folder });
                }
            }
            Action::Reveal(path) => fs::shell::reveal(&path),
            Action::OpenTerminal(path) => fs::shell::open_terminal(&path),
            Action::CopyPaths(paths) => {
                let text = paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("\r\n");
                ctx.copy_text(text);
            }
            Action::AddBookmark(path) => {
                if !path.as_os_str().is_empty() && !self.bookmarks.contains(&path) {
                    self.bookmarks.push(path);
                    self.config_dirty = true;
                }
            }
            Action::RemoveBookmark(path) => {
                self.bookmarks.retain(|p| *p != path);
                self.config_dirty = true;
            }
            Action::MoveBookmark { from, to } => {
                // `to` is an insertion point in the list *before* the move, so removing
                // first shifts every later position down by one.
                if from < self.bookmarks.len() && to <= self.bookmarks.len() {
                    let moved = self.bookmarks.remove(from);
                    let at = if to > from { to - 1 } else { to };
                    self.bookmarks.insert(at.min(self.bookmarks.len()), moved);
                    self.config_dirty = true;
                }
            }
            Action::ToggleBookmark(path) => {
                if self.is_bookmarked(&path) {
                    self.bookmarks.retain(|p| *p != path);
                } else if !path.as_os_str().is_empty() {
                    self.bookmarks.push(path);
                }
                self.config_dirty = true;
            }
            Action::SetTheme { dark } => {
                if dark == self.theme.dark {
                    return;
                }
                self.theme = if dark { Theme::dark() } else { Theme::light() };
                // The style has to be reinstalled, and only then — installing it every
                // frame would throw away egui's galley and shape caches.
                self.installed = false;
                self.config_dirty = true;
            }

            Action::Window(what) => {
                use egui::ViewportCommand as Cmd;
                match what {
                    WindowAction::Minimize => ctx.send_viewport_cmd(Cmd::Minimized(true)),
                    WindowAction::ToggleMaximize => {
                        ctx.send_viewport_cmd(Cmd::Maximized(!self.maximized));
                    }
                    WindowAction::ResetSize => {
                        let [w, h] = crate::config::WINDOW_SIZE;
                        // Un-maximised first, and said out loud rather than relied on. On
                        // Windows an `InnerSize` alone is enough — measured: from a maximised
                        // 2560×1392 the window comes back to 1024×600 with this line taken out,
                        // because `SetWindowPos` on a maximised window restores it on the way.
                        // That is winit's platform behaviour and not a promise, and asking for
                        // the state this wants costs one command.
                        if self.maximized {
                            ctx.send_viewport_cmd(Cmd::Maximized(false));
                        }
                        ctx.send_viewport_cmd(Cmd::InnerSize(egui::vec2(w, h)));
                        // Both remembered now rather than left to the frame that observes the
                        // new shape. That frame does set them, so this is belt and braces for
                        // the window being closed in between — which would otherwise save the
                        // size and the maximised flag this has just replaced.
                        self.maximized = false;
                        self.window_size = Some([w, h]);
                        self.config_dirty = true;
                    }
                    WindowAction::Close => ctx.send_viewport_cmd(Cmd::Close),
                    WindowAction::Drag => ctx.send_viewport_cmd(Cmd::StartDrag),
                }
            }
        }
    }

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
        let last_pane = self.panes.len() == 1;
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
        if let Some(path) = self.panes[position].tabs.get(index).map(|t| t.path.clone()) {
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

    /// Whether a path is bookmarked, for the sidebar's star.
    pub fn is_bookmarked(&self, path: &Path) -> bool {
        self.bookmarks.iter().any(|p| p == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app with one pane per path, and no scan allowed to land — the tests are
    /// about structure, and a listing arriving would only add noise.
    fn app(paths: &[&str]) -> (App, egui::Context) {
        let ctx = egui::Context::default();
        let open: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let app = App::opening(&ctx, Config::default(), open, Side::Right);
        (app, ctx)
    }

    #[test]
    fn a_window_gesture_keeps_frames_coming_and_then_stops() {
        let mut s = Settling::default();
        let at_rest = Shape {
            pixels: (1024, 600),
            scale: 1000,
            native_scale: 1000,
            focused: true,
            minimized: false,
        };

        // The first frame is a change from nothing, so it paints -- which is correct and
        // also why the baseline has to be established before anything is asserted.
        assert!(s.observe(at_rest, 1.0));

        // Then nothing is moving, and no frames are asked for. This is the case that must
        // cost nothing, because it is every frame of every session.
        assert!(!s.observe(at_rest, 1.0 + Settling::QUIET));
        assert!(!s.observe(at_rest, 9.0));

        // Restored onto a monitor at 125%: both the pixel size and the scale change.
        let rescaled = Shape {
            pixels: (1280, 750),
            scale: 1250,
            ..at_rest
        };
        assert!(s.observe(rescaled, 10.0), "the change itself has to repaint");
        // And it keeps painting while the gesture settles, without needing more changes --
        // which is the point: the stretched frame is on screen during these.
        assert!(s.observe(rescaled, 10.2));
        assert!(s.observe(rescaled, 10.0 + Settling::QUIET - 0.01));
        // Then stops.
        assert!(!s.observe(rescaled, 10.0 + Settling::QUIET));
        assert!(!s.observe(rescaled, 20.0));

        // Losing focus counts too: it is what a restore animation and an occlusion both
        // come with, and it was the reported trigger.
        assert!(s.observe(
            Shape {
                focused: false,
                ..rescaled
            },
            21.0
        ));
    }

    #[test]
    fn a_scale_change_too_small_to_see_is_not_a_change() {
        // The scale arrives as an `f32`. Comparing it exactly would let a value that
        // differs in its last bit request repaints for the rest of the session.
        let mut s = Settling::default();
        let a = Shape {
            pixels: (1024, 600),
            scale: (1.0_f32 * 1000.0).round() as u32,
            native_scale: 1000,
            focused: true,
            minimized: false,
        };
        let b = Shape {
            scale: (1.000_04_f32 * 1000.0).round() as u32,
            ..a
        };
        assert_eq!(a, b, "a difference this small is not a rescale");
        s.observe(a, 1.0); // the baseline
        assert!(!s.observe(b, 1.0 + Settling::QUIET));
    }

    fn titles(app: &App, pane: PaneId) -> Vec<String> {
        app.panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| p.tabs.iter().map(|t| t.title.clone()).collect())
            .unwrap_or_default()
    }

    fn tree(app: &App) -> Vec<PaneId> {
        let mut out = Vec::new();
        app.layout.panes(&mut out);
        out
    }

    #[test]
    fn one_pane_per_open_path() {
        let (app, _ctx) = app(&["/a", "/b", "/c"]);
        assert_eq!(app.panes.len(), 3);
        assert_eq!(tree(&app).len(), 3);
        assert_eq!(app.layout.count(), 3);
    }

    /// A window comes back divided the way it was left.
    ///
    /// The end-to-end claim, and the only place the two halves of it meet: `settings` writes
    /// the tree over pane *numbers* and the panes in the order the tree numbers them, and
    /// `opening` has to line those numbers back up with the panes it builds. Either half alone
    /// can be right while the pair is wrong — a window whose panes come back in the other
    /// order, showing the right folders in the wrong places — so this goes all the way through
    /// the text of the file.
    #[test]
    fn the_panes_come_back_the_way_they_were_left() {
        let (mut app, ctx) = app(&["/left", "/right"]);
        let (left, right) = (app.panes[0].id, app.panes[1].id);
        // A second tab in the left pane, a third pane under the right one, and the focus
        // somewhere that is not the first pane — so that every part of what is written down
        // has something to say.
        app.perform(&ctx, Action::NewTab { pane: left });
        app.perform(
            &ctx,
            Action::Navigate {
                pane: left,
                path: PathBuf::from("/left/deeper"),
            },
        );
        // To the *left* of the right-hand pane, deliberately: the new pane goes in front of it
        // in the tree while being pushed to the back of `panes`, so layout order and the order
        // the panes happen to sit in the vector are no longer the same list. Written in the
        // wrong one of those two, everything below still passes.
        app.perform(
            &ctx,
            Action::OpenInSplit {
                pane: right,
                path: PathBuf::from("/under"),
                side: Side::Left,
            },
        );
        app.perform(&ctx, Action::Focus(right));
        *app.layout.ratio_at(&[]).unwrap() = 0.4;

        let order = tree(&app);
        assert_eq!(order.len(), 3, "three panes to write down");
        assert_ne!(
            order,
            app.panes.iter().map(|p| p.id).collect::<Vec<_>>(),
            "this test is only worth running while the two orders differ"
        );
        let titles: Vec<Vec<String>> = order.iter().map(|id| self::titles(&app, *id)).collect();
        let had_focus = order.iter().position(|id| *id == right).expect("in the tree");
        let shape = app.layout.encode();

        // Through the text of the file, not merely through the struct: the numbering is the
        // part that can go wrong, and it only exists in the text.
        let reopened = Config::parse(&app.settings().to_text());
        let back = App::opening(&ctx, reopened, Vec::new(), Side::Right);

        let recovered = tree(&back);
        assert_eq!(recovered.len(), order.len(), "a pane went missing");
        assert_eq!(back.layout.encode(), shape, "the same shape, with the same ratios");
        assert_eq!(
            recovered
                .iter()
                .map(|id| self::titles(&back, *id))
                .collect::<Vec<_>>(),
            titles,
            "the folders came back in the wrong panes"
        );
        assert_eq!(
            back.focused, recovered[had_focus],
            "the pane that had the keyboard has to be the one that gets it"
        );
        assert_eq!(
            back.panes
                .iter()
                .find(|p| p.id == recovered[0])
                .map(|p| p.active),
            Some(1),
            "and the tab that was in front stays in front"
        );
    }

    /// A settings file from before panes were remembered still opens every tab it names.
    #[test]
    fn remembered_tabs_with_no_layout_reopen_in_one_pane() {
        let ctx = egui::Context::default();
        let config = Config::parse("path=/a\npath=/b\npath=/c\n");
        let app = App::opening(&ctx, config, Vec::new(), Side::Right);
        assert_eq!(app.panes.len(), 1, "there is no layout to build");
        assert_eq!(app.panes[0].tabs.len(), 3, "but every tab is still opened");
    }

    /// And a layout that does not describe the panes beside it is not half-applied.
    #[test]
    fn a_layout_that_does_not_fit_its_panes_opens_plainly() {
        let ctx = egui::Context::default();
        // Two panes named, three in the tree.
        let config = Config::parse("layout=h0.5(0,v0.5(1,2))\npane=0\npath=/a\npane=0\npath=/b\n");
        let app = App::opening(&ctx, config, Vec::new(), Side::Right);
        assert_eq!(app.panes.len(), 1);
        assert_eq!(
            app.panes[0].tabs.len(),
            2,
            "the folders that were open have to open, whatever the tree said"
        );
    }

    #[test]
    fn a_tab_dragged_to_another_pane_changes_hands() {
        let (mut app, ctx) = app(&["/left", "/right"]);
        let (left, right) = (app.panes[0].id, app.panes[1].id);
        app.perform(&ctx, Action::NewTab { pane: left });
        assert_eq!(titles(&app, left).len(), 2);

        app.perform(
            &ctx,
            Action::MoveTab {
                from: left,
                tab: 0,
                to: right,
                index: 0,
            },
        );
        assert_eq!(titles(&app, left).len(), 1);
        assert_eq!(titles(&app, right), ["left", "right"]);
        assert_eq!(app.focused, right, "the tab you moved is the one you wanted");
    }

    #[test]
    fn moving_a_panes_last_tab_away_collapses_the_split() {
        let (mut app, ctx) = app(&["/left", "/right"]);
        let (left, right) = (app.panes[0].id, app.panes[1].id);

        app.perform(
            &ctx,
            Action::MoveTab {
                from: left,
                tab: 0,
                to: right,
                index: usize::MAX,
            },
        );
        assert_eq!(app.panes.len(), 1, "the emptied pane goes");
        assert_eq!(tree(&app), [right], "and so does its half of the tree");
        assert_eq!(titles(&app, right), ["right", "left"]);
    }

    #[test]
    fn reordering_inside_a_strip_accounts_for_the_gap_left_behind() {
        let (mut app, ctx) = app(&["/a"]);
        let pane = app.panes[0].id;
        // Three tabs: a, a, a -- retitled so the order is checkable.
        app.perform(&ctx, Action::NewTab { pane });
        app.perform(&ctx, Action::NewTab { pane });
        for (index, name) in ["one", "two", "three"].into_iter().enumerate() {
            app.panes[0].tabs[index].title = name.to_owned();
        }

        // Drop the first tab where the third one starts: it lands between two and
        // three, because removing it shifted everything after it down.
        app.perform(
            &ctx,
            Action::MoveTab {
                from: pane,
                tab: 0,
                to: pane,
                index: 2,
            },
        );
        assert_eq!(titles(&app, pane), ["two", "one", "three"]);
        assert_eq!(app.panes[0].active, 1, "the moved tab stays the active one");
    }

    #[test]
    fn appending_to_a_strip_puts_the_tab_last() {
        let (mut app, ctx) = app(&["/a"]);
        let pane = app.panes[0].id;
        app.perform(&ctx, Action::NewTab { pane });
        app.panes[0].tabs[0].title = "one".to_owned();
        app.panes[0].tabs[1].title = "two".to_owned();

        app.perform(
            &ctx,
            Action::MoveTab {
                from: pane,
                tab: 0,
                to: pane,
                index: usize::MAX,
            },
        );
        assert_eq!(titles(&app, pane), ["two", "one"]);
    }

    #[test]
    fn dropping_a_tab_on_a_pane_edge_splits_it() {
        let (mut app, ctx) = app(&["/a"]);
        let pane = app.panes[0].id;
        app.perform(&ctx, Action::NewTab { pane });

        app.perform(
            &ctx,
            Action::SplitTab {
                from: pane,
                tab: 1,
                target: pane,
                side: Side::Right,
            },
        );
        assert_eq!(app.panes.len(), 2);
        assert_eq!(app.layout.count(), 2);
        let new = tree(&app)[1];
        assert_eq!(new, app.focused, "focus follows the tab you pulled out");
        assert_eq!(titles(&app, pane).len(), 1);
        assert_eq!(titles(&app, new).len(), 1);
    }

    #[test]
    fn closing_the_last_tab_of_a_split_pane_removes_the_pane() {
        let (mut app, ctx) = app(&["/left", "/right"]);
        let (left, right) = (app.panes[0].id, app.panes[1].id);

        app.perform(&ctx, Action::CloseTab { pane: left, tab: 0 });
        assert_eq!(app.panes.len(), 1);
        assert_eq!(tree(&app), [right]);
        assert_eq!(app.focused, right, "focus cannot stay on a pane that is gone");
    }

    #[test]
    fn the_last_tab_of_the_last_pane_is_the_window() {
        let (mut app, ctx) = app(&["/only"]);
        let pane = app.panes[0].id;
        app.perform(&ctx, Action::CloseTab { pane, tab: 0 });
        // Nothing is torn down -- the close is a viewport command, and the state has
        // to stay coherent for however many frames it takes to arrive.
        assert_eq!(app.panes.len(), 1);
        assert_eq!(app.panes[0].tabs.len(), 1);
    }

    #[test]
    fn a_new_tab_points_where_the_old_one_did() {
        let (mut app, ctx) = app(&["/somewhere/deep"]);
        let pane = app.panes[0].id;
        app.perform(&ctx, Action::NewTab { pane });
        assert_eq!(app.panes[0].tabs[0].path, app.panes[0].tabs[1].path);
        assert_eq!(app.panes[0].active, 1);
    }

    #[test]
    fn a_closed_tab_comes_back_with_nothing_behind_it() {
        let (mut app, ctx) = app(&["/one"]);
        let pane = app.panes[0].id;
        app.perform(
            &ctx,
            Action::NavigateNewTab {
                pane,
                path: PathBuf::from("/two"),
            },
        );
        // Somewhere for it to have come *from*, so that the assertion below is about a history
        // being dropped rather than about a tab that never had one.
        app.perform(
            &ctx,
            Action::Navigate {
                pane,
                path: PathBuf::from("/two/deep"),
            },
        );
        assert_eq!(app.panes[0].tabs[1].history.len(), 2);

        app.perform(&ctx, Action::CloseTab { pane, tab: 1 });
        assert_eq!(app.panes[0].tabs.len(), 1);

        app.perform(&ctx, Action::ReopenTab);
        let back = app.panes[0].tabs.last().expect("the tab that came back");
        assert_eq!(
            back.path,
            PathBuf::from("/two/deep"),
            "the folder it was showing"
        );
        assert_eq!(
            back.history,
            [PathBuf::from("/two/deep")],
            "the path and nothing else -- not the trail it got there by"
        );
        assert_eq!(back.at, 0);
        assert!(!back.can_go_back());
        assert!(app.closed.is_empty(), "and it is spent, not repeatable");
    }

    #[test]
    fn only_the_last_ten_closed_tabs_are_kept() {
        let (mut app, ctx) = app(&["/keep"]);
        let pane = app.panes[0].id;
        let path = |n: usize| PathBuf::from(format!("/gone/{n}"));

        // Twelve opened and closed, so the two oldest fall off the back.
        for n in 0..12 {
            app.perform(&ctx, Action::NavigateNewTab { pane, path: path(n) });
            app.perform(&ctx, Action::CloseTab { pane, tab: 1 });
        }
        assert_eq!(app.closed.len(), CLOSED_TABS);
        assert_eq!(app.closed.first(), Some(&path(2)), "0 and 1 are gone");

        // Two more presses than there is anything to answer them with.
        for _ in 0..CLOSED_TABS + 2 {
            app.perform(&ctx, Action::ReopenTab);
        }
        let back: Vec<PathBuf> = app.panes[0].tabs[1..].iter().map(|t| t.path.clone()).collect();
        let expected: Vec<PathBuf> = (2..12).rev().map(path).collect();
        assert_eq!(
            back, expected,
            "most recently closed comes back first, and nothing older than ten comes back at all"
        );
    }

    #[test]
    fn reopening_with_nothing_closed_does_nothing() {
        let (mut app, ctx) = app(&["/only"]);
        app.perform(&ctx, Action::ReopenTab);
        assert_eq!(app.panes[0].tabs.len(), 1);
    }

    #[test]
    fn the_close_that_is_the_window_is_not_remembered() {
        // That close is the window going, and the history goes with it. Recording it would put
        // the folder back into a window that is on its way out.
        let (mut app, ctx) = app(&["/only"]);
        let pane = app.panes[0].id;
        app.perform(&ctx, Action::CloseTab { pane, tab: 0 });
        assert!(app.closed.is_empty());
    }

    #[test]
    fn a_tab_pulled_out_into_a_pane_of_its_own_was_not_closed() {
        let (mut app, ctx) = app(&["/a"]);
        let pane = app.panes[0].id;
        app.perform(
            &ctx,
            Action::NavigateNewTab {
                pane,
                path: PathBuf::from("/b"),
            },
        );
        app.perform(
            &ctx,
            Action::SplitTab {
                from: pane,
                tab: 1,
                target: pane,
                side: Side::Right,
            },
        );
        assert_eq!(app.panes.len(), 2);
        assert!(
            app.closed.is_empty(),
            "a tab that moved somewhere else was never closed"
        );
    }

    #[test]
    fn bookmarks_toggle_and_never_pin_this_pc() {
        let (mut app, ctx) = app(&["/a"]);
        let path = PathBuf::from("/a");
        app.perform(&ctx, Action::ToggleBookmark(path.clone()));
        assert!(app.is_bookmarked(&path));
        app.perform(&ctx, Action::ToggleBookmark(path.clone()));
        assert!(!app.is_bookmarked(&path));

        app.perform(&ctx, Action::ToggleBookmark(PathBuf::new()));
        assert!(app.bookmarks.is_empty(), "This PC is not a folder to pin");
    }

    /// Windows' `Pin to Quick access` pins *here*, to the sidebar in front of the user, and never
    /// reaches the shell.
    ///
    /// By verb and not by label, because the entry reads `Épingler à l'accès rapide` on this
    /// machine and `Pin to Quick access` on an English one. Everything else in the menu still
    /// belongs to the shell, which is what the last case holds down: an empty answer here is what
    /// sends a command on to [`crate::shell::Modal`], so a rule that matched too much would take
    /// entries away from Windows rather than adding one to this program.
    #[test]
    fn pin_to_quick_access_bookmarks_here_instead() {
        use crate::shell::menu::Command;

        // Real directories, because the rule only pins folders and asks the disk which is which.
        let dir = crate::sandbox::dir("pin-verb");
        let sub = dir.join("inner");
        std::fs::create_dir_all(&sub).expect("a folder to pin");
        let file = dir.join("one.txt");
        std::fs::write(&file, b"x").expect("a file that is not one");

        let shell = |verb: &str| Command::Shell {
            verb: Some(verb.to_owned()),
            id: 0,
            path: Vec::new(),
            label: "whatever Windows calls it".to_owned(),
        };
        let menu = |items: Vec<PathBuf>| {
            crate::ui::menu::Open::new(
                1,
                pos2(0.0, 0.0),
                items,
                dir.clone(),
                Vec::new(),
                crate::shell::menu::Depth::Full,
                0,
            )
        };

        // A selected folder.
        let on_selection = menu(vec![sub.clone()]);
        match App::pin_is_a_bookmark(&on_selection, &shell("pintohome")).as_slice() {
            [Action::AddBookmark(path)] => assert_eq!(*path, sub),
            other => panic!("`pintohome` on a folder gave {:?}", names(other)),
        }
        match App::pin_is_a_bookmark(&on_selection, &shell("unpinfromhome")).as_slice() {
            [Action::RemoveBookmark(path)] => assert_eq!(*path, sub),
            other => panic!("`unpinfromhome` on a folder gave {:?}", names(other)),
        }

        // Nothing selected is the folder the menu was raised in.
        match App::pin_is_a_bookmark(&menu(Vec::new()), &shell("pintohome")).as_slice() {
            [Action::AddBookmark(path)] => assert_eq!(*path, dir),
            other => panic!("`pintohome` on a background menu gave {:?}", names(other)),
        }

        // Several folders at once, which is what the shell would have pinned.
        let both = menu(vec![sub.clone(), dir.clone()]);
        assert_eq!(App::pin_is_a_bookmark(&both, &shell("pintohome")).len(), 2);

        // A file is not a bookmark, so the shell keeps it.
        let on_file = menu(vec![file.clone()]);
        assert!(
            App::pin_is_a_bookmark(&on_file, &shell("pintohome")).is_empty(),
            "a file was turned into a sidebar entry"
        );

        // And every other verb in the menu is still Windows'.
        for verb in ["open", "copy", "delete", "properties", "pintohomefile", "PinToStartScreen"] {
            assert!(
                App::pin_is_a_bookmark(&on_selection, &shell(verb)).is_empty(),
                "`{verb}` was taken off the shell"
            );
        }
        assert!(
            App::pin_is_a_bookmark(&on_selection, &Command::Own(crate::shell::menu::Own::CopyHere))
                .is_empty()
        );

        crate::sandbox::remove(&dir);
    }

    /// Windows' Open, Couper, Copier and Coller act *here*, and everything else in the menu is
    /// still the shell's.
    ///
    /// By verb throughout, for the reason on [`App::ours_rather_than_the_shell_s`]: the labels are
    /// in whatever language this Windows is in. What the shell offers under each of those verbs is
    /// held down separately, by
    /// `shell::menu::tests::the_verbs_this_program_takes_over_are_still_the_shell_s` — this test is
    /// the other half, that the verbs turn into the right actions once they arrive.
    ///
    /// The last two cases are the ones worth having: an empty answer is what sends a command on to
    /// [`crate::shell::Modal`], so a rule that matched too much would take entries *away* from
    /// Windows, which is the opposite of the point.
    #[test]
    fn the_shell_s_open_cut_copy_and_paste_act_in_this_explorer() {
        use crate::shell::menu::Command;

        // Real files and folders: the rules ask the disk which is which.
        let dir = crate::sandbox::fresh("redirected-verbs");
        let sub = dir.join("inner");
        let other = dir.join("second");
        std::fs::create_dir_all(&sub).expect("a folder");
        std::fs::create_dir_all(&other).expect("another folder");
        let file = dir.join("one.txt");
        std::fs::write(&file, b"x").expect("a file");

        let shell = |verb: &str| Command::Shell {
            verb: Some(verb.to_owned()),
            id: 0,
            path: Vec::new(),
            label: "whatever Windows calls it".to_owned(),
        };
        let menu = |items: Vec<PathBuf>| {
            crate::ui::menu::Open::new(
                7,
                pos2(0.0, 0.0),
                items,
                dir.clone(),
                Vec::new(),
                crate::shell::menu::Depth::Full,
                0,
            )
        };
        let ours = |items: Vec<PathBuf>, verb: &str| {
            App::ours_rather_than_the_shell_s(&menu(items), &shell(verb))
        };

        // Open on one folder navigates the pane the menu came from -- 7, not the focused one.
        match ours(vec![sub.clone()], "open").as_slice() {
            [Action::Navigate { pane, path }] => {
                assert_eq!(*pane, 7, "Open navigated a pane the menu was not raised in");
                assert_eq!(*path, sub);
            }
            other => panic!("Open on a folder gave {:?}", names(other)),
        }

        // Several folders get a tab each instead, leaving the pane where it is.
        match ours(vec![sub.clone(), other.clone()], "open").as_slice() {
            [
                Action::NavigateNewTab { path: first, .. },
                Action::NavigateNewTab { path: second, .. },
            ] => {
                assert_eq!(*first, sub);
                assert_eq!(*second, other);
            }
            got => panic!("Open on two folders gave {:?}", names(got)),
        }

        // A shortcut to a folder is a place too, and leads to the folder rather than to the `.lnk`.
        #[cfg(windows)]
        {
            let link = dir.join("to-inner.lnk");
            assert!(
                crate::shell::links::write_shortcut(&link, &sub),
                "could not write the shortcut this case is about"
            );
            match ours(vec![link.clone()], "open").as_slice() {
                [Action::Navigate { path, .. }] => assert_eq!(
                    *path, sub,
                    "Open on a folder shortcut did not follow it -- so it would have opened \
                     Explorer"
                ),
                got => panic!("Open on a folder shortcut gave {:?}", names(got)),
            }
        }

        // A file is the shell's: its `open` is the registered default verb, and this program has
        // nowhere to show a file anyway.
        for (what, items) in [
            ("one file", vec![file.clone()]),
            ("two files", vec![file.clone(), file.clone()]),
            ("the background", Vec::new()),
        ] {
            assert!(
                ours(items, "open").is_empty(),
                "Open on {what} was taken off the shell"
            );
        }

        // A folder and a file together: the folder here, the file the way `Enter` opens one. The
        // gesture is one `InvokeCommand` for the whole selection, so half of it cannot be left to
        // Windows -- and leaving all of it would open the folder in Explorer.
        match ours(vec![sub.clone(), file.clone()], "open").as_slice() {
            [Action::Navigate { path, .. }, Action::Open(opened)] => {
                assert_eq!(*path, sub);
                assert_eq!(*opened, file);
            }
            got => panic!("Open on a folder and a file gave {:?}", names(got)),
        }

        // Cut and Copy carry the selection the menu was raised over.
        match ours(vec![sub.clone(), file.clone()], "cut").as_slice() {
            [Action::CutItems(items)] => assert_eq!(*items, vec![sub.clone(), file.clone()]),
            got => panic!("Cut gave {:?}", names(got)),
        }
        match ours(vec![file.clone()], "copy").as_slice() {
            [Action::CopyItems(items)] => assert_eq!(*items, vec![file.clone()]),
            got => panic!("Copy gave {:?}", names(got)),
        }

        // Paste means into the selected folder, not into the folder the pane is showing.
        match ours(vec![sub.clone()], "paste").as_slice() {
            [Action::PasteIntoFolder(into)] => assert_eq!(*into, sub),
            got => panic!("Paste on a folder gave {:?}", names(got)),
        }
        // The shell offers it on any selection with a folder somewhere in it. The first folder is
        // the one; the file beside it is not pasted into.
        match ours(vec![file.clone(), sub.clone()], "paste").as_slice() {
            [Action::PasteIntoFolder(into)] => assert_eq!(*into, sub),
            got => panic!("Paste on a file and a folder gave {:?}", names(got)),
        }
        assert!(
            ours(vec![file.clone()], "paste").is_empty(),
            "Paste was answered for a selection with no folder in it to paste into"
        );

        // Nothing selected is the folder's background menu, which carries none of the three --
        // Explorer synthesises its own Paste there and this program has Ctrl+V. A shell that grew
        // one would fall through to it rather than being answered against no selection at all.
        for verb in ["cut", "copy", "paste"] {
            assert!(
                ours(Vec::new(), verb).is_empty(),
                "`{verb}` on a background menu was answered with no selection to act on"
            );
        }

        // Pinning still comes through here, unchanged.
        match ours(vec![sub.clone()], "pintohome").as_slice() {
            [Action::AddBookmark(path)] => assert_eq!(*path, sub),
            got => panic!("`pintohome` gave {:?}", names(got)),
        }

        // And the whole rest of the menu is Windows'. `openas`, `opennew` and `opencontaining`
        // are here because they *start with* the verb that is hooked, which is the mistake a
        // `starts_with` would make.
        for verb in [
            "delete", "rename", "properties", "link", "copyaspath", "edit", "print", "runas",
            "openas", "opennew", "opencontaining", "pintohomefile", "PinToStartScreen",
            "NewFolder", "ShareX", "{6A1F6B13-3B82-48A1-9E06-7BB0A6D0BFFD}",
        ] {
            assert!(
                ours(vec![sub.clone()], verb).is_empty(),
                "`{verb}` was taken off the shell"
            );
        }
        // An empty verb matches nothing rather than falling into a hook.
        assert!(ours(vec![sub.clone()], "").is_empty());
        // And an entry the shell gave no canonical verb for at all is the shell's too: there is
        // nothing to recognise it by, and guessing from the label is what none of this does. The
        // label here is the one a French Windows puts on `copy`, which is the mistake being ruled
        // out.
        assert!(App::ours_rather_than_the_shell_s(
            &menu(vec![sub.clone()]),
            &Command::Shell {
                verb: None,
                id: 25,
                path: Vec::new(),
                label: "Copier".to_owned(),
            },
        )
        .is_empty(), "an entry with no verb was matched on its label");

        crate::sandbox::remove(&dir);
    }

    /// This program's Paste on empty space pastes into the folder the menu was raised in.
    ///
    /// The counterpart of the redirected `paste` verb, which is the one on a *selected* folder. Both
    /// end at [`Action::PasteIntoFolder`] so that the two entries and Ctrl+V are one paste — see
    /// [`crate::shell::menu::Own::Paste`] for why this one has to be ours at all.
    #[test]
    fn our_paste_on_empty_space_pastes_into_the_folder_being_shown() {
        let dir = crate::sandbox::dir("own-paste");
        let (app, _ctx) = app(&["/a"]);
        // A background menu: nothing selected, raised in `dir`.
        let background = crate::ui::menu::Open::new(
            1,
            pos2(0.0, 0.0),
            Vec::new(),
            dir.clone(),
            Vec::new(),
            crate::shell::menu::Depth::Full,
            0,
        );
        match app.own_menu_action(&background, crate::shell::menu::Own::Paste) {
            Some(Action::PasteIntoFolder(into)) => assert_eq!(
                into, dir,
                "Paste went somewhere other than the folder the menu was raised in"
            ),
            other => panic!(
                "Paste gave {:?}",
                other.as_ref().map(Action::name)
            ),
        }
        // And Cancel is still the one own entry that means "do nothing", so a menu dismissed
        // through it does not paste.
        assert!(
            app.own_menu_action(&background, crate::shell::menu::Own::Cancel)
                .is_none()
        );
    }

    /// The join: the entry the shell really puts in the menu, through the real dispatch.
    ///
    /// Everything either side of this is covered on its own —
    /// `shell::menu::tests::the_verbs_this_program_takes_over_are_still_the_shell_s` holds the shell
    /// to the four verb names, and
    /// [`the_shell_s_open_cut_copy_and_paste_act_in_this_explorer`] holds the dispatch to the right
    /// actions — but both halves pass while the two are wired to different strings. So this one
    /// hands over a `Command` that was read out of an `HMENU` rather than one written here, and
    /// nothing about it is synthesised except which pane it came from.
    #[test]
    #[cfg(windows)]
    fn the_real_open_entry_from_the_real_menu_navigates_here() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();

        let dir = crate::sandbox::fresh("real-open-entry");
        let sub = dir.join("inner");
        std::fs::create_dir_all(&sub).expect("a folder to right-click");

        let entries = crate::shell::menu::build(&dir, std::slice::from_ref(&sub));
        assert!(
            !entries.is_empty(),
            "the shell gave no menu at all for a folder, so this test proves nothing"
        );
        let open = entries
            .iter()
            .find_map(|entry| match &entry.kind {
                crate::shell::menu::Kind::Command(
                    command @ crate::shell::menu::Command::Shell { verb: Some(verb), .. },
                ) if verb == "open" => Some(command.clone()),
                _ => None,
            })
            .expect("the shell's menu for a folder has an Open in it");

        let menu = crate::ui::menu::Open::new(
            3,
            pos2(0.0, 0.0),
            vec![sub.clone()],
            dir.clone(),
            entries,
            crate::shell::menu::Depth::Full,
            0,
        );
        match App::ours_rather_than_the_shell_s(&menu, &open).as_slice() {
            [Action::Navigate { pane, path }] => {
                assert_eq!(*pane, 3);
                assert_eq!(
                    *path, sub,
                    "the shell's own Open on a folder has to navigate this pane; anything else \
                     opens a second file manager over the top of this one"
                );
            }
            got => panic!(
                "the shell's real Open entry gave {:?} instead of navigating here",
                names(got)
            ),
        }

        crate::sandbox::remove(&dir);
    }

    /// Action names, for a panic message that says which ones came back.
    #[cfg(test)]
    fn names(actions: &[Action]) -> Vec<&'static str> {
        actions.iter().map(Action::name).collect()
    }

    #[test]
    fn split_focused_opens_the_folder_already_showing() {
        let (mut app, ctx) = app(&["/here"]);
        app.perform(&ctx, Action::SplitFocused { side: Side::Bottom });
        assert_eq!(app.panes.len(), 2);
        assert_eq!(app.panes[1].tab().path, PathBuf::from("/here"));
    }
}

/// Driving the interface with real pointer events.
///
/// Everything else in this program can be checked by reading it. Click targets
/// cannot: an interaction rect that another one happens to cover reads perfectly
/// correctly at the call site and simply does not respond, and the only way to know is
/// to press the pointer down at a coordinate and see what moves. So these tests run
/// whole frames through a real [`egui::Context`] with synthetic input and assert on
/// what the application actually did.
#[cfg(test)]
mod click_tests {
    use super::*;
    use egui::{pos2, vec2, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect};

    struct Harness {
        app: App,
        ctx: egui::Context,
        size: egui::Vec2,
        /// The harness drives the clock rather than letting it drift. egui counts
        /// clicks that arrive within 300ms of each other as a double or a triple, so
        /// two gestures in quick succession would run together — which is a property
        /// of the test, not of the program.
        time: f64,
        /// Held modifiers. `Event::PointerButton` carries a copy, but the code under
        /// test reads `InputState::modifiers`, which comes from this.
        modifiers: Modifiers,
        /// Whether the window has the platform's focus. `true` as `RawInput`'s own default is,
        /// so every test but the one about losing it is unaffected.
        focused: bool,
        /// What the last frame asked the pointer to look like.
        cursor: egui::CursorIcon,
        /// Everything the frames so far have asked the platform to do to the window, in order.
        commands: Vec<egui::ViewportCommand>,
        /// Every shape the last frame painted, in paint order.
        ///
        /// Replaced each frame rather than accumulated, because the question these answer is
        /// "what does the window look like now". They are how a test can assert on a *fill* —
        /// a colour is not a click target, and reading the source only proves the source says
        /// what it says.
        shapes: Vec<egui::Shape>,
    }

    impl Harness {
        /// One pane per requested count, all showing directories that exist and have
        /// something in them to click on.
        fn with_panes(count: usize) -> Self {
            let ctx = egui::Context::default();
            azur_egui_theme::fonts::install(&ctx);
            let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let open: Vec<PathBuf> = (0..count)
                .map(|i| if i == 0 { here.clone() } else { here.join("src") })
                .collect();
            let mut app = App::opening(&ctx, Config::default(), open, Side::Right);
            app.journal = Some(Vec::new());

            let mut harness = Harness {
                app,
                ctx,
                size: vec2(1024.0, 650.0),
                time: 1.0,
                modifiers: Modifiers::NONE,
                focused: true,
                cursor: egui::CursorIcon::Default,
                commands: Vec::new(),
                shapes: Vec::new(),
            };
            // The listing arrives by channel, so rects that depend on it do not exist
            // until a few frames have gone by.
            harness.settle();
            harness
        }

        fn new() -> Self {
            Self::with_panes(1)
        }

        /// Run frames until the listings have arrived.
        ///
        /// Not a fixed count: a scan comes back by channel from a worker thread, and eight
        /// frames of a test process sharing a machine with seven other test threads is
        /// sometimes not long enough — which showed up as this suite failing one run in
        /// three on an assertion about the *listing* rather than about anything the test
        /// was for. So it waits for the thing it is waiting for.
        fn settle(&mut self) {
            for attempt in 0..200 {
                self.frame(Vec::new());
                let arrived = self
                    .app
                    .panes
                    .iter()
                    .all(|pane| pane.tab().dir.is_some());
                if arrived && attempt >= 4 {
                    break;
                }
                if attempt >= 4 {
                    // Frames are free here; the disk is not.
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
            self.take_journal();
        }

        /// One pass, with whatever the platform brought and whatever the app had to add.
        ///
        /// The injected events matter as much as the given ones and are appended in the same
        /// order the real input hook appends them. A harness that skipped them would be testing
        /// a program that does not exist — which is how "the second drag does nothing" survived
        /// a suite that was green.
        fn frame(&mut self, events: Vec<Event>) {
            let mut events = events;
            events.extend(self.app.take_injected());
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                time: Some(self.time),
                modifiers: self.modifiers,
                focused: self.focused,
                events,
                ..Default::default()
            };
            let app = &mut self.app;
            let out = self.ctx.run_ui(input, |ui| app.frame(ui));
            // Taken from the frame's own output rather than read back off the context
            // afterwards: `run_ui` moves the platform output out on the way through, so a test
            // that asked the context what the cursor was got whatever the *next* frame had
            // accumulated so far, which is nothing.
            self.cursor = out.platform_output.cursor_icon;
            // What the frame asked the platform to do to the window. Collected rather than
            // sampled, because the interesting thing about a pair of these is their order.
            for output in out.viewport_output.values() {
                self.commands.extend(output.commands.iter().cloned());
            }
            self.shapes.clear();
            self.shapes
                .extend(out.shapes.into_iter().map(|clipped| clipped.shape));
        }

        /// Every rectangle the last frame filled, innermost last, as `(rect, corner, fill)`.
        ///
        /// Flattened out of the nested `Shape::Vec`s egui builds, because a shape's depth in
        /// that tree is an artefact of which `Ui` painted it.
        fn rects(&self) -> Vec<(Rect, egui::CornerRadius, egui::Color32)> {
            fn walk(shape: &egui::Shape, into: &mut Vec<(Rect, egui::CornerRadius, egui::Color32)>) {
                match shape {
                    egui::Shape::Rect(r) => into.push((r.rect, r.corner_radius, r.fill)),
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, into);
                        }
                    }
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for shape in &self.shapes {
                walk(shape, &mut out);
            }
            out
        }

        /// Where the last frame put each run of text, as `(top-left, the string)`.
        ///
        /// The counterpart to [`Self::rects`], and what lets a test assert that a word is in the
        /// same place in two different frames without re-deriving where either frame put it.
        fn texts(&self) -> Vec<(egui::Pos2, String)> {
            fn walk(shape: &egui::Shape, into: &mut Vec<(egui::Pos2, String)>) {
                match shape {
                    egui::Shape::Text(text) => {
                        into.push((text.pos, text.galley.text().to_owned()));
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, into);
                        }
                    }
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for shape in &self.shapes {
                walk(shape, &mut out);
            }
            out
        }

        /// Where the tooltip that is up ended up, frame and all, or `None` if none is.
        ///
        /// A tooltip is an `Area` in the `Tooltip` layer order, and its state is what knows the
        /// rect — the shapes only know where the *text* went, and the padding and the shadow are
        /// exactly the part a test about overlapping the cursor is asking after.
        fn tooltip_rect(&self) -> Option<Rect> {
            let layer = self.ctx.memory(|m| {
                m.areas()
                    .visible_layer_ids()
                    .into_iter()
                    .find(|layer| layer.order == egui::Order::Tooltip)
            })?;
            egui::AreaState::load(&self.ctx, layer.id).map(|area| area.rect())
        }

        /// Every run of text the last frame painted, as `(left edge and **baseline**, the string)`.
        ///
        /// The counterpart to [`Self::texts`], which gives where a galley's *box* was put. A box
        /// says nothing about whether two texts are level: two fonts have different line heights
        /// and different ascents, so galleys centred in the same rect end up with their baselines
        /// a point or two apart — which is exactly the fault that is invisible in the source and
        /// obvious on screen. This asks the question the eye asks.
        ///
        /// The x is the galley's left edge, unchanged, so a caller can pick out the runs inside
        /// one panel with `rect.contains`.
        fn baselines(&self) -> Vec<(Pos2, String)> {
            fn walk(shape: &egui::Shape, into: &mut Vec<(Pos2, String)>) {
                match shape {
                    egui::Shape::Text(text) => into.push((
                        pos2(
                            text.pos.x,
                            text.pos.y + azur::components::galley_baseline(&text.galley),
                        ),
                        text.galley.text().to_owned(),
                    )),
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, into);
                        }
                    }
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for shape in &self.shapes {
                walk(shape, &mut out);
            }
            out
        }

        /// The text of every run the last frame painted **that did not fit**, as the string it was
        /// asked to draw.
        ///
        /// Necessary because `Galley::text` hands back the *source* string and not the glyphs: a
        /// galley elided to `Fi…` still reports `Fit`, so a test that looked for an ellipsis in the
        /// drawn text was asking a question nothing could answer no. `Galley::elided` is the flag
        /// that knows, and it is the only way from out here to tell a label from a cropped one.
        fn cropped(&self) -> Vec<String> {
            fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(text) if text.galley.elided => {
                        into.push(text.galley.text().to_owned());
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, into);
                        }
                    }
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for shape in &self.shapes {
                walk(shape, &mut out);
            }
            out
        }

        /// Every outlined rectangle the last frame painted, as `(rect, the stroke's colour)`.
        ///
        /// The counterpart to [`Self::rects`], for the things that are a border and not a fill —
        /// a field's outline says what it is with its edge, and a test that only looked at fills
        /// could not tell it from nothing at all.
        fn outlines(&self) -> Vec<(Rect, egui::Color32)> {
            fn walk(shape: &egui::Shape, into: &mut Vec<(Rect, egui::Color32)>) {
                match shape {
                    egui::Shape::Rect(r) if r.stroke.width > 0.0 => {
                        into.push((r.rect, r.stroke.color));
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, into);
                        }
                    }
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for shape in &self.shapes {
                walk(shape, &mut out);
            }
            out
        }

        /// The fill of every polygon the last frame painted inside `rect`.
        ///
        /// How a painted glyph is asserted on: this program's icons are convex polygons, so
        /// "there is a glyph here, in this ink" is a question about the polygons whose ink lands
        /// in a given box.
        fn glyph_inks(&self, rect: Rect) -> Vec<egui::Color32> {
            self.glyph_shapes(rect)
                .into_iter()
                .map(|(_, fill)| fill)
                .collect()
        }

        /// Where a glyph's ink actually is: the union of the polygons painted inside `rect`.
        ///
        /// The rect a glyph is *handed* says nothing about where the drawing inside it ended up,
        /// which is exactly how a glyph in a row comes out misaligned while every measurement in
        /// the source reads `center()`.
        fn glyph_bounds(&self, rect: Rect) -> Option<Rect> {
            self.glyph_shapes(rect)
                .into_iter()
                .map(|(bounds, _)| bounds)
                .reduce(|a, b| a.union(b))
        }

        fn glyph_shapes(&self, rect: Rect) -> Vec<(Rect, egui::Color32)> {
            fn walk(shape: &egui::Shape, want: Rect, into: &mut Vec<(Rect, egui::Color32)>) {
                match shape {
                    egui::Shape::Path(path) => {
                        let bounds = path.visual_bounding_rect();
                        if want.contains_rect(bounds) {
                            into.push((bounds, path.fill));
                        }
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, want, into);
                        }
                    }
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for shape in &self.shapes {
                walk(shape, rect, &mut out);
            }
            out
        }

        /// The last fill painted at `rect`, to within half a point on every edge.
        ///
        /// The *last*, because that is the one you can see.
        fn fill_at(&self, rect: Rect) -> Option<(egui::CornerRadius, egui::Color32)> {
            self.rects()
                .into_iter()
                .rev()
                .find(|(painted, _, _)| {
                    painted.min.distance(rect.min) < 0.5 && painted.max.distance(rect.max) < 0.5
                })
                .map(|(_, corner, fill)| (corner, fill))
        }

        /// Run frames until the window stops asking for more, and say whether it did.
        ///
        /// A window with animations still running or icons still arriving asks for a repaint
        /// every frame, which makes "did anything ask for a repaint" an assertion that passes
        /// on its own. This is how a test gets a silent window to measure against.
        fn quiesce(&mut self) -> bool {
            for _ in 0..240 {
                self.frame(Vec::new());
                if !self.ctx.has_requested_repaint() {
                    self.take_journal();
                    return true;
                }
                self.time += 1.0 / 60.0;
            }
            false
        }

        /// Let enough time pass that the next click starts a fresh gesture.
        fn wait(&mut self) {
            self.time += 1.0;
            self.frame(Vec::new());
        }

        /// Move the pointer there and report whether `id` is the widget under it.
        ///
        /// This is the assertion that actually matters: a widget the pointer cannot
        /// reach is a widget that does not work, however correct its rect looks.
        fn hovers(&mut self, id: Id, at: Pos2) -> bool {
            self.frame(vec![Event::PointerMoved(at)]);
            self.ctx
                .read_response(id)
                .is_some_and(|response| response.hovered())
        }

        /// Sweep down a column of the window looking for `id`, and return where it
        /// was found. Beats hard-coding a y that any layout change invalidates.
        fn find(&mut self, id: Id, x: f32, ys: std::ops::Range<i32>) -> Option<Pos2> {
            for y in ys.step_by(2) {
                let at = pos2(x, y as f32);
                if self.hovers(id, at) {
                    return Some(at);
                }
            }
            None
        }

        /// Move, press, release — three frames, which is how a real click arrives.
        fn click_at(&mut self, at: Pos2) -> Vec<&'static str> {
            self.click_with(at, PointerButton::Primary, Modifiers::NONE)
        }

        fn click_with(
            &mut self,
            at: Pos2,
            button: PointerButton,
            modifiers: Modifiers,
        ) -> Vec<&'static str> {
            self.take_journal();
            self.modifiers = modifiers;
            self.frame(vec![Event::PointerMoved(at)]);
            for pressed in [true, false] {
                self.frame(vec![Event::PointerButton {
                    pos: at,
                    button,
                    pressed,
                    modifiers,
                }]);
            }
            // One more, so an action queued by the release is applied and drawn.
            self.frame(Vec::new());
            self.modifiers = Modifiers::NONE;
            self.take_journal()
        }

        fn double_click_at(&mut self, at: Pos2) -> Vec<&'static str> {
            // A fresh gesture: without this, the clicks of the previous one are still
            // inside egui's double-click window and these two count as a triple and a
            // quadruple.
            self.wait();
            self.take_journal();
            self.frame(vec![Event::PointerMoved(at)]);
            for _ in 0..2 {
                self.frame(vec![
                    Event::PointerButton {
                        pos: at,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    },
                    Event::PointerButton {
                        pos: at,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    },
                ]);
            }
            self.frame(Vec::new());
            self.take_journal()
        }

        /// Press, travel, release.
        fn drag(&mut self, from: Pos2, to: Pos2) -> Vec<&'static str> {
            self.take_journal();
            self.frame(vec![Event::PointerMoved(from)]);
            self.frame(vec![Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }]);
            // Several steps, because a drag threshold is a distance travelled.
            for step in 1..=6 {
                let t = step as f32 / 6.0;
                self.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
            }
            self.frame(vec![Event::PointerButton {
                pos: to,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }]);
            self.frame(Vec::new());
            self.take_journal()
        }

        /// Press and travel, and never release — which is what an OLE drag looks like from
        /// here. `DoDragDrop` takes the capture and its own loop swallows the button-up, so
        /// the release genuinely never arrives.
        fn drag_and_hold(&mut self, from: Pos2, to: Pos2) -> Vec<&'static str> {
            self.take_journal();
            self.frame(vec![Event::PointerMoved(from)]);
            self.frame(vec![Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }]);
            for step in 1..=6 {
                let t = step as f32 / 6.0;
                self.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
            }
            self.take_journal()
        }

        fn take_journal(&mut self) -> Vec<&'static str> {
            let journal = self.app.journal.take().unwrap_or_default();
            self.app.journal = Some(Vec::new());
            journal
        }

        fn pane_rect(&self, index: usize) -> Rect {
            self.app.panes[index].rect
        }

        /// Where a pane's own content starts: its path bar, one hairline in.
        fn pane_content_top(&self, index: usize) -> f32 {
            self.pane_rect(index).top() + 1.0
        }

        /// The vertical middle of the path bar of a pane.
        fn path_bar_y(&self, index: usize) -> f32 {
            self.pane_content_top(index) + crate::ui::breadcrumb::HEIGHT * 0.5
        }

        /// The vertical middle of the column header of a pane.
        fn header_y(&self, index: usize) -> f32 {
            self.pane_content_top(index)
                + crate::ui::breadcrumb::HEIGHT
                + crate::ui::filelist::HEADER_HEIGHT * 0.5
        }

        /// The middle of row `row` of a pane's listing.
        fn row_center(&self, index: usize, row: usize) -> Pos2 {
            let pane = self.pane_rect(index);
            let top = self.pane_content_top(index)
                + crate::ui::breadcrumb::HEIGHT
                + crate::ui::filelist::HEADER_HEIGHT;
            pos2(
                pane.center().x,
                top + crate::pane::ROW_HEIGHT * (row as f32 + 0.5),
            )
        }

        fn tab(&self, index: usize) -> &Tab {
            self.app.panes[index].tab()
        }

        /// How many textured quads the last frame drew.
        ///
        /// In the large-icon view that *is* the number of tiles showing a picture: a tile with one draws
        /// an image out of [`crate::shell::thumbs`]' atlas, and a tile without draws a painted glyph,
        /// which is shapes out of the font atlas rather than an image. Counting the paint rather than
        /// asking the service is deliberate — the question is what is on screen.
        fn images(&self) -> usize {
            fn walk(shape: &egui::Shape, found: &mut usize) {
                match shape {
                    // `Painter::image` builds a one-quad `Mesh` carrying the texture, which is what
                    // makes this countable at all; text is still a `Text` shape at this stage and only
                    // becomes a mesh in the tessellator. The font atlas is `Managed(0)`, so anything
                    // else is an atlas quad — a thumbnail, or a shell icon on a tree's folder row.
                    egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                        *found += 1;
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, found);
                        }
                    }
                    _ => {}
                }
            }
            let mut found = 0;
            for shape in &self.shapes {
                walk(shape, &mut found);
            }
            found
        }

        /// Run frames **only while the window asks for them**, and answer how many pictures ended up on
        /// screen.
        ///
        /// The "only while it asks" is the whole point and it took a wrong version of this to see why.
        /// A loop that simply runs six hundred frames cannot catch the failure it was written for: this
        /// program **paints on demand**, and the bug was a view that could not fill itself in *without*
        /// frames it never asked for. Handed frames for free it filled in perfectly, and the test passed
        /// against the broken code and the fixed one alike — which is worse than no test.
        ///
        /// So this waits instead. A frame is drawn while egui says one is wanted; when nothing is, it
        /// sits for [`QUIET`] and only carries on if something asks — a worker landing an answer calls
        /// `request_repaint`, so real work still wakes it. Nothing asking for [`QUIET`] means the window
        /// is genuinely idle and whatever is on screen is what the user would be looking at.
        fn pictures_once_settled(&mut self) -> usize {
            /// Longer than the heartbeat `Thumbs::request` books when it has nowhere to put an answer,
            /// so a view that recovers slowly is counted as recovering rather than as stuck.
            const QUIET: std::time::Duration = std::time::Duration::from_millis(900);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut best = 0;
            while std::time::Instant::now() < deadline {
                self.frame(Vec::new());
                best = best.max(self.images());
                if self.ctx.has_requested_repaint() {
                    continue;
                }
                let quiet = std::time::Instant::now() + QUIET;
                while std::time::Instant::now() < quiet && !self.ctx.has_requested_repaint() {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                if !self.ctx.has_requested_repaint() {
                    break;
                }
            }
            best
        }
    }


    /// Which widget is actually on top at a position, and which merely contain it.
    ///
    /// A blocked click is invisible in the source, so this asks egui directly: any id
    /// that `contains_pointer` but is not `hovered` has something over it, and the one
    /// that is `hovered` is what took the click.
    #[test]
    #[ignore = "diagnostic; run explicitly"]
    fn what_is_under_the_pointer() {
        let mut h = Harness::with_panes(1);
        let pane = h.app.panes[0].id;
        let rect = h.pane_rect(0);
        println!("window {:?}  pane {:?}", h.size, rect);

        let probes: Vec<(String, Id)> = vec![
            ("rows-hit".into(), Id::new(("rows-hit", pane))),
            ("pane-claim".into(), Id::new(("pane-claim", pane))),
            ("caption-drag".into(), Id::new("caption-drag")),
            ("sidebar-grip".into(), Id::new("sidebar-grip")),
            ("app-menu".into(), Id::new("app-menu")),
            ("th0".into(), Id::new(("th", pane, 0usize))),
            ("th1".into(), Id::new(("th", pane, 1usize))),
            ("th2".into(), Id::new(("th", pane, 2usize))),
            ("th3".into(), Id::new(("th", pane, 3usize))),
            ("refresh".into(), Id::new(("refresh", pane))),
            ("bookmark".into(), Id::new(("bookmark-toggle", pane))),
            ("resize-n".into(), Id::new(("yafe-resize", "n"))),
            ("resize-s".into(), Id::new(("yafe-resize", "s"))),
            ("resize-e".into(), Id::new(("yafe-resize", "e"))),
            ("resize-w".into(), Id::new(("yafe-resize", "w"))),
            ("resize-nw".into(), Id::new(("yafe-resize", "nw"))),
            ("resize-ne".into(), Id::new(("yafe-resize", "ne"))),
            ("resize-sw".into(), Id::new(("yafe-resize", "sw"))),
            ("resize-se".into(), Id::new(("yafe-resize", "se"))),
        ];

        let row_y = h.row_center(0, 0).y;
        let header_y = h.header_y(0);
        let bar_y = h.path_bar_y(0);
        let spots = [
            ("row left", pos2(rect.left() + 14.0, row_y)),
            ("row name", pos2(rect.left() + 120.0, row_y)),
            ("row middle", pos2(rect.center().x, row_y)),
            ("row right", pos2(rect.right() - 12.0, row_y)),
            ("header left", pos2(rect.left() + 60.0, header_y)),
            ("header right", pos2(rect.right() - 60.0, header_y)),
            ("path bar right", pos2(rect.right() - 30.0, bar_y)),
            ("title bar left", pos2(16.0, crate::ui::chrome::HEIGHT * 0.5)),
            ("sidebar row", pos2(crate::ui::GUTTER + 80.0, 360.0)),
        ];

        for (label, at) in spots {
            h.frame(vec![Event::PointerMoved(at)]);
            let mut hovered = Vec::new();
            let mut blocked = Vec::new();
            for (name, id) in &probes {
                if let Some(r) = h.ctx.read_response(*id) {
                    if r.hovered() {
                        hovered.push(name.clone());
                    } else if r.contains_pointer() {
                        blocked.push(name.clone());
                    }
                }
            }
            println!(
                "{at:?} {label:<16} hovered={hovered:?}  blocked={blocked:?}"
            );
        }

        println!("
--- rects ---");
        h.frame(vec![Event::PointerMoved(pos2(rect.center().x, row_y))]);
        for (name, id) in &probes {
            if let Some(r) = h.ctx.read_response(*id) {
                println!(
                    "{name:<14} rect={:?}  interact={:?}",
                    r.rect, r.interact_rect
                );
            }
        }
    }

    #[test]
    fn the_harness_has_something_to_click() {
        let h = Harness::new();
        assert!(h.tab(0).dir.is_some(), "the listing has to have arrived");
        assert!(!h.tab(0).order.is_empty());
        assert!(h.pane_rect(0).width() > 200.0, "and the pane has to be laid out");
    }

    // ---- The details view ----------------------------------------------

    #[test]
    fn a_row_is_clickable_across_its_whole_width() {
        let mut h = Harness::new();
        let pane = h.pane_rect(0);
        let y = h.row_center(0, 0).y;

        for (label, x) in [
            ("the glyph", pane.left() + 14.0),
            ("the name", pane.left() + 120.0),
            ("the middle", pane.center().x),
            ("the far right", pane.right() - 12.0),
        ] {
            h.app.panes[0].tab_mut().clear_selection();
            h.click_at(pos2(x, y));
            assert_eq!(
                h.tab(0).selected_count,
                1,
                "clicking {label} (x={x}) did not select the row"
            );
            assert_eq!(h.tab(0).cursor, Some(0), "and it is the first row");
        }
    }

    #[test]
    fn double_clicking_anywhere_on_a_row_opens_it() {
        let mut h = Harness::new();
        assert!(h.tab(0).is_dir_at(0), "the first row should be a folder");
        let pane = h.pane_rect(0);

        for x in [pane.left() + 120.0, pane.center().x, pane.right() - 12.0] {
            let done = h.double_click_at(pos2(x, h.row_center(0, 0).y));
            assert!(
                done.contains(&"Navigate"),
                "a double click at x={x} has to open the folder, got {done:?}"
            );
            h.app.panes[0].tab_mut().navigate(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
            h.settle();
            h.wait();
        }
    }

    #[test]
    fn clicking_below_the_rows_clears_the_selection() {
        let mut h = Harness::new();
        h.click_at(h.row_center(0, 0));
        assert_eq!(h.tab(0).selected_count, 1);

        let pane = h.pane_rect(0);
        h.click_at(pos2(pane.center().x, pane.bottom() - 40.0));
        assert_eq!(
            h.tab(0).selected_count,
            0,
            "empty space below the rows cancels a selection"
        );
    }

    #[test]
    fn ctrl_click_adds_to_the_selection() {
        let mut h = Harness::new();
        h.click_at(h.row_center(0, 0));
        h.click_with(
            h.row_center(0, 2),
            PointerButton::Primary,
            Modifiers::COMMAND,
        );
        assert_eq!(h.tab(0).selected_count, 2);
    }

    #[test]
    fn shift_click_selects_a_range() {
        let mut h = Harness::new();
        h.click_at(h.row_center(0, 0));
        h.click_with(h.row_center(0, 3), PointerButton::Primary, Modifiers::SHIFT);
        assert_eq!(h.tab(0).selected_count, 4);
    }

    #[test]
    fn a_column_header_sorts() {
        let mut h = Harness::new();
        let pane = h.pane_rect(0);
        let before = (h.tab(0).sort_by, h.tab(0).ascending);
        let done = h.click_at(pos2(pane.left() + 60.0, h.header_y(0)));
        assert!(done.contains(&"Sort"), "the Name header has to sort, got {done:?}");
        assert_ne!(
            (h.tab(0).sort_by, h.tab(0).ascending),
            before,
            "and the order has to change"
        );
    }

    #[test]
    fn every_column_header_is_reachable() {
        let mut h = Harness::new();
        let y = h.header_y(0);
        for (index, column) in crate::fs::Column::ALL.into_iter().enumerate() {
            let id = Id::new(("th", h.app.panes[0].id, index));
            let pane = h.pane_rect(0);
            let found = (0..pane.width() as i32)
                .step_by(4)
                .map(|dx| pos2(pane.left() + dx as f32, y))
                .find(|at| h.hovers(id, *at));
            assert!(
                found.is_some(),
                "the {} header is not reachable by the pointer",
                column.header()
            );
        }
    }

    // ---- The path bar ---------------------------------------------------

    #[test]
    fn the_history_buttons_respond() {
        let mut h = Harness::new();
        let y = h.path_bar_y(0);
        let pane = h.app.panes[0].id;
        let id = Id::new(("nav", pane, "Up (Alt+Up)"));
        let left = h.pane_rect(0).left();
        let at = (0..120)
            .step_by(2)
            .map(|dx| pos2(left + dx as f32, y))
            .find(|at| h.hovers(id, *at))
            .expect("the Up button is not reachable by the pointer");
        let done = h.click_at(at);
        assert!(done.contains(&"Up"), "Up did not respond, got {done:?}");
    }

    /// Refresh is in the group at the left, and the star that was beside the filter is gone.
    ///
    /// Both halves matter. Refresh moved *into* the never-dropped group, so it is found by
    /// scanning from the pane's left edge and its id is the group's — a test still looking for
    /// `("refresh", pane)` out on the right would pass on a bar that had lost the button
    /// entirely. And a button removed from a toolbar has to leave its *function* reachable, or
    /// "we tidied the bar" means "we deleted the feature": `Ctrl+D` is checked here for that
    /// reason, not for the shortcut's own sake.
    #[test]
    fn refresh_is_beside_up_and_the_bookmark_star_is_gone() {
        let mut h = Harness::new();
        let y = h.path_bar_y(0);
        let pane = h.app.panes[0].id;
        let left = h.pane_rect(0).left();

        let id = Id::new(("nav", pane, "Refresh (F5)"));
        let at = (0..140)
            .step_by(2)
            .map(|dx| pos2(left + dx as f32, y))
            .find(|at| h.hovers(id, *at))
            .expect("Refresh is not reachable from the left-hand group");
        let done = h.click_at(at);
        assert!(done.contains(&"Refresh"), "Refresh did not fire, got {done:?}");

        // Immediately after Up: the buttons touch, so this is a claim about the two rects and not
        // about wherever the sweep above happened to land.
        let rect_of = |h: &Harness, tip: &str| {
            h.ctx
                .read_response(Id::new(("nav", pane, tip)))
                .map(|r| r.rect)
                .unwrap_or_else(|| panic!("the {tip} button was not laid out"))
        };
        let up = rect_of(&h, "Up (Alt+Up)");
        let refresh = rect_of(&h, "Refresh (F5)");
        assert_eq!(
            refresh.left(),
            up.right(),
            "Refresh starts at {} and Up ends at {}",
            refresh.left(),
            up.right()
        );

        h.frame(Vec::new());
        assert!(
            h.ctx
                .read_response(Id::new(("bookmark-toggle", pane)))
                .is_none(),
            "the bookmark star is still on the bar"
        );

        // And the thing it used to do is still done. The modifiers go on the harness as well as
        // on the event, because the code under test reads `InputState::modifiers`.
        h.take_journal();
        h.modifiers = Modifiers::COMMAND;
        h.frame(vec![Event::Key {
            key: egui::Key::D,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }]);
        h.modifiers = Modifiers::NONE;
        h.frame(Vec::new());
        let done = h.take_journal();
        assert!(
            done.contains(&"ToggleBookmark"),
            "Ctrl+D no longer bookmarks, so removing the star removed the feature: {done:?}"
        );
    }

    #[test]
    #[ignore = "diagnostic; run explicitly"]
    fn where_are_the_crumbs() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let crumbs = crate::fs::breadcrumb_segments(&h.tab(0).trail);
        println!(
            "path={:?}\ntrail={:?}\nactive={} of {}",
            h.tab(0).path,
            h.tab(0).trail,
            crate::ui::breadcrumb::active_index(&crumbs, &h.tab(0).path),
            crumbs.len()
        );
        let y = h.path_bar_y(0);
        h.frame(vec![Event::PointerMoved(pos2(h.pane_rect(0).left() + 40.0, y))]);
        for (index, (label, _)) in crumbs.iter().enumerate() {
            let rect = h.ctx.read_response(Id::new(("crumb", pane, index))).map(|r| r.rect);
            println!("  {index} {label:<32} {rect:?}");
        }
        println!(
            "  overflow {:?}\n  pane {:?}  bar_y {y}",
            h.ctx
                .read_response(Id::new(("crumb-overflow", pane)))
                .map(|r| r.rect),
            h.pane_rect(0)
        );
    }

    #[test]
    fn a_breadcrumb_segment_navigates() {
        let mut h = Harness::new();
        let y = h.path_bar_y(0);
        let pane = h.app.panes[0].id;

        // The parent of the folder being shown, found rather than assumed: which segments
        // are on the bar depends on how much of the path fits, and the leading ones collapse
        // into the `…`. This one is next to the current folder, so it is there whenever
        // anything is. (It used to look for segment 0, This PC, on the grounds that it is
        // always present — which stopped being true the day the path bar got 27px narrower.)
        let crumbs = crate::fs::breadcrumb_segments(&h.tab(0).trail);
        let parent = crate::ui::breadcrumb::active_index(&crumbs, &h.tab(0).path) - 1;
        let id = Id::new(("crumb", pane, parent));
        let left = h.pane_rect(0).left();
        let at = (0..900)
            .step_by(2)
            .map(|dx| pos2(left + dx as f32, y))
            .find(|at| h.hovers(id, *at))
            .expect("the parent breadcrumb segment is not reachable");

        let done = h.click_at(at);
        assert!(
            done.contains(&"Navigate"),
            "a breadcrumb segment has to navigate, got {done:?}"
        );
        assert_eq!(
            h.tab(0).path,
            crumbs[parent].1,
            "and it has to go to the folder it names"
        );
    }

    #[test]
    fn after_going_up_the_folder_left_is_still_on_the_breadcrumb() {
        // The whole point of `Tab::trail`: walk up, and the folder just left is a segment
        // you can click rather than a name to go hunting for in a chevron menu.
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let was = h.tab(0).path.clone();

        h.app.panes[0].tab_mut().go_up();
        h.settle();
        assert_eq!(h.tab(0).trail, was, "the trail has to outlive the move");

        // The segment for the folder we came out of is the last one on the bar, past the
        // one now in bold.
        let crumbs = crate::fs::breadcrumb_segments(&h.tab(0).trail);
        let deepest = crumbs.len() - 1;
        assert_eq!(crumbs[deepest].1, was);
        assert!(
            crate::ui::breadcrumb::active_index(&crumbs, &h.tab(0).path) < deepest,
            "the bold segment should be an ancestor of the end of the trail"
        );

        // And it is reachable by the pointer and navigates -- the two things a segment
        // drawn in the right place can still fail to do.
        let y = h.path_bar_y(0);
        let left = h.pane_rect(0).left();
        let at = (0..900)
            .step_by(2)
            .map(|dx| pos2(left + dx as f32, y))
            .find(|at| h.hovers(Id::new(("crumb", pane, deepest)), *at))
            .expect("the segment past the current folder cannot be reached");
        let done = h.click_at(at);
        assert!(
            done.contains(&"Navigate"),
            "clicking back down the trail has to navigate, got {done:?}"
        );
        assert_eq!(h.tab(0).path, was, "and it goes back where it came from");
    }

    // ---- The title bar --------------------------------------------------

    #[test]
    fn the_application_mark_is_reachable() {
        let mut h = Harness::new();
        let found = h.find(
            Id::new("app-menu"),
            crate::ui::GUTTER + crate::ui::TOOL_SIZE * 0.5,
            0..crate::ui::chrome::HEIGHT as i32,
        );
        assert!(
            found.is_some(),
            "the application mark cannot be reached -- something is covering the \
             top-left corner of the title bar"
        );
    }

    #[test]
    fn the_window_buttons_are_reachable() {
        let mut h = Harness::new();
        for (which, from_right) in [
            (WindowAction::Close, 23.0),
            (WindowAction::ToggleMaximize, 69.0),
            (WindowAction::Minimize, 115.0),
        ] {
            let id = Id::new(("caption", which as u8));
            let found = h.find(
                id,
                h.size.x - from_right,
                0..crate::ui::chrome::HEIGHT as i32,
            );
            assert!(
                found.is_some(),
                "{which:?} cannot be reached -- the top-right corner is covered"
            );
        }
    }

    #[test]
    fn a_tab_and_its_close_button_are_clickable() {
        let mut h = Harness::new();
        h.app.panes[0]
            .tabs
            .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
        h.app.panes[0].active = 1;
        h.settle();

        let pane = h.app.panes[0].id;
        let strip_y = crate::ui::chrome::HEIGHT * 0.5;

        let at = (0..500)
            .step_by(2)
            .map(|dx| pos2(dx as f32, strip_y))
            .find(|at| h.hovers(Id::new(("tab", pane, 0usize)), *at))
            .expect("the first tab is not reachable");
        let done = h.click_at(at);
        assert!(
            done.contains(&"ActivateTab"),
            "a tab did not respond, got {done:?}"
        );
        assert_eq!(h.app.panes[0].active, 0);

        let close = (0..500)
            .step_by(2)
            .map(|dx| pos2(dx as f32, strip_y))
            .find(|at| h.hovers(Id::new(("tab-close", pane, 0usize)), *at))
            .expect("a tab's close button is not reachable");
        let done = h.click_at(close);
        assert!(
            done.contains(&"CloseTab"),
            "the close button did not fire, got {done:?}"
        );
        assert_eq!(h.app.panes[0].tabs.len(), 1);
    }

    /// The console's switch takes a click, and it is the same toggle the shortcut is.
    ///
    /// **The reachability is the point of it.** `chrome::resize_borders` is registered *last* so that
    /// it sits over everything, egui gives the pointer to the last widget that asked for it, and the
    /// bottom band overlaps the status line — which the layout table in the README notes as costing
    /// nothing precisely because nothing there was ever clickable before. So this sweeps the bar for
    /// something under the pointer before it asserts anything about what a click does; a switch that
    /// looks perfect and cannot be reached is a switch that does not work.
    ///
    /// It stops one frame short of letting the console *draw*, and puts the flag back by hand.
    /// Drawing the panel starts a real shell — with the developer's own `.bashrc` — and a test process
    /// has no business doing that. What is being checked is the switch, and the switch has done its
    /// job by the time the flag moves.
    #[test]
    fn the_console_switch_is_reachable_and_toggles_the_console() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let id = Id::new(("console-switch", pane));
        let rect = h.app.panes[0].rect;
        let y = rect.bottom() - crate::ui::filelist::STATUS_HEIGHT * 0.5;
        let at = (0..120)
            .step_by(2)
            .map(|dx| pos2(rect.left() + dx as f32, y))
            .find(|at| h.hovers(id, *at))
            .expect("the console switch is not reachable along its own bar");

        assert!(!h.app.panes[0].console_open, "it starts shut");
        h.take_journal();
        h.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
        let done = h.take_journal();
        assert!(
            done.contains(&"ToggleConsole"),
            "the switch did not fire, got {done:?}"
        );
        assert!(h.app.panes[0].console_open, "the console did not open");
        h.app.panes[0].console_open = false;
    }

    /// The view switch takes a click, and what it switches to can be clicked as well.
    ///
    /// **Two claims, and both of them are only checkable this way.**
    ///
    /// The switch sits in the status line, which the window's bottom resize band overlaps — and
    /// `chrome::resize_borders` is registered last, so it wins the pointer wherever the two meet. It is
    /// also the *first* thing on that bar now, with the console's switch four points to its right, so
    /// there are two 18-point targets to tell apart in a 22-point strip. So the bar is swept for
    /// something under the pointer before anything is asserted about a click, and the sweep runs
    /// **outwards from the left**, where a version of this that had drifted onto its neighbour would
    /// show up as the wrong id being hovered rather than as nothing at all.
    ///
    /// Then the grid itself. A tile is not a widget — the whole view is one interaction with the cell
    /// derived from the pointer — so "the tiles are where the arithmetic says" is not something
    /// `read_response` can be asked. It is asked by clicking and seeing what gets selected, and the
    /// first line is *swept* rather than computed: the tiles are spread across the pane, so where the
    /// first one's box begins is arithmetic this test would only be restating.
    #[test]
    fn the_view_switch_is_reachable_and_its_tiles_can_be_clicked() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let rect = h.pane_rect(0);
        let y = rect.bottom() - crate::ui::filelist::STATUS_HEIGHT * 0.5;
        let id = Id::new(("view-switch", pane));
        let at = (0..60)
            .step_by(2)
            .map(|dx| pos2(rect.left() + dx as f32, y))
            .find(|at| h.hovers(id, *at))
            .expect("the view switch is not reachable along its own bar");
        // And it is in *front* of the console's, which is the position that was asked for. Checked
        // from the far side of the pointer's own reach: the console switch has to be somewhere to the
        // right of where this one answered.
        let console = (0..120)
            .step_by(2)
            .map(|dx| pos2(rect.left() + dx as f32, y))
            .find(|probe| h.hovers(Id::new(("console-switch", pane)), *probe))
            .expect("the console switch went missing when the view switch moved in beside it");
        assert!(
            console.x > at.x,
            "the view switch is at {} and the console's at {}, which is the wrong way round",
            at.x,
            console.x
        );

        assert_eq!(
            h.tab(0).view_mode,
            crate::pane::ViewMode::Details,
            "it starts in the details view"
        );
        let done = h.click_at(at);
        assert!(
            done.contains(&"SetView"),
            "the switch did not fire, got {done:?}"
        );
        assert_eq!(h.tab(0).view_mode, crate::pane::ViewMode::Icons);

        // A frame of the grid, then a sweep along its first line of tiles for a click that lands on
        // one. The selection is cleared between tries, since a click in the gap between two tiles is
        // a click on the folder and cancels it — which is the other half of what is being checked.
        h.settle();
        let body_top = h.pane_content_top(0) + crate::ui::breadcrumb::HEIGHT;
        let line = body_top + crate::ui::grid::CELL_H * 0.5;
        let mut landed = None;
        for dx in (0..260).step_by(6) {
            let at = pos2(rect.left() + dx as f32, line);
            h.click_at(at);
            if h.tab(0).selected_count == 1 {
                landed = Some(at);
                break;
            }
        }
        landed.expect("no click along the first line of tiles selected anything");
        assert_eq!(h.tab(0).selected_count, 1);
        assert_eq!(
            h.tab(0).cursor,
            Some(0),
            "the leftmost tile of the first line is the first row of the order"
        );

        // And the switch goes back, which is the whole of what a two-state switch has to do. Found
        // again rather than reused, so that a latched switch is proved reachable as well as an
        // unlatched one — a fill is drawn under it in that state and a hit rect is not a fill.
        let back = (0..60)
            .step_by(2)
            .map(|dx| pos2(rect.left() + dx as f32, y))
            .find(|at| h.hovers(id, *at))
            .expect("the switch is not reachable while it is latched");
        let done = h.click_at(back);
        assert!(done.contains(&"SetView"), "got {done:?}");
        assert_eq!(h.tab(0).view_mode, crate::pane::ViewMode::Details);
    }

    /// The status line's `N changed` is a button, and pressing it shows what has changed.
    ///
    /// Three things, and the middle one is the reason this is a test rather than a reading of the
    /// source. **It has to be reachable**: it sits in the same bar the window's bottom resize band
    /// overlaps, and that band is registered last — the console switch at the other end of the line
    /// needed the same sweep for the same reason. **It has to be quiet until the pointer is on it**,
    /// which is what "subtle" means here and is a claim about a fill, not about the source. And the
    /// press has to do *both* halves: a flatten on its own is the whole tree, and a filter on its own
    /// is one folder's children.
    ///
    /// The count is stubbed rather than read. Git is asked for real in this suite — the harness opens
    /// this repository — so the button's existence would otherwise depend on whether the checkout
    /// happens to be dirty, which is to say on who is running the test.
    #[test]
    fn the_changed_count_is_a_button_that_shows_what_has_changed() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let ctx = h.ctx.clone();

        // `src`, not the repository root the harness opens: the press this test ends with starts a
        // deep walk, and the root has `target` under it.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        h.app.perform(&ctx, Action::Navigate { pane, path });
        h.settle();
        // The real answer first, and then over the top of it — an answer still in flight would
        // otherwise land on a later frame and replace the stub mid-test.
        for _ in 0..200 {
            if h.tab(0).git_answered {
                break;
            }
            h.frame(Vec::new());
        }
        {
            let mut repo = crate::git::Repo::of([("app.rs", crate::git::State::Modified)]);
            repo.head = "master".to_owned();
            repo.changed = 1;
            repo.unstaged = 1;
            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
            tab.git = Some(std::sync::Arc::new(repo));
            tab.git_answered = true;
        }
        h.frame(Vec::new());

        let id = Id::new(("status-changed", pane));
        let rect = h.app.panes[0].rect;
        let y = rect.bottom() - crate::ui::filelist::STATUS_HEIGHT * 0.5;
        let at = (0..300)
            .step_by(2)
            .map(|dx| pos2(rect.left() + dx as f32, y))
            .find(|at| h.hovers(id, *at))
            .expect("`N changed` is not reachable along its own bar");

        // Nothing under it at rest, and something under it hovered. The pointer is parked off the
        // bar for the first half: `hovers` left it on the button.
        let hover_fill = crate::ui::control_fills(&h.app.theme, h.app.theme.bg.layer_alt).0;
        let button_shaped = move |rect: &Rect, fill: &egui::Color32| {
            *fill == hover_fill
                && rect.contains(at)
                && rect.height() <= crate::ui::filelist::STATUS_HEIGHT
        };
        h.frame(vec![Event::PointerMoved(pos2(rect.center().x, y - 200.0))]);
        assert!(
            !h.rects()
                .iter()
                .any(|(r, _, fill)| button_shaped(r, fill)),
            "the count is wearing a control's fill with the pointer nowhere near it"
        );
        h.frame(vec![Event::PointerMoved(at)]);
        assert!(
            h.rects().iter().any(|(r, _, fill)| button_shaped(r, fill)),
            "hovering the count did not light it up"
        );

        let done = h.click_at(at);
        assert!(
            done.contains(&"ShowChanges"),
            "the count did not fire, got {done:?}"
        );
        assert!(h.tab(0).flat, "the flatten was not turned on");
        assert_eq!(
            h.tab(0).filter,
            crate::fs::sort::CHANGED,
            "the filter does not ask git"
        );

        // And pressed again it stays on, because it is not a toggle: the second press is about the
        // filter, and a button labelled with a fact about the repository should not undo itself.
        //
        // Through the action rather than through the pixels, and the reason is worth writing down: a
        // flatten **re-asks git**, because the marks are keyed by path relative to the folder and a
        // flattened listing's rows are not the same paths. So the count is legitimately absent for
        // the frame or two that takes, and a second sweep of the bar would be waiting on a real
        // `git status` of a real checkout to say something in particular.
        h.app.perform(&ctx, Action::ShowChanges(pane));
        assert!(h.tab(0).flat, "a second press turned the flatten back off");
        assert_eq!(h.tab(0).filter, crate::fs::sort::CHANGED);
    }

    /// Scrolled to the end, the space under the last file is [`filelist::TAIL`].
    ///
    /// The figure is the point of the test, but the *mechanism* is why it is worth having: the slack
    /// is a row and a half, `ScrollArea::show_rows` reserves whole rows only, and `filelist::rows`
    /// therefore does that function's arithmetic itself over `show_viewport`. Nothing on screen says
    /// which of the two it is using — a listing virtualised wrongly looks perfect until it is scrolled
    /// — so this measures the one thing that would change if it drifted back.
    ///
    /// Measured off the rows rather than off the scroll extent, and against the body rect the pane
    /// was actually given — `Pane::drop_area` is that rect — rather than one added up again from the
    /// furniture's heights, which would make this a test of its own arithmetic.
    #[test]
    fn the_listing_leaves_half_a_row_of_slack_under_the_last_file() {
        let mut h = Harness::new();
        // Short enough that the folder overflows it, so there is an end to scroll to at all.
        h.size = egui::vec2(1024.0, 300.0);
        h.settle();

        let pane = h.app.panes[0].id;
        let body = h.app.panes[0].drop_area;
        let count = h.tab(0).order.len();
        let rows = count as f32 * crate::pane::ROW_HEIGHT;
        assert!(
            rows > body.height(),
            "this folder fits in the pane, so there is no end to scroll to"
        );

        // As far as it will go. `ScrollArea` clamps an offset to the extent it has, which is the
        // extent this is about — so asking for the whole listing's worth is asking for exactly the
        // end, whatever that turns out to be.
        h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(rows);
        h.frame(Vec::new());
        h.frame(Vec::new());

        let slack = body.bottom() - (body.top() + rows - h.tab(0).scroll_y);
        assert!(
            (slack - filelist::TAIL).abs() < 0.5,
            "at the end of the listing the gap under the last row is {slack}, not {}",
            filelist::TAIL
        );
    }

    #[test]
    fn the_new_tab_button_makes_a_tab() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let before = h.app.panes[0].tabs.len();
        let at = (0..600)
            .step_by(2)
            .map(|dx| pos2(dx as f32, crate::ui::chrome::HEIGHT * 0.5))
            .find(|at| h.hovers(Id::new(("new-tab", pane)), *at))
            .expect("the + button is not reachable");
        let done = h.click_at(at);
        assert!(done.contains(&"NewTab"), "the + did not fire, got {done:?}");
        assert_eq!(h.app.panes[0].tabs.len(), before + 1);
    }

    /// One hover grey, everywhere, whatever kind of thing is under the pointer.
    ///
    /// The window had two for a while and it was not obvious which: the rows, the sidebar and the
    /// path bar's segments moved to this program's own grey, and Back, Forward, Up and Refresh went
    /// on wearing Azur's `control-hover` because they reach it through `ui::control_fills` rather
    /// than through `ui::row_fill`. Four buttons in the middle of the window, a rung and a half off
    /// everything around them. The fix was to stop having two sources — `background-control-hover`
    /// is set by `crate::theme` and everything reads it — and this is the guard, over four widgets
    /// that get there by four different routes.
    #[test]
    fn every_hover_in_the_window_is_the_same_grey() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let grey = crate::ui::hover_fill(&h.app.theme);
        let bar = h.path_bar_y(0);
        let strip = crate::ui::chrome::HEIGHT * 0.5;

        // `Back` and `Forward` are disabled in a tab that has been nowhere, and a disabled button
        // paints no fill at all — so the two of the four that are always live stand for them.
        let widgets: [(Id, f32, &str); 4] = [
            (Id::new(("nav", pane, "Up (Alt+Up)")), bar, "Up"),
            (Id::new(("nav", pane, "Refresh (F5)")), bar, "Refresh"),
            (Id::new(("th", pane, 0usize)), h.header_y(0), "a column header"),
            (Id::new(("new-tab", pane)), strip, "the new-tab button"),
        ];
        for (id, y, what) in widgets {
            let at = (0..1024)
                .step_by(2)
                .map(|x| pos2(x as f32, y))
                .find(|at| h.hovers(id, *at))
                .unwrap_or_else(|| panic!("{what} is not reachable at y {y}"));
            let _ = at;
            let rect = h
                .ctx
                .read_response(id)
                .unwrap_or_else(|| panic!("{what} was not drawn"))
                .rect;
            assert_eq!(
                h.fill_at(rect).map(|(_, fill)| fill),
                Some(grey),
                "{what} hovers in a grey of its own"
            );
        }
    }

    #[test]
    fn losing_the_window_puts_the_path_bar_and_the_context_menu_away() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;

        // The path bar's dropdown, opened the way anyone opens it.
        let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
        let index = crumbs.len() - 1;
        let y = h.path_bar_y(0);
        let at = (0..1200)
            .step_by(2)
            .map(|x| pos2(x as f32, y))
            .find(|at| h.hovers(Id::new(("crumb-chevron", pane, index)), *at))
            .expect("the chevron before the current folder is not reachable");
        h.wait();
        h.click_at(at);
        h.frame(Vec::new());
        assert_eq!(h.app.crumbs.showing(), Some((pane, index)), "nothing opened");

        // And a context menu. Built from this program's own entries, the way a right-button drag
        // builds one, so that opening it does not involve asking the shell anything.
        let own = vec![crate::shell::menu::Entry::own(
            crate::shell::menu::Own::Cancel,
        )];
        h.app.menu = Some(crate::ui::menu::Open::new(
            pane,
            pos2(200.0, 300.0),
            Vec::new(),
            PathBuf::new(),
            own,
            crate::shell::menu::Depth::Full,
            0,
        ));

        h.focused = false;
        h.frame(Vec::new());

        assert!(h.app.menu.is_none(), "the context menu is still up");
        assert_eq!(
            h.app.crumbs.showing(),
            None,
            "the path bar's dropdown is still up, and with it the tracking mode"
        );
    }

    #[test]
    fn losing_the_window_puts_the_application_menu_away() {
        // The one at the top left, which egui tracks in its own memory rather than this program
        // — so it needs `Popup::close_all` and would not be caught by the test above.
        let mut h = Harness::new();
        let at = (0..64)
            .step_by(2)
            .map(|x| pos2(x as f32, crate::ui::chrome::HEIGHT * 0.5))
            .find(|at| h.hovers(Id::new("app-menu"), *at))
            .expect("the application mark is not reachable");
        h.click_at(at);
        h.frame(Vec::new());
        assert!(
            egui::Popup::is_any_open(&h.ctx),
            "the mark did not open its menu"
        );

        h.focused = false;
        h.frame(Vec::new());
        assert!(
            !egui::Popup::is_any_open(&h.ctx),
            "the application menu is still open"
        );
    }

    #[test]
    fn ctrl_shift_t_puts_a_tab_back_and_does_not_also_open_a_new_one() {
        // Both of these hang off `Ctrl` and the same key, so the interesting half of the
        // assertion is the one about `NewTab` *not* being in the journal: without the `Shift`
        // test on the other arm, this gesture reopened the closed tab and opened a blank one
        // beside it, and only a test that drives the real keyboard path can see that.
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        h.app.panes[0].tabs.push(Tab::new(path.clone()));
        h.app.panes[0].active = 1;
        h.settle();

        h.app
            .perform(&h.ctx.clone(), Action::CloseTab { pane, tab: 1 });
        assert_eq!(h.app.panes[0].tabs.len(), 1);

        h.take_journal();
        let held = Modifiers::COMMAND | Modifiers::SHIFT;
        h.modifiers = held;
        h.frame(vec![Event::Key {
            key: egui::Key::T,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: held,
        }]);
        h.modifiers = Modifiers::NONE;
        h.frame(Vec::new());
        let done = h.take_journal();

        assert!(
            done.contains(&"ReopenTab"),
            "Ctrl+Shift+T did nothing, got {done:?}"
        );
        assert!(
            !done.contains(&"NewTab"),
            "Ctrl+Shift+T opened a blank tab as well, got {done:?}"
        );
        assert_eq!(h.app.panes[0].tabs.len(), 2);
        assert_eq!(h.app.panes[0].tabs[1].path, path);
    }

    // ---- The sidebar ----------------------------------------------------

    #[test]
    fn a_sidebar_place_navigates() {
        let mut h = Harness::new();
        let at = h
            .find(Id::new(("place", "Home")), crate::ui::GUTTER + 80.0, 40..700)
            .expect("the Home row is not reachable by the pointer");
        let done = h.click_at(at);
        assert!(
            done.contains(&"Navigate"),
            "a sidebar place did not navigate, got {done:?}"
        );
    }

    #[test]
    fn a_sidebar_group_header_folds_it() {
        let mut h = Harness::new();
        let before = h.app.sections.drives;
        let at = h
            .find(
                Id::new(("sidebar-group", "Drives")),
                crate::ui::GUTTER + 80.0,
                30..200,
            )
            .expect("the Drives heading is not reachable");
        h.click_at(at);
        assert_ne!(
            h.app.sections.drives, before,
            "a group heading has to fold its rows"
        );
    }

    #[test]
    fn a_drive_row_navigates() {
        let mut h = Harness::new();
        let letter = h
            .app
            .volumes
            .all()
            .first()
            .map(|d| d.letter.clone())
            .expect("this machine has at least one volume");
        let at = h
            .find(Id::new(("drive", &letter)), crate::ui::GUTTER + 100.0, 40..300)
            .expect("no drive row is reachable by the pointer");
        let done = h.click_at(at);
        assert!(
            done.contains(&"Navigate"),
            "a drive row did not navigate, got {done:?}"
        );
    }

    #[test]
    fn every_sidebar_place_is_reachable() {
        let mut h = Harness::new();
        let labels: Vec<String> = h.app.places.iter().map(|p| p.label.clone()).collect();
        for label in labels {
            let found = h.find(
                Id::new(("place", &label)),
                crate::ui::GUTTER + 80.0,
                30..760,
            );
            assert!(found.is_some(), "the `{label}` row is not reachable");
        }
    }

    #[test]
    fn hovering_a_drive_puts_its_free_space_in_a_tooltip() {
        // The free-space numbers left the row and became a tooltip, so the tooltip is now
        // the only place they exist. Whether one actually appears is not something the
        // source shows: it needs a real hover, which is what this harness is for.
        let mut h = Harness::new();
        let letter = h
            .app
            .volumes
            .all()
            .first()
            .expect("a machine has a drive")
            .letter
            .clone();
        let at = h
            .find(Id::new(("drive", &letter)), crate::ui::GUTTER + 80.0, 30..300)
            .unwrap_or_else(|| panic!("the `{letter}` row is not reachable"));

        // A tooltip is an area in its own layer order, so this asks egui whether one is up
        // rather than hunting for the text.
        let shown = |h: &Harness| {
            h.ctx.memory(|m| {
                m.areas()
                    .visible_layer_ids()
                    .iter()
                    .any(|layer| layer.order == egui::Order::Tooltip)
            })
        };
        assert!(!shown(&h), "a tooltip is up before anything was hovered");

        // Held still, not moved repeatedly: egui delays a tooltip until the pointer has
        // stopped, so a test that re-sends `PointerMoved` every frame resets the timer and
        // waits for ever. Move once, then let time pass.
        h.frame(vec![Event::PointerMoved(at)]);
        for _ in 0..20 {
            h.time += 0.25;
            h.frame(Vec::new());
            if shown(&h) {
                return;
            }
        }
        panic!("hovering the `{letter}` drive row shows no tooltip");
    }


    // ---- The rubber band ------------------------------------------------

    #[test]
    fn dragging_from_empty_space_bands_over_rows() {
        let mut h = Harness::new();
        let pane = h.pane_rect(0);
        let rows = h.tab(0).order.len();
        assert!(rows >= 4, "need a few rows to band over");

        // Start below the last row and drag up across the first three.
        let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
        let up_to = h.row_center(0, 2);
        h.drag(below, up_to);

        assert_eq!(
            h.tab(0).selected_count,
            rows - 2,
            "the band has to select every row it crossed"
        );
        assert!(
            h.tab(0).band.is_none(),
            "and let go of the band when the button comes up"
        );
    }

    #[test]
    fn a_band_shrinking_back_deselects() {
        let mut h = Harness::new();
        let pane = h.pane_rect(0);
        let rows = h.tab(0).order.len();
        let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);

        h.wait();
        h.frame(vec![Event::PointerMoved(below)]);
        h.frame(vec![Event::PointerButton {
            pos: below,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        // Out to the top of the list...
        h.frame(vec![Event::PointerMoved(h.row_center(0, 0))]);
        let wide = h.tab(0).selected_count;
        assert!(wide > 1, "the band should have caught several rows");
        // ...and back down to just below the last row.
        h.frame(vec![Event::PointerMoved(below)]);
        assert!(
            h.tab(0).selected_count < wide,
            "pulling the band back has to let rows go again, not keep them"
        );
        h.frame(vec![Event::PointerButton {
            pos: below,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
    }

    #[test]
    fn ctrl_dragging_a_band_keeps_what_was_selected() {
        let mut h = Harness::new();
        let pane = h.pane_rect(0);
        let rows = h.tab(0).order.len();

        // Select the last row on its own first.
        h.click_at(h.row_center(0, rows - 1));
        assert_eq!(h.tab(0).selected_count, 1);

        // Then Ctrl-band across the first two, which must not lose it.
        let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
        h.wait();
        h.modifiers = Modifiers::COMMAND;
        h.frame(vec![Event::PointerMoved(below)]);
        h.frame(vec![Event::PointerButton {
            pos: below,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::COMMAND,
        }]);
        h.frame(vec![Event::PointerMoved(h.row_center(0, rows - 2))]);
        h.frame(vec![Event::PointerButton {
            pos: below,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::COMMAND,
        }]);
        h.modifiers = Modifiers::NONE;
        h.frame(Vec::new());

        assert!(
            h.tab(0).selected_count >= 2,
            "a Ctrl band adds to the selection instead of replacing it"
        );
    }

    #[test]
    fn a_plain_band_replaces_the_selection() {
        let mut h = Harness::new();
        let pane = h.pane_rect(0);
        let rows = h.tab(0).order.len();

        h.click_at(h.row_center(0, 0));
        let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
        h.drag(below, h.row_center(0, rows - 1));

        // Only the last row was crossed, so the first one is no longer selected.
        assert!(!h.tab(0).is_selected(0), "a plain band starts from nothing");
        assert!(h.tab(0).is_selected(rows - 1));
    }

    // ---- Panes ----------------------------------------------------------

    #[test]
    fn clicking_a_pane_focuses_it() {
        let mut h = Harness::with_panes(2);
        assert_eq!(h.app.panes.len(), 2);
        let second = h.app.panes[1].id;
        h.app.focused = h.app.panes[0].id;

        h.click_at(h.row_center(1, 0));
        assert_eq!(
            h.app.focused, second,
            "clicking in a pane has to move the keyboard there"
        );
    }

    #[test]
    fn dragging_a_tab_onto_a_pane_edge_splits_it() {
        let mut h = Harness::new();
        h.app.panes[0]
            .tabs
            .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
        h.settle();

        let pane_id = h.app.panes[0].id;
        let strip_y = crate::ui::chrome::HEIGHT * 0.5;
        let from = (0..500)
            .step_by(2)
            .map(|dx| pos2(dx as f32, strip_y))
            .find(|at| h.hovers(Id::new(("tab", pane_id, 1usize)), *at))
            .expect("the second tab is not reachable");

        let pane = h.pane_rect(0);
        let to = pos2(pane.right() - 12.0, pane.center().y);
        let done = h.drag(from, to);

        assert!(
            done.contains(&"BeginTabDrag"),
            "the drag never started, got {done:?}"
        );
        assert!(
            done.contains(&"SplitTab"),
            "dropping a tab on a pane's right edge has to split it, got {done:?}"
        );
        assert_eq!(h.app.layout.count(), 2);
    }

    #[test]
    fn a_breadcrumb_chevron_opens_the_folder_it_points_at() {
        // It could not, and nothing about the code said so: the dropdown was built only
        // when it was *already* open, and the thing that opens it is the same call that
        // builds it. So this asserts the popup is open after the click and that the folder
        // behind it was actually read.
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
        assert!(crumbs.len() > 2, "the test folder has a path to walk");

        // The chevron before the last segment: its menu lists the parent's subfolders,
        // one of which is the folder now open.
        let index = crumbs.len() - 1;
        let id = Id::new(("crumb-chevron", pane, index));
        let y = h.path_bar_y(0);
        let at = (0..1200)
            .step_by(2)
            .map(|x| pos2(x as f32, y))
            .find(|at| h.hovers(id, *at))
            .expect("the chevron before the current folder is not reachable");

        h.wait();
        h.click_at(at);
        h.frame(Vec::new());

        assert_eq!(
            h.app.crumbs.showing(),
            Some((pane, index)),
            "clicking the chevron did not open anything"
        );
        let (read, count) = h
            .app
            .crumbs
            .listing()
            .expect("the dropdown never read a folder");
        assert_eq!(read, crumbs[index - 1].1, "it read the wrong folder");
        assert!(count > 0, "this crate's parent has subfolders in it");
    }

    /// An open chevron is welded to the segment before it: one fill over both.
    ///
    /// They are one control — the chevron lists that folder's subfolders, so the pair reads
    /// "this folder, and what is inside it" — and a fill each would put a seam down the middle
    /// of it. The rect is what makes this checkable: no arrangement of two fills lands a single
    /// rectangle on the union of the two.
    #[test]
    fn an_open_chevron_and_the_name_before_it_are_highlighted_together() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
        let index = crumbs.len() - 1;

        let y = h.path_bar_y(0);
        let at = (0..1200)
            .step_by(2)
            .map(|x| pos2(x as f32, y))
            .find(|at| h.hovers(Id::new(("crumb-chevron", pane, index)), *at))
            .expect("the chevron before the current folder is not reachable");
        h.wait();
        h.click_at(at);
        h.frame(Vec::new());
        assert_eq!(h.app.crumbs.showing(), Some((pane, index)), "nothing opened");

        let of = |id: Id| {
            h.ctx
                .read_response(id)
                .map(|r| r.rect)
                .unwrap_or_else(|| panic!("{id:?} was not drawn"))
        };
        let text = of(Id::new(("crumb", pane, index - 1)));
        let chevron = of(Id::new(("crumb-chevron", pane, index)));

        // In the hover grey, not the pressed one: the pair, the dropdown's own entries and a
        // plain hover on the bar are one gesture and wear one colour.
        assert_eq!(
            h.fill_at(text.union(chevron)).map(|(_, fill)| fill),
            Some(crate::ui::hover_fill(&h.app.theme)),
            "the open chevron and the name before it are not one shape"
        );
    }

    /// Once a dropdown is open the whole bar is one control: the pointer carries it along.
    ///
    /// Explorer's address bar does this, and it is the difference between reading a menu to find
    /// a sibling folder and running the pointer along the trail until the right list appears.
    /// Hovering a *name* opens the chevron that belongs to it — the one after it, which lists
    /// that folder's subfolders — so the name and its chevron are one target in both directions.
    #[test]
    fn with_a_dropdown_open_hovering_the_bar_moves_it() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
        assert!(crumbs.len() > 3, "the test folder has a path to walk");
        let index = crumbs.len() - 1;

        let y = h.path_bar_y(0);
        let sweep = |h: &mut Harness, id: Id| {
            (0..1200)
                .step_by(2)
                .map(|x| pos2(x as f32, y))
                .find(|at| h.hovers(id, *at))
                .unwrap_or_else(|| panic!("{id:?} is not reachable"))
        };

        // Open the last chevron, the ordinary way.
        let at = sweep(&mut h, Id::new(("crumb-chevron", pane, index)));
        h.wait();
        h.click_at(at);
        h.frame(Vec::new());
        assert_eq!(h.app.crumbs.showing(), Some((pane, index)));

        // Now hover a name further back along the trail. No click.
        let target = index - 2;
        let over = sweep(&mut h, Id::new(("crumb", pane, target)));
        h.frame(vec![Event::PointerMoved(over)]);
        // One more, because the switch is applied at the end of the frame that notices it: the
        // dropdown going away is already painted by then.
        h.frame(Vec::new());

        assert_eq!(
            h.app.crumbs.showing(),
            Some((pane, target + 1)),
            "the dropdown did not follow the pointer to the name at {target}"
        );
        let (read, _) = h
            .app
            .crumbs
            .listing()
            .expect("the dropdown that moved never read a folder");
        assert_eq!(
            read, crumbs[target].1,
            "it moved to the chevron of the wrong name"
        );

        // And it lets go when something is clicked. The name under the pointer is a link, so
        // this also navigates — which is the same click doing both, as it does in Explorer.
        h.click_at(over);
        assert_eq!(
            h.app.crumbs.showing(),
            None,
            "the bar is still tracking after a click"
        );
    }

    /// The bar says where a click will open the path field: a pen, and a border.
    ///
    /// The pointer already turned into an I-beam over the empty space past the last segment and
    /// the bar itself said nothing that could be seen — its hairline was `stroke-subtle`, which
    /// is the colour the bar is *painted*, so it was a line drawn in the colour behind it.
    ///
    /// Two cues now, sharing one ink. The **pen** is always there, because a hint that appears
    /// only once the pointer has arrived is not a hint; the **border** appears with the pointer,
    /// over the whole shape the field will take, and the pen brightens to match it. No fill:
    /// washing the bar to announce something that is only an announcement was too much.
    #[test]
    fn the_breadcrumb_shows_where_the_path_field_opens() {
        use crate::ui::breadcrumb::pen_ink;

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let (rest, lit) = (pen_ink(&h.app.theme, false), pen_ink(&h.app.theme, true));
        assert_ne!(rest, lit, "the two states of the hint are the same colour");
        assert_ne!(
            rest,
            crate::ui::seam(&h.app.theme),
            "the hint is drawn in the colour of the bar it is drawn on"
        );

        let of = |id: Id| {
            h.ctx
                .read_response(id)
                .map(|r| r.rect)
                .unwrap_or_else(|| panic!("{id:?} was not drawn"))
        };
        let empty = of(Id::new(("crumb-empty", pane)));
        // The pen sits at the right-hand end of the bar, in the room reserved out of the trail's.
        // A couple of points of slack around it: the glyph fills its box corner to corner.
        let pen = Rect::from_center_size(
            pos2(empty.right() - crate::ui::breadcrumb::PEN * 0.5, empty.center().y),
            egui::Vec2::splat(crate::ui::breadcrumb::PEN + 6.0),
        );
        // The field's own shape: the whole path area, so it ends where the bar does and starts
        // well before the empty part the pointer is over.
        let bar_wide = |(rect, _): &(Rect, egui::Color32)| {
            (rect.right() - empty.right()).abs() < 0.5 && rect.left() < empty.left() - 1.0
        };

        // ---- At rest: the pen, and no border ------------------------------
        assert_eq!(
            h.glyph_inks(pen),
            vec![rest, rest],
            "the pen is not drawn at rest, or not in the resting ink"
        );

        // **Centred on the row by its ink**, which is not the same claim as its box being
        // centred: the box was right the whole time the drawing inside it sat two units low.
        // See `icons::pencil`, and the rule this is an instance of — anything in a row is
        // vertically centred, and text on its baseline.
        let ink = h.glyph_bounds(pen).expect("the pen's own shapes");
        let off = ink.center().y - empty.center().y;
        assert!(
            off.abs() < 0.5,
            "the pen's ink sits {off:+.2} points off the middle of the bar ({:?} in {:?})",
            ink,
            empty
        );
        assert!(
            !h.outlines().iter().any(bar_wide),
            "the bar is outlined before the pointer is anywhere near it"
        );

        // ---- Under the pointer: both, in the lit ink ----------------------
        h.frame(vec![Event::PointerMoved(pos2(
            empty.center().x,
            h.path_bar_y(0),
        ))]);
        assert_eq!(
            h.cursor,
            egui::CursorIcon::Text,
            "this is not the part of the bar that opens the field"
        );
        assert_eq!(
            h.glyph_inks(pen),
            vec![lit, lit],
            "the pen did not light up with the border"
        );
        let outline = h
            .outlines()
            .into_iter()
            .find(bar_wide)
            .expect("the pointer says the field opens here and the bar does not");
        assert_eq!(outline.1, lit, "the border is not the hint's own ink");
        assert!(
            outline.0.contains(pen.center()),
            "the border stops short of the pen: {:?} against {:?}",
            outline.0,
            pen
        );

        // And no fill: the bar does not change colour to say this. Against the hover grey, which is
        // what a segment of the bar is washed with -- naming `control_fills`'s hover, as this once
        // did, stopped naming a colour the bar is ever painted, and the assertion passed because
        // nothing could match it rather than because nothing was washed.
        let hover = crate::ui::hover_fill(&h.app.theme);
        assert!(
            !h.rects()
                .into_iter()
                .any(|(rect, _, fill)| fill == hover && bar_wide(&(rect, fill))),
            "the bar is washed as well as outlined"
        );
    }

    /// A field's border lights up wherever the pointer is over it, not only over its icon.
    ///
    /// The design system's fault, and worth a test here because this is where it showed: a
    /// field's wrapper is allocated *before* the `TextEdit` that goes inside it, so the edit is
    /// the topmost widget over the text area and took the hover from it. The border therefore lit
    /// up only where the edit was not — the prefix icon and the padding — so hovering the part of
    /// the filter box you type in did nothing, which reads as a control that does not respond.
    ///
    /// Asserted at the *far* end of the box from its icon, which is the part that was dead.
    #[test]
    fn a_field_lights_up_over_all_of_itself() {
        let mut h = Harness::new();
        let y = h.path_bar_y(0);
        let right = h.pane_rect(0).right();

        // The run of x where the pointer says "text", coming in from the right-hand end of the
        // bar: the filter box. Found by sweeping rather than by deriving its rect, for the same
        // reason `arriving_in_a_field_selects_what_is_there` does it — the box's own id is
        // generated inside the design system.
        let mut run: Vec<f32> = Vec::new();
        for dx in (8..400).step_by(2) {
            let at = pos2(right - dx as f32, y);
            h.frame(vec![Event::PointerMoved(at)]);
            if h.cursor == egui::CursorIcon::Text {
                run.push(at.x);
            } else if !run.is_empty() {
                break;
            }
        }
        assert!(run.len() > 8, "the filter box is not reachable by the pointer");

        // The rightmost end of the run: the text area. The icon is at the other end, and the
        // clearable ✕ only exists once something has been typed.
        let text_area = pos2(run[0], y);
        h.frame(vec![Event::PointerMoved(text_area)]);
        let border = |h: &Harness, at: Pos2| {
            h.outlines()
                .into_iter()
                .find(|(rect, _)| rect.contains(at))
                .map(|(_, color)| color)
        };
        assert_eq!(
            border(&h, text_area),
            Some(h.app.theme.stroke.strong),
            "hovering the part of the field you type in did not light its border"
        );

        // And it goes out again, which is what makes the assertion above about the hover rather
        // than about the border always being that colour.
        h.frame(vec![Event::PointerMoved(pos2(right - 500.0, y))]);
        assert_eq!(
            border(&h, text_area),
            Some(h.app.theme.stroke.control),
            "the field stayed lit with the pointer somewhere else"
        );
    }

    /// The rows a tab is showing, in display order, by the name each one carries.
    ///
    /// Which is the whole of what "flattened" means from the outside: the same listing, with
    /// relative paths in it instead of bare names.
    fn shown_names(h: &Harness, pane: usize) -> Vec<String> {
        let tab = h.app.panes[pane].tab();
        let dir = tab.dir.as_ref().expect("the listing has not arrived");
        tab.order
            .iter()
            .map(|&i| dir.name(i as usize).to_owned())
            .collect()
    }

    /// Flatten: the button on the bar, the shortcut, and the listing each of them produces.
    ///
    /// Driven through the real button and the real keyboard rather than through `perform`,
    /// because the two things most likely to be wrong are the ones only that can see: a
    /// button nothing can reach, and a shortcut that never arrives.
    #[test]
    fn flattening_a_folder_shows_its_whole_tree_and_turning_it_off_puts_it_back() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        // `src`, not the crate root — the root has `target` in it, and walking a few hundred
        // thousand build artefacts would prove nothing this does not.
        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: sources,
            },
        );
        h.settle();
        assert!(
            shown_names(&h, 0).iter().all(|name| !name.contains('\\')),
            "a folder's own listing is one level deep"
        );

        // ---- Where the button is ------------------------------------------
        let rect_of = |h: &Harness, id: Id| {
            h.ctx
                .read_response(id)
                .map(|r| r.rect)
                .unwrap_or_else(|| panic!("{id:?} was not laid out"))
        };
        let button = rect_of(&h, Id::new(("flatten", pane)));
        let bar = h.app.panes[0].rect;
        // Past the end of the path — it is one of the right-hand group, not part of the trail —
        // and with a filter box's worth of room still to its right, which is what "just before
        // the filter" comes to in geometry. 90 is the width below which `breadcrumb` gives up
        // drawing the field at all.
        let empty = rect_of(&h, Id::new(("crumb-empty", pane)));
        assert!(
            button.left() >= empty.right(),
            "the flatten button is sitting in the path's room: {button:?} against {empty:?}"
        );
        assert!(
            bar.right() - button.right() >= 90.0,
            "nothing but {} points to the right of it, so the filter is not there",
            bar.right() - button.right()
        );

        // ---- Clicking it --------------------------------------------------
        let done = h.click_at(button.center());
        assert!(
            done.contains(&"ToggleFlat"),
            "the button did nothing, got {done:?}"
        );
        h.settle();
        assert!(h.app.panes[0].tab().flat, "the tab is not flattened");

        let flat = shown_names(&h, 0);
        assert!(
            flat.iter().any(|name| name == "ui\\filelist.rs"),
            "the listing did not reach a second level: {} rows, first few {:?}",
            flat.len(),
            &flat[..flat.len().min(5)]
        );
        assert!(
            flat.len() > shown_names_at_rest(),
            "a flattened tree should have more rows in it than the folder did"
        );

        // A rename edits the *file's* name, not the whole of what the row shows. Otherwise the
        // field would open with `ui\filelist.rs` in it, and accepting that unchanged would ask
        // the shell to rename a file to a path.
        {
            let tab = h.app.panes[0].tab_mut();
            let at = tab
                .order
                .iter()
                .position(|&i| {
                    tab.dir
                        .as_ref()
                        .is_some_and(|dir| dir.name(i as usize) == "ui\\filelist.rs")
                })
                .expect("the row is in the listing");
            tab.select_only(at);
            tab.begin_rename();
            assert_eq!(
                tab.renaming.as_ref().map(|(_, text)| text.as_str()),
                Some("filelist.rs"),
                "the rename field opened on the path rather than on the name"
            );
            tab.renaming = None;
        }

        // ---- And Ctrl+E, which is the same gesture from the keyboard ------
        h.take_journal();
        let held = Modifiers::COMMAND;
        h.modifiers = held;
        h.frame(vec![Event::Key {
            key: egui::Key::E,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: held,
        }]);
        h.modifiers = Modifiers::NONE;
        let done = h.take_journal();
        assert!(
            done.contains(&"ToggleFlat"),
            "Ctrl+E did nothing, got {done:?}"
        );
        h.settle();
        assert!(!h.app.panes[0].tab().flat);
        assert!(
            shown_names(&h, 0).iter().all(|name| !name.contains('\\')),
            "turning it off left the tree on screen"
        );
    }

    /// **The other flatten mode: the same rows as the tree they came from.**
    ///
    /// Three things this holds, and each of them is a thing only a driven frame can see:
    ///
    /// - the order is **pre-order** and the drawing **indents** by it — a tree whose rows are all
    ///   at the same x is a list with chevrons on it;
    /// - the **twisty is where the pointer can reach it**, which is the class of bug that looks
    ///   perfectly correct in the source: a rect a few points off, or covered by the row's own
    ///   hit-test, responds to nothing and reads as a dead control;
    /// - and a shut folder **takes its subtree out of the listing**, rather than merely off screen.
    ///
    /// The mode is set through `perform` and the collapse through a real click, which is the split
    /// the suite makes everywhere: the menu entry is checked where the menu is, and the gesture
    /// nothing else can verify is driven for real.
    #[test]
    fn flattening_as_a_tree_indents_the_rows_and_a_twisty_shuts_a_branch() {
        use crate::pane::FlatMode;

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        // A tree of its own, in the sandbox, rather than this crate's `src`.
        //
        // It used to read `src`, and that made the test a hostage of the source layout: adding one
        // file at the top of `src` moved every row down by one and pushed `ui\filelist.rs` off the
        // bottom of the window, so a test about *indentation* failed because of a file it never
        // named. Eight rows, shaped like what it asserts, and nothing outside the sandbox.
        let sources = crate::sandbox::fresh("tree");
        for folder in ["fs", "ui"] {
            std::fs::create_dir_all(sources.join(folder)).expect("a fixture folder");
        }
        for file in [
            "fs/dir.rs",
            "fs/sort.rs",
            "ui/filelist.rs",
            "ui/menu.rs",
            "main.rs",
            "theme.rs",
        ] {
            std::fs::write(sources.join(file), b"// a row\n").expect("a fixture file");
        }
        let ctx = h.ctx.clone();
        h.app.perform(
            &ctx,
            Action::Navigate {
                pane,
                path: sources,
            },
        );
        h.settle();
        h.app.perform(&ctx, Action::SetFlatMode(FlatMode::Tree));
        h.app.perform(&ctx, Action::ToggleFlat(pane));
        h.settle();
        assert!(h.app.panes[0].tab().is_tree(), "the tab is not a tree");

        // ---- Pre-order: a folder, then what is under it --------------------
        let rows = shown_names(&h, 0);
        let depth = |name: &str| name.matches('\\').count();
        let ui_at = rows
            .iter()
            .position(|name| name == "ui")
            .unwrap_or_else(|| panic!("`src\\ui` is not a row: {:?}", &rows[..rows.len().min(8)]));
        assert!(
            rows[ui_at + 1..]
                .iter()
                .take_while(|name| depth(name) > 0)
                .any(|name| name == r"ui\filelist.rs"),
            "the rows under `ui` are not the rows after it: {:?}",
            &rows[ui_at..rows.len().min(ui_at + 8)]
        );
        // And every row of a tree is directly under the folder it is in, which is the assertion
        // that catches an order that is *sorted* by path rather than walked: `a\b\c` would still
        // come after `a\b` there, but a folder's rows would not be one block.
        for (at, name) in rows.iter().enumerate().skip(1) {
            let previous = depth(&rows[at - 1]);
            assert!(
                depth(name) <= previous + 1,
                "row {at} `{name}` is {} levels under `{}`",
                depth(name) - previous,
                rows[at - 1]
            );
        }

        // ---- The indent is on screen, not merely in the order --------------
        let painted = h.texts();
        let x_of = |name: &str| {
            painted
                .iter()
                .find(|(_, text)| text == name)
                .map(|(at, _)| at.x)
                .unwrap_or_else(|| panic!("`{name}` was not drawn"))
        };
        assert!(
            x_of("filelist.rs") > x_of("ui") + 8.0,
            "`filelist.rs` is drawn at {} and its folder at {}: the tree is not indented",
            x_of("filelist.rs"),
            x_of("ui")
        );
        // The row shows its own name and not its path — the indent is what says where it is, so
        // the dimmed `ui` after the name that the *list* mode draws would be saying it twice.
        assert!(
            !painted
                .iter()
                .any(|(_, text)| text.starts_with("filelist.rs") && text.contains("ui")),
            "the tree is still drawing the folder after the name"
        );

        // ---- The twisty, clicked where it is actually drawn ----------------
        //
        // The rows' own hit-test rect is where the geometry comes from — the same rect the row
        // was drawn in — so nothing here restates a number the layout could have moved.
        let block = h
            .ctx
            .read_response(Id::new(("rows-hit", pane)))
            .map(|r| r.rect)
            .expect("the listing was not laid out");
        let row_rect = |at: usize| {
            Rect::from_min_size(
                pos2(block.left(), block.top() + at as f32 * crate::pane::ROW_HEIGHT),
                vec2(block.width(), crate::pane::ROW_HEIGHT),
            )
        };
        let twisty = crate::ui::filelist::twisty_rect(row_rect(ui_at), 0).center();

        let before = shown_names(&h, 0).len();
        let done = h.click_at(twisty);
        assert!(
            done.contains(&"ToggleCollapsed"),
            "the twisty on `ui` did nothing, got {done:?}"
        );
        let after = shown_names(&h, 0);
        assert!(
            after.len() < before,
            "the branch is still open: {before} rows before, {} after",
            after.len()
        );
        assert!(
            after.iter().any(|name| name == "ui"),
            "shutting the folder took the folder itself away"
        );
        assert!(
            !after.iter().any(|name| name == r"ui\filelist.rs"),
            "what was under `ui` is still in the listing"
        );
        // And a shut folder is not a selected one: opening a branch is a way of *looking*, and a
        // click that also moved the selection would make it unusable as one.
        assert_eq!(
            h.app.panes[0].tab().selected_count,
            0,
            "the twisty selected the row as well as shutting it"
        );

        // ---- And a click *beside* it is still an ordinary click ------------
        //
        // The twisty test comes first and returns, so a rect that was too wide — or a `return`
        // that fired on any click at all — would leave a tree in which nothing can be selected.
        // Nothing else in the suite would notice: every other selection test is over a listing
        // that has no twisties in it.
        let shut_rows = shown_names(&h, 0).len();
        let done = h.click_at(pos2(
            block.center().x,
            row_rect(ui_at).center().y,
        ));
        assert!(
            done.contains(&"Focus"),
            "a click on the row itself did nothing, got {done:?}"
        );
        assert_eq!(
            h.app.panes[0].tab().selected_count,
            1,
            "a click on the row did not select it"
        );
        assert_eq!(
            shown_names(&h, 0).len(),
            shut_rows,
            "a click on the row opened the branch as well"
        );

        // ---- Open again, from the keyboard --------------------------------
        //
        // `Right` on the cursor's row, which is what the key means in every tree on the platform
        // — and the only way through a tree for somebody not using the pointer.
        h.app.panes[0].tab_mut().select_only(ui_at);
        h.frame(Vec::new());
        h.frame(vec![Event::Key {
            key: egui::Key::ArrowRight,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        h.frame(Vec::new());
        assert_eq!(
            shown_names(&h, 0).len(),
            before,
            "Right did not open the branch again"
        );
    }

    /// **The flatten button's menu is reachable, and picking a mode from it takes.**
    ///
    /// A right click on the control that opens a thing is where the settings of that thing belong —
    /// and a menu nothing can reach is one of the two failures only a driven frame can see. So the
    /// gesture is real all the way through: right-click the button, find the entry on screen by its
    /// label, click it.
    ///
    /// Picking a mode deliberately does *not* turn the flatten on, exactly as picking a preview
    /// position does not open the panel — the entry above it is what does that. Asserted, because
    /// it is a decision rather than an omission.
    #[test]
    fn the_flatten_buttons_menu_picks_the_mode_without_turning_the_view_on() {
        use crate::pane::FlatMode;

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.settle();
        let button = h
            .ctx
            .read_response(Id::new(("flatten", pane)))
            .map(|r| r.rect)
            .expect("the flatten button was not laid out");

        h.click_with(button.center(), PointerButton::Secondary, Modifiers::NONE);
        // The menu's own entries, found the way a user finds them: by reading the words.
        let entry = |h: &Harness, label: &str| {
            h.texts()
                .into_iter()
                .find(|(_, text)| text == label)
                .map(|(at, _)| at)
        };
        let tree = entry(&h, "Tree").unwrap_or_else(|| {
            panic!(
                "the menu did not open, or has no `Tree` in it: {:?}",
                h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
            )
        });
        assert!(
            entry(&h, "Flatten this folder").is_some(),
            "the menu has no toggle in it, so the mode is all it can do"
        );

        // A couple of points into the label, which is inside the entry whatever its padding is.
        let done = h.click_at(pos2(tree.x + 2.0, tree.y + 6.0));
        assert!(
            done.contains(&"SetFlatMode"),
            "clicking `Tree` did nothing, got {done:?}"
        );
        assert_eq!(h.app.flat_mode, FlatMode::Tree);
        assert!(
            !h.app.panes[0].tab().flat,
            "picking a mode turned the flatten on as well"
        );
        // And it is a setting, so it is part of what the window writes down. Asked of `settings()`
        // rather than of `config_dirty`: the frame after the one that sets the flag is the frame
        // that saves and clears it, and by here several have gone by. What matters is that the mode
        // is in the answer — a value that is chosen and never written is the failure the settings
        // round trip exists for.
        assert_eq!(h.app.settings().flat_mode, FlatMode::Tree);

        // The third setting in the same menu, and the one whose default is *on*: so the click under
        // test turns it off, which is also the only way to tell a tick that means something from a
        // tick that is painted on. Everything else about it is the mode's story — it is the window's,
        // it is written down, and it does not turn the flatten on by itself.
        assert!(h.app.regroup, "regrouping starts on");
        h.click_with(button.center(), PointerButton::Secondary, Modifiers::NONE);
        let regroup = entry(&h, "Regroup single folders").unwrap_or_else(|| {
            panic!(
                "the menu has no regroup entry: {:?}",
                h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
            )
        });
        let done = h.click_at(pos2(regroup.x + 2.0, regroup.y + 6.0));
        assert!(
            done.contains(&"SetRegroup"),
            "clicking `Regroup single folders` did nothing, got {done:?}"
        );
        assert!(!h.app.regroup, "the entry did not turn it off");
        assert!(!h.app.settings().regroup, "and it was not written down");
        assert!(
            !h.app.panes[0].tab().flat,
            "a setting in this menu turned the flatten on"
        );
    }

    /// Switching between the two flatten modes **does not read the folder again.**
    ///
    /// Which is the whole reason they are two orders over one listing: a tree big enough to be
    /// worth flattening took seconds to walk, and a mode switch that walked it again would make
    /// the menu something you avoid using. The listing is an `Arc`, so the assertion is that it is
    /// the very same one — not merely one that looks alike.
    #[test]
    fn switching_flatten_modes_reorders_the_listing_it_already_has() {
        use crate::pane::FlatMode;

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let ctx = h.ctx.clone();
        h.app.perform(
            &ctx,
            Action::Navigate {
                pane,
                path: sources,
            },
        );
        h.settle();
        h.app.perform(&ctx, Action::ToggleFlat(pane));
        h.settle();
        assert_eq!(
            h.app.panes[0].tab().flat_mode,
            FlatMode::List,
            "the default is the list"
        );

        let listing = h.app.panes[0].tab().dir.clone().expect("the walk's answer");
        let as_list = shown_names(&h, 0);

        h.app.perform(&ctx, Action::SetFlatMode(FlatMode::Tree));
        h.frame(Vec::new());
        let after = h.app.panes[0].tab().dir.clone().expect("still a listing");
        assert!(
            std::sync::Arc::ptr_eq(&listing, &after),
            "the mode switch dropped the listing, so the tree was walked a second time"
        );

        // The same rows, differently arranged — which is the other half of "one listing".
        let as_tree = shown_names(&h, 0);
        assert_ne!(as_list, as_tree, "the order did not change");
        let (mut sorted_list, mut sorted_tree) = (as_list.clone(), as_tree.clone());
        sorted_list.sort();
        sorted_tree.sort();
        assert_eq!(
            sorted_list, sorted_tree,
            "the two modes are not showing the same rows"
        );
    }

    /// How many rows `src` itself has. Read once, so the comparison above is against the
    /// folder rather than against a number written down here.
    fn shown_names_at_rest() -> usize {
        let dir = crate::fs::scan::scan(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"));
        dir.len()
    }

    /// **A folder shortcut opens in this window.**
    ///
    /// Handing a `.lnk` to the shell is what it gets by default, and for one pointing at a
    /// folder that means Explorer opening over the top of this program — a file manager whose
    /// rows open a *different* file manager. Both gestures are driven for real here, because
    /// what could break is the wiring rather than the resolving: `Enter` through the keyboard
    /// path, and a middle click through the pointer's.
    #[cfg(windows)]
    #[test]
    fn opening_a_folder_shortcut_stays_in_this_window() {
        let root = crate::sandbox::dir("open-lnk");
        let folder = root.join("somewhere");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&folder).expect("a directory in the temp folder");
        let link = root.join("somewhere.lnk");
        if !crate::shell::links::write_shortcut(&link, &folder) {
            println!("the shell would not write a shortcut here; skipping");
            crate::sandbox::remove(&root);
            return;
        }

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();

        let position = {
            let tab = h.app.panes[0].tab();
            let dir = tab.dir.as_ref().expect("the listing");
            tab.order
                .iter()
                .position(|&i| dir.name(i as usize) == "somewhere.lnk")
                .expect("the shortcut is in the listing")
        };
        assert!(
            h.app.panes[0].tab().is_shortcut_at(position),
            "the row is not recognised as a shortcut, so nothing below can work"
        );
        assert!(
            !h.app.panes[0].tab().is_dir_at(position),
            "a `.lnk` is a file as far as the enumeration is concerned — if it were not, this \
             test would be passing for the wrong reason"
        );

        // ---- Enter ---------------------------------------------------------
        h.app.panes[0].tab_mut().select_only(position);
        h.frame(Vec::new());
        h.take_journal();
        h.frame(vec![Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        let done = h.take_journal();
        assert!(
            done.contains(&"Navigate"),
            "Enter on a folder shortcut went to the shell instead of navigating, got {done:?}"
        );
        h.settle();
        assert_eq!(
            h.app.panes[0].tab().path,
            folder,
            "it navigated somewhere else"
        );

        // ---- And a middle click, into a tab of its own ----------------------
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();
        let tabs = h.app.panes[0].tabs.len();
        let at = h.row_center(0, position);
        h.take_journal();
        let done = h.click_with(at, PointerButton::Middle, Modifiers::NONE);
        assert!(
            done.contains(&"OpenNewTab"),
            "a middle click on a folder shortcut did nothing, got {done:?}"
        );
        h.settle();
        assert_eq!(
            h.app.panes[0].tabs.len(),
            tabs + 1,
            "no tab was opened for it"
        );
        assert_eq!(h.app.panes[0].tab().path, folder);

        crate::sandbox::remove(&root);
    }

    /// **The filter waits for the typing to stop, and each keystroke restarts the wait.**
    ///
    /// One pass costs up to 240 ms on a large listing — see [`crate::pane::FILTER_DELAY`] — so
    /// what this is really about is that typing a word should cost one pass and not one per
    /// letter. Driven through the real field with the real clock, because the whole behaviour is
    /// a relationship between keystrokes and time.
    #[test]
    fn the_filter_waits_for_the_typing_to_stop() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
            },
        );
        h.settle();
        let all = h.tab(0).order.len();
        assert!(all > 8, "the fixture folder is too small to filter");

        // The caret into the box, which is where the keystrokes have to land.
        let held = Modifiers::COMMAND;
        h.modifiers = held;
        h.frame(vec![Event::Key {
            key: egui::Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: held,
        }]);
        h.modifiers = Modifiers::NONE;
        h.frame(Vec::new());
        assert!(
            h.ctx.memory(|m| m.focused()).is_some(),
            "the caret is not in the filter box, so nothing below is being tested"
        );

        // Each step is most of the wait but not all of it, so two of them cross the deadline and
        // one does not. In terms of the constant rather than in milliseconds: what is being
        // tested is the relationship, and it should still hold when the wait is retuned.
        let step = crate::pane::FILTER_DELAY * 0.6;

        // ---- One letter: noted, not applied ---------------------------------
        h.frame(vec![Event::Text("c".to_owned())]);
        assert_eq!(h.tab(0).filter, "c", "the keystroke never reached the field");
        assert!(h.tab(0).filter_at.is_some(), "no deadline was set");
        assert_eq!(
            h.tab(0).order.len(),
            all,
            "the filter was applied on the keystroke, which is the thing this prevents"
        );

        // A frame most of the way through the wait changes nothing.
        h.time += step;
        h.frame(Vec::new());
        assert_eq!(h.tab(0).order.len(), all, "applied before the wait was up");

        // ---- A second letter restarts it ------------------------------------
        h.frame(vec![Event::Text("o".to_owned())]);
        assert_eq!(h.tab(0).filter, "co");
        // The same step again, which is past the deadline the *first* letter set and short of the
        // one the second set. This is where it would have fired without the restart.
        h.time += step;
        h.frame(Vec::new());
        assert_eq!(
            h.tab(0).order.len(),
            all,
            "the second keystroke did not restart the wait"
        );

        // ---- And then it fires, once, on the whole word ---------------------
        h.time += step;
        h.frame(Vec::new());
        assert!(h.tab(0).filter_at.is_none(), "the deadline is still standing");
        let dir = h.tab(0).dir.clone().expect("the listing");

        // Everything above is about the clock and holds wherever this repository lives. What is
        // left is about the *answer*, and that does depend on where it lives: the filter is asked
        // about the whole path (`fs::sort::prefix`), so a checkout under a folder whose own name
        // holds a `co` keeps every row, and the two assertions below would be reading a listing
        // that was never narrowed. Which is not a failure — it is a fixture that cannot show the
        // difference, so say so instead of asserting nothing.
        let base = dir.path.to_string_lossy().to_lowercase();
        if base.contains("co") {
            println!("`{base}` matches the filter this test types; skipping what it keeps");
            return;
        }

        let rows = h.tab(0).order.len();
        assert!(
            rows < all,
            "the filter never applied at all: still {rows} of {all} rows"
        );
        for &i in &h.tab(0).order {
            let path = dir.target(i as usize).to_string_lossy().to_lowercase();
            assert!(
                path.contains("co"),
                "`{path}` does not match the filter that was typed"
            );
        }
    }

    /// **Two words typed in the box narrow by both.**
    ///
    /// What the words *mean* belongs to `azur_egui_theme::filter` and is tested there. What is
    /// tested here is the one character that has to survive the journey from the keyboard to
    /// [`crate::fs::sort::build_order`] for any of it to work: the **space**. It is the only
    /// punctuation in the box that now carries meaning, and it is exactly the kind of key
    /// something else takes — a shortcut on `Space`, a guard that trims, an event a focused field
    /// never sees. Any of those leaves a filter that quietly keeps nothing.
    #[test]
    fn two_words_in_the_filter_box_narrow_by_both() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
            },
        );
        h.settle();

        // `Ctrl+F` for the caret, then the whole query — the keystroke-by-keystroke behaviour is
        // `the_filter_waits_for_the_typing_to_stop`'s subject, not this one's.
        let held = Modifiers::COMMAND;
        h.modifiers = held;
        h.frame(vec![Event::Key {
            key: egui::Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: held,
        }]);
        h.modifiers = Modifiers::NONE;
        h.frame(vec![Event::Text("view pre".to_owned())]);
        assert_eq!(
            h.tab(0).filter,
            "view pre",
            "the space did not reach the field"
        );

        h.time += crate::pane::FILTER_DELAY * 1.5;
        h.frame(Vec::new());
        assert!(h.tab(0).filter_at.is_none(), "the filter never applied");

        let dir = h.tab(0).dir.clone().expect("the listing");
        let order = &h.tab(0).order;
        let kept: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();

        // What "both words, in any order, over the whole path" comes to, worked out here rather
        // than written down as a list of names: whether this repository happens to live somewhere
        // with a `pre` in it then changes both sides together instead of only one, and the test
        // still says what it means to say.
        let want: Vec<&str> = (0..dir.len())
            .filter(|&i| {
                let path = dir.target(i).to_string_lossy().to_lowercase();
                path.contains("view") && path.contains("pre")
            })
            .map(|i| dir.name(i))
            .collect();
        assert!(
            want.contains(&"preview.rs"),
            "the fixture folder has no `preview.rs`, so this asserts nothing"
        );
        assert_eq!(kept, want, "the two words did not both narrow the listing");
    }

    /// **A changed filter opens the listing at the top.**
    ///
    /// Driven through the real `ScrollArea`, because the thing being claimed is about an offset
    /// egui owns: setting [`Tab::scroll_y`] alone records where the listing *is*, and only
    /// [`Tab::scroll_to`] moves it. A test that set the field and read it back would pass whatever
    /// the program did.
    ///
    /// The fixture is `src` **flattened**, and it has to be: the point only exists where there is
    /// somewhere to be scrolled *to*, both before the filter and after it. A listing the filter
    /// cuts down to less than a viewport-full comes back to the top on its own — `ScrollArea`
    /// clamps to the content it has — and a test built on that one would hold with none of this
    /// here.
    #[test]
    fn a_changed_filter_opens_the_listing_at_the_top() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
            },
        );
        h.settle();
        h.app.perform(&h.ctx.clone(), Action::ToggleFlat(pane));
        h.settle();

        // Down the listing, for real. `scroll_to` is what the rubber-band's auto-scroll uses, so
        // this is a gesture the program already makes rather than a back door into egui.
        let rows = h.tab(0).order.len();
        h.app.panes[0].tab_mut().scroll_to = Some(300.0);
        h.frame(Vec::new());
        let was = h.tab(0).scroll_y;
        assert!(
            was > 100.0,
            "the flattened fixture ({rows} rows) does not scroll far enough to show anything: \
             stopped at {was}"
        );

        // A filter typed into the real field, and then the wait it takes to be applied.
        let held = Modifiers::COMMAND;
        h.modifiers = held;
        h.frame(vec![Event::Key {
            key: egui::Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: held,
        }]);
        h.modifiers = Modifiers::NONE;
        h.frame(vec![Event::Text(".rs$".to_owned())]);
        h.time += crate::pane::FILTER_DELAY * 1.5;
        h.frame(Vec::new());

        let tab = h.tab(0);
        assert!(tab.filter_at.is_none(), "the filter never applied");
        assert_eq!(
            tab.scroll_y, 0.0,
            "the listing kept its old offset through a change of filter"
        );
        // And the filtered listing is still long enough that the offset above was thrown away
        // rather than clamped away, which is what makes the assertion mean anything.
        let left = tab.order.len();
        assert!(
            left as f32 * crate::pane::ROW_HEIGHT > was + 300.0,
            "only {left} rows survived `.rs$`, which cannot hold an offset of {was}"
        );
    }

    /// **`F3` puts the caret in the filter box, exactly as `Ctrl+F` does.**
    ///
    /// One key rather than a chord, and the key most Windows programs have meant "find" with for
    /// longer than `Ctrl+F` has been the convention. Both are driven here rather than one: what
    /// would break them is the same line, and a shortcut that quietly stopped working is invisible
    /// until somebody presses it.
    #[test]
    fn ctrl_f_and_f3_both_reach_the_filter_box() {
        for key in [egui::Key::F, egui::Key::F3] {
            let mut h = Harness::new();
            // The chord for one and nothing at all for the other — `consume_shortcut` matches the
            // modifiers exactly, so `F3` would not fire if it were asked for with `COMMAND` held,
            // and `Ctrl+F` would not fire without it.
            let held = if key == egui::Key::F {
                Modifiers::COMMAND
            } else {
                Modifiers::NONE
            };
            h.modifiers = held;
            h.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: held,
            }]);
            h.modifiers = Modifiers::NONE;
            // The bar asks for focus on the frame it sees the shortcut; egui grants it at the end.
            h.frame(Vec::new());
            assert!(
                h.ctx.memory(|m| m.focused()).is_some(),
                "{key:?} did not put the caret anywhere"
            );

            // And it is the filter box that has it, not merely something: the proof is that
            // typing lands in `Tab::filter`.
            h.frame(vec![Event::Text("z".to_owned())]);
            assert_eq!(
                h.tab(0).filter,
                "z",
                "{key:?} focused something that is not the filter box"
            );
        }
    }

    // -----------------------------------------------------------------------
    // The preview panel
    // -----------------------------------------------------------------------

    /// Put the focused pane in the folder the test binary is in, select it, and open the preview
    /// panel on it — then let the walk land.
    ///
    /// The binary is this test process, which is the one fixture on any machine that is a real PE
    /// image, is always there, and imports something. Selected rather than handed to the panel
    /// directly, because the panel follows the selection every frame: pointing it at a file that
    /// is *not* selected is a state the running program never reaches, and one that would be
    /// cleared on the next frame anyway.
    fn open_preview(h: &mut Harness) {
        let me = std::env::current_exe().expect("a test process has an executable");
        let folder = me.parent().expect("it is in a folder").to_path_buf();
        let pane = h.app.panes[0].id;
        h.app
            .perform(&h.ctx.clone(), Action::Navigate { pane, path: folder });
        h.settle();

        let name = me
            .file_name()
            .expect("it has a name")
            .to_string_lossy()
            .into_owned();
        let at = {
            let tab = h.app.panes[0].tab();
            let dir = tab.dir.as_ref().expect("the listing arrived");
            tab.order
                .iter()
                .position(|&i| dir.name(i as usize) == name)
                .unwrap_or_else(|| panic!("{name} is not in its own folder's listing"))
        };
        h.app.panes[0].tab_mut().select_only(at);
        h.app.panes[0].tab_mut().preview.open = true;
        h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
        for attempt in 0..400 {
            h.frame(Vec::new());
            if !h.app.preview_pending() && attempt > 2 {
                break;
            }
            // Frames are free here; the walk is on a worker thread sharing a machine with
            // seven other test threads.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!h.app.preview_pending(), "the read never came back");
        assert_eq!(
            h.app.panes[0].tab().preview.showing(),
            Some(me.as_path()),
            "the panel is not showing the binary that was selected"
        );
        h.take_journal();
    }

    /// Open a text preview on this program's own `pe.rs` — a real file, in the monospace face, with
    /// plenty in it to find.
    fn open_text_preview(h: &mut Harness) {
        let pane = h.app.panes[0].id;
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        h.app
            .perform(&h.ctx.clone(), Action::Navigate { pane, path: src });
        h.settle();
        let at = {
            let tab = h.app.panes[0].tab();
            let dir = tab.dir.as_ref().expect("the listing arrived");
            tab.order
                .iter()
                .position(|&i| dir.name(i as usize) == "pe.rs")
                .expect("this program's own `src` has a `pe.rs` in it")
        };
        h.app.panes[0].tab_mut().select_only(at);
        h.app.panes[0].tab_mut().preview.open = true;
        h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
        for attempt in 0..400 {
            h.frame(Vec::new());
            if !h.app.preview_pending() && attempt > 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!h.app.preview_pending(), "the read never came back");
        h.take_journal();
    }

    /// Where the panel is, once the layout has looked at the pane it is in.
    fn preview_rect(h: &Harness) -> Rect {
        crate::ui::preview::split(pane_body(h), true, h.app.preview)
            .1
            .expect("the panel has room in a harness-sized pane")
    }

    /// Everything under the focused pane's path bar, which the listing and the panel share.
    fn pane_body(h: &Harness) -> Rect {
        let pane = h.pane_rect(0);
        Rect::from_min_max(
            pos2(
                pane.left(),
                h.pane_content_top(0) + crate::ui::breadcrumb::HEIGHT,
            ),
            pane.max,
        )
    }

    /// How many rows the focused pane's dependency view is showing.
    fn shown_deps(h: &Harness) -> usize {
        h.app.panes[0]
            .tab()
            .preview
            .dependency_rows()
            .expect("the panel is showing a dependency tree")
    }

    /// **The panel is inside the pane**, on whichever side the layout says, and its close button
    /// gives the room back.
    ///
    /// Driven through the real button rather than through `perform`, because the one thing only
    /// that can catch is a button nothing can reach — and this one sits on a bar inside a pane,
    /// over a listing that also claims every point of it for its own hit-testing.
    #[test]
    fn the_preview_panel_takes_room_from_the_listing_and_its_close_button_gives_it_back() {
        use crate::ui::preview::{Side, Where};

        for at in [Where::Right, Where::Bottom] {
            let mut h = Harness::new();
            h.app.preview.at = at;
            open_preview(&mut h);
            let body = pane_body(&h);
            let panel = preview_rect(&h);

            // The listing gave up room on one side, and only there.
            match at.side(body) {
                Side::Right => {
                    assert!(panel.right() >= body.right() - 0.5, "{at:?}: not at the edge");
                    assert_eq!(panel.top(), body.top(), "{at:?}: it is not full height");
                    assert!(panel.width() < body.width() * 0.6);
                }
                Side::Bottom => {
                    assert!(panel.bottom() >= body.bottom() - 0.5, "{at:?}");
                    assert_eq!(panel.left(), body.left(), "{at:?}: it is not full width");
                    assert!(panel.height() < body.height() * 0.6);
                }
            }
            // And it is showing the walk rather than the word `Reading…`.
            let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
            assert!(
                texts.iter().any(|text| text.contains("API set")),
                "{at:?}: the panel is not showing a finished walk: {texts:?}"
            );

            // **A row folds when it is clicked.** Worth driving for real rather than through the
            // view's own method, because the rows live inside a `ScrollArea` inside a panel inside
            // a pane — and a click landing on any of those instead of on the row is precisely the
            // kind of thing that looks correct in the source and does nothing at all.
            let before = shown_deps(&h);
            assert!(before > 2, "{at:?}: the root's imports are not on show");
            // **The row is asked for rather than assumed.** This used to click the row under the
            // root, on the reasoning that the first import would have imports of its own. That is
            // not a fact about this program: the panel is pointed at the test binary itself, so the
            // order of these rows is the order the linker wrote *its* import table in, and adding
            // anything to this crate can rearrange it. It did — an API set came to the top, an API
            // set has nothing under it, and the click landed on a row that could not unfold. Which
            // says nothing about whether the click reached it, and that is the whole question here.
            // See `crate::ui::deps::View::first_foldable`.
            let row = h.app.panes[0]
                .tab()
                .preview
                .dependency_first_foldable()
                .unwrap_or_else(|| panic!("{at:?}: nothing in the tree can be unfolded at all"));
            let first_import = pos2(
                panel.left() + 80.0,
                panel.top()
                    + crate::ui::preview::HEADER
                    + crate::ui::deps::ROW * (row as f32 + 0.5),
            );
            // **Settled first.** The panel has just been filled by a worker thread, and a click on a
            // row of it in the same breath is a click on a view that is still arriving: co-executing
            // with the other two tests in this group, this went from unfolding the row to doing
            // nothing at all, deterministically enough to bisect and racy enough to flip on an
            // unrelated `println!`. One quiet frame is what a person's hand gives it for free.
            h.wait();
            h.click_at(first_import);
            let opened = shown_deps(&h);
            assert!(
                opened > before,
                "{at:?}: clicking a row did not unfold it: {before} rows, then {opened}"
            );
            h.wait();
            h.click_at(first_import);
            assert_eq!(shown_deps(&h), before, "{at:?}: it did not fold up again");

            // The close button, found by hovering rather than by arithmetic.
            let close = egui::Id::new(("preview-close", h.app.panes[0].id));
            let found = h
                .find(
                    close,
                    panel.right() - 16.0,
                    (panel.top() as i32)..(panel.top() as i32 + 34),
                )
                .unwrap_or_else(|| panic!("{at:?}: nothing answers to the close button"));
            assert_eq!(h.click_at(found), vec!["ClosePreview"]);
            assert!(!h.app.panes[0].tab().preview.open);
            // And the listing has the whole body back.
            assert_eq!(
                crate::ui::preview::split(pane_body(&h), false, h.app.preview),
                (pane_body(&h), None),
                "{at:?}: the room did not come back"
            );
        }
    }

    /// **The find bar over a text preview**, driven the way it is used: the button on the panel's
    /// bar, then typing, then the arrows.
    ///
    /// Every part of this is the kind that looks right in the source and does nothing on screen. The
    /// button is on a bar inside a panel inside a pane, over a listing that hit-tests every point of
    /// it. The bar itself is a floating layer over a `ScrollArea` holding a selectable label, so a
    /// click that reached the label instead of the bar would start selecting text, and a keystroke
    /// that missed the field would go to the *listing* and move the selection — which would change
    /// the file being previewed out from under the search.
    #[test]
    fn the_find_bar_searches_the_text_on_show() {
        let mut h = Harness::new();
        open_text_preview(&mut h);
        let panel = preview_rect(&h);
        let pane = h.app.panes[0].id;
        assert!(
            !h.app.panes[0].tab().preview.finding().0,
            "the bar starts shut"
        );

        // The button, hovered for rather than measured to: it is the third from the right-hand end
        // of the bar, and the buttons are 24 points at a 32-point pitch.
        let strip = (panel.top() as i32)..(panel.top() as i32 + 34);
        let id = egui::Id::new(("preview-find", pane));
        let button = (0..5)
            .find_map(|step| h.find(id, panel.right() - 20.0 - step as f32 * 32.0, strip.clone()))
            .expect("nothing on the panel's bar answers to the find button");
        h.click_at(button);
        assert!(
            h.app.panes[0].tab().preview.finding().0,
            "the button did not open the bar"
        );

        // And the caret is in the field, because the button that opens it puts it there. If this
        // keystroke went anywhere else it would be moving the selection in the listing.
        h.frame(vec![Event::Text("machine".to_owned())]);
        h.frame(Vec::new());
        let (open, at, hits, counter) = h.app.panes[0].tab().preview.finding();
        assert!(open);
        assert!(
            hits >= 4,
            "`machine` should be all over `pe.rs`, found {hits}"
        );
        assert_eq!(at, 0);
        assert_eq!(counter, format!("1 of {hits}"));

        // `Enter` is the next-match arrow, and `Shift+Enter` the previous one — which is what makes
        // the bar usable without the pointer going near it again.
        h.frame(vec![Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        assert_eq!(
            h.app.panes[0].tab().preview.finding().1,
            1,
            "Enter did not step to the next hit"
        );
        h.modifiers = Modifiers::SHIFT;
        h.frame(vec![Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::SHIFT,
        }]);
        h.modifiers = Modifiers::NONE;
        assert_eq!(
            h.app.panes[0].tab().preview.finding().1,
            0,
            "Shift+Enter did not step back"
        );

        // **`F3` and `Shift+F3` are the arrows too**, and the interesting half is what they must
        // *not* do: `F3` is also the shortcut that puts the caret in the pane's filter box, the path
        // bar is drawn before this panel, and a keystroke taken there would never reach here. So the
        // filter's text is checked as well as the hit — a `z` appearing in it would mean the path bar
        // had swallowed the key and the caret with it.
        for (shift, want, what) in [
            (Modifiers::NONE, 1, "F3"),
            (Modifiers::SHIFT, 0, "Shift+F3"),
        ] {
            h.modifiers = shift;
            h.frame(vec![Event::Key {
                key: egui::Key::F3,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: shift,
            }]);
            h.modifiers = Modifiers::NONE;
            assert_eq!(
                h.app.panes[0].tab().preview.finding().1,
                want,
                "{what} did not step the match"
            );
            assert!(
                h.tab(0).filter.is_empty(),
                "{what} put the caret in the filter box instead"
            );
        }

        // The regex toggle turns the same text into a pattern. `\bmachine\b` finds fewer than
        // `machine` does, because `machine_name` stops counting.
        h.app.panes[0].tab_mut().preview.set_regex(true);
        h.app.panes[0].tab_mut().preview.look_for(r"machine\b");
        h.frame(Vec::new());
        let (_, _, whole, _) = h.app.panes[0].tab().preview.finding();
        assert!(
            whole > 0 && whole < hits,
            "`machine\\b` found {whole} where `machine` found {hits}"
        );

        // And a pattern that will not compile says so rather than emptying the panel.
        h.app.panes[0].tab_mut().preview.look_for("(unclosed");
        h.frame(Vec::new());
        let (_, _, none, complaint) = h.app.panes[0].tab().preview.finding();
        assert_eq!(none, 0);
        assert_eq!(complaint, "Bad pattern");

        // Escape shuts it, from the keyboard, with the caret still in the field.
        h.frame(vec![Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        h.frame(Vec::new());
        assert!(
            !h.app.panes[0].tab().preview.finding().0,
            "Escape did not shut the bar"
        );
        // The panel is still showing the file, which is the point of a find bar that closes.
        assert!(h.app.panes[0].tab().preview.showing().is_some());
    }

    /// **A source file reaches the canvas coloured**, which is a claim about the galley rather than
    /// about the lexer.
    ///
    /// `crate::syntax` has its own tests and they are about byte ranges. What none of them can say is
    /// that those ranges become *layout sections with colours in them* on the shape list — the path
    /// from a span to a section runs through `Text::spans`, `coloured` and `overlay`, and a body that
    /// arrived on screen in one flat colour would pass every test in that module.
    #[test]
    fn a_source_file_reaches_the_canvas_with_its_colours() {
        let mut h = Harness::new();
        // This program's own `pe.rs`, which is Rust and long enough to hold every token kind.
        open_text_preview(&mut h);
        h.frame(Vec::new());

        // The body's galley: the one on the shape list with the most text in it by a wide margin.
        let body = h
            .texts()
            .into_iter()
            .map(|(_, text)| text)
            .max_by_key(String::len)
            .expect("the panel painted something");
        assert!(
            body.len() > 10_000,
            "that is not the body: {} bytes",
            body.len()
        );

        // Its sections' colours, which is where the answer is. Gathered off the shapes rather than
        // recomputed, so this is what epaint was handed.
        let mut inks = std::collections::BTreeSet::new();
        fn walk(shape: &egui::Shape, into: &mut std::collections::BTreeSet<[u8; 4]>) {
            match shape {
                egui::Shape::Text(text) if text.galley.job.text.len() > 10_000 => {
                    for section in &text.galley.job.sections {
                        into.insert(section.format.color.to_array());
                    }
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, into)),
                _ => {}
            }
        }
        for shape in &h.shapes {
            walk(shape, &mut inks);
        }

        let t = crate::theme::Theme::dark();
        assert!(
            inks.len() >= 5,
            "the body is set in {} colours, so it is not coloured",
            inks.len()
        );
        for (role, want) in [
            ("the body's own", t.text.primary),
            ("a keyword", t.syntax.keyword),
            ("a comment", t.syntax.comment),
            ("a string", t.syntax.string),
            ("a type", t.syntax.kind),
        ] {
            assert!(
                inks.contains(&want.to_array()),
                "{role} colour is not on the canvas: {inks:?}"
            );
        }
    }

    /// **A Markdown file is drawn as a document**, and the button beside it goes back to the markup.
    ///
    /// End to end rather than over `markdown::parse`, which has its own tests, because what those
    /// cannot say is that the *panel* chose the document — the decision runs from the extension on
    /// the worker, through `Text::doc`, to which of two functions `show` calls, and every step of
    /// that is somewhere the two views could have been swapped.
    ///
    /// The find bar is here for a reason of its own. Its hits are byte offsets, and there are now two
    /// strings they could be offsets into: the markup and the document. `bold word` exists in one of
    /// them and not the other, so a search that answers the same in both views is a search running
    /// against the wrong one — which would not be a wrong count, it would be a highlight painted over
    /// an unrelated word.
    #[test]
    fn a_markdown_file_is_rendered_and_the_toggle_shows_its_source() {
        let root = crate::sandbox::dir("md");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("a directory in the temp folder");
        std::fs::write(
            root.join("notes.md"),
            "# The heading\n\nA **bold** word and `code`.\n\n- an item\n",
        )
        .expect("a file in the temp folder");

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();
        h.app.panes[0].tab_mut().preview.open = true;
        h.app.panes[0].tab_mut().select_only(0);
        settle_preview(&mut h);

        let shown =
            |h: &Harness| -> Vec<String> { h.texts().into_iter().map(|(_, text)| text).collect() };

        // ---- Rendered: the marks are gone and the words are not ----------------
        let texts = shown(&h);
        assert!(
            texts.iter().any(|text| text == "The heading"),
            "the heading is not on screen as a heading: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text == "A bold word and code."),
            "the paragraph is not on screen with its markup followed: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("**") || text.contains("# ")),
            "the markup is still being drawn: {texts:?}"
        );

        // ---- Inline code sits on the paragraph's own baseline --------------------
        //
        // One galley, two faces, and epaint places a glyph at `ascent + valign × (row height −
        // line height)` — so the monospace run comes out three pixels above the prose unless the
        // renderer corrects it, and the tinted fill behind it stays put, which is what made it
        // read as an underline. See `ui::preview::Faces::of`.
        //
        // Asserted on the *painted* galley rather than on the correction: what matters is where
        // the glyphs ended up, and `Glyph::pos.y` is the baseline.
        fn baselines(shape: &egui::Shape, into: &mut Vec<(String, Vec<f32>)>) {
            match shape {
                egui::Shape::Text(text) => {
                    for row in &text.galley.rows {
                        let mut ys: Vec<f32> = row.glyphs.iter().map(|g| g.pos.y).collect();
                        ys.dedup();
                        into.push((text.galley.text().to_owned(), ys));
                    }
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| baselines(s, into)),
                _ => {}
            }
        }
        let mut rows = Vec::new();
        for shape in &h.shapes {
            baselines(shape, &mut rows);
        }
        let mixed: Vec<&(String, Vec<f32>)> = rows
            .iter()
            .filter(|(text, _)| text.contains("A bold word and code."))
            .collect();
        assert!(
            !mixed.is_empty(),
            "the paragraph with inline code in it was not painted"
        );
        for (text, ys) in mixed {
            assert_eq!(
                ys.len(),
                1,
                "{text:?} is drawn on {} baselines, {ys:?} — the inline code is off the line",
                ys.len()
            );
        }

        // The line-number toggle is not offered over a document, because a document has no lines of
        // the file's. Asked of the *bar*, since a control that exists but is never drawn is exactly
        // what this is checking against.
        let panel = preview_rect(&h);
        let strip = (panel.top() as i32)..(panel.top() as i32 + 34);
        let button_at = |h: &mut Harness, id: egui::Id| {
            (0..6).find_map(|step| {
                h.find(id, panel.right() - 20.0 - step as f32 * 32.0, strip.clone())
            })
        };
        assert!(
            button_at(&mut h, egui::Id::new(("preview-numbers", pane))).is_none(),
            "the line-number toggle is on the bar over a rendered document"
        );

        // ---- The find bar searches what is on the screen -----------------------
        h.app.panes[0].tab_mut().preview.look_for("bold word");
        h.frame(Vec::new());
        let (_, _, rendered_hits, _) = h.app.panes[0].tab().preview.finding();
        assert_eq!(
            rendered_hits, 1,
            "`bold word` is two words in the rendered document and should be found once"
        );

        // ---- And the toggle goes to the markup ---------------------------------
        let markup = button_at(&mut h, egui::Id::new(("preview-markup", pane)))
            .expect("nothing on the panel's bar answers to the markup toggle");
        h.click_at(markup);
        h.frame(Vec::new());
        let texts = shown(&h);
        assert!(
            texts.iter().any(|text| text.contains("# The heading")),
            "the toggle did not show the markup: {texts:?}"
        );
        assert!(
            h.app.preview.markup,
            "the toggle did not record itself as a preference"
        );
        // The same query, against the other string. Nought, because the source has two asterisks
        // between `bold` and `word` — which is the proof that the search followed the view.
        let (_, _, source_hits, counter) = h.app.panes[0].tab().preview.finding();
        assert_eq!(
            source_hits, 0,
            "`bold word` was found in the markup, where those two words are not adjacent"
        );
        assert_eq!(counter, "No results");
        // And the gutter's toggle is back, because there are lines to number again.
        assert!(
            button_at(&mut h, egui::Id::new(("preview-numbers", pane))).is_some(),
            "the line-number toggle did not come back over the markup"
        );

        crate::sandbox::remove(&root);
    }

    /// **The preview button's context menu**: show or hide, and then the three positions.
    ///
    /// Driven with a real right click, because a menu hung off a button on a bar that a listing
    /// also hit-tests is the kind of thing that looks perfectly correct in the source and simply
    /// never opens. And it is **sticky** — ticking a position leaves it up, since three radio
    /// buttons you have to reopen the menu between are three menus.
    #[test]
    fn the_preview_button_carries_the_panels_position() {
        use crate::ui::preview::Where;

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let eye = egui::Id::new(("preview", pane));

        // Find the button, which sits between the path and the flatten toggle rather than at a
        // position this test gets to assume.
        let y = h.path_bar_y(0);
        let right = h.pane_rect(0).right();
        let sweep: Vec<Pos2> = (0..40)
            .map(|step| pos2(right - 20.0 - step as f32 * 6.0, y))
            .collect();
        let at = sweep
            .into_iter()
            .find(|&at| h.hovers(eye, at))
            .expect("nothing on the path bar answers to the preview button");

        // A right click puts the menu up, with the position group in it.
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        h.frame(Vec::new());
        let entries: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
        for want in ["Show preview", "Position", "Right", "Bottom", "Auto"] {
            assert!(
                entries.iter().any(|text| text == want),
                "`{want}` is not in the menu: {entries:?}"
            );
        }

        // Ticking a position changes the window's preference — and the menu stays up, which is
        // what `sticky` is for. Found by its label rather than by arithmetic over menu rows.
        let bottom = h
            .texts()
            .into_iter()
            .find(|(_, text)| text == "Bottom")
            .map(|(at, _)| at + vec2(8.0, 6.0))
            .expect("the entry was drawn a moment ago");
        assert_eq!(h.app.preview.at, Where::Right, "the default has moved");
        h.click_at(bottom);
        assert_eq!(
            h.app.preview.at,
            Where::Bottom,
            "clicking the entry did not move the panel"
        );
        assert!(
            h.texts().iter().any(|(_, text)| text == "Position"),
            "the menu closed on a tick, so setting two of these means opening it twice"
        );
    }

    /// **A double click in the text view selects the word under it**, and keeps it.
    ///
    /// egui's own selectable `Label` implements the gesture, so this is a test that the *panel* does
    /// not get in its way — and the reason it is worth having is that nothing in the source says the
    /// gesture exists, so nothing in the source would say if it stopped. Three things could take it
    /// away: the Label losing the pointer to something drawn over it, a `Sense` that stops sensing
    /// clicks, and — the subtle one — the Label's auto-generated id changing between frames, which
    /// makes egui drop the selection on the frame after it was made. So the assertions are that the
    /// band covers **the word and not the line**, and that it is still there three frames later.
    ///
    /// The band is read out of the galley's own row mesh rather than off a `Shape::Rect`, because
    /// that is where epaint puts it: a text selection is vertices in the row it belongs to.
    #[test]
    fn a_double_click_in_the_preview_selects_the_word_under_it() {
        let root = crate::sandbox::dir("sel");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("notes.txt");
        std::fs::write(&path, "alpha beta gamma\ndelta epsilon zeta\n").unwrap();

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();
        h.app.open_preview_here();
        // Waited for by asking whether it is on screen yet, not by spending a fixed 200 ms and
        // hoping. The file is read on a worker thread, so the fixed budget was a race that this
        // test won on its own and lost in the suite, where the rest of it has the machine busy.
        let mut body_at = None;
        for _ in 0..240 {
            h.frame(Vec::new());
            body_at = h
                .texts()
                .into_iter()
                .find(|(_, text)| text.starts_with("alpha beta gamma"));
            if body_at.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Where the body was drawn, found by its own text rather than by arithmetic over the panel.
        let (at, body) = body_at.expect("the body is not on screen");
        let over = at + vec2(20.0, 8.0);
        h.frame(vec![Event::PointerMoved(over)]);
        assert_eq!(
            h.cursor,
            egui::CursorIcon::Text,
            "the text is not what the pointer is over, so nothing below means anything"
        );

        h.double_click_at(over);
        let ink = h.ctx.style_of(egui::Theme::Dark).visuals.selection.bg_fill;
        let selected = |h: &Harness| -> Vec<(usize, Rect, f32)> {
            fn walk(shape: &egui::Shape, ink: egui::Color32, into: &mut Vec<(usize, Rect, f32)>) {
                match shape {
                    egui::Shape::Text(text) => {
                        for (which, row) in text.galley.rows.iter().enumerate() {
                            let mut band = Rect::NOTHING;
                            for vertex in row.visuals.mesh.vertices.iter().filter(|v| v.color == ink)
                            {
                                band.extend_with(vertex.pos);
                            }
                            if band.is_finite() {
                                into.push((which, band, row.size.x));
                            }
                        }
                    }
                    egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, ink, into)),
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for shape in &h.shapes {
                walk(shape, ink, &mut out);
            }
            out
        };

        let bands = selected(&h);
        assert_eq!(bands.len(), 1, "a double click selected {bands:?}");
        let (row, band, row_width) = bands[0];
        assert_eq!(row, 0, "the word is on the row that was clicked");
        assert!(
            band.left() < 1.0 && band.width() < row_width * 0.5,
            "the band is {:.1} wide of a {row_width:.1} row — a line rather than a word",
            band.width()
        );
        // The word, checked against the text itself rather than against a number: `alpha ` is the
        // first word and a space, so the band has to stop inside that span.
        assert!(
            body.starts_with("alpha "),
            "the fixture is not what this assertion is about"
        );

        // And it is still there afterwards. A selection that lasts one frame is a selection nobody
        // has — this is what a changing widget id would look like.
        for after in 1..=3 {
            h.frame(Vec::new());
            assert_eq!(
                selected(&h).first().map(|(_, band, _)| band.width()),
                Some(band.width()),
                "the selection was gone {after} frame(s) later"
            );
        }
        crate::sandbox::remove(&root);
    }

    /// **A row says what it is when the pointer rests on it**, including what git says.
    ///
    /// The four columns are on the row already; what the tooltip adds is the part a column cannot hold
    /// — the exact byte count, a name too long for the Name column — and the badge's meaning in words.
    /// So the assertions are on the name, the exact size, and the git line, and on the tooltip *not*
    /// appearing while the pointer is doing something: over a rubber band it would be in the way.
    #[test]
    #[cfg(windows)]
    fn a_row_says_what_it_is_when_the_pointer_rests_on_it() {
        let root = crate::sandbox::dir("tip");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A size with a grouping comma in it, and a name longer than a narrow Name column.
        std::fs::write(root.join("a-rather-long-name.txt"), vec![b'x'; 9605]).unwrap();

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();

        let over = h.row_center(0, 0);
        // egui holds a tooltip back for `interaction.tooltip_delay` *and* until the pointer has come
        // to rest, so this moves once and then waits.
        h.frame(vec![Event::PointerMoved(over)]);
        for _ in 0..40 {
            h.frame(Vec::new());
        }
        // The tooltip is a **table**, so it is a galley per cell rather than one with newlines in it:
        // `Name` is its first key, and the row's own name is the value beside it. Found by the key,
        // because the value is a string the row itself also paints — see `tooltip_table`.
        let painted = h.texts();
        let lines: Vec<String> = painted.iter().map(|(_, text)| text.clone()).collect();
        let tip = |h: &Harness, key: &str| -> Option<(Pos2, String)> {
            // Inside the tooltip's own area and nowhere else: `Name`, `Size`, `Type` and `Modified`
            // are the column headers as well, four inches up the same window.
            let frame = h.tooltip_rect()?;
            let painted: Vec<(Pos2, String)> = h
                .texts()
                .into_iter()
                .filter(|(at, _)| frame.contains(*at))
                .collect();
            // The keys are drawn down the left-hand column and the values down the right, so the
            // value of a key is the next text along the same line.
            let (at, _) = painted.iter().find(|(_, text)| text == key)?;
            let value = painted
                .iter()
                .filter(|(pos, _)| (pos.y - at.y).abs() < 1.0 && pos.x > at.x)
                .min_by(|(a, _), (b, _)| a.x.total_cmp(&b.x))?;
            Some((*at, value.1.clone()))
        };
        let (at, name) = tip(&h, "Name")
            .unwrap_or_else(|| panic!("no tooltip over the row: {lines:?}"));
        assert_eq!(
            name, "a-rather-long-name.txt",
            "the name is the reason the tooltip exists"
        );

        // **And it is at the pointer.** The response it hangs off is the whole visible block of
        // rows, so anchored the way a button's tooltip is it would come up below the *last* row on
        // screen — hundreds of points from the row it describes, and about a row the pointer is
        // nowhere near. The bound is loose on purpose: what is being asserted is the anchor, not
        // the frame's padding or which way egui flipped it to stay on screen.
        let away = (at - over).abs();
        assert!(
            away.x < 60.0 && away.y < 60.0,
            "the tooltip is at {at:?} and the pointer is at {over:?}: {away:?} away"
        );

        // **And not under the cursor**, which is the other half of being at the pointer: the arrow
        // hangs down and to the right of the position it is pointing at, so a tooltip flush against
        // that position is a tooltip with an arrow drawn over its first word. The rect is the
        // frame's own, not the text's, because the frame is what the cursor would be seen on top of
        // — and it is below the pointer here because row 0 has the whole list under it to open into.
        let frame = h.tooltip_rect().expect("the tooltip is up, so it has an area");
        assert!(
            !frame.contains(over),
            "the pointer is inside the tooltip: {frame:?} around {over:?}"
        );
        assert!(
            frame.top() - over.y >= azur_egui_theme::components::CURSOR_CLEARANCE,
            "the tooltip clears the pointer by {}, less than the arrow is tall: {frame:?}",
            frame.top() - over.y
        );

        let size = tip(&h, "Size").expect("a file has a size").1;
        assert!(
            size.contains("9,605 bytes"),
            "the exact size is the thing the Size column cannot say: {size:?}"
        );
        assert!(
            size.contains("9.38 KB"),
            "and the rounded one is what the column does say: {size:?}"
        );
        assert!(tip(&h, "Type").is_some() && tip(&h, "Modified").is_some());
        assert!(
            tip(&h, "Git").is_none(),
            "there is no repository here, so there is no line about one"
        );
        assert!(
            tip(&h, "In").is_none(),
            "and the folder is not flattened, so every row is in it"
        );

        // And it stays out of the way of a gesture. A press starts a band, and a tooltip over a band
        // is a tooltip over the thing being dragged.
        h.frame(vec![Event::PointerButton {
            pos: over,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        h.frame(vec![Event::PointerMoved(over + vec2(0.0, 8.0))]);
        let while_down: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
        assert!(
            !while_down
                .iter()
                .any(|text| text.contains("9,605 bytes")),
            "the tooltip is up while the button is down: {while_down:?}"
        );
        crate::sandbox::remove(&root);

        // ---- And what git says, which is a line the row can only draw as a badge ----
        //
        // In this repository, which is where the harness opens: `README.md` is a file git has an
        // opinion about whenever this suite is run from a working tree with changes in it. Skipped
        // rather than failed where git has nothing to say — a clean checkout is not a broken tooltip.
        let mut h = Harness::new();
        let row = (0..h.tab(0).order.len()).find(|&at| {
            let tab = h.tab(0);
            tab.entry_at(at)
                .zip(tab.dir.as_ref())
                .and_then(|(entry, dir)| {
                    tab.git
                        .as_ref()
                        .and_then(|repo| repo.state(dir.name(entry)))
                })
                .is_some_and(|state| state != crate::git::State::Clean)
        });
        let Some(row) = row else {
            eprintln!("nothing in this working tree has changed: skipping the git half");
            return;
        };
        let name = {
            let tab = h.tab(0);
            let entry = tab.entry_at(row).expect("the row");
            tab.dir.as_ref().expect("a listing").leaf(entry).to_owned()
        };
        let over = h.row_center(0, row);
        h.frame(vec![Event::PointerMoved(over)]);
        for _ in 0..40 {
            h.frame(Vec::new());
        }
        let frame = h.tooltip_rect().expect("the tooltip is up");
        let painted: Vec<(Pos2, String)> = h
            .texts()
            .into_iter()
            .filter(|(at, _)| frame.contains(*at))
            .collect();
        let lines: Vec<String> = painted.iter().map(|(_, text)| text.clone()).collect();
        let value_of = |key: &str| -> Option<String> {
            let (at, _) = painted.iter().find(|(_, text)| text == key)?;
            painted
                .iter()
                .filter(|(pos, _)| (pos.y - at.y).abs() < 1.0 && pos.x > at.x)
                .min_by(|(a, _), (b, _)| a.x.total_cmp(&b.x))
                .map(|(_, text)| text.clone())
        };
        assert_eq!(
            value_of("Name").as_deref(),
            Some(name.as_str()),
            "no tooltip over `{name}`: {lines:?}"
        );
        let said = value_of("Git")
            .unwrap_or_else(|| panic!("`{name}` has changed and the tooltip has no `Git` line"));
        assert!(
            !said.is_empty(),
            "`{name}` has changed, and the tooltip does not say what: {said:?}"
        );
    }

    /// `Ctrl+P` opens this folder's preview panel, and closes it again.
    ///
    /// On the pane the keyboard is in and on that pane's tab, which is the whole of what "each
    /// folder has one" means: the shortcut is not a window-wide switch.
    #[test]
    fn ctrl_p_opens_and_shuts_this_folders_preview() {
        let mut h = Harness::with_panes(2);
        let second = h.app.panes[1].id;
        h.app.perform(&h.ctx.clone(), Action::Focus(second));
        h.frame(Vec::new());

        let press = |h: &mut Harness| {
            h.take_journal();
            h.modifiers = Modifiers::COMMAND;
            h.frame(vec![Event::Key {
                key: egui::Key::P,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            }]);
            h.modifiers = Modifiers::NONE;
            h.frame(Vec::new());
            h.take_journal()
        };

        assert!(!h.app.panes[1].tab().preview.open);
        assert_eq!(press(&mut h), vec!["TogglePreview"]);
        assert!(h.app.panes[1].tab().preview.open, "it did not open");
        // And only in the pane the keyboard is in.
        assert!(
            !h.app.panes[0].tab().preview.open,
            "it opened in the other pane as well"
        );
        assert_eq!(press(&mut h), vec!["TogglePreview"]);
        assert!(!h.app.panes[1].tab().preview.open, "it did not shut again");
    }

    /// **Two selected pictures become a comparison**: three views, and a toggle down to one.
    ///
    /// End to end, because the interesting part is the *decision* — two selected rows rather than
    /// one cursor — and that lives in `App::selected_preview` where the panel cannot see it. The
    /// fixtures are written here: two PNGs that differ in one corner, which is a diff with a known
    /// answer.
    #[test]
    fn two_selected_pictures_are_compared_in_three_views() {
        let root = crate::sandbox::dir("diff");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("a directory in the temp folder");
        // Three of them, because the last assertion is about what *three* selected pictures do and
        // `select_all` over two would still be a pair.
        for (name, tint) in [("a.png", 40u8), ("b.png", 200), ("c.png", 40)] {
            let mut buffer = image::RgbaImage::from_pixel(60, 40, image::Rgba([9, 9, 9, 255]));
            buffer.put_pixel(50, 30, image::Rgba([tint, 9, 9, 255]));
            buffer
                .save(root.join(name))
                .expect("a PNG in the temp folder");
        }

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();
        h.app.panes[0].tab_mut().preview.open = true;

        // One picture selected is one picture: the comparison is not something a single selection
        // can produce by accident.
        h.app.panes[0].tab_mut().select_only(0);
        settle_preview(&mut h);
        assert_eq!(
            h.app.panes[0].tab().preview.frames(),
            Some((1, 1)),
            "one selected picture is not one view"
        );

        // And the second one turns it into a comparison, without a shortcut or a menu.
        h.app.panes[0].tab_mut().toggle(1);
        settle_preview(&mut h);
        assert_eq!(
            h.app.panes[0].tab().preview.frames(),
            Some((3, 3)),
            "two selected pictures did not become three views"
        );
        let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
        assert!(
            texts.iter().any(|text| text.contains("↔")),
            "the bar does not name both files: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text == "differences"),
            "the third view is not captioned: {texts:?}"
        );
        // 1 pixel of 2,400 differs — the corner each file tinted differently. Reported on a panel
        // given room for it: at the default share the bar's priority rule has already dropped the
        // comment for a name this long, which the next assertion is about.
        h.app.preview.share = 0.66;
        h.frame(Vec::new());
        let wide: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
        assert!(
            wide.iter().any(|text| text.contains("0.04% differs")),
            "the share that differs is not reported: {wide:?}"
        );
        assert!(
            wide.iter().any(|text| text.contains("60 × 40")),
            "the size is not reported either: {wide:?}"
        );

        // **And the rule, end to end.** Swept from a wide bar to a narrow one, the two details go
        // in order and the name is on the bar the whole way. The invariant that says it is
        // *`comment` never survives `size`* — which holds at every width and does not depend on
        // what this machine's font measures. `what_fits` has the unit test for the arithmetic; this
        // is about what is actually drawn.
        let mut seen = Vec::new();
        for share in [0.66, 0.58, 0.50, 0.42, 0.34, 0.26] {
            h.app.preview.share = share;
            h.frame(Vec::new());
            let bar: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
            let comment = bar.iter().any(|text| text.contains("differs"));
            let size = bar.iter().any(|text| text.contains("60 × 40"));
            assert!(
                !(comment && !size),
                "at {share} the comment is on the bar and the size is not, which is backwards: \
                 {bar:?}"
            );
            assert!(
                bar.iter().any(|text| text.starts_with("a.png")),
                "at {share} the name is gone, and the details were droppable: {bar:?}"
            );
            seen.push((comment, size));
        }
        assert_eq!(seen.first(), Some(&(true, true)), "the widest bar: {seen:?}");
        assert_eq!(seen.last(), Some(&(false, false)), "the narrowest: {seen:?}");
        // Monotone: nothing comes back as the bar narrows.
        for pair in seen.windows(2) {
            assert!(
                !(pair[1].0 && !pair[0].0) && !(pair[1].1 && !pair[0].1),
                "a detail reappeared on a narrower bar: {seen:?}"
            );
        }
        h.app.preview.share = crate::ui::preview::SHARE;

        // The toggle takes it down to the difference alone, and back.
        h.app.panes[0].tab_mut().preview.toggle_all();
        h.frame(Vec::new());
        assert_eq!(h.app.panes[0].tab().preview.frames(), Some((3, 1)));
        h.app.panes[0].tab_mut().preview.toggle_all();
        h.frame(Vec::new());
        assert_eq!(h.app.panes[0].tab().preview.frames(), Some((3, 3)));

        // A third selected picture is not a comparison of anything, so the panel goes back to
        // having nothing to say rather than picking two of them.
        h.app.panes[0].tab_mut().toggle(0);
        h.app.panes[0].tab_mut().toggle(1);
        h.app.panes[0].tab_mut().select_all();
        settle_preview(&mut h);
        assert!(
            h.app.panes[0]
                .tab()
                .preview
                .frames()
                .is_none_or(|(_, shown)| shown == 1),
            "three selected pictures produced a comparison"
        );
        crate::sandbox::remove(&root);
    }

    /// Let the focused pane's preview panel notice the selection and finish reading.
    fn settle_preview(h: &mut Harness) {
        h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
        for attempt in 0..400 {
            h.frame(Vec::new());
            if !h.app.preview_pending() && attempt > 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!h.app.preview_pending(), "the read never came back");
        // **The read landing and the read being drawn are different frames.** The loop above stops
        // on the frame the worker's result arrived in, and that frame was already painted from what
        // the panel had before it — so a caller reading `h.texts()` straight afterwards sees the
        // previous contents. It showed up as preview tests that passed alone and failed in the
        // suite, where they are slow enough for the result to arrive a frame later.
        for _ in 0..2 {
            h.frame(Vec::new());
        }
    }

    /// **The zoom field's list opens, and the field lets the keyboard go again.**
    ///
    /// Two failures this catches, and both were real. A combo box whose popup never appears is a
    /// combo box with no presets — you can only type at it. And a text field that keeps focus after
    /// you have finished with it takes every shortcut in the window with it: `Ctrl+P`, `F5`, the
    /// arrow keys, all of them go to the field instead.
    #[test]
    fn the_zoom_field_opens_its_list_and_gives_the_keyboard_back() {
        let root = crate::sandbox::dir("zoom");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("a directory in the temp folder");
        image::RgbaImage::from_pixel(80, 60, image::Rgba([30, 90, 200, 255]))
            .save(root.join("swatch.png"))
            .expect("a PNG in the temp folder");

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();
        // A wide panel, so the zoom group is on the bar at all.
        h.app.preview.share = 0.6;
        h.app.panes[0].tab_mut().preview.open = true;
        h.app.panes[0].tab_mut().select_only(0);
        settle_preview(&mut h);
        assert!(
            h.app.panes[0].tab().preview.frames().is_some(),
            "the picture is not on the canvas, so there is no zoom field"
        );

        // Find the field by its value: `100%` is what an 80×60 picture in a panel this size reads.
        let panel = preview_rect(&h);
        let at = h
            .texts()
            .into_iter()
            .find(|(_, text)| text.ends_with('%'))
            .map(|(at, _)| at + vec2(8.0, 6.0))
            .expect("the zoom field is not on the bar");
        assert!(panel.contains(at), "the field is not in the panel: {at:?}");

        // Clicking it opens the list. Every preset, not only the one the text happens to match:
        // a field that filters its own value down to one row is a list with one row in it.
        h.click_at(at);
        let drawn = h.texts();
        let list: Vec<String> = drawn.iter().map(|(_, text)| text.clone()).collect();
        for preset in ["Fit", "25%", "100%", "400%"] {
            assert!(
                list.iter().any(|text| text == preset),
                "`{preset}` is not in the list: {list:?}"
            );
        }
        // Every label **whole**, which is the other half of it: a list sized to the trigger turns
        // `400%` into `40…`, and the assertion above cannot see that — `Galley::text` reports the
        // string it was *asked* to draw, so an elided label still answers `400%`. `Harness::cropped`
        // asks the galley whether it fitted, which is the only question that distinguishes them.
        let cropped = h.cropped();
        for preset in ["Fit", "25%", "100%", "400%"] {
            assert!(
                !cropped.iter().any(|text| text == preset),
                "`{preset}` is cropped in the list: {cropped:?}"
            );
        }
        // **And flush with the field's left edge**, not indented for an icon column that nothing in
        // a list of percentages could ever fill. `azur::MenuItem::gutter` is the rule; without it
        // every entry here sits 24 points in, which reads as an indent with no cause.
        let fit = drawn
            .iter()
            .find(|(_, text)| text == "Fit")
            .map(|(at, _)| *at)
            .expect("the list was drawn a moment ago");
        assert!(
            fit.x < panel_field_left(&h) + 16.0,
            "the list is indented for an icon column: label at {}, field at {}",
            fit.x,
            panel_field_left(&h)
        );

        // And it is *below the field*, stacked, rather than somewhere arbitrary.
        let entries: Vec<Pos2> = drawn
            .iter()
            .filter(|(_, text)| text == "Fit" || text == "400%")
            .map(|(at, _)| *at)
            .collect();
        assert_eq!(entries.len(), 2, "the two ends of the list were not both drawn");
        for entry in &entries {
            assert!(
                entry.y > at.y,
                "the list is not below the field: entry at {entry:?}, field at {at:?}"
            );
        }
        assert!(
            entries[1].y > entries[0].y,
            "the list is not stacked: {entries:?}"
        );

        // And the field lets go: a click on the canvas takes the keyboard back, so the window's
        // own shortcuts work again. Tested through `Ctrl+P`, which is one of the ones it was
        // swallowing.
        h.click_at(panel.center());
        assert!(
            h.ctx.memory(|m| m.focused()).is_none(),
            "the zoom field still has the keyboard"
        );
        h.take_journal();
        h.modifiers = Modifiers::COMMAND;
        h.frame(vec![Event::Key {
            key: egui::Key::P,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }]);
        h.modifiers = Modifiers::NONE;
        h.frame(Vec::new());
        assert_eq!(
            h.take_journal(),
            vec!["TogglePreview"],
            "the shortcut went to the field rather than the window"
        );
        crate::sandbox::remove(&root);
    }

    /// Where the zoom field starts, which is where its list is anchored.
    ///
    /// Taken from the `%` the field draws rather than from the geometry: the field is laid out
    /// right-to-left off the bar's controls, and re-deriving that here would be re-deriving the
    /// thing under test.
    fn panel_field_left(h: &Harness) -> f32 {
        h.texts()
            .into_iter()
            .filter(|(_, text)| text.ends_with('%'))
            .map(|(at, _)| at.x)
            .fold(f32::INFINITY, f32::min)
            - 8.0
    }

    /// **The panel follows the keyboard**, once the keyboard stops moving.
    ///
    /// End to end through the real frame loop: the cursor lands on a binary, nothing happens for
    /// a quarter of a second, and then a walk of *that* file is what the panel is showing. Which
    /// is the whole of the gesture — open it once, then arrow down a folder and look at each file
    /// in turn — and four separate things have to be right for it: the cursor being noticed, the
    /// kind being worked out from the name, the wait, and the answer finding its way back to the
    /// panel that asked.
    ///
    /// The fixture is the test binary and the folder it is in, which is the one place on any
    /// machine guaranteed to hold a real PE image.
    #[test]
    fn the_preview_panel_follows_the_keyboard() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let me = std::env::current_exe().expect("a test process has an executable");
        let folder = me.parent().expect("it is in a folder").to_path_buf();
        h.app
            .perform(&h.ctx.clone(), Action::Navigate { pane, path: folder });
        h.settle();

        let showing = |h: &Harness| {
            h.app.panes[0]
                .tab()
                .preview
                .showing()
                .map(|path| path.to_path_buf())
        };

        // The panel, open and pointed at nothing yet.
        h.app.panes[0].tab_mut().preview.open = true;
        h.app.panes[0].tab_mut().clear_selection();
        h.frame(Vec::new());

        // The keyboard onto the test binary, the way a click leaves it.
        let name = me
            .file_name()
            .expect("it has a name")
            .to_string_lossy()
            .into_owned();
        let at = {
            let tab = h.app.panes[0].tab();
            let dir = tab.dir.as_ref().expect("the listing arrived");
            tab.order
                .iter()
                .position(|&i| dir.name(i as usize) == name)
                .unwrap_or_else(|| panic!("{name} is not in its own folder's listing"))
        };
        h.app.panes[0].tab_mut().select_only(at);

        // Not yet. This is the assertion the wait exists for: holding an arrow key through a
        // folder of images must not decode thirty of them.
        h.frame(Vec::new());
        assert!(
            showing(&h).is_none() && h.app.preview_pending(),
            "the read started on the keystroke rather than waiting for it to stop"
        );

        // And then it does, on that file, once.
        h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
        for attempt in 0..400 {
            h.frame(Vec::new());
            if !h.app.preview_pending() && attempt > 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            showing(&h).as_deref(),
            Some(me.as_path()),
            "the panel is showing something else"
        );
        let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
        assert!(
            texts.iter().any(|text| text.contains("API set")),
            "the walk landed and the panel is not showing it: {texts:?}"
        );

        // The keyboard moving onto something with no preview clears it, rather than leaving a
        // stale answer beside a different row. This panel is *inside* the pane, so what it shows
        // is read as being about the selection next to it.
        let plain = {
            let tab = h.app.panes[0].tab();
            let dir = tab.dir.as_ref().expect("the listing");
            (0..tab.order.len()).find(|&row| {
                tab.entry_at(row).is_some_and(|entry| {
                    crate::preview::kind_of(
                        dir.leaf(entry),
                        dir.ext(entry),
                        dir.entries[entry].is_dir(),
                    )
                    .is_none()
                })
            })
        }
        .expect("a build folder holds something with no preview");
        h.app.panes[0].tab_mut().select_only(plain);
        h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
        h.frame(Vec::new());
        h.frame(Vec::new());
        assert_eq!(showing(&h), None, "the panel kept a stale answer");
    }

    /// **Everything in a dependency row is on one baseline.**
    ///
    /// Three texts at two sizes — a 14-point name, a 12-point location, a 12-point processor
    /// tag — and the assertion is *exact* rather than within a tolerance, because the property
    /// is exact: the row commits to one `azur::components::ink_baseline` and every galley is
    /// placed by subtracting its own ascent from it.
    ///
    /// What it catches is the way this is usually written. Centring each galley in the row —
    /// which is what every other listing in this window does — leaves the two 12-point columns
    /// **1.5 points above** the name beside them, because the two fonts differ in line height
    /// *and* in ascent and centring the boxes cancels neither. That is invisible in the source,
    /// visible on screen as a column that steps down as the eye crosses the row, and this is the
    /// test that would fail.
    #[test]
    fn everything_in_a_dependency_row_sits_on_one_line() {
        let mut h = Harness::new();
        open_preview(&mut h);

        // The panel's rows, which is everything drawn below its header. Taken from what was
        // painted rather than from the geometry, so this does not have to re-derive the layout.
        let panel = preview_rect(&h);
        let rows_area = Rect::from_min_max(
            pos2(panel.left(), panel.top() + crate::ui::preview::HEADER),
            panel.max,
        );
        let mut rows: std::collections::BTreeMap<i64, Vec<(f32, String)>> = Default::default();
        for (at, text) in h.baselines() {
            // By rect and not by a y threshold: the panel is *inside* a pane now, so the sidebar
            // and the listing have text at the same heights and only the x tells them apart.
            if !rows_area.contains(at) || text.is_empty() {
                continue;
            }
            // Which row it is in, from the baseline itself: rows are `ROW` apart, so anything
            // within one of them belongs to the same one.
            rows.entry(((at.y - rows_area.top()) / crate::ui::deps::ROW) as i64)
                .or_default()
                .push((at.y, text));
        }
        assert!(
            rows.len() >= 4,
            "the panel drew {} rows of text; there is nothing to compare",
            rows.len()
        );

        let mut widest = 0;
        for (which, texts) in &rows {
            let first = texts[0].0;
            widest = widest.max(texts.len());
            for (baseline, text) in texts {
                assert_eq!(
                    *baseline, first,
                    "row {which}: `{text}` sits on {baseline} and `{}` on {first}",
                    texts[0].1
                );
            }
        }
        // And a row really did have all three columns in it, or the fonts never differed and
        // the assertion above proves nothing.
        assert!(
            widest >= 3,
            "no row had a name, a location and a tag in it: at most {widest} texts"
        );
    }

    /// `Ctrl+P` works with the caret in the filter box too, and the filter survives it.
    ///
    /// The same argument as `Ctrl+E` below, and the same gesture in a different order: you type two
    /// letters to find a file, then want to see what is inside it. A focused text field otherwise
    /// owns the keyboard outright — which is right for every other shortcut and wrong for these two.
    #[test]
    fn ctrl_p_reaches_through_the_filter_box() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: sources,
            },
        );
        h.settle();

        // Ctrl+F, which is how the caret gets there without a click. Then a second frame: the bar
        // asks for focus on the frame it sees the shortcut, and egui grants it at the end.
        let held = Modifiers::COMMAND;
        let press = |h: &mut Harness, key: egui::Key| {
            h.modifiers = held;
            h.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: held,
            }]);
            h.modifiers = Modifiers::NONE;
        };
        press(&mut h, egui::Key::F);
        h.frame(Vec::new());
        assert!(
            h.ctx.memory(|m| m.focused()).is_some(),
            "Ctrl+F did not put the caret in the filter box, so this test proves nothing"
        );

        h.app.panes[0].tab_mut().filter = "sort".to_owned();
        h.app.panes[0].tab_mut().rebuild_order();
        h.frame(Vec::new());
        h.take_journal();

        press(&mut h, egui::Key::P);
        let done = h.take_journal();
        assert!(
            done.contains(&"TogglePreview"),
            "Ctrl+P did not get through the filter box, got {done:?}"
        );
        assert!(h.app.panes[0].tab().preview.open, "the panel did not open");
        assert_eq!(
            h.app.panes[0].tab().filter,
            "sort",
            "the filter went with the toggle"
        );
        // And the caret is still in the box, so the two really do compose rather than one
        // interrupting the other.
        assert!(
            h.ctx.memory(|m| m.focused()).is_some(),
            "the shortcut took the caret out of the filter box"
        );

        // And again, to shut it: a shortcut you can only use one way round is a trap.
        press(&mut h, egui::Key::P);
        let done = h.take_journal();
        assert!(done.contains(&"TogglePreview"), "got {done:?}");
        assert!(!h.app.panes[0].tab().preview.open, "it did not shut again");
    }

    /// `Ctrl+E` works with the caret in the filter box, and the filter survives the toggle.
    ///
    /// The two compose, and that is the gesture: type two letters, look at what came up, and
    /// want the rest of the tree. A focused text field otherwise owns the keyboard outright —
    /// which is right for every other shortcut and wrong for this one.
    #[test]
    fn ctrl_e_reaches_through_the_filter_box() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: sources,
            },
        );
        h.settle();

        // Ctrl+F, which is how the caret gets there without a click. Then a second frame: the
        // bar asks for focus on the frame it sees the shortcut, and egui grants it at the end.
        let held = Modifiers::COMMAND;
        h.modifiers = held;
        h.frame(vec![Event::Key {
            key: egui::Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: held,
        }]);
        h.modifiers = Modifiers::NONE;
        h.frame(Vec::new());
        assert!(
            h.ctx.memory(|m| m.focused()).is_some(),
            "Ctrl+F did not put the caret in the filter box, so this test proves nothing"
        );

        // A filter typed in, so the other half of the claim can be checked: it survives.
        h.app.panes[0].tab_mut().filter = "sort".to_owned();
        h.app.panes[0].tab_mut().rebuild_order();
        h.frame(Vec::new());
        h.take_journal();

        h.modifiers = held;
        h.frame(vec![Event::Key {
            key: egui::Key::E,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: held,
        }]);
        h.modifiers = Modifiers::NONE;
        let done = h.take_journal();
        assert!(
            done.contains(&"ToggleFlat"),
            "Ctrl+E did not get through the filter box, got {done:?}"
        );
        h.settle();

        let tab = h.app.panes[0].tab();
        assert!(tab.flat);
        assert_eq!(tab.filter, "sort", "the filter went with the toggle");
        let names = shown_names(&h, 0);
        assert!(
            !names.is_empty() && names.iter().all(|name| name.contains("sort")),
            "the filter is not being applied to the flattened listing: {names:?}"
        );
        assert!(
            names.iter().any(|name| name.contains('\\')),
            "nothing from a subfolder came up, so the tree was not walked: {names:?}"
        );
    }

    /// A flatten is a question about *this* folder, so it does not come along to the next one.
    ///
    /// Which is also what makes opening a row the way out of the view: without this, clicking a
    /// folder three levels down in a flattened listing would start a second tree walk on
    /// arrival, and there would be no gesture that ended one.
    #[test]
    fn a_flattened_view_does_not_follow_you_into_the_next_folder() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let ctx = h.ctx.clone();
        h.app.perform(
            &ctx,
            Action::Navigate {
                pane,
                path: sources.clone(),
            },
        );
        h.app.perform(&ctx, Action::ToggleFlat(pane));
        h.settle();
        assert!(h.app.panes[0].tab().flat);

        h.app.perform(
            &ctx,
            Action::Navigate {
                pane,
                path: sources.join("ui"),
            },
        );
        h.settle();
        assert!(
            !h.app.panes[0].tab().flat,
            "the flatten followed the navigation"
        );
        assert!(
            shown_names(&h, 0).iter().all(|name| !name.contains('\\')),
            "and the listing that arrived was still a flattened one"
        );

        // A *refresh* is the same question again, so that one keeps it.
        h.app.perform(&ctx, Action::ToggleFlat(pane));
        h.settle();
        h.app.perform(&ctx, Action::Refresh(pane));
        h.settle();
        assert!(
            h.app.panes[0].tab().flat,
            "F5 turned the flatten off, which is not what a re-read means"
        );
        // Only the flag, and deliberately: the folder here is `src\ui`, which has no subfolders of
        // its own, so its flattened listing and its shallow one are the same rows. There is
        // nothing about the *listing* this fixture could tell you — which is exactly why the flag
        // is not enough on its own. That half is
        // `deep_reads_survive_a_folder_changing_underneath_them`, over a folder that has a tree.
    }

    /// **A folder changing on disk re-reads a flattened tab as a flattened tab.**
    ///
    /// This is the flatten "resetting on its own". [`App::folder_changed`] is what
    /// [`crate::watch`] calls when a folder changes underneath the window, and it asked for the
    /// folder's own children whatever view the tab was in — so saving a file into a flattened
    /// folder replaced the tree with one level of it, with [`Tab::flat`] still set and the button
    /// still lit. Nothing announced itself: the listing was simply shallow again.
    ///
    /// What made it hard to pin down is that the trigger is invisible and rare. The watch is not
    /// recursive, so only a change to the flattened folder's *own* files could do it — a file
    /// saved three folders down did nothing at all — and a burst settles for 150 ms before
    /// anything is asked. So it fired on some saves and not others, minutes apart, with no gesture
    /// of the user's anywhere near it.
    ///
    /// Driven through `folder_changed` rather than by writing a file and waiting on the watcher:
    /// the thread is real and so is the settle window, and what is being tested is what the
    /// program does with the notification, not that Windows sends one.
    #[test]
    fn deep_reads_survive_a_folder_changing_underneath_them() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let ctx = h.ctx.clone();
        h.app.perform(
            &ctx,
            Action::Navigate {
                pane,
                path: sources.clone(),
            },
        );
        h.app.perform(&ctx, Action::ToggleFlat(pane));
        h.settle();
        let flat = shown_names(&h, 0);
        assert!(
            flat.iter().any(|name| name.contains('\\')),
            "the fixture is not flattened, so this test proves nothing: {:?}",
            &flat[..flat.len().min(5)]
        );

        // What the watcher does when something writes into `src`.
        h.app.folder_changed(&sources);
        h.settle();

        let tab = h.app.panes[0].tab();
        assert!(tab.flat, "the flag went, which was never the half that went");
        let after = shown_names(&h, 0);
        assert!(
            after.iter().any(|name| name.contains('\\')),
            "the re-read put the folder's own children on screen under a lit button: {:?}",
            &after[..after.len().min(5)]
        );
        assert_eq!(after, flat, "the tree came back different from how it went");

        // And `F5` over the same folder, which is the other way a listing is re-read. It was
        // already right — `Tab::refresh` drops the listing and lets `App::start_scans` re-ask,
        // and that one branches — but the two paths are a pair, and the assertion that can see
        // the difference belongs where both of them can be put through it.
        h.app.perform(&ctx, Action::Refresh(pane));
        h.settle();
        assert_eq!(
            shown_names(&h, 0),
            flat,
            "F5 did not come back with the tree"
        );
    }

    #[test]
    fn a_folder_dropped_on_the_sidebar_is_bookmarked() {
        // The bookmarks group publishes itself as a drop zone, and a drop there pins
        // instead of copying. The zone has to *exist*: without it the drag reports "no"
        // to the pointer and the drop never happens at all.
        let mut h = Harness::new();
        let rect = h
            .app
            .bookmarks_rect
            .expect("the bookmarks group did not publish a drop zone");
        assert!(rect.height() > 0.0 && rect.width() > 0.0);

        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        h.app.perform(
            &h.ctx,
            Action::AddBookmark(here.join("src")),
        );
        assert!(
            h.app.bookmarks.contains(&here.join("src")),
            "pinning is what a drop on that zone performs"
        );
    }

    #[test]
    fn bookmarks_reorder_to_where_they_are_dropped() {
        // Insertion-point arithmetic, which is where an off-by-one is invisible until a
        // list quietly reverses itself.
        let ctx = egui::Context::default();
        let mut app = App::opening(&ctx, Config::default(), Vec::new(), Side::Right);
        app.bookmarks = ["a", "b", "c", "d"].iter().map(PathBuf::from).collect();
        let names = |app: &App| -> Vec<String> {
            app.bookmarks
                .iter()
                .map(|p| p.display().to_string())
                .collect()
        };

        // The first one to the end.
        app.perform(&ctx, Action::MoveBookmark { from: 0, to: 4 });
        assert_eq!(names(&app), ["b", "c", "d", "a"]);
        // The last one to the front.
        app.perform(&ctx, Action::MoveBookmark { from: 3, to: 0 });
        assert_eq!(names(&app), ["a", "b", "c", "d"]);
        // One place down: `to` is an insertion point in the list as it was, so 2 means
        // "before what is currently at 2".
        app.perform(&ctx, Action::MoveBookmark { from: 0, to: 2 });
        assert_eq!(names(&app), ["b", "a", "c", "d"]);
        // Nowhere, twice: onto itself, and just after itself.
        app.perform(&ctx, Action::MoveBookmark { from: 1, to: 1 });
        app.perform(&ctx, Action::MoveBookmark { from: 1, to: 2 });
        assert_eq!(names(&app), ["b", "a", "c", "d"]);
        // Out of range, from a stale drag: nothing moves and nothing panics.
        app.perform(&ctx, Action::MoveBookmark { from: 9, to: 0 });
        app.perform(&ctx, Action::MoveBookmark { from: 0, to: 9 });
        assert_eq!(names(&app), ["b", "a", "c", "d"]);
    }

    #[test]
    fn dragging_a_name_picks_the_file_up_and_dragging_beside_it_bands() {
        let mut h = Harness::new();
        assert!(h.tab(0).order.len() > 4, "the crate root has rows to drag");

        // ---- From the name: the file is picked up ------------------------
        let row = h.row_center(0, 0);
        let pane = h.pane_rect(0);
        // Just past the icon, which is where the first row's name starts.
        let on_name = pos2(pane.left() + 46.0, row.y);
        let done = h.drag(on_name, pos2(on_name.x + 60.0, on_name.y + 90.0));
        assert!(
            done.contains(&"DragOut"),
            "dragging a name has to pick the file up, got {done:?}"
        );
        // The OLE drag itself is not started here and cannot be: it needs a window, to find
        // the pointer gesture it is following. What is being tested is that the gesture is
        // read as a drag of the file, which is `DragOut` being dispatched at all.
        assert!(h.app.file_drag.is_none(), "and no drag left in flight");

        // ---- From the blank space on the same row: a band ----------------
        //
        // The gap between the end of the name and the Size column, which is the widest
        // blank stretch of any row and the one a user reaches for. *Not* the far right: the
        // last column is fitted to its content, so a date long enough fills it right up to
        // the edge — and a press there is genuinely on the row's ink, as this test used to
        // discover the hard way when the glyph advances moved by a pixel.
        h.wait();
        let blank = pos2(pane.left() + pane.width() * 0.45, row.y);
        let done = h.drag(blank, pos2(blank.x - 40.0, blank.y + crate::pane::ROW_HEIGHT * 3.5));
        assert!(
            !done.contains(&"DragOut"),
            "a drag from the empty part of a row is a band, not a drag of the file: {done:?}"
        );
        assert!(
            h.tab(0).selected_count >= 3,
            "the band should have swept the rows it crossed, got {}",
            h.tab(0).selected_count
        );
    }

    #[test]
    fn the_bare_title_bar_moves_and_maximises_the_window() {
        let mut h = Harness::new();
        // Between the application mark and the first tab there is bar and nothing else,
        // which is where a window is grabbed.
        let bar_y = crate::ui::chrome::HEIGHT * 0.5;
        let bare = pos2(
            crate::ui::chrome::content_left(crate::ui::chrome::bar_rect(Rect::from_min_size(
                Pos2::ZERO,
                h.size,
            ))) + 60.0,
            bar_y,
        );

        let done = h.drag(bare, pos2(bare.x + 80.0, bare.y + 40.0));
        assert!(
            done.contains(&"Window"),
            "dragging the bar has to move the window, got {done:?}"
        );

        let done = h.double_click_at(bare);
        assert!(
            done.contains(&"Window"),
            "double-clicking the bar has to maximise, got {done:?}"
        );
    }

    #[test]
    fn a_pane_stacked_below_another_gets_a_reachable_strip_of_its_own() {
        let mut h = Harness::new();
        h.app.actions.push(Action::SplitFocused {
            side: crate::pane::Side::Bottom,
        });
        h.settle();
        assert_eq!(h.app.panes.len(), 2);

        // The lower pane is the one whose top is furthest down.
        let lower = h
            .app
            .panes
            .iter()
            .max_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
            .map(|p| p.id)
            .expect("two panes");
        let slot = h
            .app
            .tab_slots
            .iter()
            .find(|s| s.pane == lower)
            .map(|s| s.rect)
            .expect("the lower pane has no tab on screen at all");

        assert!(
            slot.top() > crate::ui::chrome::HEIGHT,
            "a pane in the row below cannot have its tabs in the title bar, but {slot:?} is"
        );
        let pane_top = h
            .app
            .panes
            .iter()
            .find(|p| p.id == lower)
            .map(|p| p.rect.top())
            .unwrap();
        assert!(
            slot.bottom() <= pane_top,
            "the strip has to be above the pane it governs, not inside it"
        );

        // And it is a tab, not a picture of one.
        let index = h.app.tab_slots.iter().position(|s| s.pane == lower).unwrap();
        let id = Id::new(("tab", lower, h.app.tab_slots[index].tab));
        assert!(
            h.hovers(id, slot.center()),
            "the tab on the band is not reachable"
        );
    }

    #[test]
    fn split_or_not_every_tab_is_in_the_title_bar() {
        let mut h = Harness::new();
        h.app.panes[0]
            .tabs
            .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
        h.settle();
        assert_eq!(h.app.tab_slots.len(), 2, "one pane, two tabs");

        h.app.actions.push(Action::SplitFocused {
            side: crate::pane::Side::Right,
        });
        h.settle();
        assert_eq!(h.app.panes.len(), 2, "the split happened");

        // Every tab in the window, whichever pane governs it, is in the title bar — and
        // each pane has its own group there, so the tabs are grouped by owner rather than
        // merged into one strip.
        let bar = crate::ui::chrome::HEIGHT;
        for slot in &h.app.tab_slots {
            assert!(
                slot.rect.top() >= 0.0 && slot.rect.bottom() <= bar,
                "a tab for pane {} is at {:?}, outside the title bar",
                slot.pane,
                slot.rect
            );
        }
        for pane in &h.app.panes {
            let group: Vec<&crate::ui::chrome::Slot> = h
                .app
                .tab_slots
                .iter()
                .filter(|s| s.pane == pane.id)
                .collect();
            assert_eq!(
                group.len(),
                pane.tabs.len(),
                "pane {} has {} tabs and {} of them are on screen",
                pane.id,
                pane.tabs.len(),
                group.len()
            );
        }
        // Groups do not interleave: every slot of the left pane is left of every slot of
        // the right one, which is what makes the divider between them mean anything.
        let left = h.app.pane_order[0];
        let split = h
            .app
            .tab_slots
            .iter()
            .filter(|s| s.pane == left)
            .map(|s| s.rect.right())
            .fold(f32::MIN, f32::max);
        assert!(
            h.app
                .tab_slots
                .iter()
                .filter(|s| s.pane != left)
                .all(|s| s.rect.left() >= split),
            "the two panes' tabs are mixed together in the bar"
        );
    }

    // ---- Does browsing let go of what it read? --------------------------
    //
    // "The more I browse, the more memory" has two causes that look identical from Task
    // Manager: a cache filling up to its budget and then holding steady, and something that
    // never lets go. These tell them apart by measuring, across a few hundred real folders.
    //
    // Run on their own, because two of the three numbers are process-wide and every other
    // test allocates too:
    //
    //     cargo test --release browsing_hundreds -- --ignored --nocapture --test-threads=1

    /// Live heap bytes: every Rust allocation, minus every free. See [`crate::counting`].
    fn live_heap() -> isize {
        crate::counting::LIVE.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Directories under `from`, breadth-first, up to `want` of them.
    ///
    /// Real folders with real contents: a synthetic tree of empty directories would exercise
    /// none of the per-entry storage this is about.
    fn folders(from: &Path, want: usize) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut queue = std::collections::VecDeque::from([from.to_path_buf()]);
        while let Some(dir) = queue.pop_front() {
            if found.len() >= want {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    queue.push_back(entry.path());
                    found.push(entry.path());
                }
            }
        }
        found
    }

    #[test]
    #[ignore = "measures the whole process; run explicitly, single-threaded"]
    fn scrolling_the_same_folder_up_and_down_costs_nothing() {
        // The reported case, exactly: one folder, scrolled up and down, the same rows drawn
        // over and over. Nothing about that is new work — every name, every icon and every
        // formatted size has been seen before — so anything that grows here grows *per frame*
        // rather than per folder, which is why leaving the folder would not give it back.
        let dir = PathBuf::from(r"C:\Windows");
        if !dir.is_dir() {
            println!("no C:\\Windows; skipping");
            return;
        }
        let mut h = Harness::new();
        h.app.panes[0].tab_mut().navigate(dir);
        h.settle();
        let rows = h.tab(0).order.len();
        assert!(rows > 40, "need a folder taller than the window");

        // One full pass first, so every icon, galley and column width is already resolved:
        // whatever the first sweep costs is work, not growth.
        let sweep = |h: &mut Harness| {
            for top in (0..rows).step_by(11) {
                h.app.panes[0].tab_mut().scroll_to = Some(top as f32 * crate::pane::ROW_HEIGHT);
                h.frame(Vec::new());
                h.app.icons.poll(&h.ctx.clone());
                h.app.deliver_icons();
            }
            for top in (0..rows).step_by(11).rev() {
                h.app.panes[0].tab_mut().scroll_to = Some(top as f32 * crate::pane::ROW_HEIGHT);
                h.frame(Vec::new());
            }
        };
        sweep(&mut h);
        h.settle();

        let heap0 = live_heap();
        let (private0, gdi0, _) = process_memory();
        println!("after one pass: heap {} KB, private {} KB, gdi {gdi0}", heap0 / 1024, private0 / 1024);

        for pass in 1..=10 {
            sweep(&mut h);
            let (private, gdi, _) = process_memory();
            println!(
                "pass {pass:>2}:  heap {:+7} KB   private {:+7} KB   gdi {gdi:>4}",
                (live_heap() - heap0) / 1024,
                (private as isize - private0 as isize) / 1024
            );
        }

        // Ten more passes over rows that were already drawn ten times must cost nothing.
        // A megabyte of slack for egui's own per-frame reuse.
        assert!(
            live_heap() - heap0 < (1 << 20),
            "scrolling the same folder grew the heap by {:+} KB -- something allocates per \
             frame and keeps it",
            (live_heap() - heap0) / 1024
        );
    }

    #[test]
    #[ignore = "measures the whole process; run explicitly, single-threaded"]
    fn what_a_very_large_folder_costs() {
        // A tab holds its listing, its display order and a selection flag per entry, and the
        // cache holds the listing again until it is evicted. This is what one folder of
        // 27,000 comes to, and it is the number to multiply by if a session keeps several
        // such folders open in tabs.
        let dir = PathBuf::from(r"C:\Windows\WinSxS");
        if !dir.is_dir() {
            println!("no WinSxS; skipping");
            return;
        }
        let mut h = Harness::new();
        h.settle();
        let (private0, _, _) = process_memory();
        let heap0 = live_heap();

        h.app.panes[0].tab_mut().navigate(dir);
        h.settle();
        let rows = h.tab(0).order.len();
        let (private, _, _) = process_memory();
        println!(
            "{rows} entries:  heap {:+} KB   private {:+} KB   = {} bytes an entry (heap)",
            (live_heap() - heap0) / 1024,
            (private as isize - private0 as isize) / 1024,
            (live_heap() - heap0) / rows.max(1) as isize
        );

        // Leave it, and see what comes back once neither the tab nor the cache holds it.
        h.app.panes[0].tab_mut().navigate(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        h.settle();
        h.app.loader.invalidate(Path::new(r"C:\Windows\WinSxS"));
        h.settle();
        println!(
            "after leaving and dropping it from the cache: heap {:+} KB",
            (live_heap() - heap0) / 1024
        );
    }

    #[test]
    #[ignore = "measures the whole process; run explicitly, single-threaded"]
    fn what_scrolling_a_folder_of_executables_costs() {
        // The worst case for icons, and the one a synthetic walk of small folders never
        // reaches: a folder of thousands of files that each carry their own icon. Every one
        // that comes on screen is a separate question to the shell, which opens the file and
        // reads its resources — so this is where "memory grows as I browse" would come from.
        let dir = PathBuf::from(r"C:\Windows\System32");
        if !dir.is_dir() {
            println!("no System32; skipping");
            return;
        }

        let mut h = Harness::new();
        h.app.panes[0].tab_mut().navigate(dir);
        h.settle();
        let rows = h.tab(0).order.len();
        println!("{rows} rows");

        let (private0, gdi0, _) = process_memory();
        let heap0 = live_heap();
        // Scroll the whole listing past, a screenful at a time, so every row is drawn once.
        let step = 20;
        for top in (0..rows).step_by(step) {
            h.app.panes[0].tab_mut().scroll_to = Some(top as f32 * crate::pane::ROW_HEIGHT);
            for _ in 0..3 {
                h.frame(Vec::new());
            }
            h.app.icons.poll(&h.ctx.clone());
            h.app.deliver_icons();
            if top % (step * 40) == 0 {
                let (private, gdi, _) = process_memory();
                let (kinds, paths, textures) = h.app.icons.held();
                println!(
                    "row {top:>5}:  private {:+8} KB   heap {:+7} KB   gdi {gdi:>5}   \
                     icons {kinds:>4}/{paths:>5}/{textures:>4}",
                    (private as isize - private0 as isize) / 1024,
                    (live_heap() - heap0) / 1024
                );
                let _ = gdi0;
            }
        }
        let (private, gdi, _) = process_memory();
        let (kinds, paths, textures) = h.app.icons.held();
        println!(
            "after the whole listing: private {:+} KB   gdi {gdi}   icons {kinds}/{paths}/{textures}",
            (private as isize - private0 as isize) / 1024
        );
    }

    /// The whole chain, through the app: a right click opens a menu straight away, the
    /// builder thread fills in the shell's entries, and hovering a submenu fetches that.
    ///
    /// The pieces are tested where they live -- `shell::menu` for the shell, `ui::menu` for
    /// the drawing -- and what is only tested here is the wiring between them: `pump_menu`
    /// taking delivery against the right token, and draining what the menu asked for on the
    /// way back out. That wiring is easy to get subtly wrong and impossible to notice,
    /// because a menu that never fills its submenus looks exactly like a menu whose
    /// submenus are empty.
    #[test]
    #[cfg(windows)]
    fn a_right_click_opens_a_menu_now_and_fills_it_from_the_shell() {
        let _serialised = crate::shell::serialised();
        let mut h = Harness::new();

        // What a baseline frame of this window costs, to compare the asking one against.
        let mut baseline = std::time::Duration::ZERO;
        for _ in 0..5 {
            let at = std::time::Instant::now();
            h.frame(Vec::new());
            baseline = baseline.max(at.elapsed());
        }

        h.app.open_folder_menu(&h.ctx.clone());
        let at = std::time::Instant::now();
        h.frame(Vec::new());
        let asking = at.elapsed();
        // The cheapest `QueryContextMenu` measured on this machine was 130 ms, for a folder;
        // a file was 130-690. So a frame that raised the menu and came in well under a tenth
        // of a second did not ask the shell anything on the way, which is the whole point.
        // Compared against a frame of the same window rather than against a constant, because
        // the constant that matters is the shell's and this bound only has to be under it.
        assert!(
            asking < baseline + std::time::Duration::from_millis(100),
            "the frame that raised the menu took {asking:?} against a {baseline:?} baseline \
             -- something on it went to the shell"
        );
        // And nothing is on screen yet: a menu that appeared here would be a menu that grew
        // afterwards, which is what this deliberately does not do.
        assert!(h.app.menu.is_none(), "the menu appeared before it was ready");
        assert!(h.app.menu_pending(), "and it is not on its way either");

        // It arrives whole, a moment later.
        let waited = std::time::Instant::now();
        while h.app.menu_pending() {
            h.frame(Vec::new());
            assert!(
                waited.elapsed() < std::time::Duration::from_secs(20),
                "the builder never delivered"
            );
        }
        let menu = h.app.menu.as_ref().expect("open by now");
        // Windows' menu, with exactly one entry of ours in front of it. This program used to put
        // half a dozen above the shell's — those are gone. What is left here is Paste, and this is
        // the *background* menu ([`App::open_folder_menu`] raises that one), which is the single
        // menu the shell hands over with a gap in it: see `crate::shell::menu::Own::Paste`. On a
        // file's or a folder's own menu there is still nothing of ours, which
        // `the_drag_answers_and_paste_are_the_only_entries_of_our_own` and
        // `the_background_menu_gets_this_program_s_paste` hold between them.
        assert!(
            menu.entries.len() > 3,
            "the shell should have filled the menu: {:?}",
            menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
        );
        let ours: Vec<&str> = menu
            .entries
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    crate::shell::menu::Kind::Command(crate::shell::menu::Command::Own(_))
                )
            })
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(
            ours,
            ["Paste"],
            "the background menu should be Windows' own with this program's Paste above it, and \
             nothing else of ours"
        );

        // Every shell submenu is there and empty, which is the point of the lazy fill.
        let submenus: Vec<usize> = menu
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind.unasked().is_some())
            .map(|(i, _)| i)
            .collect();
        assert!(
            !submenus.is_empty(),
            "a folder's menu has 7-Zip, Send To or Give access to in it: {:?}",
            menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
        );

        // Opening each one asks for it, and what comes back has to be what the shell has in
        // it. Every one of them, and counting what arrived rather than merely that an answer
        // did: an answer of "nothing" also marks a submenu filled, so a version of this that
        // waited for the flag passed while every submenu in the program was empty.
        let mut counts: Vec<(String, usize)> = Vec::new();
        for index in submenus {
            h.app.menu.as_mut().unwrap().open = vec![index];
            let waited = std::time::Instant::now();
            loop {
                h.frame(Vec::new());
                let done = h
                    .app
                    .menu
                    .as_ref()
                    .is_some_and(|m| m.entries[index].kind.unasked().is_none());
                if done {
                    break;
                }
                assert!(
                    waited.elapsed() < std::time::Duration::from_secs(20),
                    "the submenu was never filled -- nothing carried the ask to the builder"
                );
            }
            let menu = h.app.menu.as_ref().unwrap();
            let count = match &menu.entries[index].kind {
                crate::shell::menu::Kind::Submenu { children, .. } => children.len(),
                _ => 0,
            };
            counts.push((menu.entries[index].label.clone(), count));
        }
        assert!(
            counts.iter().any(|(_, count)| *count > 0),
            "every submenu in the menu came back empty: {counts:?}"
        );

        h.app.close_menu();
        h.frame(Vec::new());
    }

    /// A menu the shell has not finished making can be given up on, and says so while it lasts.
    ///
    /// The window used to have nothing to say about a menu that was on its way, which is fine
    /// for the tenth of a second a folder takes. On an executable on a mapped share it is
    /// twenty-four seconds — measured, see [`crate::shell::menu::Builder`] — and twenty-four
    /// seconds of a window that shows nothing, reacts to nothing you can see, and then puts up a
    /// menu at a place you have long since stopped pointing at is not a wait, it is a fault.
    ///
    /// So there is a cursor for it, and Escape means never mind. What Escape cannot do is stop
    /// `QueryContextMenu`, so what is tested at the end is the part that matters: that the answer
    /// nobody wants any more does not arrive later and open a menu on its own.
    #[test]
    #[cfg(windows)]
    fn escape_gives_up_on_a_menu_the_shell_is_still_making() {
        use std::sync::atomic::Ordering;

        let _serialised = crate::shell::serialised();
        let mut h = Harness::new();
        h.settle();

        // Stand in for the share. Long enough to press Escape inside, short enough that the
        // abandoned worker is finished before the assertion that it changed nothing.
        const STALL: u64 = 1_500;
        crate::shell::menu::STALLED.store(0, Ordering::SeqCst);
        crate::shell::menu::STALL_MS.store(STALL, Ordering::SeqCst);

        h.app.open_folder_menu(&h.ctx.clone());
        h.frame(Vec::new());
        // Cleared once the worker has picked it up, and not before: clearing it straight after
        // the frame races the thread that is about to read it.
        let waited = std::time::Instant::now();
        while crate::shell::menu::STALLED.load(Ordering::SeqCst) == 0 {
            h.frame(Vec::new());
            assert!(
                waited.elapsed() < std::time::Duration::from_secs(5),
                "the worker never started the stalled build"
            );
        }
        crate::shell::menu::STALL_MS.store(0, Ordering::SeqCst);
        assert!(h.app.menu_pending(), "the ask never went out");
        assert!(h.app.menu.is_none());

        // A second frame, and the cursor says the window is working on something.
        h.frame(Vec::new());
        assert!(h.app.menu_pending(), "the stalled build answered immediately");
        assert_eq!(
            h.cursor,
            egui::CursorIcon::Progress,
            "nothing on screen says a menu is being waited for"
        );

        h.frame(vec![Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        assert!(
            !h.app.menu_pending(),
            "Escape did not give up on the menu that was being built"
        );

        // And the answer, when it finally comes, is nobody's: no menu appears out of the blue a
        // second and a half after the click that asked for it was called off.
        let waited = std::time::Instant::now();
        while waited.elapsed() < std::time::Duration::from_millis(STALL + 800) {
            h.frame(Vec::new());
            assert!(
                h.app.menu.is_none() && !h.app.menu_pending(),
                "the abandoned menu turned up {:?} after Escape",
                waited.elapsed()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            crate::shell::menu::STALLED.load(Ordering::SeqCst),
            1,
            "the build under test never actually stalled, so nothing here was given up on"
        );
    }

    /// `Reset window size` asks the platform for the default size, and un-maximises on the way.
    ///
    /// The order is the part worth pinning. A window that has been maximised is the one most
    /// likely to be sitting there when somebody reaches for this, and asking for a size while
    /// still maximised is asking for something a window manager is entitled to ignore. Windows
    /// happens not to — measured: from a maximised 2560×1392 the window comes back to 1024×600
    /// with the un-maximise taken out, because `SetWindowPos` restores on the way — but that is
    /// winit's platform behaviour rather than a promise, so the state is asked for explicitly.
    #[test]
    fn reset_window_size_asks_for_the_default_and_unmaximises_first() {
        use egui::ViewportCommand as Cmd;

        let mut h = Harness::new();
        h.settle();
        let [w, hh] = crate::config::WINDOW_SIZE;

        // From maximised: both commands, un-maximise first.
        h.app.maximized = true;
        h.commands.clear();
        h.app
            .perform(&h.ctx.clone(), Action::Window(WindowAction::ResetSize));
        // Checked before the next frame, not after. The harness feeds a viewport of a fixed size
        // for ever, so the frame that follows re-samples that and writes it back over this —
        // which is the right behaviour against a real window, where the frame that follows a
        // resize is the one that sees the new shape.
        assert!(!h.app.maximized, "the flag still says maximised");
        assert_eq!(h.app.window_size, Some([w, hh]));

        h.frame(Vec::new());
        let asked: Vec<&Cmd> = h
            .commands
            .iter()
            .filter(|c| matches!(c, Cmd::Maximized(_) | Cmd::InnerSize(_)))
            .collect();
        assert_eq!(
            asked,
            vec![&Cmd::Maximized(false), &Cmd::InnerSize(egui::vec2(w, hh))],
            "from maximised, the window was asked for {asked:?}"
        );

        // From a restored window there is nothing to un-maximise, so only the size is asked for.
        h.app.maximized = false;
        h.app.window_size = Some([1380.0, 840.0]);
        h.commands.clear();
        h.app
            .perform(&h.ctx.clone(), Action::Window(WindowAction::ResetSize));
        h.frame(Vec::new());
        let asked: Vec<&Cmd> = h
            .commands
            .iter()
            .filter(|c| matches!(c, Cmd::Maximized(_) | Cmd::InnerSize(_)))
            .collect();
        assert_eq!(
            asked,
            vec![&Cmd::InnerSize(egui::vec2(w, hh))],
            "a restored window should only be asked for a size: {asked:?}"
        );

        // And the default is one value, not two: the size the window opens at on a first run is
        // the size this puts it back to.
        assert_eq!(Config::default().window.unwrap_or(crate::config::WINDOW_SIZE), [w, hh]);
    }

    /// **Two panes side by side can be resized, scrollbar or no scrollbar.**
    ///
    /// [`dock::GRAB`] reaches four points into the pane on each side of the divider, because a
    /// one-point grab target is not one — and in a horizontal split those four points are exactly
    /// where the listing's vertical scrollbar is. Within a layer egui gives a click to the *last*
    /// widget registered, so while the splitters went up before the panes the scrollbar took the
    /// pointer and **the divider between two side-by-side panes did nothing at all**. A stacked pair
    /// never showed it: there the grab reaches the pane's bottom edge, where a vertical scroll area
    /// has nothing to claim.
    ///
    /// **What this does and does not show.** It drives a real double-click at the divider over a
    /// listing long enough to have a scrollbar, and the splitter answers by evening the split up —
    /// so the divider is reachable and its `Sense` is right. It is *not* a guard on the ordering:
    /// moving the block back above the panes was tried, and this still passed. Whatever competes for
    /// those four points in the window does not exist in the harness, so the ordering itself is only
    /// checked by using the window. Worth knowing before trusting this test to catch a regression in
    /// it.
    #[test]
    fn side_by_side_panes_can_be_resized_over_the_scrollbar() {
        // Long enough to overflow the pane and put a scrollbar down its right edge.
        let deep = crate::sandbox::fresh("splitter-over-scrollbar");
        for i in 0..200 {
            std::fs::write(deep.join(format!("file-{i:03}.txt")), b"x").expect("a fixture file");
        }

        let mut h = Harness::with_panes(2);
        h.settle();
        let route = h
            .app
            .splitters
            .iter()
            .find(|s| s.horizontal)
            .map(|s| s.route.clone())
            .expect("two panes opened side by side have a horizontal splitter");

        // The pane on the left of it is the one whose scrollbar is in the way.
        let left = h
            .app
            .pane_rects
            .iter()
            .min_by(|a, b| a.1.left().total_cmp(&b.1.left()))
            .map(|(id, _)| *id)
            .expect("a pane");
        let index = h.app.panes.iter().position(|p| p.id == left).expect("its tab");
        let ctx = h.ctx.clone();
        h.app.perform(
            &ctx,
            Action::Navigate {
                pane: left,
                path: deep.clone(),
            },
        );
        h.settle();
        assert!(
            h.tab(index).order.len() > 100,
            "the left pane holds {} rows, which may not overflow it -- then there is no scrollbar \
             here and this test is not testing anything",
            h.tab(index).order.len()
        );

        // Somewhere other than even, so evening it up is a visible change.
        if let Some(ratio) = h.app.layout.ratio_at(&route) {
            *ratio = 0.3;
        }
        h.settle();
        let moved = h
            .app
            .splitters
            .iter()
            .find(|s| s.route == route)
            .map(|s| s.rect)
            .expect("the splitter after the ratio moved");

        let at = moved.center();
        assert!(
            h.hovers(egui::Id::new(("splitter", &route)), at),
            "the divider at {at:?} is not the widget under the pointer -- something registered \
             later is on top of it, which is how the scrollbar used to win"
        );
        h.double_click_at(at);
        assert_eq!(
            h.app.layout.ratio_at(&route).copied(),
            Some(0.5),
            "double-clicking the divider did not even the split up, so the gesture never reached it"
        );
    }

    /// Double-clicking the sidebar splitter puts it back where it started.
    ///
    /// The gesture the column edges in the listing already answer to, and the way out of a
    /// sidebar dragged somewhere silly. Driven through the real splitter rather than by calling
    /// something, because what is easy to get wrong here is the grip's rect and its `Sense`: a
    /// `drag`-only splitter reports no clicks at all, and the reset would be dead code.
    #[test]
    fn double_clicking_the_sidebar_splitter_restores_its_width() {
        let mut h = Harness::new();
        h.settle();

        let default = crate::config::SIDEBAR_WIDTH;
        assert_eq!(
            h.app.sidebar_width, default,
            "the harness did not start at the default width"
        );

        // Somewhere silly, the way a drag would leave it.
        h.app.sidebar_width = 380.0;
        h.frame(Vec::new());
        let dragged = h.app.sidebar_width;
        assert_eq!(dragged, 380.0);

        // Found by asking the splitter itself, rather than by re-deriving where the layout put
        // it: a test that computes the grip's x is a test of this test's arithmetic.
        let grip = egui::Id::new("sidebar-grip");
        let at = (0..40)
            .map(|step| pos2(dragged - 8.0 + step as f32 * 0.5, 300.0))
            .find(|&at| h.hovers(grip, at))
            .expect("the sidebar splitter is not reachable by the pointer at all");
        h.double_click_at(at);
        assert_eq!(
            h.app.sidebar_width, default,
            "double-clicking the splitter at {at:?} left the sidebar at {}",
            h.app.sidebar_width
        );
        // Not `config_dirty`: the frame after the one that sets it writes the settings and
        // clears it again, so by the time this can look it is already false — which is the flag
        // doing its job rather than a fault. That it *was* set is covered by the settings file
        // holding `sidebar_width=200` after a real window is dragged wider and double-clicked
        // back, which is a thing to check with a real window and not from here.
    }

    /// The panels are square, flush, and separated by one line of the selected tab's colour.
    ///
    /// Asserted against the shapes the frame actually painted, not against the source. Three
    /// separate things had to agree for the old look — a corner radius, a `stroke-subtle` ring
    /// and a four-point gap — and each of them was written down somewhere else, which is how a
    /// window ends up with panels that read as loose cards without anyone having decided that.
    #[test]
    fn the_panels_are_square_and_a_single_line_apart() {
        let mut h = Harness::with_panes(2);
        h.settle();

        let seam = crate::ui::seam(&h.app.theme);
        let layer = h.app.theme.bg.layer;

        // ---- The gaps, from the layout ------------------------------------
        let body = Rect::from_min_max(
            pos2(0.0, crate::ui::chrome::bar_rect(Rect::from_min_size(Pos2::ZERO, h.size)).bottom()),
            Pos2::ZERO + h.size,
        );
        let (sidebar, panes_area) = App::split_body(body, h.app.sidebar_width);
        assert_eq!(
            panes_area.left() - sidebar.right(),
            crate::ui::SEAM,
            "the sidebar and the panes are {} apart",
            panes_area.left() - sidebar.right()
        );

        let mut rects: Vec<Rect> = h.app.pane_rects.iter().map(|(_, r)| *r).collect();
        rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
        assert_eq!(rects.len(), 2, "this test wants two panes side by side");
        assert_eq!(
            rects[1].left() - rects[0].right(),
            crate::ui::SEAM,
            "two panes are {} apart",
            rects[1].left() - rects[0].right()
        );

        // ---- The fills, from the frame ------------------------------------
        //
        // The seam is not a shape of its own: it is what the fill behind the whole block leaves
        // showing, so what is checked is that the block is there, in that colour, under panels
        // that are square and do not cover it.
        let (corner, fill) = h
            .fill_at(sidebar)
            .expect("nothing was painted at the sidebar's rect");
        assert_eq!(fill, layer, "the sidebar is not `background-layer`");
        assert_eq!(corner, egui::CornerRadius::ZERO, "the sidebar has rounded corners");

        for rect in &rects {
            let (corner, fill) = h
                .fill_at(*rect)
                .unwrap_or_else(|| panic!("nothing was painted at the pane's rect {rect:?}"));
            assert_eq!(fill, layer, "a pane is not `background-layer`");
            assert_eq!(corner, egui::CornerRadius::ZERO, "a pane has rounded corners");
        }

        let block = sidebar.union(panes_area);
        let (corner, fill) = h
            .fill_at(block)
            .expect("nothing is painted behind the panels for the seams to show");
        assert_eq!(fill, seam, "the seams are not the selected tab's colour");
        assert_eq!(corner, egui::CornerRadius::ZERO);

        // ---- And nothing but the window's border outside them --------------
        //
        // The block fills the body: no canvas ring, and the panels reach three of the window's
        // four edges. Written as edges rather than as "no gutter" because that is the thing that
        // can be seen — a stripe of `background-canvas` down the side of the window.
        assert_eq!(block, body, "there is canvas showing around the panels");
        assert_eq!(sidebar.left(), 0.0, "the sidebar stops short of the window");
        assert_eq!(
            rects[1].right(),
            h.size.x,
            "the last pane stops short of the window's right edge"
        );
        for rect in &rects {
            assert_eq!(
                rect.bottom(),
                h.size.y,
                "a pane stops short of the window's bottom edge"
            );
        }
    }

    /// Opening a rename leaves the name exactly where it was.
    ///
    /// It used to jump two points right and one point down — a field brings its own padding and
    /// its own idea of where a line sits — and on the one word you are looking at that reads as a
    /// flinch. Measured here the same way it was found: the position of the *text* in the frame
    /// before and the frame after, which is the thing the eye is complaining about. Asserting on
    /// the field's rect instead would only be checking this test's own arithmetic.
    #[test]
    fn opening_a_rename_does_not_move_the_name() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;

        let name = {
            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
            tab.select_only(0);
            let dir = tab.dir.clone().expect("a listing");
            let entry = tab.entry_at(0).expect("a first row");
            dir.name(entry).to_owned()
        };
        h.frame(Vec::new());

        let find = |h: &Harness, what: &str| {
            h.texts()
                .into_iter()
                .find(|(_, text)| text == &name)
                .map(|(at, _)| at)
                .unwrap_or_else(|| panic!("`{name}` was not drawn {what}"))
        };
        let before = find(&h, "in the listing");

        h.app.perform(&h.ctx.clone(), Action::BeginRename(pane));
        h.frame(Vec::new());
        assert!(
            h.tab(0).renaming.is_some(),
            "the rename did not open, so this test proves nothing"
        );
        let after = find(&h, "in the rename field");

        assert_eq!(
            after, before,
            "the name moved by {:?} when the rename opened",
            after - before
        );
    }

    /// Arriving in a text field selects what is in it, so the next keystroke replaces it.
    ///
    /// Asserted by *typing* rather than by reading egui's cursor state, for two reasons. The
    /// selection lives in `TextEdit`'s state under an id the design system generates internally,
    /// so there is nothing to read from out here without the library handing it over. And "the
    /// text is selected" is not the point — "one keystroke replaces the path" is, and that is a
    /// claim about what happens when you type, which is a thing a test can do.
    #[test]
    fn arriving_in_a_field_selects_what_is_there() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;

        // ---- The path field ------------------------------------------------
        {
            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
            tab.editing_path = true;
            tab.edit_text = r"C:\Windows\System32".to_owned();
        }
        // It asks for focus itself on the frame it opens; two frames for egui to grant it and
        // for the field to see it arrive.
        h.frame(Vec::new());
        h.frame(Vec::new());
        h.frame(vec![Event::Text("D".to_owned())]);
        assert_eq!(
            h.tab(0).edit_text,
            "D",
            "typing into a freshly opened path field appended instead of replacing"
        );
        h.app.pane_mut(pane).expect("the pane").tab_mut().editing_path = false;
        h.frame(Vec::new());

        // ---- The filter, reached by clicking it ----------------------------
        {
            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
            tab.filter = "old".to_owned();
        }
        h.settle();
        // Found by sweeping the bar rather than by deriving the box's rect: a text field is what
        // asks for `CursorIcon::Text`, and coming in from the right edge the filter is the first
        // thing that does. (The empty part of the breadcrumb asks for it too, and is further
        // left.)
        //
        // The whole run of it, and then the middle — not the first point that answered. The
        // clearable ✕ sits at the right-hand end of the box and is registered *after* the text
        // area, so it wins the pointer there; clicking the edge of the run emptied the filter
        // instead of typing into it, which is a real click target doing its real job.
        let y = h.path_bar_y(0);
        let right = h.pane_rect(0).right();
        let mut run: Vec<f32> = Vec::new();
        for dx in (8..400).step_by(2) {
            let at = pos2(right - dx as f32, y);
            h.frame(vec![Event::PointerMoved(at)]);
            if h.cursor == egui::CursorIcon::Text {
                run.push(at.x);
            } else if !run.is_empty() {
                break;
            }
        }
        assert!(!run.is_empty(), "the filter box is not reachable by the pointer");
        let at = pos2((run[0] + run[run.len() - 1]) * 0.5, y);
        h.click_at(at);
        h.frame(vec![Event::Text("n".to_owned())]);
        assert_eq!(
            h.tab(0).filter,
            "n",
            "clicking into the filter and typing appended instead of replacing"
        );
    }

    /// In the grid, `Down` goes to the tile below and `Right` to the one beside it.
    ///
    /// **Which is a different number of rows for each key**, and getting it wrong is a listing where
    /// the arrow keys walk the folder in an order that has nothing to do with what is on screen. So
    /// the step is measured against the column count the view actually laid out — `Layout::columns` —
    /// rather than against a number restated here, and the pane is deliberately left at the harness's
    /// own width so that more than one column fits.
    ///
    /// `Home` and `End` are checked as well, because they are the two that must *not* change: the
    /// first tile and the last are still the first and last rows of the order however it is arranged.
    #[test]
    fn in_the_grid_the_arrows_move_by_a_line_of_tiles() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let ctx = h.ctx.clone();
        // `src`, which has enough files to fill more than one line of tiles.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        h.app.perform(&ctx, Action::Navigate { pane, path });
        h.settle();
        h.app.perform(&ctx, Action::SetView { pane, mode: h.tab(0).view_mode.toggled() });
        h.settle();

        let columns = h.tab(0).grid.columns;
        assert!(
            columns > 1,
            "the harness's pane fits only {columns} column, so this test cannot tell a line from a row"
        );
        assert!(h.tab(0).order.len() > columns * 2, "and not enough rows to move through");

        h.frame(tap(egui::Key::Home));
        assert_eq!(h.tab(0).cursor, Some(0), "Home is the first tile");
        h.frame(tap(egui::Key::ArrowRight));
        assert_eq!(h.tab(0).cursor, Some(1), "Right is the next tile along");
        h.frame(tap(egui::Key::ArrowDown));
        assert_eq!(
            h.tab(0).cursor,
            Some(1 + columns),
            "Down should be a whole line of tiles"
        );
        h.frame(tap(egui::Key::ArrowUp));
        assert_eq!(h.tab(0).cursor, Some(1));
        h.frame(tap(egui::Key::ArrowLeft));
        assert_eq!(h.tab(0).cursor, Some(0));
        // And off the left edge of the first line is still the first tile rather than an underflow.
        h.frame(tap(egui::Key::ArrowLeft));
        assert_eq!(h.tab(0).cursor, Some(0));
        h.frame(tap(egui::Key::End));
        assert_eq!(h.tab(0).cursor, Some(h.tab(0).order.len() - 1), "End is the last");

        // Back in the details view the same two keys are one row and the tree's, which is the other
        // half of the rule.
        h.app.perform(&ctx, Action::SetView { pane, mode: h.tab(0).view_mode.toggled() });
        h.settle();
        h.frame(tap(egui::Key::Home));
        h.frame(tap(egui::Key::ArrowDown));
        assert_eq!(h.tab(0).cursor, Some(1), "a row is one step in the details view");
    }

    /// **A scrolled grid switched from one flatten mode to the other still gets its pictures.**
    ///
    /// The bug this is for, and it is a *paint-on-demand* bug rather than a caching one. Scroll a grid
    /// of tiles far enough and every cell of the thumbnail atlas is held by a file you have gone past.
    /// Switch the flatten mode and the tiles are all new, all their cells are held by those files, and
    /// the cells only become reusable once a frame has gone by without them being drawn. Nothing on
    /// screen is moving, so nothing asks for that frame — and the window sits on a grid of painted
    /// glyphs until you scroll and force some frames by hand. Which is exactly what it did.
    ///
    /// So the shape of the test is the shape of the report: fill the atlas, then switch, then run frames
    /// **without touching anything** and require the pictures to arrive. `Thumbs::poll` moving to the end
    /// of the frame is what makes that possible, and the repaint booked on a refused request is what
    /// makes it happen when the answer needs a frame that nobody else would ask for.
    ///
    /// **What is counted is textured quads**, which in this view is exactly "tiles with a picture": a
    /// tile that has one draws an image out of the atlas, and a tile that has not draws a painted glyph
    /// out of the font atlas. So the first show establishes the number, and the number has to come back.
    ///
    /// The folder is a few hundred `.txt` files in the sandbox rather than anything real. What matters
    /// is the *count* — enough tiles to fill the atlas twice over between the two views — and `.txt` is
    /// the cheapest thing the shell reliably draws: no thumbnail handler, so it answers from the icon
    /// every time and the test does not depend on what is in the files.
    ///
    /// **What this does and does not prove**, because that is worth being straight about. It holds the
    /// end-to-end invariant: switch a scrolled grid's mode, touch nothing, and the pictures come back.
    /// It does *not* discriminate against the version of the bug that was found, on this fixture and
    /// this window — 420 files over a 2560-wide pane never fills the atlas, so there is no starvation
    /// for the ordering to rescue, and the test passes against both. What discriminates is
    /// `shell::thumbs`' own `a_visible_cell_is_never_taken_from_the_tile_drawing_it`, which fails
    /// outright on the two-frame rule this replaced. This one is here for the *next* change: it is the
    /// only test that runs the whole path with frames drawn only when the window asks for them.
    ///
    /// `#[ignore]`d because it drives several hundred real shell calls and takes a few seconds, which is
    /// not what `cargo test` is for.
    #[test]
    #[ignore = "drives a few hundred real shell calls; run explicitly"]
    fn a_scrolled_grid_that_changes_mode_still_fills_in() {
        // Twenty folders of twenty files: enough rows that a scrolled tree and the top of a list have
        // nothing in common, which is the whole condition.
        let root = crate::sandbox::dir("tiles-refill");
        for folder in 0..20 {
            let sub = root.join(format!("f{folder:02}"));
            std::fs::create_dir_all(&sub).expect("the sandbox is writable");
            for file in 0..20 {
                let at = sub.join(format!("{folder:02}-{file:02}.txt"));
                if !at.exists() {
                    std::fs::write(&at, b"x").expect("the sandbox is writable");
                }
            }
        }

        let mut h = Harness::with_panes(1);
        // Big enough to want a lot of tiles at once, which is the condition the report has.
        h.size = vec2(2560.0, 1392.0);
        let pane = h.app.panes[0].id;
        let ctx = h.ctx.clone();

        h.app.perform(&ctx, Action::Navigate { pane, path: root });
        h.settle();
        h.app.perform(&ctx, Action::SetView { pane, mode: h.tab(0).view_mode.toggled() });
        h.app.perform(&ctx, Action::ToggleFlat(pane));
        h.settle();
        assert!(h.tab(0).order.len() > 400, "not enough rows to fill the atlas");

        // ---- The first show, which is the case that always worked ----------
        let first = h.pictures_once_settled();
        assert!(
            first > 50,
            "only {first} tiles drew a picture on the first show — this window is too small to \
             tell the bug from the arithmetic"
        );

        // ---- Fill the atlas with files the list will not be showing --------
        h.app.perform(&ctx, Action::SetFlatMode(crate::pane::FlatMode::Tree));
        h.settle();
        for step in 1..=8 {
            h.app.panes[0].tab_mut().scroll_to = Some(step as f32 * 1200.0);
            let _ = h.pictures_once_settled();
        }

        // ---- The switch back, and then **nothing but frames** --------------
        //
        // No pointer, no keys, no scrolling. This is the whole report: the window is left alone, and it
        // has to fill itself in.
        h.app.perform(&ctx, Action::SetFlatMode(crate::pane::FlatMode::List));
        let again = h.pictures_once_settled();
        assert_eq!(
            again, first,
            "{} of {first} tiles never got a picture back after the mode changed, without the view \
             being touched",
            first.saturating_sub(again)
        );
    }

    /// One key, with nothing held.
    fn tap(key: egui::Key) -> Vec<Event> {
        vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]
    }

    /// Open the path field on `pane` with `text` in it, and run frames until the completion has
    /// worked out what to offer.
    ///
    /// The waiting is the point: the folder being completed in comes from [`crate::loader`], on a
    /// worker, so the offers are not there on the frame the text lands. A fixed frame count is how
    /// this suite used to be flaky about scans — see `Harness::settle`.
    fn open_path_field(h: &mut Harness, pane: PaneId, text: &str) {
        {
            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
            tab.editing_path = true;
            tab.edit_text = text.to_owned();
        }
        for attempt in 0..200 {
            h.frame(Vec::new());
            if attempt >= 2 && !h.app.complete.offering().0.is_empty() {
                return;
            }
            if attempt >= 2 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        panic!("the path field never offered anything for `{text}`");
    }

    /// **The completion gesture, driven by the keys that make it.**
    ///
    /// Every claim in [`crate::ui::breadcrumb::PathComplete`]'s documentation that a test can
    /// reach: the offers are matched without regard to case, nothing is highlighted and no
    /// dropdown is up until an arrow is pressed, `Down` does both at once, `Right` appends the
    /// name *and the separator*, and what is offered after that is what is inside the folder just
    /// named — which is the whole `Down Right Down Right` walk.
    ///
    /// Driven through `Harness::frame` rather than by calling the methods, because the interesting
    /// half is not the state machine: it is whether the keys ever reach it. `Down`, `Up`, `Right`
    /// and `Tab` all mean something to a focused `TextEdit` and to egui's focus machinery, and a
    /// test that skipped the field would pass against a bar where the arrows only moved the caret.
    #[test]
    fn the_path_field_completes_what_is_typed_into_it() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;
        // This pane's own folder, which is therefore already in the loader's cache — the case a
        // real `Ctrl+L` starts from.
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        // A capital `S` against a folder called `src`, which is the case-insensitivity claim.
        open_path_field(&mut h, pane, &format!("{}\\S", here.display()));
        let (offers, hot) = h.app.complete.offering();
        assert_eq!(
            offers, ["src"],
            "the completion offered the wrong folders for `S`"
        );
        assert_eq!(hot, None, "something was highlighted before an arrow was pressed");
        assert!(
            !h.app.complete.showing(),
            "the dropdown came up on its own -- `Ctrl+L` fills the field with where you already \
             are, and a list of that is a list in the way"
        );

        // Down: the dropdown, and an offer under the keyboard, in one press.
        h.frame(tap(egui::Key::ArrowDown));
        assert!(h.app.complete.showing(), "Down did not open the dropdown");
        assert_eq!(
            h.app.complete.offering().1,
            Some(0),
            "Down opened the dropdown without landing on anything, so every offer costs two presses"
        );

        // Right: the name, and the separator that starts the next one.
        h.frame(tap(egui::Key::ArrowRight));
        let walked = format!("{}\\src\\", here.display());
        assert_eq!(
            h.tab(0).edit_text, walked,
            "Right did not put the highlighted name in the field"
        );

        // And the caret went with it. Typing is the only honest way to ask: the caret lives in
        // `TextEdit`'s own memory under an id the design system generates internally, and "the
        // next keystroke lands at the end" is the claim that matters.
        h.frame(vec![Event::Text("u".to_owned())]);
        assert_eq!(
            h.tab(0).edit_text,
            format!("{walked}u"),
            "the caret stayed where the typing left it, so the next keystroke landed inside the \
             name that had just been completed"
        );

        // What is offered now is what is inside the folder that was just named -- the second half
        // of the walk. Backspace first, so the `u` is not narrowing it.
        h.frame(tap(egui::Key::Backspace));
        for attempt in 0..200 {
            h.frame(Vec::new());
            if attempt >= 2 && h.app.complete.offering().0.len() > 1 {
                break;
            }
            if attempt >= 2 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        let (offers, _) = h.app.complete.offering();
        assert!(
            offers.contains(&"ui") && offers.contains(&"shell"),
            "after Right the offers are still the old folder's: {offers:?}"
        );
    }

    /// **`Tab` completes and stays in the field**, which is the one key here that consuming is not
    /// enough for.
    ///
    /// egui decides whether a `Tab` moves the focus at the top of the frame, from the raw events,
    /// before any widget has run — so the bar cannot take the key back, it has to have said in
    /// advance that it wants it. That is `breadcrumb::keep_tab`, and this is the test of it: a
    /// green assertion on the *text* alone would pass against a bar that completed the name and
    /// then threw the field away in the same keystroke, which is not a completion anybody can use.
    ///
    /// From a field that has only just opened, with the dropdown still down and nothing
    /// highlighted, because that is where the reflex puts it: `Ctrl+L`, three letters, `Tab`.
    #[test]
    fn tab_completes_without_leaving_the_field() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        open_path_field(&mut h, pane, &format!("{}\\s", here.display()));
        assert!(
            !h.app.complete.showing(),
            "the dropdown is already up, so this proves nothing about Tab reaching past it"
        );

        h.frame(tap(egui::Key::Tab));
        assert_eq!(
            h.tab(0).edit_text,
            format!("{}\\src\\", here.display()),
            "Tab did not complete the one offer there was"
        );
        h.frame(Vec::new());
        assert!(
            h.tab(0).editing_path,
            "Tab completed and then handed the keyboard to the next widget, which closed the field"
        );
        // And it is still the field that has the keyboard, so the next keystroke is still a path.
        h.frame(vec![Event::Text("u".to_owned())]);
        assert_eq!(
            h.tab(0).edit_text,
            format!("{}\\src\\u", here.display()),
            "the field kept the keyboard but not the caret"
        );
    }

    /// **`Use / in path`, ticked in the path field's own menu.**
    ///
    /// Three claims, and only the first is about the setting. The menu is *reachable* — a menu
    /// nothing can open is one of the two failures only a driven frame can see, which is why this
    /// gesture is real all the way through: right-click the field, find the entry by reading it,
    /// click it. The path in the field turns over on the tick, which is the whole of what the
    /// setting does and what somebody ticking it is looking at. And **the field survives it**: a
    /// press in a popup is a press outside the field, so egui takes the keyboard off it, and the
    /// frame that ticked would otherwise be the frame that closed the field and put the breadcrumb
    /// back — with nothing on screen to show what the tick did. See `breadcrumb::edit_field`.
    ///
    /// Then off again, because a setting that cannot be untied is not a setting, and because the
    /// swap has to be exact in the direction nobody thinks about.
    #[test]
    fn the_path_fields_menu_switches_which_slash_it_writes() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;
        let ctx = h.ctx.clone();

        assert!(!h.app.forward_slashes, "`\\` is what a fresh profile shows");
        // `Ctrl+L`'s own path into the field, so what it is prefilled with is under test too.
        h.app.perform(&ctx, Action::EditPath(pane));
        h.frame(Vec::new());
        h.frame(Vec::new());
        let filled = h.tab(0).edit_text.clone();
        assert!(
            filled.contains('\\') && !filled.contains('/'),
            "the field opened with the wrong separator: {filled}"
        );

        // The field's rect, taken from the hover-only anchor that shares it — the `TextEdit` inside
        // a `TextField` has an id the design system generates, and there is nothing to read it by
        // from out here.
        let field = h
            .ctx
            .read_response(Id::new(("crumb-complete", pane)))
            .map(|r| r.rect)
            .expect("the path field was not laid out");
        h.click_with(field.center(), PointerButton::Secondary, Modifiers::NONE);

        let entry = |h: &Harness, label: &str| {
            h.texts()
                .into_iter()
                .find(|(_, text)| text == label)
                .map(|(at, _)| at)
        };
        let tick = entry(&h, "Use / in path").unwrap_or_else(|| {
            panic!(
                "the field's menu did not open, or has no slash entry in it: {:?}",
                h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
            )
        });
        // A couple of points into the label, which is inside the entry whatever its padding is.
        let done = h.click_at(pos2(tick.x + 2.0, tick.y + 6.0));
        assert!(
            done.contains(&"SetForwardSlashes"),
            "clicking `Use / in path` did nothing, got {done:?}"
        );
        assert!(h.app.forward_slashes, "the entry did not turn it on");
        // A setting, so it is part of what the window writes down. Asked of `settings()` rather
        // than of `config_dirty`, which the frame after the one that sets it has already cleared.
        assert!(h.app.settings().forward_slashes, "and it was not written down");

        assert!(
            h.tab(0).editing_path,
            "the tick closed the field, so there is nothing left on screen to show what it did"
        );
        let turned = h.tab(0).edit_text.clone();
        assert_eq!(
            turned,
            filled.replace('\\', "/"),
            "the path in the field did not turn over on the tick"
        );
        // The dropdown is *not* what a tick brings up: the text changed without anybody typing, and
        // a list of the folder you are standing in is the list `Ctrl+L` is careful not to show.
        assert!(
            !h.app.complete.showing(),
            "the completion came up on its own after the tick"
        );
        // And the keyboard came back, so the field is still a field. Whether what is in it is
        // selected is the design system's business — an `x` landing in it at all is the claim.
        h.frame(vec![Event::Text("x".to_owned())]);
        assert!(
            h.tab(0).edit_text.contains('x'),
            "the field kept its text but not the keyboard: {:?}",
            h.tab(0).edit_text
        );

        // Off again, from a field reopened on the same folder — which is now filled with `/`,
        // the other half of what the setting is for.
        h.app.perform(&ctx, Action::EditPath(pane));
        h.frame(Vec::new());
        h.frame(Vec::new());
        assert_eq!(
            h.tab(0).edit_text, turned,
            "the setting did not survive the field being reopened"
        );
        h.click_with(field.center(), PointerButton::Secondary, Modifiers::NONE);
        let tick = entry(&h, "Use / in path").expect("the menu did not open a second time");
        h.click_at(pos2(tick.x + 2.0, tick.y + 6.0));
        assert!(!h.app.forward_slashes, "the entry does not untick");
        assert_eq!(
            h.tab(0).edit_text, filled,
            "turning it off did not put the path back the way Windows writes it"
        );
    }

    /// **A click away from the bar closes the field even with the field's menu open**, which is the
    /// other half of the rule the tick above rests on.
    ///
    /// The two pull in opposite directions from one frame. `egui::Popup` decides to close *after* its
    /// body has run, so the frame a click outside the popup lands on is a frame the popup is still
    /// open for — the same state a tick leaves behind, and the field is forgiving its focus loss in
    /// exactly one of the two cases. Getting that wrong does not show up as a menu that misbehaves:
    /// it shows up here, as a path field welded over the breadcrumb with the keyboard, over a
    /// listing that has just been clicked in. Which is why this sits beside the test above rather
    /// than inside it.
    #[test]
    fn a_click_past_the_path_fields_menu_still_closes_the_field() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;
        let ctx = h.ctx.clone();

        h.app.perform(&ctx, Action::EditPath(pane));
        h.frame(Vec::new());
        h.frame(Vec::new());
        let field = h
            .ctx
            .read_response(Id::new(("crumb-complete", pane)))
            .map(|r| r.rect)
            .expect("the path field was not laid out");
        h.click_with(field.center(), PointerButton::Secondary, Modifiers::NONE);
        assert!(
            h.texts().iter().any(|(_, text)| text == "Use / in path"),
            "the menu is not open, so this proves nothing"
        );

        // The listing, well clear of both the field and the menu the right click put up.
        h.click_at(h.row_center(0, 4));
        assert!(
            !h.tab(0).editing_path,
            "the field stayed open with the keyboard, over a listing that had just been clicked in"
        );
    }

    /// The keyboard's highlight is a band across the highlighted offer, in the fill a row under
    /// the pointer wears — and it moves with the highlight.
    ///
    /// `MenuItem` has no keyboard state of its own: it lights up hovered or focused, and focusing a
    /// row here would take the keyboard off the field being typed into. So the fill is painted
    /// *behind* the row, into a slot reserved before the row is added, and this is the test that it
    /// lands on the row rather than under it. The colour is asserted too, because two different
    /// greys for one meaning would read as two different things.
    ///
    /// The band is found by shape and the *move* is measured between two presses, rather than
    /// either being checked against a computed row position — a test that re-derives where the
    /// layout put something is a test of its own arithmetic, and the offers' own names are no help
    /// here: the listing behind the dropdown is showing most of the same folders.
    ///
    /// Frames are run out before the shapes are read, because a popup fades in. Sampled three
    /// frames after it opens, every colour in it comes back part transparent.
    #[test]
    fn the_highlight_is_a_band_that_follows_the_keyboard() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        // A folder with several to choose from, so a band across the whole dropdown rather than
        // across one row of it comes out as the wrong height.
        open_path_field(&mut h, pane, &format!(r"{}\", here.display()));

        let hover = h.app.theme.bg.control_hover;
        let row = azur::components::menu_item_height();
        let band = |h: &Harness| -> Rect {
            let found: Vec<Rect> = h
                .rects()
                .into_iter()
                .filter(|(rect, _, fill)| {
                    *fill == hover && (rect.height() - row).abs() < 0.5 && rect.width() > 200.0
                })
                .map(|(rect, _, _)| rect)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected one highlight band the height of a menu row, found {}: {found:?}",
                found.len()
            );
            found[0]
        };

        h.frame(tap(egui::Key::ArrowDown));
        for _ in 0..12 {
            h.frame(Vec::new());
        }
        assert!(
            h.app.complete.offering().0.len() >= 3,
            "not enough offers to tell a row from a list: {:?}",
            h.app.complete.offering().0
        );
        assert_eq!(h.app.complete.offering().1, Some(0));
        let first = band(&h);

        h.frame(tap(egui::Key::ArrowDown));
        for _ in 0..12 {
            h.frame(Vec::new());
        }
        assert_eq!(h.app.complete.offering().1, Some(1));
        let second = band(&h);

        assert_eq!(
            second.left(),
            first.left(),
            "the band moved sideways between two offers"
        );
        assert!(
            (second.top() - first.top() - row).abs() < 0.5,
            "the band moved by {} between the first offer and the second, and a row is {row}",
            second.top() - first.top()
        );
    }

    /// **The dropdown holds ten offers and not eleven**, which is `breadcrumb::OFFERS_SHOWN` and
    /// the reason it is a whole number of rows.
    ///
    /// Asserted on the dropdown's *height* rather than by counting the names in the frame, and the
    /// difference is the whole point of the constant: a scroll area lays out the row past its
    /// bottom edge and clips it, so eleven names are in the shape list either way and only ten of
    /// them have any pixels. Height is what a reader sees, and bracketing it — ten whole rows fit,
    /// eleven do not — says the ceiling lands between two rows without this test having to know
    /// what the frame adds around them.
    ///
    /// Fourteen folders in a sandbox rather than a real one deep enough to overflow, so the count
    /// does not depend on what is installed on the machine running the suite.
    #[test]
    fn the_dropdown_holds_ten_offers_and_scrolls_the_rest() {
        let root = crate::sandbox::dir("offers");
        crate::sandbox::remove(&root);
        let names: Vec<String> = (1..=14).map(|i| format!("folder-{i:02}")).collect();
        for name in &names {
            std::fs::create_dir_all(root.join(name)).expect("a directory in the temp folder");
        }

        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;

        // The sandbox is not the folder either pane is showing, so this is also the case where the
        // completion has to wait on the loader for a folder nothing has read yet.
        open_path_field(&mut h, pane, &format!("{}\\", root.display()));
        // A field filled from outside keeps its dropdown down until a key asks for it, exactly as a
        // `Ctrl+L` does. `Down` is that key, and it leaves the list unscrolled at its first row.
        h.frame(tap(egui::Key::ArrowDown));
        for _ in 0..8 {
            h.frame(Vec::new());
        }
        let (offers, hot) = h.app.complete.offering();
        assert_eq!(
            offers.len(),
            names.len(),
            "every folder in the sandbox should be on offer: {offers:?}"
        );
        assert_eq!(hot, Some(0), "the list should be sitting at its first row");

        let height = dropdown_rect(&h, pane).height();
        let row = azur::components::menu_item_height();
        let shown = crate::ui::breadcrumb::OFFERS_SHOWN as f32;
        assert!(
            height >= shown * row,
            "the dropdown is {height} tall and {shown} rows of {row} do not fit in it"
        );
        assert!(
            height < (shown + 1.0) * row,
            "the dropdown is {height} tall, which is room for more than {shown} rows -- the              ceiling has to land between two of them or the last one comes out half drawn"
        );
    }

    /// The completion dropdown's own frame, found by the surface `menu_frame` paints it in.
    ///
    /// By its three colour channels only: a popup fades in, and the last step of that fade lands on
    /// 254 of 255 rather than on opaque, so an equality that counted the alpha would find nothing.
    /// Anchored on the widget that the dropdown hangs off rather than on a computed rect -- a test
    /// that re-derives where the layout put something is a test of its own arithmetic.
    fn dropdown_rect(h: &Harness, pane: PaneId) -> Rect {
        let field = h
            .ctx
            .read_response(Id::new(("crumb-complete", pane)))
            .expect("the path field's dropdown anchor")
            .rect;
        let surface = h.app.theme.bg.layer_alt.to_array();
        let found: Vec<Rect> = h
            .rects()
            .into_iter()
            .filter(|(rect, _, fill)| {
                fill.to_array()[..3] == surface[..3]
                    && fill.a() > 200
                    && rect.top() >= field.bottom()
                    && (rect.width() - field.width()).abs() < 8.0
            })
            .map(|(rect, _, _)| rect)
            .collect();
        assert_eq!(
            found.len(),
            1,
            "expected one dropdown under the field, found {}: {found:?}",
            found.len()
        );
        found[0]
    }

    /// **A dropdown that was short stays short — the bug this is really about.**
    ///
    /// Typing `d:/` showed two rows where there were fifteen folders to show. An `Area` hands its
    /// content last frame's size as this frame's room, and a `ScrollArea` fits itself into whatever
    /// room it is given without asking for more, so the two lock each other: `d` offers one drive,
    /// the popup becomes one row tall, and the fifteen folders that `d:/` turns up then shrink to
    /// fit the one row of room that is left. It never recovers, because nothing ever asks it to.
    ///
    /// So this drives the list *through* a small one, with real keystrokes, and then asks how tall
    /// it is. Setting the text wholesale would not reproduce it — the first list would already be
    /// the long one, which is exactly why every capture of this looked right.
    #[test]
    fn a_dropdown_that_was_short_grows_back() {
        let root = crate::sandbox::dir("grow");
        crate::sandbox::remove(&root);
        let names: Vec<String> = (1..=14).map(|i| format!("folder-{i:02}")).collect();
        for name in &names {
            std::fs::create_dir_all(root.join(name)).expect("a directory in the temp folder");
        }

        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;

        // One offer: `folder-01` matches only itself.
        open_path_field(&mut h, pane, &format!(r"{}\folder-01", root.display()));
        h.frame(tap(egui::Key::ArrowDown));
        for _ in 0..8 {
            h.frame(Vec::new());
        }
        assert_eq!(
            h.app.complete.offering().0.len(),
            1,
            "this has to start from a one-row dropdown or it proves nothing"
        );
        let short = dropdown_rect(&h, pane).height();

        // `End` first: a field selects what it holds when focus arrives, so a `Backspace` here
        // would take the whole path out rather than one character of the name.
        h.frame(tap(egui::Key::End));
        // Two backspaces leave `folder-`, which every one of the fourteen matches.
        h.frame(tap(egui::Key::Backspace));
        h.frame(tap(egui::Key::Backspace));
        for _ in 0..8 {
            h.frame(Vec::new());
        }
        assert_eq!(
            h.app.complete.offering().0.len(),
            names.len(),
            "the offers did not grow, so the height cannot be what this is measuring"
        );

        let row = azur::components::menu_item_height();
        let shown = crate::ui::breadcrumb::OFFERS_SHOWN as f32;
        let grown = dropdown_rect(&h, pane).height();
        assert!(
            grown > short,
            "the dropdown stayed {short} tall after its list grew from 1 offer to {}",
            names.len()
        );
        assert!(
            grown >= shown * row,
            "the dropdown grew to {grown}, which is not room for {shown} rows of {row} -- it is              still wearing the size it had when the list was short"
        );
        assert!(
            grown < (shown + 1.0) * row,
            "the dropdown is {grown} tall, which is room for more than {shown} rows"
        );

        crate::sandbox::remove(&root);
    }

    /// `Enter` on a highlighted offer goes there, without asking the disk whether the text names
    /// anything: the offer came out of a listing and carries its own path.
    #[test]
    fn enter_on_a_highlighted_completion_goes_there() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        open_path_field(&mut h, pane, &format!("{}\\s", here.display()));
        h.frame(tap(egui::Key::ArrowDown));
        assert_eq!(h.app.complete.offering().1, Some(0), "nothing to press Enter on");

        h.take_journal();
        h.frame(tap(egui::Key::Enter));
        h.settle();
        assert_eq!(
            h.tab(0).path,
            here.join("src"),
            "Enter on the highlighted offer did not navigate to it"
        );
        assert!(
            !h.tab(0).editing_path,
            "the field is still open after going somewhere"
        );
    }

    /// Every text field in the window is square, and wears the fill it is supposed to.
    ///
    /// All three at once, in one frame, because they do not come from one place: the rename field
    /// is egui's `TextEdit` and takes its frame from `Style::interact`, while the path field and
    /// the filter are Azur's `TextField` and took theirs from a hard-coded `control_radius()`
    /// until the design system was changed to read the installed style too. One mechanism now —
    /// `ui::squared` — but a test that checked only one of them would pass while the other stayed
    /// a bubble.
    ///
    /// Found by what a field *is* rather than by re-deriving where the layout put it — a test that
    /// computes a field's rect is a test of its own arithmetic. `background-control` is the fill,
    /// and that alone is not enough: in the dark theme it is the same `GRAY_4` as `stroke-subtle`,
    /// so the block behind the panels, the path bar, the selected tab, the divider on the bar and
    /// the drive gauges all match it too. Adding "no taller than a control, and wider than a line"
    /// leaves exactly the three, and the count is asserted so that stops being true loudly.
    #[test]
    fn the_text_fields_are_square_and_wear_the_right_fill() {
        let mut h = Harness::new();
        h.settle();

        let pane = h.app.panes[0].id;
        let ctx = h.ctx.clone();
        // The filter is up whenever the pane is wide enough, which it is. The other two have to
        // be opened.
        {
            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
            tab.select_only(0);
            tab.filter = "r".to_owned();
            tab.editing_path = true;
        }
        h.app.perform(&ctx, Action::BeginRename(pane));
        h.frame(Vec::new());

        // Two fills, not one: the path field and the filter are `background-control`, and the
        // rename field is `background-layer` on purpose — it stands in for a row and has to look
        // like the panel rather than like a control dropped on top of one.
        let fills = [h.app.theme.bg.control, h.app.theme.bg.layer];
        let field_shaped = |rect: &Rect| {
            rect.height() >= 16.0
                && rect.height() <= azur::tokens::control::SMALL + 0.5
                && rect.width() >= 40.0
        };
        let fields: Vec<(Rect, egui::CornerRadius, egui::Color32)> = h
            .rects()
            .into_iter()
            .filter(|(rect, _, fill)| fills.contains(fill) && field_shaped(rect))
            .collect();
        assert_eq!(
            fields.len(),
            3,
            "expected the rename field, the path field and the filter, found {}: {:?}",
            fields.len(),
            fields.iter().map(|(r, _, _)| *r).collect::<Vec<_>>()
        );
        // All of them, not the first: the three come from two different widgets, and a failure
        // that names only one leaves you guessing whether the other is square or merely earlier
        // in the paint order.
        let rounded: Vec<Rect> = fields
            .iter()
            .filter(|(_, corner, _)| *corner != egui::CornerRadius::ZERO)
            .map(|(rect, _, _)| *rect)
            .collect();
        assert!(
            rounded.is_empty(),
            "{} of 3 text fields have rounded corners: {rounded:?}",
            rounded.len()
        );

        // And exactly one of them is the panel's own colour: the rename field, which stands in for
        // a row. `background-control` on that one drew a grey slab over the name.
        let on_panel = fields
            .iter()
            .filter(|(_, _, fill)| *fill == h.app.theme.bg.layer)
            .count();
        assert_eq!(
            on_panel, 1,
            "the rename field should be the only `background-layer` one of the three"
        );
    }

    /// The window's resize band does not swallow the listing's scrollbar.
    ///
    /// This is the bill for the panels reaching the window's edges. The east band is registered
    /// last and so wins any click in the right-hand four points, which used to be canvas and is
    /// now the outer edge of a `10`-point scrollbar. Six points is enough — but "enough" is a
    /// claim about a click target, and the only way to know is to press the pointer down there
    /// and see whether the listing moves.
    #[test]
    fn the_scrollbar_survives_the_window_resize_band() {
        let mut h = Harness::new();
        // Short enough that the folder overflows and there is a bar to grab at all.
        h.size = egui::vec2(1024.0, 300.0);
        h.settle();
        assert_eq!(h.app.panes[0].tab().scroll_y, 0.0, "it starts at the top");

        // As far out as the pointer can go and still be the scrollbar's: one point inside the
        // band, which is the pixel this test exists to defend.
        let x = h.size.x - crate::ui::GUTTER - 1.0;
        let y = h.size.y * 0.5;
        h.frame(vec![Event::PointerMoved(pos2(x, y))]);
        h.frame(vec![Event::PointerButton {
            pos: pos2(x, y),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        for step in 1..=4 {
            h.frame(vec![Event::PointerMoved(pos2(x, y + step as f32 * 10.0))]);
        }
        let scrolled = h.app.panes[0].tab().scroll_y;
        h.frame(vec![Event::PointerButton {
            pos: pos2(x, y + 40.0),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        assert!(
            scrolled > 0.0,
            "dragging the scrollbar {} points from the window's edge scrolled nothing",
            h.size.x - x
        );
    }

    /// The path bar is the selected tab's colour, and the tab is the same colour it is.
    ///
    /// Both from one frame, because the point of the change is that they *match*: a test that
    /// checked the bar alone would pass just as well if the tab drifted, and the two are painted
    /// by different modules.
    #[test]
    fn the_path_bar_and_the_selected_tab_are_one_surface() {
        let mut h = Harness::new();
        h.settle();

        let seam = crate::ui::seam(&h.app.theme);
        // The pane's own rect, not an inset of it: the bar reaches the seams on both sides.
        let pane = h.app.pane_rects[0].1;
        let bar = Rect::from_min_size(
            pane.min,
            egui::vec2(pane.width(), crate::ui::breadcrumb::HEIGHT),
        );

        let (corner, fill) = h
            .fill_at(bar)
            .expect("nothing was painted at the path bar's rect");
        assert_eq!(fill, seam, "the path bar is not the selected tab's colour");
        assert_eq!(corner, egui::CornerRadius::ZERO);

        // The tab above it, found among the slots this frame resolved rather than derived.
        let tab = h
            .app
            .tab_slots
            .iter()
            .find(|slot| slot.pane == h.app.focused && slot.tab == 0)
            .map(|slot| slot.rect)
            .expect("the focused pane's tab was not laid out");
        // Its fill reaches a point past its own rect, over the strip's bottom hairline. That
        // overhang *is* the weld — with the tab stopping at its rect, a line of `stroke-subtle`
        // ran between two surfaces of the same colour — so the test looks for the welded rect
        // and would fail if the tab went back to painting only itself.
        let welded = Rect::from_min_max(tab.min, pos2(tab.max.x, tab.max.y + 1.0));
        let filled = h
            .rects()
            .into_iter()
            .rev()
            .find(|(rect, _, _)| {
                rect.min.distance(welded.min) < 0.5 && rect.max.distance(welded.max) < 0.5
            })
            .expect("the focused pane's tab does not reach over the strip's bottom edge");
        assert_eq!(
            filled.2, seam,
            "the focused pane's selected tab is not the colour its path bar is"
        );
    }

    /// A folder waiting to name a new file is not silenced by the git-write filter.
    ///
    /// The filter drops a folder's own change for [`GIT_WRITE_SETTLE`] after git last answered,
    /// because `git::read` writes the index and that write comes straight back through the folder's
    /// own watch — a loop that never stops on its own. The documented cost is that a real change in
    /// the same window is missed until something else touches the folder.
    ///
    /// For `New >` that cost is the whole feature. Measured before this: git answered at 1.23 s and
    /// the shell's file landed before 2.57 s, so in **any** folder git has something to say about —
    /// which is most of the ones this program gets used in — the new file did not appear at all.
    ///
    /// Shell-free on purpose: a plain write is the same notification, so this runs in the normal
    /// suite. `git_settled_at` is re-stamped every round to hold the filter's condition open, which
    /// makes the two halves a clean A/B on `name_the_new` alone rather than a race with the clock.
    #[test]
    #[cfg(windows)]
    fn a_folder_waiting_for_a_new_file_hears_about_it_through_the_git_filter() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("new-through-filter");
        crate::sandbox::remove(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        std::fs::write(dir.join("already.txt"), b"x").expect("a file");

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: dir.clone(),
            },
        );
        h.settle();

        // `git_settled_at` is the whole of what arms the filter — it does not also check that there
        // is a repository — so stamping it stands in for git having just answered, without waiting
        // on a real `git status` and without making this a race against the clock. Re-stamped every
        // round, so the window stays open for as long as the round loop runs.
        let run = |h: &mut Harness, rounds: usize| {
            for _ in 0..rounds {
                h.app.panes[0].tab_mut().git_settled_at = Some(h.time);
                std::thread::sleep(std::time::Duration::from_millis(15));
                h.time += 0.05;
                h.frame(Vec::new());
            }
        };

        // The control: nothing is waiting, so the filter does what it is there for.
        std::fs::write(dir.join("unasked.txt"), b"y").expect("a file");
        run(&mut h, 60);
        assert_eq!(
            h.tab(0).names(),
            ["already.txt".to_owned()],
            "the filter is not armed, so this test's other half proves nothing"
        );

        // And the same notification, with a name waiting to be given.
        h.app.panes[0].tab_mut().name_the_new = Some(h.tab(0).names());
        std::fs::write(dir.join("asked-for.txt"), b"z").expect("a file");
        run(&mut h, 60);

        let (entry, text) = h
            .tab(0)
            .renaming
            .clone()
            .expect("the change was swallowed, so the new file never arrived to be named");
        assert_eq!(
            h.tab(0).dir.as_ref().expect("a listing").leaf(entry),
            "asked-for.txt",
            "the wrong row is being renamed"
        );
        assert_eq!(text, "asked-for.txt");
        assert!(
            h.tab(0).name_the_new.is_none(),
            "the snapshot has to be consumed, or the filter stays bypassed for good"
        );

        crate::sandbox::remove(&dir);
    }

    /// The whole chain behind `New > Text Document`: the shell makes the file, the watcher notices,
    /// the folder is re-read, and the row that appeared arrives selected with its name being edited.
    ///
    /// Real from end to end — a real verb through [`crate::shell::Modal`], a real
    /// `ReadDirectoryChangesW`, a real re-read — because the timing is where this goes wrong and
    /// no harness reproduces it. The two halves race by construction: the rename *starts* on a
    /// re-read, and the file appearing is what asks for one. So the second half of this test is
    /// simply waiting, with frames running, to see whether the field is still on the same file
    /// after every notification the creation produced has been and gone.
    #[test]
    #[ignore = "asks the shell to create a real file; run explicitly, single-threaded"]
    #[cfg(windows)]
    fn new_from_the_shell_menu_ends_in_a_rename_field() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("new-menu");
        crate::sandbox::remove(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        std::fs::write(dir.join("already.txt"), b"x").expect("a file");

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: dir.clone(),
            },
        );
        h.settle();
        assert_eq!(
            h.tab(0).names(),
            ["already.txt".to_owned()],
            "the sandbox as it starts"
        );

        // The real entry, off the real background menu. `.txt` because `Document texte` is a label
        // and this has to run on an English Windows too.
        let entries = crate::shell::menu::build(&dir, &[]);
        let command = entries
            .iter()
            .filter_map(|e| match &e.kind {
                crate::shell::menu::Kind::Submenu { children, .. } => Some(children),
                _ => None,
            })
            .flatten()
            .find_map(|c| match &c.kind {
                crate::shell::menu::Kind::Command(
                    command @ crate::shell::menu::Command::Shell { verb: Some(verb), .. },
                ) if verb == ".txt" => Some(command.clone()),
                _ => None,
            })
            .expect("`New > Text Document` on the folder's background menu");

        // The same decision the menu makes, then the same invoke.
        h.app.watch_for_a_new_item(pane, &dir, &command);
        assert!(
            h.tab(0).name_the_new.is_some(),
            "the listing was not written down, so there is nothing to recognise the new file by"
        );
        assert!(h.app.modal.send(crate::shell::Request::Invoke {
            parent: dir.clone(),
            items: Vec::new(),
            command,
            depth: crate::shell::menu::Depth::Full,
            owner: crate::shell::Owner::default(),
        }));

        // Frames *and* real time: the shell's write and the watcher's settle are both real, and the
        // watcher's clock is the frame's.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while h.tab(0).renaming.is_none() {
            let on_disk: Vec<String> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect();
            assert!(
                std::time::Instant::now() < deadline,
                "20 s after the verb nothing is being renamed. On disk: {on_disk:?}. In the \
                 listing: {:?}. Snapshot still pending: {}",
                h.tab(0).names(),
                h.tab(0).name_the_new.is_some()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
            h.time += 0.05;
            h.frame(Vec::new());
        }

        let (entry, text) = h.tab(0).renaming.clone().expect("a rename");
        let leaf = h.tab(0).dir.as_ref().expect("a listing").leaf(entry).to_owned();
        assert_ne!(
            leaf, "already.txt",
            "the file that was here before is the one being renamed"
        );
        assert!(leaf.ends_with(".txt"), "renaming `{leaf}`");
        assert_eq!(
            text, leaf,
            "the field has to start from the name the file actually has"
        );
        assert_eq!(h.tab(0).selected_count, 1, "and be the only thing selected");

        // Now the part that matters: it has to survive every later notification the creation set
        // off. A stale entry index would still be on screen here, pointing at another file.
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            h.time += 0.05;
            h.frame(Vec::new());
        }
        let (entry, still) = h
            .tab(0)
            .renaming
            .clone()
            .expect("the rename field went away while nothing was touching it");
        assert_eq!(
            h.tab(0).dir.as_ref().expect("a listing").leaf(entry),
            leaf,
            "a later re-read moved the rename field onto a different file -- committing it would \
             have renamed the wrong one"
        );
        assert_eq!(still, leaf, "and the text has to be untouched");

        crate::sandbox::remove(&dir);
    }

    /// Which items get the short menu, and that a short one admits to being short.
    ///
    /// The rule is narrow on purpose. What is slow is not the network: on the share this was
    /// measured against, a 41 MB `.lib` builds a full menu in 1.8 s and the folder itself in
    /// 0.2 s. It is an *executable* on a share, where the time is linear in the file's size
    /// because something reads all of it. A blanket rule for network paths would drop 7-Zip and
    /// Send To from every file on the share to fix a problem only executables have.
    #[test]
    #[cfg(windows)]
    fn only_a_network_executable_gets_the_short_menu() {
        use crate::shell::menu::Depth;

        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let unc = PathBuf::from(r"\\somehost\someshare\bin");

        for (what, folder, items, want) in [
            (
                "a local executable",
                here.clone(),
                vec![here.join("thing.exe")],
                Depth::Full,
            ),
            (
                "an executable on a share",
                unc.clone(),
                vec![unc.join("thing.exe")],
                Depth::Fast,
            ),
            (
                "a DLL on a share",
                unc.clone(),
                vec![unc.join("thing.DLL")],
                Depth::Fast,
            ),
            (
                "a text file on a share",
                unc.clone(),
                vec![unc.join("notes.txt")],
                Depth::Full,
            ),
            (
                "a big archive on a share",
                unc.clone(),
                vec![unc.join("everything.7z")],
                Depth::Full,
            ),
            (
                "the folder itself, on a share",
                unc.clone(),
                Vec::new(),
                Depth::Full,
            ),
            (
                "a mixed selection with an executable in it, on a share",
                unc.clone(),
                vec![unc.join("notes.txt"), unc.join("thing.exe")],
                Depth::Fast,
            ),
        ] {
            assert_eq!(
                App::menu_depth(&folder, &items),
                want,
                "{what}: wrong depth"
            );
        }

        // A UNC path is a network path by construction; the extended-length and device forms
        // start the same way and are not.
        assert!(crate::shell::over_network(&unc));
        assert!(!crate::shell::over_network(&here));
        assert!(!crate::shell::over_network(Path::new(r"\\?\C:\Windows")));
        assert!(!crate::shell::over_network(Path::new(r"\\.\PhysicalDrive0")));
        assert!(!crate::shell::over_network(Path::new("")));
    }

    /// A short menu says so, because a missing 7-Zip should not look like a broken program.
    #[test]
    #[cfg(windows)]
    fn a_short_menu_admits_to_being_short() {
        let _serialised = crate::shell::serialised();
        let mut h = Harness::new();
        h.settle();

        // A local file, so the menu is quick; the point here is the depth it was asked at, not
        // where the file is.
        let pane = h.app.panes[0].id;
        let folder = h.app.pane_mut(pane).expect("the pane").tab().path.clone();
        let items = vec![folder.join("Cargo.toml")];
        h.app.asking = Some(Asking {
            token: h
                .app
                .menu_builder
                .build(&folder, &items, crate::shell::menu::Depth::Fast),
            pane,
            at: egui::pos2(200.0, 200.0),
            items,
            folder,
            depth: crate::shell::menu::Depth::Fast,
            since: h.ctx.cumulative_pass_nr(),
            asked: std::time::Instant::now(),
        });

        let waited = std::time::Instant::now();
        while h.app.menu_pending() {
            h.frame(Vec::new());
            assert!(
                waited.elapsed() < std::time::Duration::from_secs(20),
                "the reduced menu never arrived"
            );
        }
        let menu = h.app.menu.as_ref().expect("a menu");
        assert!(
            menu.entries.len() > 3,
            "the reduced menu is too short to be one: {:?}",
            menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
        );
        let notice = h.app.notice.clone().unwrap_or_default();
        assert!(
            notice.contains("network"),
            "nothing told the user why the menu is short: {notice:?}"
        );
        h.app.close_menu();
        h.frame(Vec::new());
    }

    /// Anything on a share that is *still* slow gets asked for again with less.
    ///
    /// The extension list in [`App::menu_depth`] covers what was measured. This covers what was
    /// not: whatever this machine's extensions decide to read a whole file for next. Two and a
    /// half seconds, because the slowest *full* menu measured on that share for something that
    /// was not an executable was 1.8 s — so past this it is somebody inspecting the file rather
    /// than the share being busy.
    #[test]
    #[cfg(windows)]
    fn a_slow_network_menu_is_asked_for_again_with_less() {
        use std::sync::atomic::Ordering;
        use crate::shell::menu::Depth;

        let _serialised = crate::shell::serialised();
        let mut h = Harness::new();
        h.settle();

        // Nothing here should reach the shell: a UNC path to a host that does not exist would
        // spend the test's whole budget in DNS and SMB timeouts. The stall stands in for the
        // slow build, and is left set so the retry stalls too.
        crate::shell::menu::STALLED.store(0, Ordering::SeqCst);
        crate::shell::menu::STALL_MS.store(4_000, Ordering::SeqCst);

        let pane = h.app.panes[0].id;
        let folder = PathBuf::from(r"\\somehost\someshare\bin");
        let items = vec![folder.join("mystery.dat")];
        let first = h
            .app
            .menu_builder
            .build(&folder, &items, Depth::Full);
        h.app.asking = Some(Asking {
            token: first,
            pane,
            at: egui::pos2(200.0, 200.0),
            items,
            folder,
            depth: Depth::Full,
            // Already past the deadline, so the test does not have to sit through it.
            since: h.ctx.cumulative_pass_nr(),
            asked: std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(3))
                .expect("a monotonic clock three seconds old"),
        });

        h.frame(Vec::new());

        let asking = h.app.asking.as_ref().expect("still asking, with less");
        assert_eq!(
            asking.depth,
            Depth::Fast,
            "a full menu on a share was still being waited for three seconds in"
        );
        assert_ne!(
            asking.token, first,
            "the depth changed but the ask did not, so nothing was re-asked"
        );

        h.app.close_menu();
        crate::shell::menu::STALL_MS.store(0, Ordering::SeqCst);
        h.frame(Vec::new());
    }

    /// Ctrl+C then Ctrl+V, in the folder you are already looking at.
    ///
    /// The most ordinary thing anybody does with a clipboard, and the one case the first
    /// end-to-end test skipped: it copied from one folder and pasted into another, which is the
    /// *easy* half. Pasting into the folder the file is already in is where the shell has to be
    /// told not to ask, and where a paste that quietly does nothing is hardest to notice.
    ///
    /// Driven by real key events rather than by pushing actions, so the bindings are on trial
    /// too -- `Ctrl+C` and `Ctrl+V` being wired to the right actions is part of what is claimed.
    #[test]
    #[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
    #[cfg(windows)]
    fn copy_and_paste_in_the_same_folder_makes_a_copy() {
        use crate::shell::clipboard;

        let _serialised = crate::shell::serialised();
        // The paste below has to be the shell's real one. Inside `target/sandbox`; see
        // `crate::shell::ops::FOR_REAL`.
        let _for_real = crate::shell::ops::for_real();
        crate::shell::init();
        clipboard::settle_for_tests();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("samefolder");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        std::fs::write(root.join("one.txt"), b"one").expect("write");

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();

        assert_eq!(
            h.app.pane_mut(pane).expect("the pane").tab().order.len(),
            1,
            "one file to start with"
        );

        // Selected by *clicking* it, which is how anybody selects a file -- and which is the
        // difference between this test and the version that set the selection directly and
        // passed against a broken program. A click gives the row keyboard focus, and every
        // shortcut in this program was switched off while anything at all had focus.
        let body = h.app.panes[0].rect;
        let mut clicked = false;
        for step in 0..60 {
            let at = egui::pos2(body.left() + 60.0, body.top() + 40.0 + step as f32 * 4.0);
            if !body.contains(at) {
                break;
            }
            h.click_at(at);
            if h.app.pane_mut(pane).expect("the pane").tab().selected_count == 1 {
                clicked = true;
                break;
            }
        }
        assert!(clicked, "could not find the row to click");
        eprintln!("PROBE focused after the click: {:?}", h.ctx.memory(|m| m.focused()));

        /// One frame carrying what pressing Ctrl+C or Ctrl+V *actually* delivers.
        ///
        /// Not `Event::Key`. `egui-winit` recognises these combinations itself and queues
        /// `Event::Copy` or `Event::Paste` in place of the key press, returning before the key
        /// event is ever added -- so a test that synthesises `Event::Key { key: C }` is testing a
        /// keystroke this program will never receive. This one did, and it passed against a
        /// program in which Ctrl+C and Ctrl+V did nothing whatsoever.
        ///
        /// The modifiers still go on the input, since `InputState::modifiers` is what the other
        /// shortcuts read.
        fn shortcut(h: &mut Harness, event: Event) {
            h.modifiers = Modifiers {
                command: true,
                ctrl: true,
                ..Modifiers::NONE
            };
            h.frame(vec![event]);
            h.modifiers = Modifiers::NONE;
        }

        // Three times over, because once is what it managed. A file operation ends in a
        // re-read of the folder, the re-read used to clear the selection, and a cleared selection
        // is nothing to copy — so the second Ctrl+C copied nothing and everything after it was
        // a paste of whatever the first round had left on the clipboard.
        for round in 1..=3 {
            shortcut(&mut h, Event::Copy);
            assert!(
                clipboard::has_files(),
                "round {round}: Ctrl+C put nothing on the clipboard; the program said {:?}",
                h.app.notice
            );
            assert_eq!(
                h.app
                    .pane_mut(pane)
                    .expect("the pane")
                    .tab()
                    .selection_paths()
                    .len(),
                1,
                "round {round}: the file stopped being selected, so there was nothing to copy"
            );

            // What `paste_keystroke` in `main.rs` puts back when the clipboard holds files rather
            // than text, which is the case that matters here.
            shortcut(&mut h, Event::Paste(String::new()));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while h.app.ops.in_progress().is_some() {
                h.frame(Vec::new());
                assert!(
                    std::time::Instant::now() < deadline,
                    "round {round}: the paste never finished"
                );
            }
            h.settle();

            let mut names: Vec<String> = std::fs::read_dir(&root)
                .expect("read the folder back")
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            assert_eq!(
                names.len(),
                round + 1,
                "round {round}: Ctrl+C then Ctrl+V left {names:?}; the program said {:?}",
                h.app.notice
            );
        }

        clipboard::clear();
        crate::sandbox::remove(&root);
    }

    /// Which action each of the clipboard events becomes, and Shift+Delete among them.
    ///
    /// On Windows `egui-winit` recognises Ctrl+X, Ctrl+C, Ctrl+V, Ctrl+Insert, Shift+Insert *and
    /// Shift+Delete* itself, and queues `Event::Cut`, `Event::Copy` or `Event::Paste` for all of
    /// them -- no key event at all. So Shift+Delete arrives as a `Cut`, indistinguishable from
    /// Ctrl+X except by the modifiers, and the arm that handled cuts was guarding itself with
    /// `!m.shift`. Shift+Delete therefore did nothing whatsoever: the cut arm refused it and the
    /// `Delete` key it was hoping for never came.
    ///
    /// The last case is the one worth keeping: with Ctrl held it stays a cut. Reading a stray
    /// Ctrl+Shift+X as "delete this for ever" would be the worst mistake this program could make,
    /// so ambiguity resolves to the recoverable answer.
    #[test]
    fn the_clipboard_events_map_to_the_right_actions() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;
        {
            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
            tab.select_only(0);
        }

        let fired = |h: &mut Harness, mods: Modifiers, event: Event| -> Vec<&'static str> {
            h.app.journal = Some(Vec::new());
            h.modifiers = mods;
            h.frame(vec![event]);
            h.modifiers = Modifiers::NONE;
            h.app.journal.clone().unwrap_or_default()
        };

        let ctrl = Modifiers {
            command: true,
            ctrl: true,
            ..Modifiers::NONE
        };
        let shift = Modifiers {
            shift: true,
            ..Modifiers::NONE
        };
        let ctrl_shift = Modifiers {
            command: true,
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };

        assert!(fired(&mut h, ctrl, Event::Copy).contains(&"Copy"));
        assert!(fired(&mut h, ctrl, Event::Cut).contains(&"Cut"));
        assert!(fired(&mut h, ctrl, Event::Paste(String::new())).contains(&"Paste"));
        // Shift+Delete: a permanent delete, not a cut.
        let shift_delete = fired(&mut h, shift, Event::Cut);
        assert!(
            shift_delete.contains(&"Delete"),
            "Shift+Delete produced {shift_delete:?}"
        );
        assert!(
            !shift_delete.contains(&"Cut"),
            "and it must not also cut: {shift_delete:?}"
        );
        // With Ctrl held it is a cut, whatever else is down.
        let both = fired(&mut h, ctrl_shift, Event::Cut);
        assert!(both.contains(&"Cut"), "Ctrl+Shift+X produced {both:?}");
        assert!(
            !both.contains(&"Delete"),
            "and it must never delete: {both:?}"
        );
    }

    /// A right drag asks even inside the folder the files are already in.
    ///
    /// A left drag there means "move this to where it already is", which is nothing, and dropping
    /// a folder into itself is nothing whatever the button. But right-dragging a file onto its own
    /// folder is how Explorer is asked for a copy of it, and filtering that out before the question
    /// was asked meant a right drag inside a folder did nothing at all -- which is the most obvious
    /// way anybody would try the gesture.
    #[test]
    fn a_right_drag_can_land_in_the_folder_it_started_in() {
        let here = std::path::PathBuf::from(r"C:\Temp");
        let file = here.join("one.txt");
        let elsewhere = std::path::PathBuf::from(r"C:\Other\two.txt");

        // A left drag inside the same folder has nothing to do.
        assert!(App::droppable(vec![file.clone()], &here, false).is_empty());
        // The same drag with the right button is a question worth asking.
        assert_eq!(
            App::droppable(vec![file.clone()], &here, true),
            vec![file.clone()]
        );
        // A folder dropped into itself is nothing either way.
        assert!(App::droppable(vec![here.clone()], &here, true).is_empty());
        assert!(App::droppable(vec![here.clone()], &here, false).is_empty());
        // And anything from somewhere else is fine with either button.
        assert_eq!(
            App::droppable(vec![elsewhere.clone()], &here, false),
            vec![elsewhere.clone()]
        );
        // A mixed batch keeps what it can.
        assert_eq!(
            App::droppable(vec![file, elsewhere.clone()], &here, false),
            vec![elsewhere]
        );
    }

    /// The highlight marks the folder a drop would land in, and nothing that takes no drop.
    ///
    /// A drop can go into the folder being shown or into any folder row in it, and which one it
    /// will be is the thing worth showing. Lighting up the whole pane while the pointer sits on a
    /// subfolder promises the wrong destination — and so does lighting up the column header,
    /// which sorts, or the status line, which counts. The listing is the target.
    ///
    /// Driven by setting the hover point directly, because that is what a drag does to it from
    /// wherever it was started: the OLE callbacks write it and the frame reads it.
    #[test]
    fn the_drop_highlight_marks_the_row_and_not_the_pane() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;

        let rows = h.app.panes[0].drop_rows.clone();
        let (row, _) = rows
            .iter()
            .find(|(_, path)| path.file_name().is_some_and(|n| n == "src"))
            .expect("`src` should be one of the folder rows");
        let scale = h.ctx.pixels_per_point();

        // Over the row: the preview covers the row.
        let at = row.center();
        h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
        let drawn = h.app.preview_rect_for_tests(pane, scale);
        assert_eq!(
            drawn,
            Some(*row),
            "over a folder row the highlight has to be that row"
        );

        // Away from any row: the listing, since that is where the drop would go.
        let pane_rect = h.app.panes[0].rect;
        let listing = h.app.panes[0].drop_area;
        assert!(
            listing.top() > pane_rect.top() && listing.bottom() < pane_rect.bottom(),
            "the listing has to stop short of the header above it and the status line below:              listing {listing:?} in pane {pane_rect:?}"
        );
        let below = rows.iter().map(|(r, _)| r.bottom()).fold(f32::MIN, f32::max);
        if below + 4.0 < listing.bottom() {
            let at = egui::pos2(listing.center().x, below + 2.0);
            h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
            assert_eq!(
                h.app.preview_rect_for_tests(pane, scale),
                Some(listing),
                "away from a row the highlight is the listing, whose folder takes the drop"
            );
        }

        // Over the column header, which sorts rather than receives: no highlight, because
        // there is no drop to promise there.
        let at = egui::pos2(listing.center().x, listing.top() - 6.0);
        h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
        assert_eq!(
            h.app.preview_rect_for_tests(pane, scale),
            None,
            "the column header is not a drop target"
        );

        // No drag, no highlight.
        h.app.drop_hover = None;
        assert_eq!(h.app.preview_rect_for_tests(pane, scale), None);
    }

    /// A drag in flight keeps asking for frames, and ends by re-reading what a move emptied.
    ///
    /// This is the whole reason the drag runs on a thread of its own. While one is running the
    /// pointer belongs to OLE: not one mouse or keyboard event reaches winit, so nothing in
    /// egui's own event flow would ever ask for a repaint — and with no repaint there is no
    /// highlight of the folder the drop will land in and no sign of the selection the drag just
    /// made. Feedback that only appears once the gesture is over is not feedback.
    #[test]
    fn a_drag_in_flight_keeps_the_window_painting() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;

        // Down to a window that has stopped asking for frames, which is the baseline the
        // assertion below needs: a freshly opened one is still finishing its icons and its
        // animations, and against *that* every frame looks like a repaint somebody wanted.
        assert!(h.quiesce(), "the window should settle into asking for nothing");

        let (drag, finish) = crate::shell::dnd::Drag::pretend();
        h.app.file_drag = Some((pane, drag));

        for _ in 0..3 {
            h.frame(Vec::new());
            assert!(
                h.ctx.has_requested_repaint(),
                "a drag in flight has to keep the frames coming, or nothing about it is visible"
            );
        }

        // A move took the files out of this folder and OLE does not say which, so it is re-read.
        finish
            .send(Some(crate::shell::clipboard::Effect::Move))
            .unwrap();
        h.frame(Vec::new());
        assert!(h.app.file_drag.is_none(), "the drag is over and let go of");
        assert!(
            h.take_journal().contains(&"Refresh"),
            "and the folder the files left is re-read"
        );
    }

    /// A folder that changed on disk is re-read without blanking what is on screen.
    ///
    /// `Tab::refresh` drops the listing, which is right for F5 — somebody asked, and a moment
    /// of "Reading..." is the honest answer. It is wrong for a watcher: a build writing into the
    /// folder would flash the pane on every file. So the old rows stay up until the new ones
    /// land, and the selection comes across with them.
    #[test]
    fn a_changed_folder_is_re_read_without_blanking_it() {
        let mut h = Harness::new();
        h.settle();
        let path = h.tab(0).path.clone();
        assert!(h.tab(0).order.len() > 2, "the crate root has rows");
        h.app.panes[0].tab_mut().select_only(1);
        let chosen = {
            let tab = h.tab(0);
            tab.entry_at(1)
                .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
        };
        assert!(chosen.is_some(), "and a row to select");

        h.app.folder_changed(&path);
        assert!(
            h.tab(0).dir.is_some(),
            "the rows on screen have to stay on screen"
        );
        assert!(
            h.tab(0).awaiting.is_some(),
            "and a fresh read has to be on its way"
        );
        assert!(
            h.app.loader.cached(&path).is_none(),
            "with the cached copy dropped, or the re-read hands back what it already had \
             and the change is never seen"
        );

        h.settle();
        assert!(h.tab(0).dir.is_some(), "the re-read landed");
        assert_eq!(
            h.tab(0).selected_count,
            1,
            "and what was selected is still selected"
        );
        let after = {
            let tab = h.tab(0);
            tab.cursor
                .and_then(|at| tab.entry_at(at))
                .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
        };
        assert_eq!(after, chosen, "and it is the same row, by name");
    }

    /// The Bookmarks group lights up for a drag, and only over itself.
    ///
    /// Dropping a folder there pins it, which is a real destination and needs to look like one.
    #[test]
    fn the_bookmarks_group_shows_a_drop_target() {
        let mut h = Harness::new();
        h.settle();
        let rect = h
            .app
            .bookmarks_rect
            .expect("the Bookmarks group is on screen");
        let scale = h.ctx.pixels_per_point();
        let put = |h: &mut Harness, at: egui::Pos2| {
            h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
        };

        put(&mut h, rect.center());
        assert_eq!(h.app.bookmarks_preview(scale), Some(rect));

        // Below the group, in Places: pinning is not what a drop there would mean.
        put(&mut h, egui::pos2(rect.center().x, rect.bottom() + 40.0));
        assert_eq!(h.app.bookmarks_preview(scale), None);

        h.app.drop_hover = None;
        assert_eq!(h.app.bookmarks_preview(scale), None);
    }

    /// A second drag picks files up, and a third, and every one after that.
    ///
    /// The release that ends a drag is consumed by `DoDragDrop`'s own loop and never reaches
    /// this window, so egui went on believing the button was held — and a press arriving while
    /// a button is already down starts no drag. One drag per window, then nothing, until some
    /// unrelated click happened to put the state right.
    ///
    /// The sequence below is the real one: press, travel, *no release*, the drag ends by
    /// itself. Every earlier test released the button, which is precisely why a suite of them
    /// stayed green while dragging twice did not work.
    #[test]
    fn a_second_drag_still_picks_the_files_up() {
        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;
        let body = h.app.panes[0].rect;

        for round in 0..3 {
            // Just past the icon, which is where a row's name starts and where a drag of the
            // file rather than a rubber band begins.
            let from = pos2(body.left() + 46.0, h.row_center(0, round).y);
            let done = h.drag_and_hold(from, pos2(from.x + 60.0, from.y + 90.0));
            assert!(
                done.contains(&"DragOut"),
                "round {round}: a drag of a name has to pick the file up, got {done:?}"
            );

            // The drag a real window would have started, and its end. This is the only step
            // the platform does for us and the harness cannot.
            let (drag, finish) = crate::shell::dnd::Drag::pretend();
            h.app.file_drag = Some((pane, drag));
            finish.send(None).unwrap();
            h.frame(Vec::new());
            h.wait();
        }
    }

    /// Only the two buttons that mean something drag, and the thumb buttons navigate.
    ///
    /// A middle-button or thumb-button drag over the listing used to pick files up and start an
    /// OLE drag, because `drag_started()` without a button is true for any of them.
    #[test]
    fn only_the_left_and_right_buttons_drag_and_the_thumbs_navigate() {
        use egui::PointerButton as B;

        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;
        let body = h.app.panes[0].rect;

        // A middle-button drag across a row does nothing at all.
        for button in [B::Middle, B::Extra1, B::Extra2] {
            h.app.journal = Some(Vec::new());
            let from = egui::pos2(body.left() + 60.0, body.top() + 60.0);
            h.frame(vec![Event::PointerMoved(from)]);
            h.frame(vec![Event::PointerButton {
                pos: from,
                button,
                pressed: true,
                modifiers: Modifiers::NONE,
            }]);
            for step in 1..6 {
                h.frame(vec![Event::PointerMoved(from + vec2(0.0, step as f32 * 12.0))]);
            }
            let drawn = h.app.pane_mut(pane).expect("the pane").tab().band.is_some();
            let journal = h.app.journal.clone().unwrap_or_default();
            h.frame(vec![Event::PointerButton {
                pos: from,
                button,
                pressed: false,
                modifiers: Modifiers::NONE,
            }]);
            assert!(
                !drawn,
                "{button:?} drew a selection band; only the left and right buttons should"
            );
            assert!(
                !journal.contains(&"DragOut"),
                "{button:?} started a file drag: {journal:?}"
            );
        }

        // The thumb buttons navigate instead.
        h.app.journal = Some(Vec::new());
        h.frame(vec![Event::PointerButton {
            pos: body.center(),
            button: B::Extra1,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        assert!(
            h.app.journal.clone().unwrap_or_default().contains(&"Back"),
            "the first thumb button should go back: {:?}",
            h.app.journal
        );
        h.app.journal = Some(Vec::new());
        h.frame(vec![Event::PointerButton {
            pos: body.center(),
            button: B::Extra2,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        assert!(
            h.app.journal.clone().unwrap_or_default().contains(&"Forward"),
            "the second thumb button should go forward: {:?}",
            h.app.journal
        );
    }

    /// A new folder arrives selected, with its name open for editing.
    ///
    /// `New folder` on its own is half a gesture: nobody wants a folder called `New folder`, and
    /// in Explorer creating one and naming it is a single action. Which name to open is the
    /// shell's answer rather than this program's guess -- ask for `New folder` when one already
    /// exists and what appears is `New folder (2)` -- so it comes back through
    /// `IFileOperationProgressSink`, and this is the test that it comes back at all.
    #[test]
    #[cfg(windows)]
    fn a_new_folder_opens_its_name_for_editing() {
        let _serialised = crate::shell::serialised();
        // The whole point is that the *shell* picks the name, so the shell has to be asked.
        // Inside `target/sandbox`; see `crate::shell::ops::FOR_REAL`.
        let _for_real = crate::shell::ops::for_real();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("newfolder");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        // One already there, so the shell has to pick a different name and this cannot pass by
        // guessing "New folder".
        std::fs::create_dir_all(root.join("New folder")).expect("sandbox");

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();

        h.app.perform(&h.ctx.clone(), Action::NewFolder(pane));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            h.frame(Vec::new());
            let renaming = h
                .app
                .pane_mut(pane)
                .expect("the pane")
                .tab()
                .renaming
                .clone();
            if let Some((_, name)) = renaming {
                assert_ne!(
                    name, "New folder",
                    "the shell had to pick another name, and this is editing the old folder"
                );
                assert!(
                    name.starts_with("New folder"),
                    "the row opened for editing is `{name}`"
                );
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the new folder never opened for editing; the program said {:?}",
                h.app.notice
            );
        }

        crate::sandbox::remove(&root);
    }

    /// Dropping onto a folder row means *into that folder*.
    ///
    /// The destination used to be worked out again when the drop landed, from the pane under the
    /// pointer, which made every drop go into the folder being shown. Dragging a file onto a
    /// folder is the one gesture where that is exactly wrong. The zones the application publishes
    /// are what the OLE callbacks answer from, so they are what this checks: a folder row has to
    /// be a target of its own, and it has to win over the pane it sits in.
    #[test]
    #[cfg(windows)]
    fn a_folder_row_is_its_own_drop_target() {
        use crate::shell::dnd::Onto;

        let mut h = Harness::new();
        h.settle();
        let pane = h.app.panes[0].id;

        // The crate's own folder, which has `src` in it.
        let rows = h.app.panes[0].drop_rows.clone();
        assert!(
            !rows.is_empty(),
            "the listing reported no folder rows at all, so nothing can be dropped onto one"
        );
        let (row, folder) = rows
            .iter()
            .find(|(_, path)| path.file_name().is_some_and(|n| n == "src"))
            .expect("`src` should be one of the folder rows");

        h.app.publish_drop_targets(&h.ctx.clone());
        let scale = h.ctx.pixels_per_point();
        let at = (
            (row.center().x * scale) as i32,
            (row.center().y * scale) as i32,
        );
        let resolved = h.app.drops.resolve(at).expect("a zone under the row");
        assert_eq!(
            resolved,
            Onto::Folder(folder.clone()),
            "the row resolved to {resolved:?} rather than to the folder it shows"
        );

        // And the pane's own folder is still the target away from any row: the status line at the
        // bottom of the pane is inside the pane and below the last row.
        let pane_rect = h.app.panes[0].rect;
        let below = rows
            .iter()
            .map(|(row, _)| row.bottom())
            .fold(f32::MIN, f32::max);
        if below + 4.0 < pane_rect.bottom() {
            let at = (
                (pane_rect.center().x * scale) as i32,
                ((below + 2.0) * scale) as i32,
            );
            let resolved = h.app.drops.resolve(at).expect("the pane's own zone");
            assert_eq!(
                resolved,
                Onto::Folder(
                    h.app.pane_mut(pane).expect("the pane").tab().path.clone()
                ),
                "away from a row, a drop belongs to the folder being shown"
            );
        }
    }

    /// Right-clicking a folder with nothing in it has to give the folder's own menu.
    ///
    /// It gave nothing. `rows` is what wires a listing's clicks up, and a listing with nothing
    /// in it never reaches `rows` -- an empty folder draws one line of text over a body that
    /// nothing was listening to. Which is the one place `New folder` and `Paste` are most
    /// wanted, so it was also the least forgiving place to do nothing.
    ///
    /// Asserted on the *action*, not on the menu: whether the shell then has entries for that
    /// folder is `shell::menu`'s business and is tested there. What was missing here was
    /// anything happening at all.
    #[test]
    #[cfg(windows)]
    fn a_right_click_in_an_empty_folder_still_raises_a_menu() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("empty");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();
        assert_eq!(
            h.app.pane_mut(pane).expect("the pane").tab().order.len(),
            0,
            "the folder has to be empty, or this proves nothing"
        );

        // The middle of the listing, which in an empty folder is the middle of the message.
        let body = h.app.panes[0].rect;
        let at = body.center();
        let raised = h.click_with(at, PointerButton::Secondary, Modifiers::NONE);
        assert!(
            raised.contains(&"ShellMenu"),
            "a right click in an empty folder produced {raised:?}"
        );

        crate::sandbox::remove(&root);
    }

    /// A right click in a row asks about the file if it lands on it, and about the folder if not.
    ///
    /// A row is mostly space, and the space around a name is the listing's background as much as
    /// the gap under the last file is — so the two halves of a row are two different questions.
    /// The alternative was that the only way to reach the folder's own menu, in a folder taller
    /// than the pane, was to find a gap that might not be there.
    ///
    /// The same rule the drag already followed, and asserted the same way: by clicking at a
    /// coordinate and seeing what the program did with it.
    #[test]
    #[cfg(windows)]
    fn a_right_click_asks_about_the_file_or_the_folder_by_where_it_lands() {
        let mut h = Harness::new();
        let pane = h.app.panes[0].id;

        // The second row's name, and where it was drawn — a point on the name is a point on the
        // file, whatever the padding around it happens to be.
        let name = {
            let tab = h.tab(0);
            let entry = tab.entry_at(1).expect("the test folder has a second row");
            tab.dir.as_ref().expect("a listing").name(entry).to_owned()
        };
        let rows = h
            .ctx
            .read_response(Id::new(("rows-hit", pane)))
            .map(|r| r.rect)
            .expect("the listing takes the pointer");
        let y = h.row_center(0, 1).y;
        let ink = h
            .texts()
            .into_iter()
            .find(|(_, text)| *text == name)
            .map(|(at, _)| at)
            .unwrap_or_else(|| panic!("`{name}` is not drawn in the listing"));

        // ---- On the name: that file, and the menu for it --------------------
        let raised = h.click_with(
            pos2(ink.x + 2.0, y),
            PointerButton::Secondary,
            Modifiers::NONE,
        );
        assert!(raised.contains(&"ShellMenu"), "{raised:?}");
        assert_eq!(
            h.tab(0).selected_count,
            1,
            "a right click on a name has to select it first"
        );
        let asked = h
            .app
            .asking
            .as_ref()
            .expect("the menu is built off-thread and is still on its way")
            .items
            .clone();
        assert_eq!(asked.len(), 1, "the menu was asked about {asked:?}");
        assert!(
            asked[0].ends_with(&name),
            "the menu was asked about {asked:?} rather than about `{name}`"
        );

        // ---- In the space of a row that *is* selected ------------------------
        //
        // The exception, and the same one the drag makes: the files are picked out already, so
        // the menu is the selection's wherever in the row the click lands. Taking the selection
        // away because the pointer was between two columns would undo work rather than ask a
        // question.
        h.wait();
        // Two points into the row, which is left of the icon and so on nothing.
        let (beside_first, beside_second) = (
            pos2(rows.left() + 2.0, y),
            pos2(rows.left() + 2.0, h.row_center(0, 2).y),
        );
        let raised = h.click_with(beside_first, PointerButton::Secondary, Modifiers::NONE);
        assert!(raised.contains(&"ShellMenu"), "{raised:?}");
        assert_eq!(
            h.tab(0).selected_count,
            1,
            "a right click in a selected row's own space dropped the selection"
        );
        assert_eq!(
            h.app.asking.as_ref().expect("on its way").items.len(),
            1,
            "and it stopped being the selection's menu"
        );

        // ---- In the space of a row that is not -------------------------------
        h.wait();
        let at = beside_second;
        assert!(
            h.hovers(Id::new(("rows-hit", pane)), at),
            "the point beside the icon is not in the listing at all"
        );
        let raised = h.click_with(at, PointerButton::Secondary, Modifiers::NONE);
        assert!(raised.contains(&"ShellMenu"), "{raised:?}");
        assert_eq!(
            h.tab(0).selected_count,
            0,
            "a right click that was not on a file left one selected"
        );
        assert!(
            h.app
                .asking
                .as_ref()
                .expect("on its way")
                .items
                .is_empty(),
            "the menu is not the folder's"
        );
    }

    /// Two folders of enough files to scroll, in the crate's own `target`.
    #[cfg(windows)]
    fn tall_sandbox(name: &str, files: usize) -> PathBuf {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join(name);
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        for i in 0..files {
            std::fs::write(root.join(format!("file-{i:02}.txt")), b"x").expect("a file");
        }
        root
    }

    /// Where the listing is scrolled to survives everything except going somewhere else.
    ///
    /// Two claims, and they pull in opposite directions, which is why they are one test:
    ///
    /// - **A re-read keeps its place.** Every file operation ends in one, and so does anything
    ///   the context menu does — so a scroll that does not survive it means acting on a file
    ///   two hundred rows down and being sent back to the top to find it again.
    /// - **A different folder starts at the top.** Nothing else makes sense: row 200 of the
    ///   folder you just left is not row 200 of anything.
    #[test]
    #[cfg(windows)]
    fn a_re_read_keeps_its_place_and_a_new_folder_does_not() {
        let one = tall_sandbox("scroll-a", 80);
        let two = tall_sandbox("scroll-b", 80);

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        let go = |h: &mut Harness, path: &std::path::Path| {
            h.app.perform(
                &h.ctx.clone(),
                Action::Navigate {
                    pane,
                    path: path.to_path_buf(),
                },
            );
            h.settle();
        };
        go(&mut h, &one);

        // Down a long way, the way the wheel does it.
        h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(30.0 * crate::pane::ROW_HEIGHT);
        h.frame(Vec::new());
        h.frame(Vec::new());
        let was = h.tab(0).scroll_y;
        assert!(was > 100.0, "the listing did not scroll at all ({was})");

        // A re-read: what every file operation and every context-menu action ends in.
        h.app.perform(&h.ctx.clone(), Action::Refresh(pane));
        h.settle();
        assert_eq!(
            h.tab(0).scroll_y, was,
            "a re-read of the same folder moved the listing"
        );

        // And the right click that asks for a context menu, which is where this was reported:
        // the menu is what the user was looking at, and the listing behind it had gone back to
        // the top.
        let row = h.row_center(0, 4);
        h.click_with(row, PointerButton::Secondary, Modifiers::NONE);
        assert_eq!(
            h.tab(0).scroll_y, was,
            "the right click itself sent the listing back to the top"
        );
        // And once the menu is actually on screen, which is a few hundred milliseconds later:
        // the shell builds it off-thread, so the frames above have only asked for it.
        let waited = std::time::Instant::now();
        while h.app.menu_pending() {
            h.frame(Vec::new());
            assert!(
                waited.elapsed() < std::time::Duration::from_secs(20),
                "the builder never delivered"
            );
        }
        h.frame(Vec::new());
        assert!(h.app.menu.is_some(), "the menu is not up, so this proves nothing");
        assert_eq!(
            h.tab(0).scroll_y, was,
            "the menu appearing sent the listing back to the top"
        );
        h.app.close_menu();
        h.frame(Vec::new());
        assert_eq!(
            h.tab(0).scroll_y, was,
            "dismissing the menu sent the listing back to the top"
        );

        // Somewhere else, and back: both start at the top.
        go(&mut h, &two);
        assert_eq!(
            h.tab(0).scroll_y, 0.0,
            "a different folder opened part-way down"
        );
        go(&mut h, &one);
        assert_eq!(
            h.tab(0).scroll_y, 0.0,
            "coming back to a folder opened where the last visit left it"
        );

        // And each tab keeps its own place, which is the same fact from the other side: the
        // offset egui remembers belongs to the pane, so without this a tab coming to the front
        // shows wherever the tab before it had got to.
        h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(20.0 * crate::pane::ROW_HEIGHT);
        h.frame(Vec::new());
        h.frame(Vec::new());
        let deep = h.tab(0).scroll_y;
        assert!(deep > 100.0, "the first tab did not scroll ({deep})");

        h.app.perform(&h.ctx.clone(), Action::NavigateNewTab { pane, path: two.clone() });
        h.settle();
        assert_eq!(h.tab(0).scroll_y, 0.0, "a new tab opened part-way down");

        h.app.perform(&h.ctx.clone(), Action::ActivateTab { pane, tab: 0 });
        h.frame(Vec::new());
        h.frame(Vec::new());
        assert_eq!(
            h.tab(0).scroll_y, deep,
            "coming back to the first tab lost where it was"
        );

        crate::sandbox::remove(&one);
        crate::sandbox::remove(&two);
    }

    /// A change made by anybody re-reads the folder on its own.
    ///
    /// **This is what a context-menu action now relies on.** Invoking a shell verb used to
    /// re-read the folder unconditionally, because there is no way to be told what the verb did
    /// — so `Copy`, `Properties` and `Open with` each threw the listing away and built it again
    /// to discover that nothing had changed. Nothing does that any more, which is only correct
    /// because the folder is watched: this is the test that says the watching works, end to end,
    /// through a real `ReadDirectoryChangesW` handle and a real file appearing.
    #[test]
    #[cfg(windows)]
    fn a_change_on_disk_re_reads_the_folder_by_itself() {
        let root = tall_sandbox("watched", 3);

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();
        assert_eq!(h.tab(0).order.len(), 3, "the sandbox should have three files");

        // Somebody else's change — a shell verb, Explorer, a terminal. `std::fs`, never the
        // shell: see `shell::ops::FOR_REAL`.
        std::fs::write(root.join("arrived.txt"), b"x").expect("a fourth file");

        let waited = std::time::Instant::now();
        while h.tab(0).order.len() != 4 {
            h.frame(Vec::new());
            std::thread::sleep(std::time::Duration::from_millis(5));
            assert!(
                waited.elapsed() < std::time::Duration::from_secs(10),
                "the folder was never re-read: still {} rows",
                h.tab(0).order.len()
            );
        }

        crate::sandbox::remove(&root);
    }

    /// A folder that reads quickly says nothing about reading.
    ///
    /// `Reading…` used to be drawn the moment a folder was asked for, and a local folder comes
    /// back in single-digit milliseconds — so it was a word that flashed up and vanished on every
    /// navigation, in the exact place the listing was about to be. It now waits for
    /// [`crate::pane::SLOW_SCAN`], which is what the second half of this checks: silence is not
    /// the same thing as never saying it.
    #[test]
    #[cfg(windows)]
    fn a_quick_folder_never_says_it_is_reading() {
        let root = tall_sandbox("quick", 4);
        let reading = |h: &Harness| h.texts().iter().any(|(_, text)| text.contains("Reading"));

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        assert!(!reading(&h), "it starts by claiming to read something");

        // Somewhere it has never been, so the scan really goes to the loader rather than coming
        // straight back out of its cache.
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        // Real sleeps between frames, so the scan lands in a frame or two rather than in fifty:
        // the clock this is asserting against is egui's, which the harness advances by a
        // sixtieth per frame, so a listing that took fifty frames to arrive would be half a
        // *second* as far as the program is concerned and would be right to say so.
        let started = h.time;
        for _ in 0..12 {
            h.frame(Vec::new());
            assert!(
                !reading(&h),
                "a folder that read in under {}s said it was reading",
                crate::pane::SLOW_SCAN
            );
            if h.tab(0).dir.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        assert!(h.tab(0).dir.is_some(), "the listing never arrived");
        assert!(
            h.time - started < crate::pane::SLOW_SCAN,
            "the frames above took {:.2}s of the program's own time, which is past the \
             threshold — so the assertion inside the loop proved nothing",
            h.time - started
        );

        // And a scan that *is* slow says so. Faked by putting the clock back rather than by
        // finding a slow disk: `awaiting` is set so the frame does not start a new scan and
        // stamp the time again.
        {
            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
            tab.refresh();
            tab.awaiting = Some(u64::MAX);
            tab.asked_at = Some(h.time - crate::pane::SLOW_SCAN - 1.0);
        }
        h.frame(Vec::new());
        assert!(
            reading(&h),
            "a scan a second old still has not admitted to waiting"
        );

        crate::sandbox::remove(&root);
    }

    /// The listing keeps three rows of nothing under it, in a folder of any size.
    ///
    /// The folder's own menu — the one with `New` on it — is what you get by right-clicking a
    /// part of the listing that is not a file. In a folder taller than the pane
    /// there was no such part: every pixel from the header to the status line was a row, and the
    /// gap at the end was whatever the last row happened to leave, which was frequently nothing.
    #[test]
    #[cfg(windows)]
    fn the_listing_keeps_room_under_it_for_the_folder() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("tall");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        for i in 0..60 {
            std::fs::write(root.join(format!("file-{i:02}.txt")), b"x").expect("a file");
        }

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;
        h.app.perform(
            &h.ctx.clone(),
            Action::Navigate {
                pane,
                path: root.clone(),
            },
        );
        h.settle();

        let body = h.app.panes[0].drop_area;
        let count = h.tab(0).order.len();
        assert!(
            count as f32 * crate::pane::ROW_HEIGHT > body.height(),
            "{count} rows fit inside the pane, so this proves nothing"
        );

        // To the end of it, which is where there used to be nothing to click.
        h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(100_000.0);
        h.frame(Vec::new());
        h.frame(Vec::new());

        let tail = h
            .ctx
            .read_response(Id::new(("rows-empty", pane)))
            .map(|r| r.rect)
            .expect("nothing is listening below the last row");
        assert!(
            (tail.height() - 3.0 * crate::pane::ROW_HEIGHT).abs() < 1.0,
            "the space under the last file is {:.0} points, not three rows",
            tail.height()
        );

        let raised = h.click_with(tail.center(), PointerButton::Secondary, Modifiers::NONE);
        assert!(raised.contains(&"ShellMenu"), "{raised:?}");
        assert!(
            h.app
                .asking
                .as_ref()
                .expect("the menu is still on its way")
                .items
                .is_empty(),
            "the space under the last file gave a file's menu"
        );

        crate::sandbox::remove(&root);
    }

    /// Copy, cut, paste and delete, driven the way the keyboard drives them, on real files.
    ///
    /// The pieces are tested where they live -- `shell::clipboard` for the data object,
    /// `shell::ops` for the engine. What is only testable here is the sequence: that Ctrl+C
    /// puts the selection on the clipboard, that Ctrl+V into another folder brings it, that a
    /// cut leaves its sources alone until something pastes and *then* empties the clipboard,
    /// and that Delete goes through the shell.
    ///
    /// Only collision-free operations and a permanent delete of nothing: every case that
    /// raises a dialog is in `shell::ops`, behind `run_watching`, because a test with a modal
    /// dialog up and nobody to answer it is a test that never finishes.
    ///
    /// Ignored because it takes over the desktop's one clipboard.
    #[test]
    #[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
    #[cfg(windows)]
    fn copy_cut_paste_and_delete_end_to_end() {
        use crate::shell::clipboard::{self, Effect};

        let _serialised = crate::shell::serialised();
        // The one test allowed to hand a job to the real shell, and it says so out loud. Everything
        // below happens inside `target/sandbox/keys`; `crate::shell::ops::FOR_REAL` documents what
        // went wrong when this was the default rather than an opt-in.
        let _for_real = crate::shell::ops::for_real();
        // This thread has to be an OLE apartment before it can own the clipboard. In the real
        // program `main` does it before anything else; a test harness does not, and without it
        // `OleSetClipboard` simply refuses and a copy puts nothing anywhere.
        crate::shell::init();
        // Another clipboard test in this process may have left a live data object on the one
        // clipboard the desktop has; this lets go of it and answers what comes of that.
        crate::shell::clipboard::settle_for_tests();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("keys");
        crate::sandbox::remove(&root);
        let from = root.join("from");
        let into = root.join("into");
        std::fs::create_dir_all(&from).expect("sandbox");
        std::fs::create_dir_all(&into).expect("sandbox");
        std::fs::write(from.join("copied.txt"), b"c").expect("write");
        std::fs::write(from.join("moved.txt"), b"m").expect("write");
        std::fs::write(from.join("binned.txt"), b"b").expect("write");
        // For the context menu's Copier and Coller, which name their items and their destination
        // rather than reading either off a pane.
        std::fs::write(from.join("menu-copied.txt"), b"n").expect("write");
        let inner = from.join("target");
        std::fs::create_dir_all(&inner).expect("sandbox");

        let mut h = Harness::new();
        let pane = h.app.panes[0].id;

        /// Show a folder, and wait for its listing.
        fn show(h: &mut Harness, pane: PaneId, path: &std::path::Path) {
            h.app.perform(
                &h.ctx.clone(),
                Action::Navigate {
                    pane,
                    path: path.to_path_buf(),
                },
            );
            h.settle();
        }

        /// Select one file by name in the shown folder.
        ///
        /// By *display position*, which is what `select_only` takes -- not by entry index.
        /// The two are only the same in an unsorted, unfiltered listing, and a version of this
        /// that passed the entry index selected the wrong row or none at all.
        fn select(h: &mut Harness, pane: PaneId, name: &str) {
            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
            let dir = tab.dir.clone().expect("a listing");
            let position = (0..tab.order.len())
                .find(|p| tab.entry_at(*p).is_some_and(|e| dir.name(e) == name))
                .unwrap_or_else(|| panic!("`{name}` is not in the listing"));
            tab.select_only(position);
            assert_eq!(
                tab.selection_paths().len(),
                1,
                "`{name}` should be the one thing selected"
            );
        }

        /// Drain this thread's message queue, which is what a real window does constantly.
        #[cfg(windows)]
        fn pump() {
            use windows::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
            };
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }

        /// Run frames until every file operation has finished.
        fn settle_ops(h: &mut Harness) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while h.app.ops.in_progress().is_some() {
                h.frame(Vec::new());
                assert!(
                    std::time::Instant::now() < deadline,
                    "a file operation never finished"
                );
            }
            h.settle();
        }

        // ---- Ctrl+C, then Ctrl+V somewhere else ----
        show(&mut h, pane, &from);
        select(&mut h, pane, "copied.txt");
        h.app.perform(&h.ctx.clone(), Action::Copy(pane));
        let on_clipboard = clipboard::get().unwrap_or_else(|| {
            panic!(
                "Ctrl+C put nothing on the clipboard; the program said {:?}",
                h.app.notice
            )
        });
        assert_eq!(on_clipboard.effect, Effect::Copy);

        show(&mut h, pane, &into);
        h.app.perform(&h.ctx.clone(), Action::Paste(pane));
        settle_ops(&mut h);
        assert!(into.join("copied.txt").is_file(), "the paste should have copied it; the program said {:?}", h.app.notice);
        assert!(from.join("copied.txt").is_file(), "and left the original");
        assert!(
            clipboard::has_files(),
            "a copy stays on the clipboard, so it can be pasted twice"
        );

        // ---- Ctrl+X, then Ctrl+V ----
        show(&mut h, pane, &from);
        select(&mut h, pane, "moved.txt");
        pump();
        h.app.perform(&h.ctx.clone(), Action::Cut(pane));
        assert_eq!(
            clipboard::get().map(|p| p.effect),
            Some(Effect::Move),
            "a cut has to say so, or a paste would copy; the program said {:?}",
            h.app.notice
        );
        assert!(
            from.join("moved.txt").is_file(),
            "a cut moves nothing on its own -- that is the whole difference from a move"
        );
        assert!(
            !h.app.cut.is_empty(),
            "and the sources have to be marked, so they can be drawn as pending"
        );

        show(&mut h, pane, &into);
        h.app.perform(&h.ctx.clone(), Action::Paste(pane));
        settle_ops(&mut h);
        assert!(into.join("moved.txt").is_file(), "the paste should have moved it; the program said {:?}", h.app.notice);
        assert!(!from.join("moved.txt").exists(), "and taken it out of the source");
        assert!(
            !clipboard::has_files(),
            "a cut that has been pasted has to leave the clipboard empty, or Ctrl+V again \
             would move files that are no longer where it says"
        );
        assert!(h.app.cut.is_empty(), "and nothing is pending any more");

        // ---- The context menu's Copier and Coller ----
        //
        // The pair the shell's `copy` and `paste` verbs are redirected into -- see
        // `ours_rather_than_the_shell_s`. They differ from Ctrl+C and Ctrl+V in exactly one way and
        // it is the thing worth a test: they act on what the *menu* named. So the pane stays on
        // `from` throughout, and the paste has to land in the selected folder rather than in the
        // folder being shown -- which is what `Action::Paste(pane)` would have done, and what a
        // redirect that reached for the pane instead of the entry would silently do.
        show(&mut h, pane, &from);
        h.app.perform(
            &h.ctx.clone(),
            Action::CopyItems(vec![from.join("menu-copied.txt")]),
        );
        assert_eq!(
            clipboard::get().map(|p| p.effect),
            Some(Effect::Copy),
            "the menu's Copier put nothing on the clipboard; the program said {:?}",
            h.app.notice
        );
        h.app
            .perform(&h.ctx.clone(), Action::PasteIntoFolder(inner.clone()));
        settle_ops(&mut h);
        assert!(
            inner.join("menu-copied.txt").is_file(),
            "the menu's Coller should have pasted into the selected folder; the program said {:?}",
            h.app.notice
        );
        assert!(
            from.join("menu-copied.txt").is_file(),
            "and left the original where it was"
        );
        // Nothing landed in the folder the pane was showing. A paste that reached for the pane
        // would have copied the file onto itself and left a `menu-copied (2).txt` beside it.
        let strays: Vec<String> = std::fs::read_dir(&from)
            .expect("read the source folder back")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("menu-copied") && name != "menu-copied.txt")
            .collect();
        assert!(
            strays.is_empty(),
            "the paste also went into the folder the pane was showing: {strays:?}"
        );
        clipboard::clear();

        // ---- Delete ----
        show(&mut h, pane, &from);
        select(&mut h, pane, "binned.txt");
        h.app.perform(
            &h.ctx.clone(),
            Action::Delete {
                pane,
                permanent: false,
            },
        );
        settle_ops(&mut h);
        assert!(
            !from.join("binned.txt").exists(),
            "Delete should have sent it to the Recycle Bin"
        );

        clipboard::clear();
        crate::sandbox::remove(&root);
    }

    #[test]
    #[ignore = "measures the whole process; run explicitly, single-threaded"]
    fn what_the_shell_menu_costs() {
        // Not an assertion — a measurement, and the answer to where the memory in this
        // process actually is. Opening a folder's context menu makes Windows load every
        // installed shell extension into *this* process: an archiver, a screenshot tool, a
        // cloud client, a rename tool, whatever else. Each is a DLL with its own heap, none
        // of them is ever unloaded, and none of it is visible to the Rust allocator counter.
        let mut h = Harness::new();
        h.settle();
        let (private_before, gdi_before, user_before) = process_memory();
        let heap_before = live_heap();
        println!(
            "before the menu: private {:>7} KB   heap {:>7} KB   gdi {gdi_before:>4}   \
             user {user_before:>4}",
            private_before / 1024,
            heap_before / 1024
        );

        // Five times over, because the answer that matters is whether it *repeats*: a DLL
        // loads once and stays, so a one-off cost of a few megabytes is very different from
        // a few megabytes every time somebody right-clicks.
        let mut last = private_before as isize;
        for round in 1..=5 {
            h.app.open_folder_menu(&h.ctx.clone());
            // The shell's entries come from a thread now, and it is exactly the extensions
            // this test is weighing that make it slow — so wait for them rather than
            // measuring a menu that never got any.
            let waited = std::time::Instant::now();
            while h.app.menu.is_none() {
                h.frame(Vec::new());
                if waited.elapsed() > std::time::Duration::from_secs(20) {
                    panic!("the menu builder never answered");
                }
            }
            for _ in 0..20 {
                h.frame(Vec::new());
            }
            h.settle();
            h.app.close_menu();
            h.frame(Vec::new());

            let (private, gdi, user) = process_memory();
            println!(
                "menu {round}:  private {:>7} KB  ({:+} KB this time)   gdi {gdi:>4}   user {user:>4}",
                private / 1024,
                (private as isize - last) / 1024
            );
            last = private as isize;
        }

        let (private, gdi, user) = process_memory();
        let heap = live_heap();
        println!(
            "after the menus: private {:>7} KB   heap {:>7} KB   gdi {gdi:>4}   user {user:>4}",
            private / 1024,
            heap / 1024
        );
        println!(
            "the menu cost:   private {:+} KB   heap {:+} KB   gdi {:+}   user {:+}",
            (private as isize - private_before as isize) / 1024,
            (heap - heap_before) / 1024,
            gdi as i64 - gdi_before as i64,
            user as i64 - user_before as i64
        );
    }

    #[test]
    #[ignore = "measures the whole process; run explicitly, single-threaded"]
    fn closing_a_tab_gives_its_listing_back() {
        // The cache has a budget; a *tab* does not. Every open tab pins its own listing
        // through an `Arc`, which is why the cache's own figure understates what is held —
        // and it is the one shape of browsing that grows without a bound: a tab per folder.
        //
        // That much is by design. What would be a leak is a tab that is closed and does not
        // give the memory back, so this opens a pile of them, closes them all, and looks.
        let dirs = folders(Path::new(env!("CARGO_MANIFEST_DIR")), 40);
        assert!(dirs.len() >= 20, "need folders to open tabs on");

        let mut h = Harness::new();
        h.settle();
        let before = live_heap();

        let pane = h.app.panes[0].id;
        for dir in &dirs {
            h.app.perform(&h.ctx.clone(), Action::NavigateNewTab {
                pane,
                path: dir.clone(),
            });
            h.settle();
        }
        let tabs = h.app.panes[0].tabs.len();
        let open = live_heap();
        println!(
            "{tabs} tabs open: {:+} KB  ({} KB a tab)",
            (open - before) / 1024,
            (open - before) / 1024 / tabs as isize
        );

        // Close them from the back, leaving the one the window started with.
        while h.app.panes[0].tabs.len() > 1 {
            let last = h.app.panes[0].tabs.len() - 1;
            h.app
                .perform(&h.ctx.clone(), Action::CloseTab { pane, tab: last });
            h.settle();
        }
        // And empty the cache, which legitimately still holds what the tabs were showing.
        for dir in &dirs {
            h.app.loader.invalidate(dir);
        }
        h.settle();
        let closed = live_heap();
        println!(
            "after closing: {:+} KB on the baseline (was {:+} KB with {tabs} tabs open)",
            (closed - before) / 1024,
            (open - before) / 1024
        );

        // A tab's listing, its display order and its selection are the whole cost, and all
        // three go with it. Half a megabyte of slack for the cache's own bookkeeping.
        assert!(
            closed - before < (1 << 19),
            "closing every tab left {:+} KB behind -- something is holding listings after \
             their tab is gone",
            (closed - before) / 1024
        );
    }

    #[test]
    #[ignore = "measures the whole process; run explicitly, single-threaded"]
    fn browsing_hundreds_of_folders_settles_rather_than_grows() {
        // Somewhere else with `YAFE_WALK`, which is how the *ceiling* gets measured: the
        // caches are sized in entries, so a walk of this repository's small folders and a
        // walk of `C:\Windows` settle at very different heights.
        let root = std::env::var_os("YAFE_WALK")
            .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
        let dirs = folders(&root, 600);
        println!("walking {} ({} folders)", root.display(), dirs.len());
        assert!(
            dirs.len() > 120,
            "need a few hundred real folders to see a plateau, found {}",
            dirs.len()
        );

        let mut h = Harness::new();
        let browse = |h: &mut Harness, dir: &Path| {
            h.app.panes[0].tab_mut().navigate(dir.to_path_buf());
            h.settle();
        };

        // The first stretch is setup, not growth: the fonts, the glyph atlas, the loader's
        // threads and the shell's own caches are all paid for once.
        for dir in dirs.iter().take(20) {
            browse(&mut h, dir);
        }
        let (heap0, private0, gdi0, user0) = {
            let (p, g, u) = process_memory();
            (live_heap(), p, g, u)
        };
        println!(
            "baseline at 20 folders: heap {:>7} KB   private {:>7} KB   gdi {gdi0:>5}   user {user0:>4}",
            heap0 / 1024,
            private0 / 1024
        );

        let mut samples = Vec::new();
        for (i, dir) in dirs.iter().enumerate().skip(20) {
            browse(&mut h, dir);
            if (i + 1) % 50 == 0 {
                let heap = live_heap();
                let (private, gdi, user) = process_memory();
                let (dirs, entries) = h.app.loader.held();
                samples.push((heap, private as isize));
                println!(
                    "{:>4} folders:  heap {:+8} KB   private {:+8} KB   gdi {gdi:>5}   \
                     user {user:>4}   cache {dirs:>3}/{entries:>7} = {:>4} B/entry",
                    i + 1,
                    (heap - heap0) / 1024,
                    (private as isize - private0 as isize) / 1024,
                    if entries > 0 {
                        (heap - heap0) / entries as isize
                    } else {
                        0
                    }
                );
            }
        }

        // The caches have budgets and are meant to reach them. What must not happen is the
        // second half of the walk costing as much as the first — that is the signature of
        // something that never lets go.
        let mid = samples.len() / 2;
        let (heap_mid, private_mid) = samples[mid];
        let (heap_end, private_end) = samples[samples.len() - 1];
        let folders_after = (samples.len() - mid) * 50;
        println!(
            "over the last {folders_after} folders: heap {:+} KB, private {:+} KB",
            (heap_end - heap_mid) / 1024,
            (private_end - private_mid) / 1024
        );

        // A megabyte of slack over hundreds of folders, which is arena reuse and allocator
        // fragmentation rather than anything held.
        let slack = 1 << 20;
        assert!(
            heap_end - heap_mid < slack,
            "the heap is still growing after the cache should have settled: {:+} KB over \
             {folders_after} folders",
            (heap_end - heap_mid) / 1024
        );
        assert!(
            private_end - private_mid < 4 * slack,
            "the process is still growing after the caches should have settled: {:+} KB \
             over {folders_after} folders -- something outside the Rust heap is being kept",
            (private_end - private_mid) / 1024
        );
    }
}
