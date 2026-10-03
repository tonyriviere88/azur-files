//! Tabs and panes: what is being looked at, and where.
//!
//! A [`Tab`] is a place plus how it is being viewed — the sort, the filter, the
//! selection, the scroll position and its own navigation history. A [`Pane`] is a
//! stack of tabs with one of them showing. The tree that arranges panes on screen
//! lives in [`crate::dock`].
//!
//! The listing itself is an `Arc<Dir>` borrowed from the loader's cache, so two
//! tabs on the same folder share one copy of the data and still sort it
//! differently: the order is a per-tab `Vec<u32>` of indices into a `Dir` neither
//! of them owns.

use std::path::PathBuf;
use std::sync::Arc;

use crate::fs::{display_name, sort, Column, Dir};

/// Identifies a pane for the lifetime of the window. Never reused, so a stale
/// reference resolves to nothing rather than to the wrong pane.
pub type PaneId = u32;

/// How tall a row is, in points.
///
/// A deliberate departure from `tokens::row::TABLE` (36px). Azur's table row is
/// sized for a form; a file listing is read by scanning hundreds of lines, and
/// every point of row height is a file that did not fit on screen. 24 is dense
/// enough to show ~40 rows in a half-height pane and still leave the 14px glyph
/// and 14px body text their breathing room.
pub const ROW_HEIGHT: f32 = 24.0;

/// How a flattened folder is shown: as one list, or as the tree it came from.
///
/// **Both are the same listing.** [`crate::fs::scan::scan_deep`] walks the folder once into a
/// `Dir` whose names are paths relative to it, and the mode decides only how the display order is
/// built over it — [`sort::build_order`] for one, [`sort::build_tree_order`] for the other. So
/// switching costs a re-sort and never a second walk, which on a tree big enough to wait for is
/// the difference between instant and seconds.
///
/// A **window preference**, saved in the settings and defaulting to [`Self::List`]: which of the
/// two you want is a habit rather than a fact about the folder you happen to be in. The copy on
/// each [`Tab`] is the mode that tab's order is currently built in, which is what
/// [`Tab::rebuild_order`] reads; `App` owns the preference and keeps them in step.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FlatMode {
    /// Every file under the folder as one flat listing, its name the path down to it.
    ///
    /// The one the button has always produced, and the default: it is the mode that answers
    /// "where is that file" and "how much of this is build output", which is what people flatten
    /// a folder to ask.
    #[default]
    List,
    /// The same rows as the hierarchy they came out of: indented, with a twisty on every folder
    /// that has something in it.
    ///
    /// What this adds over browsing folder by folder is that it is **one read**: the whole tree is
    /// already in memory, so opening and closing folders in it costs a re-sort rather than a
    /// directory read each, and two folders five levels apart can be on screen together.
    Tree,
}

impl FlatMode {
    /// The two, in the order the menu lists them.
    pub const ALL: [Self; 2] = [Self::List, Self::Tree];

    pub fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Tree => "Tree",
        }
    }

    /// For the settings file, which is a `key=value` text file people are meant to be able to fix
    /// by hand — so the values are words rather than numbers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Tree => "tree",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str().eq_ignore_ascii_case(text))
    }
}

/// A rubber-band selection in progress.
///
/// Both corners are in *content* coordinates — distance from the top of the whole
/// listing, not from the top of the window — so the band stays anchored to the rows
/// it was drawn over while the view auto-scrolls underneath it.
pub struct Band {
    pub anchor: egui::Pos2,
    pub current: egui::Pos2,
    /// What was selected before the band started, for an additive gesture. Empty when
    /// the band replaces the selection.
    ///
    /// Holding Ctrl or Shift *adds* what the band covers to what was already there —
    /// it does not invert it. Inverting is what a Ctrl-*click* does, and Explorer keeps
    /// the two distinct, so a band dragged back over something already selected does
    /// not quietly turn it off.
    pub base: Vec<bool>,
}

impl Band {
    /// The rows the band currently covers, as positions in the display order.
    ///
    /// Only the vertical span matters: a row in the details view spans the full width,
    /// so a band that crosses its `y` crosses the row, however narrow it is. This is
    /// what Explorer's details view does too.
    pub fn rows(&self, count: usize) -> std::ops::Range<usize> {
        if count == 0 {
            return 0..0;
        }
        let (top, bottom) = (
            self.anchor.y.min(self.current.y),
            self.anchor.y.max(self.current.y),
        );
        let first = (top / ROW_HEIGHT).floor().max(0.0) as usize;
        let last = (bottom / ROW_HEIGHT).floor().max(0.0) as usize;
        if first >= count {
            return 0..0;
        }
        first..(last + 1).min(count)
    }
}

/// One place being looked at.
pub struct Tab {
    /// The folder. Empty means "This PC".
    pub path: PathBuf,
    /// The deepest folder the breadcrumb still shows: [`Tab::path`], or something under it.
    ///
    /// Going **up** leaves this alone, so the trail you came down stays on the bar and the
    /// folder you just left is one click away instead of something to go and find again.
    /// Going anywhere off the trail replaces it.
    ///
    /// Explorer trims the bar to the folder you are in, and then the only way back down is
    /// the chevron beside it — which means reading a menu to find a name that was on screen
    /// a moment ago. This keeps it on screen. The folder actually being *shown* is still
    /// obvious: it is the bold segment, wherever along the trail it sits.
    pub trail: PathBuf,
    /// What the tab strip shows.
    pub title: String,

    /// Everywhere this tab has been, and where in that trail it is. Back and
    /// Forward move `at`; navigating anywhere else truncates the future, which is
    /// how every browser and shell behaves.
    pub history: Vec<PathBuf>,
    pub at: usize,

    /// The listing, once it arrives.
    pub dir: Option<Arc<Dir>>,
    /// The request this tab is waiting for. Any other token is a stale answer.
    pub awaiting: Option<u64>,
    /// When the folder was asked for, so a wait long enough to notice can say so and a wait
    /// nobody could notice says nothing. See [`SLOW_SCAN`].
    pub asked_at: Option<f64>,

    /// This view of this folder, so an answer that arrives after the folder is gone can be
    /// recognised and thrown away. A fresh one on every move.
    pub view: u64,
    /// The icon each row's own file carries, by entry index — [`crate::shell::icons::UNASKED`]
    /// until asked about.
    ///
    /// **This is the folder-scoped rule in one field.** A per-file icon used to live in a map
    /// keyed by `PathBuf` in the icon service, which meant browsing a folder of executables
    /// left one heap-allocated path per file behind for the rest of the session. Here it is
    /// four bytes per row in a `Vec` the tab owns: sized when the listing lands, dropped when
    /// the tab moves. Leave the folder and every byte of it goes with it.
    pub file_icons: Vec<i32>,
    /// Where a shortcut row points, by entry index — see [`crate::shell::links`].
    ///
    /// A map rather than a column, because unlike an icon this is only ever wanted for a
    /// handful of rows: a `.lnk` or a reparse point. A `Vec` would be an `Option<String>` per
    /// row, which on a flattened tree of 200,000 is megabytes to say "not a shortcut" 199,990
    /// times.
    ///
    /// **A key that is present means asked.** `None` is an answer as much as `Some` is — an
    /// unreadable shortcut, or one pointing at something with no path — and both stop the row
    /// asking again. Dropped with the folder, exactly like [`Tab::file_icons`].
    pub links: std::collections::HashMap<u32, Option<String>>,

    /// What git says about this folder, once it has been asked.
    ///
    /// **The same lifetime as the listing beside it**, which is the whole design: asked for when a
    /// listing lands, replaced when the next one does, and gone when the tab moves. Every event that
    /// re-reads the folder — an operation, `F5`, the watcher, a navigation — re-asks git, because it
    /// is the same event. Nothing here outlives what it describes, so nothing here can be stale
    /// while looking authoritative. See [`crate::git`].
    ///
    /// `None` means one of three things the window treats identically: not asked yet, not a
    /// repository, or git could not be reached.
    pub git: Option<std::sync::Arc<crate::git::Repo>>,
    /// Whether this view of this folder has already asked, so a folder outside a repository is not
    /// re-asked sixty times a second for as long as it is on screen.
    pub git_asked: bool,
    /// Whether the answer has come back — whatever it was.
    ///
    /// `git` alone cannot say: `None` is both "not asked yet" and "asked, and there is no repository
    /// here". The window does not care about the difference anywhere it *draws* git, because both mean
    /// no furniture — but the `@git` filter has to, and its two answers are opposite. Not answered
    /// yet is a question that cannot be evaluated, so it keeps every row and runs again when the answer
    /// lands; answered with nothing means there are no changes here, so it keeps none. Without the
    /// distinction, every refresh of a filtered listing would empty it for a frame or two.
    pub git_answered: bool,
    /// When the last answer landed, in [`egui::InputState::time`] — `None` before the first one.
    ///
    /// **What tells our own hand from somebody else's.** `git::read` can write the repository's
    /// index as a side effect — see its own doc — and that write lands inside `.git`, which is
    /// watched so that a commit made elsewhere is noticed. The watch cannot tell which file changed
    /// — see [`crate::watch`] — so without this, our own write reads as an external change, which
    /// asks git again, which writes again, forever: a folder that never stops spawning `git`
    /// underneath it, on its own, whether or not anyone is looking at the window. See
    /// [`crate::app::App::collect_changes`], which is where this is read.
    pub git_settled_at: Option<f64>,

    /// Display order: indices into `dir.entries`, sorted and filtered.
    pub order: Vec<u32>,
    /// One per row of [`Tab::order`], and only in a tree: how far in the row is drawn, and how many
    /// folders are merged into its name. Empty in every other listing. See [`sort::TreeRow`].
    ///
    /// Filled by the same call that fills the order, so the two cannot come apart — which is the
    /// only thing a parallel array has to get right.
    pub tree: Vec<sort::TreeRow>,
    pub sort_by: Column,
    pub ascending: bool,
    pub filter: String,
    /// When the filter text last changed, if the change has not been applied yet.
    ///
    /// See [`Tab::settle_filter`] and [`FILTER_DELAY`]. `None` means the order on screen is the
    /// order this filter asks for.
    pub filter_at: Option<f64>,
    pub show_hidden: bool,
    /// Show everything under this folder rather than its own children. See
    /// [`Tab::toggle_flat`] and [`crate::fs::scan::scan_deep`].
    pub flat: bool,
    /// And how: as one list, or as the tree it came from. Only read while `flat` is set.
    ///
    /// The window's preference, copied in when the flatten is turned on and when the preference
    /// changes — see [`FlatMode`]. It lives here because it is what [`Tab::rebuild_order`] reads,
    /// and that is called from a dozen places that have no business knowing about `App`.
    pub flat_mode: FlatMode,
    /// Whether a chain of folders with nothing in them but each other is one row. The window's
    /// preference, kept here for the same reason [`Tab::flat_mode`] is — see
    /// [`sort::build_tree_order`], which is what it means.
    pub regroup: bool,
    /// Which folders of a [`FlatMode::Tree`] listing are shut, by their path relative to the
    /// folder being flattened — the same string the row is stored under.
    ///
    /// **Closed rather than open**, so the empty set is a fully expanded tree. That is what the
    /// mode should show the moment it is turned on: the same rows the list mode has, arranged.
    /// Opening every folder by hand to see what you already asked to see would be the button
    /// undoing itself, and a set of what has been *closed* is a handful of strings where a set of
    /// what is open would be one per folder in the tree.
    ///
    /// By path and not by entry index, because an index is only good until the folder is read
    /// again: an `F5`, a file operation or the watcher replaces the whole listing, and a set of
    /// indices would silently start naming different folders. See [`Tab::toggle_collapsed`].
    pub collapsed: std::collections::HashSet<String>,

    /// Selection, indexed by *entry* index so it survives a re-sort.
    pub selected: Vec<bool>,
    pub selected_count: usize,
    /// The summed size of the selected *files*, for the status line.
    ///
    /// **Maintained here rather than worked out when it is shown**, which is the same rule
    /// `selected_count` follows and for the same reason: the status line is drawn every frame, and a
    /// walk over the selection is a walk over the folder — `Ctrl+A` in a folder of 300,000 files
    /// would put a 300,000-iteration loop in every frame of it. Every gesture that can change the
    /// selection already knows what it changed, so each keeps this in step; a folder's own directories
    /// contribute nothing, because [`crate::fs::dir::Entry::size`] is noise for one.
    ///
    /// `a_selection_keeps_its_own_size` walks the whole folder and compares, which is what catches a
    /// gesture that forgets.
    pub selected_size: u64,
    /// The keyboard cursor, as a position in `order`.
    pub cursor: Option<usize>,
    /// Where a shift-click range started, as a position in `order`.
    pub anchor: Option<usize>,

    /// Column widths. The first is Name, which flexes; the rest are measured from
    /// their content the first time a listing is drawn.
    pub widths: [f32; 4],
    /// Cleared whenever the listing changes, so the fitted columns are re-measured.
    pub widths_measured: bool,

    /// Select this name as soon as the listing lands.
    ///
    /// Set when going Up, so the folder you came out of is highlighted where you
    /// left it rather than making you find it again.
    pub reveal: Option<String>,
    /// Scroll to the cursor on the next frame, for keyboard movement and reveal.
    pub scroll_to_cursor: bool,

    /// Keystrokes typed recently, for type-ahead find, and when the last one was.
    pub typeahead: String,
    pub typeahead_at: f64,

    /// The breadcrumb has been turned into an editable path field.
    pub editing_path: bool,
    pub edit_text: String,

    /// Last frame's scroll offset, so a scroll-into-view can nudge rather than jump.
    pub scroll_y: f32,
    /// An absolute offset to scroll to on the next frame, for the auto-scroll a
    /// rubber-band does when it is dragged past the edge of the view.
    pub scroll_to: Option<f32>,

    /// A rubber-band selection being dragged.
    pub band: Option<Band>,

    /// A row being renamed in place: which entry, and the text so far.
    ///
    /// Explorer edits the name where it sits rather than in a dialog, which keeps the
    /// surrounding names visible — usually the whole reason you are renaming.
    pub renaming: Option<(usize, String)>,
    /// Whether the rename field still needs the caret put in it.
    pub rename_fresh: bool,
    /// What was selected when the listing was dropped for a refresh, to be selected again when
    /// the new one lands.
    ///
    /// Needed because a refresh happens in two steps with nothing in between: the listing is
    /// dropped so the folder is re-read, and the answer arrives frames later. By then the old
    /// listing is gone, and with it any way to say what the selection *was* -- which is why this
    /// is captured at the moment it is dropped rather than worked out on arrival.
    pub keep_selected: Vec<String>,
    /// Start renaming whatever [`Tab::reveal`] finds, as soon as it is found.
    ///
    /// Set when this program has just made a folder: the shell creates it, the folder is
    /// re-read, and the new row arrives selected and ready to be named — which is the whole
    /// gesture in Explorer, where `New folder` and typing its name are one action rather than
    /// two.
    pub rename_revealed: bool,
    /// Every name in the listing at the moment the shell was asked to create something here, so
    /// that the one which was not there before can be recognised when the folder is read again.
    ///
    /// The same gesture as [`Tab::rename_revealed`] — a new file arrives selected with its name
    /// open for editing — for the case where **the name cannot be known in advance**. This
    /// program's own New folder goes through `IFileOperation` and is handed the name it ended up
    /// with on [`crate::shell::ops::Done::created`]. The shell's own `New >` entries do not:
    /// `InvokeCommand` creates the file itself and returns nothing but success, and the name it
    /// picked is localised (`Nouveau document texte.txt`) and may have been taken already
    /// (`… (2).txt`). So it is arrived at by elimination instead: whatever is in the next listing
    /// and not in this snapshot is what was just made. See [`Tab::apply`], and
    /// [`crate::shell::menu::Command::creates_an_item`] for how the entry is spotted.
    pub name_the_new: Option<Vec<String>>,

    /// **This folder's preview panel**, shut by default.
    ///
    /// Per tab rather than per pane or per window, because a preview belongs to the folder it is
    /// beside: two panes each showing a build of the same DLL get their own, and switching tabs
    /// puts back the one that tab had open. Where the panel *goes* is the window's preference and
    /// lives on `App` — see [`crate::ui::preview::Layout`].
    pub preview: crate::ui::preview::Preview,
}

/// Widths for the three fitted columns before anything has been measured. Only
/// visible for the frame between a listing arriving and being drawn.
const DEFAULT_WIDTHS: [f32; 4] = [240.0, 88.0, 130.0, 132.0];

/// How long a folder may take to read before the listing admits to waiting for it.
///
/// **Under this, saying anything is worse than saying nothing.** A local folder comes back in
/// single-digit milliseconds — [`crate::fs::scan`]'s whole design is about that — so a
/// `Reading…` drawn the moment a folder is asked for is a word that flashes up and vanishes on
/// every single navigation, in the exact place the listing is about to be. Over half a second,
/// silence is the thing that misleads: a window showing an empty folder that is not empty looks
/// like a program that has finished and got it wrong.
///
/// Half a second is the usual floor for "worth telling somebody about" and it is comfortably
/// past every local case; what it catches is the disconnected share and the sleeping drive,
/// which are the two this program has always had to be honest about.
pub const SLOW_SCAN: f64 = 0.5;

/// How long the filter waits for the typing to stop before it is applied.
///
/// **A filter is re-applied from scratch on every change** — one pass over every entry, then a
/// sort of whatever survived — so on a listing big enough it is not the filter that is slow, it
/// is *typing*. Measured by [`crate::fs::sort::tests::filter_speed`] over a flattened
/// `C:\Program Files`, 188,734 entries, release build:
///
/// | filter | rows | one pass |
/// | --- | --- | --- |
/// | `""` | 188,690 | 250 ms |
/// | `"e"` | 188,690 | 237 ms |
/// | `"ex"` | 62,297 | 129 ms |
/// | `"exe"` | 3,073 | 27 ms |
/// | `"micro exe"` | 1,805 | 24 ms |
/// | `"!exe"` | 185,617 | 261 ms |
///
/// Note which passes are the expensive ones: the *intermediate* needles, because they are the
/// ones that leave enough rows to sort. Typing `exe` cost 416 ms of frozen window in three
/// stalls to arrive at an answer that costs 27 ms to compute — and the two stalls in front of it
/// were for orders nobody was going to read.
///
/// The last two rows are the shape of it stated twice over: **matching is not what costs
/// anything, the sort of what survives is.** `micro exe` walks every path looking for two words
/// rather than one and comes out a tenth of the price of `e`, which walks it looking for one and
/// keeps everything. An anchor is free on the same evidence — `^c:` keeps all 188,690 in 220 ms,
/// which is `e`'s number. So the syntax cannot make a keystroke slow; only the answer it leaves
/// can. Run to run this varies by around 15%, which is wider than any difference between the
/// terms.
///
/// So each keystroke restarts this timer and only the last one does any work.
///
/// A quarter of a second is longer than the ~150 ms at which a pause starts to read as lag, and
/// that is the trade being made deliberately: the pause is not being hidden, it is being spent
/// on *not* running the two passes above it. A keystroke gap of 250 ms is slow typing — a word
/// typed at any ordinary speed collapses into one pass — and where the answer costs a fifth of a
/// second to compute, waiting a quarter for the typing to finish is cheaper than computing three
/// answers nobody reads. On a small folder, where a pass is microseconds, all this costs is that
/// the listing settles a beat after you stop.
pub const FILTER_DELAY: f64 = 0.25;

/// A number no view has had before.
///
/// Not a hash of the path: two views of the *same* folder, and the same folder revisited
/// after a refresh, are different views, and an answer for one is not an answer for the
/// other. Wrapping is irrelevant at one per navigation.
fn next_view() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Tab {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            title: display_name(&path),
            history: vec![path.clone()],
            at: 0,
            trail: path.clone(),
            path,
            dir: None,
            awaiting: None,
            asked_at: None,
            view: next_view(),
            file_icons: Vec::new(),
            links: std::collections::HashMap::new(),
            git: None,
            git_asked: false,
            git_answered: false,
            git_settled_at: None,
            order: Vec::new(),
            tree: Vec::new(),
            // Type, not Name: a folder read by type comes up grouped — every source
            // file together, every image together — and within a group it is still in
            // name order, so nothing is harder to find than it would have been.
            sort_by: Column::Type,
            ascending: true,
            filter: String::new(),
            filter_at: None,
            show_hidden: false,
            flat: false,
            flat_mode: FlatMode::default(),
            // The window's preference, and its default is on — see `Config::regroup`. A tab that
            // has never been flattened takes the real one the moment it is.
            regroup: true,
            collapsed: std::collections::HashSet::new(),
            selected: Vec::new(),
            selected_count: 0,
            selected_size: 0,
            cursor: None,
            anchor: None,
            widths: DEFAULT_WIDTHS,
            widths_measured: false,
            reveal: None,
            keep_selected: Vec::new(),
            rename_revealed: false,
            name_the_new: None,
            scroll_to_cursor: false,
            typeahead: String::new(),
            typeahead_at: 0.0,
            editing_path: false,
            edit_text: String::new(),
            scroll_y: 0.0,
            scroll_to: None,
            band: None,
            renaming: None,
            rename_fresh: false,
            preview: crate::ui::preview::Preview::default(),
        }
    }

    /// A copy of this tab pointing at the same place — what the `+` button and a
    /// duplicate both make. The listing comes along, so a new tab on the current
    /// folder is populated in the frame it is created.
    pub fn duplicate(&self) -> Self {
        let mut tab = Self::new(self.path.clone());
        // The bar comes across as it looks, or a duplicate of a tab you had walked up
        // would silently lose the trail the original still shows.
        tab.trail = self.trail.clone();
        tab.sort_by = self.sort_by;
        tab.ascending = self.ascending;
        tab.show_hidden = self.show_hidden;
        // Including flattened, and the listing with it: a duplicate is the same place as
        // it currently *looks*, and a copy that quietly walked the tree again — for
        // seconds, on a big one — would be a worse answer than either keeping it or
        // dropping it. Which of the two flatten modes, and which folders of a tree are shut,
        // for the same reason: "as it currently looks" is the whole of what a duplicate is.
        tab.flat = self.flat;
        tab.flat_mode = self.flat_mode;
        tab.collapsed = self.collapsed.clone();
        tab.widths = self.widths;
        tab.widths_measured = self.widths_measured;
        // Open the same way, but reading for itself — see `Preview::duplicate`, which explains
        // why the decoded content is deliberately not carried across.
        tab.preview = self.preview.duplicate();
        if let Some(dir) = &self.dir {
            tab.apply(dir.clone());
        }
        tab
    }

    // ---- Navigation ----------------------------------------------------

    /// Go somewhere, recording it in the history.
    pub fn navigate(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if path == self.path {
            return;
        }
        // Anything you do after going Back replaces the forward trail.
        self.history.truncate(self.at + 1);
        self.history.push(path.clone());
        self.at = self.history.len() - 1;
        // A history that grows all session is a leak nobody notices until it is
        // one; 256 places back is more than anyone walks.
        if self.history.len() > 256 {
            let excess = self.history.len() - 256;
            self.history.drain(..excess);
            self.at -= excess;
        }
        self.go_to(path);
    }

    pub fn can_go_back(&self) -> bool {
        self.at > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.at + 1 < self.history.len()
    }

    pub fn go_back(&mut self) {
        if self.can_go_back() {
            self.at -= 1;
            let path = self.history[self.at].clone();
            // Where you were is highlighted by `go_to`, from the trail, for every arrival rather
            // than only for the one that lands on the parent.
            self.go_to(path);
        }
    }

    pub fn go_forward(&mut self) {
        if self.can_go_forward() {
            self.at += 1;
            let path = self.history[self.at].clone();
            self.go_to(path);
        }
    }

    /// Up one level. The folder just left is highlighted by [`Tab::go_to`], off the trail.
    pub fn go_up(&mut self) {
        if let Some(parent) = crate::fs::parent_of(&self.path) {
            self.navigate(parent);
        }
    }

    /// Point the tab at a path without touching the history.
    fn go_to(&mut self, path: PathBuf) {
        // The one place [`Tab::trail`] is decided, because this is the one place the path
        // changes — `navigate`, `go_back`, `go_forward` and `go_up` all come through here.
        //
        // `Path::starts_with` compares whole components, so `C:\Users` is a prefix of
        // `C:\Users\tony` and `C:\Use` is not. The empty path is a prefix of everything,
        // which is the answer this wants: This PC is where the breadcrumb starts, so going
        // there is going up rather than going somewhere else.
        if !self.trail.starts_with(&path) {
            self.trail = path.clone();
        }
        // **The child of this folder that the breadcrumb still shows, selected on arrival.**
        // Standing in `a/b` with `a/b/c` on the bar, `c` is the one thing you are most likely to
        // want next — it is where you just came from, or where you were heading before you
        // stopped off here — and the bar is already pointing at it. So the listing selects it and
        // scrolls to it, which for a folder of five thousand names is the difference between
        // going up a level and losing your place.
        //
        // Asked of [`crate::fs::breadcrumb_segments`] rather than worked out from components, so
        // that "what the breadcrumb shows" means literally that: the same walk, with the same
        // answer for a drive root and for This PC, where a raw component walk gives `C:` without
        // its root and no place at all above it.
        //
        // This subsumes what `go_up` and `go_back` each used to do for themselves, and covers
        // what neither did: clicking a segment three levels up now selects the segment below it
        // too, rather than only a single step back.
        let trail = crate::fs::breadcrumb_segments(&self.trail);
        self.reveal = trail
            .iter()
            .position(|(_, at)| *at == path)
            .and_then(|here| trail.get(here + 1))
            .map(|(_, child)| display_name(child));
        self.title = display_name(&path);
        self.path = path;
        self.dir = None;
        self.awaiting = None;
        self.asked_at = None;
        // A new view of a new folder: any icon answer still in flight for the old one is now
        // addressed to a view that no longer exists, and the column it would have gone in is
        // released here.
        self.view = next_view();
        self.file_icons = Vec::new();
        self.file_icons.shrink_to_fit();
        self.links = std::collections::HashMap::new();
        self.order.clear();
        self.tree.clear();
        self.selected.clear();
        self.selected_count = 0;
        self.selected_size = 0;
        self.cursor = None;
        self.anchor = None;
        self.filter.clear();
        self.filter_at = None;
        // **A flatten does not come along to the next folder**, for the same reason the
        // filter does not: both are a question asked of the folder you were looking at,
        // and the answer to a question about somewhere else is not the same answer. It
        // also means opening a row in a flattened listing lands in an ordinary folder,
        // which is the only way out of the view that does not need the button again — and
        // it is what stops a click on a deep folder from silently starting a second tree
        // walk. `Tab::refresh` keeps it, because that is the same question again.
        self.flat = false;
        // And with it what was shut in the tree, which named folders under the folder being
        // left. `Tab::refresh` keeps these too: the same tree, read again, is the same tree.
        self.collapsed.clear();
        self.widths_measured = false;
        self.editing_path = false;
        // **A different folder opens at the top.** Row 200 of the folder you just left is not
        // row 200 of anything, and the scroll offset does not belong to this tab as far as egui
        // is concerned — it belongs to the pane's one scroll area, which goes on showing
        // whatever it was showing unless something asks it not to. Setting `scroll_y` alone
        // records where the listing *is*; `scroll_to` is what moves it.
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
        self.band = None;
        self.renaming = None;
        self.keep_selected.clear();
        // A snapshot of somewhere else says nothing about what is new here.
        self.name_the_new = None;
    }

    /// Flatten this folder, or stop flattening it.
    ///
    /// The listing goes, because the two are different listings of the same folder —
    /// a flattened one is [`crate::fs::scan::scan_deep`]'s and holds relative paths.
    /// Turning it *off* comes straight back out of the cache, which still has the
    /// folder's own children; turning it *on* is a fresh walk every time, which is the
    /// point of the gesture.
    ///
    /// The selection is not carried across. It is kept by name, and a name means two
    /// different things on the two sides of this: `file.txt` on one and
    /// `sub\file.txt` on the other, so nothing would match anyway — and a selection
    /// that half-survived would be worse than one that plainly did not.
    pub fn toggle_flat(&mut self, mode: FlatMode, regroup: bool) {
        // "This PC" is not a folder and has no tree: its rows are volumes, each of which is a
        // place to flatten of its own. There is nothing for the button to do here, so it is
        // drawn disabled and this refuses — rather than latching over a listing that would not
        // have changed.
        if self.path.as_os_str().is_empty() {
            return;
        }
        self.flat = !self.flat;
        // Both halves of the window's preference for how a tree looks, taken at the moment the view
        // is turned on. A tree opens fully expanded, which is what an empty `collapsed` means — see
        // the field.
        self.flat_mode = mode;
        self.regroup = regroup;
        self.collapsed.clear();
        self.keep_selected.clear();
        self.selected.clear();
        self.selected_count = 0;
        self.selected_size = 0;
        self.cursor = None;
        self.anchor = None;
        self.renaming = None;
        // For the same reason as the selection just above: a name is `file.txt` on one side of
        // this and `sub\file.txt` on the other, so nothing in the snapshot would match and every
        // row of the new listing would look new.
        self.name_the_new = None;
        self.dir = None;
        self.awaiting = None;
        self.asked_at = None;
        self.order.clear();
        self.tree.clear();
        self.widths_measured = false;
        // The top, because row 200 of a folder's own children is not row 200 of its
        // whole tree — the same reason a new folder opens at the top in [`Tab::go_to`].
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
    }

    /// Show the flattened tree the other way round.
    ///
    /// **Nothing is re-read**, which is the point of the two modes being one listing: the walk's
    /// answer is already here and the mode only decides the order built over it. So this is a
    /// re-sort, and on a tree that took seconds to walk it is instant.
    ///
    /// The selection *is* kept, unlike [`Tab::toggle_flat`]'s — it is held by entry index, and
    /// both modes are orders over the same entries, so every selected row is still the same file.
    /// Its position moves, which is what [`Tab::rebuild_order`] already puts the cursor back for.
    ///
    /// Nothing happens when the tab is not flattened: the mode is the window's preference and
    /// takes effect the next time the button is pressed. Answering `false` for that case rather
    /// than doing the work anyway is what lets `App` skip the rebuild.
    /// Merge chains of only-children into one row, or stop. Answers whether it did anything.
    ///
    /// [`Tab::set_flat_mode`]'s twin, and everything said there applies: it is a re-sort and never a
    /// re-read, the preference is the window's, and a tab that is not showing a tree takes the
    /// setting for the next time its button is pressed. A tab showing the flat **list** is in the
    /// second case rather than the first — the merge is the tree's shape and the list has no shape to
    /// change.
    pub fn set_regroup(&mut self, on: bool) -> bool {
        if !self.is_tree() || self.regroup == on {
            self.regroup = on;
            return false;
        }
        self.regroup = on;
        self.rebuild_order();
        // The rows are the same rows arranged differently, which is [`Tab::set_flat_mode`]'s reason
        // for going back to the top as well.
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
        true
    }

    pub fn set_flat_mode(&mut self, mode: FlatMode) -> bool {
        if !self.flat || self.flat_mode == mode {
            self.flat_mode = mode;
            return false;
        }
        self.flat_mode = mode;
        self.rebuild_order();
        // Straight to the top for the reason a settled filter goes there: the rows are the same
        // rows, and row 200 of a list is not row 200 of the tree the same files make.
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
        true
    }

    /// Open or shut the folder at a position in the display order, in a tree.
    ///
    /// Answers whether it did anything: `false` for a row that is not a folder, and for a listing
    /// that is not a tree — a caller does not have to test either first.
    ///
    /// Keyed by the row's stored name, which is its path relative to the folder being flattened.
    /// See [`Tab::collapsed`] for why that and not the entry index.
    pub fn toggle_collapsed(&mut self, position: usize) -> bool {
        self.set_collapsed(position, !self.is_collapsed(position))
    }

    /// Shut a folder, or open it. Answers whether the tree changed.
    pub fn set_collapsed(&mut self, position: usize, shut: bool) -> bool {
        if !self.flat || self.flat_mode != FlatMode::Tree {
            return false;
        }
        let Some(dir) = self.dir.clone() else {
            return false;
        };
        let Some(entry) = self.entry_at(position) else {
            return false;
        };
        if !dir.entries.get(entry).is_some_and(|e| e.is_dir()) {
            return false;
        }
        let name = dir.name(entry);
        let changed = if shut {
            self.collapsed.insert(name.to_owned())
        } else {
            self.collapsed.remove(name)
        };
        if !changed {
            return false;
        }
        // The rows under it are leaving the order or joining it, so the cursor's *position* moves
        // even though it is still on the same file — which `rebuild_order` puts back.
        self.rebuild_order();
        true
    }

    /// Whether the folder at a position in the display order is shut.
    ///
    /// `false` for anything that is not a shut folder, including every row of a listing that is
    /// not a tree — so this is also the question "does this row draw an opened twisty".
    pub fn is_collapsed(&self, position: usize) -> bool {
        if !self.flat || self.flat_mode != FlatMode::Tree {
            return false;
        }
        let Some(dir) = self.dir.as_ref() else {
            return false;
        };
        let Some(entry) = self.entry_at(position) else {
            return false;
        };
        self.collapsed.contains(dir.name(entry))
    }

    /// Whether the listing on show is a tree — a flatten in [`FlatMode::Tree`].
    ///
    /// The one question the drawing asks, and the one place the two halves of it are spelled out
    /// together: the mode alone says nothing while the tab is showing a folder's own children.
    #[inline]
    pub fn is_tree(&self) -> bool {
        self.flat && self.flat_mode == FlatMode::Tree
    }

    /// How far in the row at `position` is drawn — its depth in the tree **as shown**.
    ///
    /// Not `Dir::depth`, and the difference is a merged chain: `src > main > java` is one row
    /// standing where `src` stood, so it is at `src`'s depth and its children are one in from
    /// *that*. `0` for every row of every listing that is not a tree. See [`sort::TreeRow`].
    #[inline]
    pub fn row_depth(&self, position: usize) -> usize {
        self.tree.get(position).map_or(0, |row| row.depth as usize)
    }

    /// How many folders are merged into the name of the row at `position`: `0` for an ordinary row,
    /// `2` for `a > b > c`.
    #[inline]
    pub fn row_merged(&self, position: usize) -> usize {
        self.tree.get(position).map_or(0, |row| row.merged as usize)
    }

    /// Whether the row at `position` has anything **inside** it on show.
    ///
    /// **Asked of the display order rather than of the file system.** In pre-order a folder's
    /// children are the rows immediately after it, so a next row drawn further in *is* the answer —
    /// one comparison, no index, nothing to keep in step.
    ///
    /// Which is why it compares the depths the rows are *drawn* at: a merged chain's path is deeper
    /// than its indent, so an empty folder followed by the chain `b > c` would otherwise look as
    /// though the chain were inside it, and wear a twisty that does nothing.
    ///
    /// `false` therefore covers a folder with nothing in it — empty, or the walk stopped at its
    /// budget, or a junction it would not follow — and every row of a listing that is not a tree.
    /// A folder the user has **shut** also answers `false`, because its children are not in the
    /// order at all; that is the other half of the twisty's test, where it is asked.
    pub fn has_children_below(&self, position: usize) -> bool {
        let Some(here) = self.tree.get(position) else {
            return false;
        };
        self.tree
            .get(position + 1)
            .is_some_and(|next| next.depth > here.depth)
    }

    /// Drop the listing so the folder is read again, keeping the selection by name.
    ///
    /// This is a *refresh*, not a navigation: the same folder, read afresh because something
    /// changed it. Every file operation ends here, so losing the selection here means copying a
    /// file and then having nothing selected to copy again.
    pub fn refresh(&mut self) {
        self.keep_selected = match &self.dir {
            Some(dir) => self
                .selected
                .iter()
                .enumerate()
                .filter(|(_, &on)| on)
                .filter(|&(entry, _)| entry < dir.len())
                .map(|(entry, _)| dir.name(entry).to_owned())
                .collect(),
            None => std::mem::take(&mut self.keep_selected),
        };
        self.dir = None;
        self.awaiting = None;
        self.asked_at = None;
    }

    /// Whether this tab has been waiting for its listing long enough to say so.
    ///
    /// See [`SLOW_SCAN`]. `false` while nothing has been asked for, which is also the
    /// answer for the frame between a folder being wanted and being requested.
    pub fn waiting_visibly(&self, now: f64) -> bool {
        self.asked_at.is_some_and(|asked| now - asked >= SLOW_SCAN)
    }

    // ---- The listing ---------------------------------------------------

    /// Take a finished listing and build the view over it.
    ///
    /// A *re-read* of the folder already being shown keeps the selection, by name. This is what
    /// a refresh has to do -- every file operation ends in one, so a selection that did not
    /// survive it meant copying a file and then finding nothing selected to copy again. Which is
    /// exactly what Ctrl+C, Ctrl+V, Ctrl+C did: the second copy had nothing to work with.
    /// [`Tab::go_to`] still clears everything, because a different folder is a different set of
    /// files.
    pub fn apply(&mut self, dir: Arc<Dir>) {
        // Whatever [`Tab::refresh`] put aside, plus the live selection when the listing is being
        // replaced under a tab that still has one.
        let mut held = std::mem::take(&mut self.keep_selected);
        if held.is_empty() {
            if let Some(old) = self.dir.as_ref().filter(|_| self.selected_count > 0) {
                held = self
                    .selected
                    .iter()
                    .enumerate()
                    .filter(|(_, &on)| on)
                    .filter(|&(entry, _)| entry < old.len())
                    .map(|(entry, _)| old.name(entry).to_owned())
                    .collect();
            }
        }

        // The row being renamed, remembered by *name* before the listing it is indexed into goes
        // away. `renaming` holds an entry index, and an index into the new listing may well mean
        // a different file — so left alone it is not a cosmetic problem: the field stays on
        // screen, stays committing, and commits the typed name onto whatever now sits at that
        // index.
        //
        // Which is not hypothetical and not rare, because the two halves collide by construction:
        // naming a file the shell has just made is a rename that *starts* on a re-read, and
        // making a file is exactly what has [`crate::watch`] asking for another one.
        let renaming = self.renaming.take().and_then(|(entry, text)| {
            let old = self.dir.as_ref()?;
            (entry < old.len()).then(|| (entry, old.leaf(entry).to_owned(), text))
        });

        self.selected = vec![false; dir.len()];
        self.file_icons = vec![crate::shell::icons::UNASKED; dir.len()];
        self.links.clear();
        // **A new listing is a new question for git**, and the old answer goes with the old listing
        // rather than being kept while it is checked. A tick that survives the read that would have
        // disproved it is the whole failure this design is avoiding.
        self.git = None;
        self.git_asked = false;
        self.git_answered = false;
        self.git_settled_at = None;
        self.selected_count = 0;
        self.selected_size = 0;
        self.cursor = None;
        self.anchor = None;
        self.widths_measured = false;
        self.dir = Some(dir);
        self.awaiting = None;
        self.asked_at = None;
        self.rebuild_order();

        // The rename field, put back on whichever row its file is now. Gone means gone: a file
        // renamed or deleted from under an open rename field has nothing left to commit onto, and
        // dropping it is what takes the field off the screen.
        if let Some((was, leaf, text)) = renaming {
            if let Some(dir) = self.dir.clone() {
                if let Some(&i) = self.order.iter().find(|&&i| dir.leaf(i as usize) == leaf) {
                    let entry = i as usize;
                    // The field's egui id is built from the entry index, so an index that moved is
                    // a different widget as far as egui is concerned, with none of the focus the
                    // old one had. Asking for it fresh is what puts the caret back in it. The text
                    // comes across either way -- it lives here, not in the widget -- so whatever
                    // was half-typed survives.
                    self.rename_fresh |= entry != was;
                    self.renaming = Some((entry, text));
                }
            }
        }

        // Whatever was selected and is still there. Anything that has gone -- moved, renamed,
        // deleted -- simply is not selected any more, which is the only sensible answer.
        if !held.is_empty() {
            if let Some(dir) = self.dir.clone() {
                for (position, &entry) in self.order.iter().enumerate() {
                    if held.iter().any(|name| name == dir.name(entry as usize)) {
                        self.selected[entry as usize] = true;
                        self.selected_count += 1;
                        self.selected_size += Self::size_at(Some(&dir), entry as usize);
                        self.cursor.get_or_insert(position);
                        self.anchor.get_or_insert(position);
                    }
                }
            }
        }

        // Highlight and scroll to whatever we came out of.
        let rename = std::mem::take(&mut self.rename_revealed);
        if let Some(name) = self.reveal.take() {
            if let Some(dir) = self.dir.clone() {
                // By leaf, so that a row revealed after a rename or a new folder is still
                // found in a flattened listing, where the name it is stored under carries
                // the folders in front of it. Identical to the name in every other
                // listing. Two files of the same name in different folders are two matches
                // and the first one wins, which is the same rule a duplicate name in one
                // folder would meet if the filesystem allowed one.
                if let Some(at) = self
                    .order
                    .iter()
                    .position(|&i| dir.leaf(i as usize) == name)
                {
                    self.select_only(at);
                    self.scroll_to_cursor = true;
                    // And it beats the top of the listing, which is where [`Tab::go_to`] asks a
                    // new folder to open. This is Back or Up, or a folder just created: the row
                    // being revealed is the whole reason for the move, and it can be row 500.
                    self.scroll_to = None;
                    if rename {
                        self.begin_rename();
                    }
                }
            }
        }

        // A file the shell has just made, whose name nothing told us: it is the row that was not
        // in the folder when the menu entry was chosen. Same ending as `rename_revealed` above —
        // selected, scrolled to, name open for editing — with the row arrived at by elimination
        // rather than handed over. See [`Tab::name_the_new`].
        //
        // Taken whether or not it finds anything, exactly as `reveal` is. A snapshot left lying
        // about would sit there until the folder next changed for any reason at all, and then open
        // a rename field on a file some other program had written, which is worse than missing the
        // one this was for.
        if let Some(before) = self.name_the_new.take() {
            if let Some(dir) = self.dir.clone() {
                // Searched over `order`, so a row a filter is hiding is not offered for renaming.
                // The snapshot itself is the whole listing rather than `order` — a hidden row is
                // still a file that was already there, and comparing against the visible ones only
                // would call it new.
                if let Some(at) = self
                    .order
                    .iter()
                    .position(|&i| !before.iter().any(|had| had == dir.name(i as usize)))
                {
                    self.select_only(at);
                    self.scroll_to_cursor = true;
                    self.scroll_to = None;
                    self.begin_rename();
                }
            }
        }
    }

    /// Every name in the listing, spelled the way [`Tab::apply`] compares them.
    ///
    /// The whole listing and not just the rows on show: a filter hides rows, and a hidden row is
    /// still a file that is already there. See [`Tab::name_the_new`], which is what wants this.
    pub fn names(&self) -> Vec<String> {
        match &self.dir {
            Some(dir) => (0..dir.len()).map(|e| dir.name(e).to_owned()).collect(),
            None => Vec::new(),
        }
    }

    /// Note that the filter text has changed, without applying it yet.
    ///
    /// Called on every keystroke; the last one before the pause is the only one that ends up
    /// doing any work. See [`FILTER_DELAY`] for what a pass costs and why that matters.
    pub fn filter_changed(&mut self, now: f64) {
        self.filter_at = Some(now);
    }

    /// Apply a filter whose keystrokes have stopped, or say how long is left until they have.
    ///
    /// Called once a frame. `Some(seconds)` means a rebuild is owed but not due, and the caller
    /// has to make sure there *is* a frame then — this program is idle between events, so
    /// without a repaint asked for, the filter would be applied whenever something else next
    /// happened to want a frame. `None` means there is nothing owed, either because nothing
    /// changed or because this call has just done it.
    pub fn settle_filter(&mut self, now: f64) -> Option<f64> {
        let at = self.filter_at?;
        let left = FILTER_DELAY - (now - at);
        if left > 0.0 {
            return Some(left);
        }
        self.filter_at = None;
        self.rebuild_order();
        // **A changed filter opens at the top**, for the same reason a different folder does in
        // [`Tab::go_to`]: row 200 of what one filter left is not row 200 of what the next one
        // leaves. Here it is sharper than that. Narrow a listing from five thousand rows to twelve
        // while scrolled halfway down it and there is no offset to keep — `ScrollArea` clamps to
        // the content it has, so what arrives is the *end* of the twelve, or the empty tail under
        // them. The rows a filter finds are worth looking at from the first one.
        //
        // This is `settle_filter` and not [`Tab::rebuild_order`] on purpose: every path into that
        // one rebuilds from the filter as it stands, and a column click or an `F5` has not changed
        // what the filter says. Losing your place in a listing you are still reading is what those
        // two must not do.
        self.scroll_y = 0.0;
        self.scroll_to = Some(0.0);
        None
    }

    /// Whether the filter line asks a question only git can answer. See [`sort::CHANGED`].
    pub fn filters_on_git(&self) -> bool {
        sort::split_special(&self.filter).0
    }

    /// Re-apply the sort and the filter from scratch.
    pub fn rebuild_order(&mut self) {
        // Whatever was owed is paid by this: every path into here builds the order from the
        // filter as it stands, so a deadline left behind would only buy a second identical pass.
        self.filter_at = None;
        let Some(dir) = self.dir.clone() else {
            self.order.clear();
            self.tree.clear();
            return;
        };
        // `@git` off the front of the filter, and what is left for the name test.
        let (on_git, filter) = sort::split_special(&self.filter);
        // Three states and three different answers — see [`Tab::git_answered`]. The closure holds its
        // own `Arc`s because the loop it is called from borrows `self.order`.
        let names = dir.clone();
        let repo = self.git.clone();
        let by_git: Option<Box<dyn Fn(usize) -> bool>> = match (on_git, self.git_answered) {
            (false, _) => None,
            // Asked and not answered: the question cannot be evaluated, so it excludes nothing. The
            // answer arriving rebuilds this — `App::collect_git`.
            (true, false) => None,
            (true, true) => match repo {
                // Every row git has anything to say about, which is the same set the status line
                // counts as `N changed` — a folder included, because it wears the strongest state
                // beneath it and a folder filtered out is a folder you cannot open to reach what is
                // inside it.
                Some(repo) => Some(Box::new(move |entry| {
                    repo.state(names.name(entry))
                        .is_some_and(|state| state != crate::git::State::Clean)
                })),
                // Answered, and there is no repository here: nothing has changed, because there is
                // nothing that could have.
                None => Some(Box::new(|_| false)),
            },
        };
        // Remember what the cursor was pointing at, since its position moves.
        let cursor_entry = self.cursor.and_then(|at| self.order.get(at).copied());
        // The two flatten modes are two orders over one listing, and this is the only place that
        // knows which — see [`FlatMode`]. Everything either side of it is the same for both: the
        // same filter, the same git question, the same cursor put back on the same file.
        if self.is_tree() {
            let collapsed = std::mem::take(&mut self.collapsed);
            sort::build_tree_order(
                &dir,
                &mut self.order,
                &mut self.tree,
                self.sort_by,
                self.ascending,
                self.show_hidden,
                &filter,
                by_git.as_deref(),
                // Taken out and put back rather than borrowed: the set and the order are both
                // fields of this tab, and the builder needs one while it fills the other.
                &|name| collapsed.contains(name),
                self.regroup,
            );
            self.collapsed = collapsed;
        } else {
            sort::build_order(
                &dir,
                &mut self.order,
                self.sort_by,
                self.ascending,
                self.show_hidden,
                &filter,
                by_git.as_deref(),
            );
            // Nothing but a tree has a shape, and a stale one would outlive the listing it described.
            self.tree.clear();
        }
        self.cursor = cursor_entry.and_then(|entry| self.order.iter().position(|&i| i == entry));
        self.anchor = self.cursor;
    }

    /// Click a column header: toggle the direction if it is already the sort,
    /// otherwise switch to it in its natural direction.
    pub fn sort_by_column(&mut self, column: Column) {
        if self.sort_by == column {
            self.ascending = !self.ascending;
        } else {
            self.sort_by = column;
            self.ascending = column.starts_ascending();
        }
        self.rebuild_order();
    }

    /// The entry index at a position in the display order.
    #[inline]
    pub fn entry_at(&self, position: usize) -> Option<usize> {
        self.order.get(position).map(|&i| i as usize)
    }

    /// Where the row at `position` leads.
    pub fn target_at(&self, position: usize) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        Some(dir.target(self.entry_at(position)?))
    }

    /// Whether the row at `position` is a shortcut of either kind — a `.lnk` or a reparse
    /// point.
    ///
    /// From the enumeration alone, so it costs nothing: it says the row is *worth* reading,
    /// not what it leads to. [`crate::shell::links::folder_target`] is what answers that, and
    /// it is only ever asked of a row this returns `true` for.
    pub fn is_shortcut_at(&self, position: usize) -> bool {
        let Some(entry) = self.entry_at(position) else {
            return false;
        };
        let Some(dir) = &self.dir else { return false };
        crate::shell::links::kind_of(dir.ext(entry), dir.entries[entry].is_link()).is_some()
    }

    /// Whether the row at `position` is a folder — which decides whether opening
    /// it navigates or hands it to the shell.
    pub fn is_dir_at(&self, position: usize) -> bool {
        self.entry_at(position)
            .and_then(|i| self.dir.as_ref().map(|d| d.entries[i].is_dir()))
            .unwrap_or(false)
    }

    // ---- Selection -----------------------------------------------------

    #[inline]
    pub fn is_selected(&self, position: usize) -> bool {
        self.entry_at(position)
            .and_then(|i| self.selected.get(i).copied())
            .unwrap_or(false)
    }

    /// What entry `entry` adds to [`Tab::selected_size`]: its bytes, or nothing for a directory.
    ///
    /// Nothing, because a directory's `size` from the find data is noise — the details view leaves
    /// that cell blank for the same reason, and a folder counted as its own few hundred bytes would
    /// make the total of a selection wrong rather than incomplete.
    #[inline]
    fn size_at(dir: Option<&Arc<Dir>>, entry: usize) -> u64 {
        dir.and_then(|dir| dir.entries.get(entry))
            .filter(|e| !e.is_dir())
            .map_or(0, |e| e.size)
    }

    pub fn clear_selection(&mut self) {
        if self.selected_count > 0 {
            self.selected.iter_mut().for_each(|s| *s = false);
            self.selected_count = 0;
            self.selected_size = 0;
        }
    }

    /// One row, alone. A plain click.
    pub fn select_only(&mut self, position: usize) {
        self.clear_selection();
        if let Some(entry) = self.entry_at(position) {
            self.selected[entry] = true;
            self.selected_count = 1;
            self.selected_size = Self::size_at(self.dir.as_ref(), entry);
        }
        self.cursor = Some(position);
        self.anchor = Some(position);
    }

    /// Flip one row. Ctrl-click.
    pub fn toggle(&mut self, position: usize) {
        if let Some(entry) = self.entry_at(position) {
            let now = !self.selected[entry];
            let size = Self::size_at(self.dir.as_ref(), entry);
            self.selected[entry] = now;
            let (count, bytes) = if now {
                (self.selected_count + 1, self.selected_size + size)
            } else {
                (
                    self.selected_count.saturating_sub(1),
                    self.selected_size.saturating_sub(size),
                )
            };
            self.selected_count = count;
            self.selected_size = bytes;
        }
        self.cursor = Some(position);
        self.anchor = Some(position);
    }

    /// Everything between the anchor and here. Shift-click.
    pub fn select_range_to(&mut self, position: usize) {
        let from = self.anchor.unwrap_or(position);
        self.clear_selection();
        let (lo, hi) = (from.min(position), from.max(position));
        for at in lo..=hi.min(self.order.len().saturating_sub(1)) {
            if let Some(entry) = self.entry_at(at) {
                if !self.selected[entry] {
                    self.selected[entry] = true;
                    self.selected_count += 1;
                    self.selected_size += Self::size_at(self.dir.as_ref(), entry);
                }
            }
        }
        self.cursor = Some(position);
    }

    /// Apply a rubber-band's coverage to the selection.
    ///
    /// Recomputed from the band's base on every frame of the drag rather than
    /// accumulated, so shrinking the band deselects again — which is what makes a
    /// rubber band feel like a rubber band instead of a paintbrush.
    pub fn apply_band(&mut self) {
        let Some(band) = &self.band else { return };
        let covered = band.rows(self.order.len());
        let base = &band.base;
        // The listing, taken out once: the loops below hold a borrow of `selected` and `order`, and
        // an `Arc` clone is a counter rather than a copy of the folder.
        let dir = self.dir.clone();

        self.selected_count = 0;
        self.selected_size = 0;
        for (entry, selected) in self.selected.iter_mut().enumerate() {
            *selected = base.get(entry).copied().unwrap_or(false);
            if *selected {
                self.selected_count += 1;
                self.selected_size += Self::size_at(dir.as_ref(), entry);
            }
        }
        for position in covered {
            let Some(&entry) = self.order.get(position) else {
                continue;
            };
            let entry = entry as usize;
            if !self.selected[entry] {
                self.selected[entry] = true;
                self.selected_count += 1;
                self.selected_size += Self::size_at(dir.as_ref(), entry);
            }
        }
    }

    /// Begin renaming the row under the cursor.
    pub fn begin_rename(&mut self) {
        let Some(position) = self.cursor else { return };
        let Some(entry) = self.entry_at(position) else {
            return;
        };
        let Some(dir) = &self.dir else { return };
        // The file's own name, not the whole of what the row shows: in a flattened
        // listing a name is a relative path, and a rename field holding `sub\file.txt`
        // would be inviting the user to type a path into a rename.
        self.renaming = Some((entry, dir.leaf(entry).to_owned()));
        self.rename_fresh = true;
    }

    /// The name being edited, if this row is the one being renamed.
    pub fn rename_text(&mut self, entry: usize) -> Option<&mut String> {
        match &mut self.renaming {
            Some((at, text)) if *at == entry => Some(text),
            _ => None,
        }
    }

    pub fn select_all(&mut self) {
        self.clear_selection();
        let dir = self.dir.clone();
        for &entry in &self.order {
            self.selected[entry as usize] = true;
            self.selected_size += Self::size_at(dir.as_ref(), entry as usize);
        }
        self.selected_count = self.order.len();
    }

    /// The paths of everything selected, in display order.
    pub fn selection_paths(&self) -> Vec<PathBuf> {
        let Some(dir) = &self.dir else { return Vec::new() };
        self.order
            .iter()
            .filter(|&&i| self.selected.get(i as usize).copied().unwrap_or(false))
            .map(|&i| dir.target(i as usize))
            .collect()
    }

    /// Move the cursor, taking the selection with it unless `extend` is set.
    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
        if self.order.is_empty() {
            return;
        }
        let last = self.order.len() as isize - 1;
        let from = self.cursor.map(|c| c as isize).unwrap_or(-1);
        let to = (from + delta).clamp(0, last) as usize;
        if extend {
            self.select_range_to(to);
        } else {
            self.select_only(to);
        }
        self.scroll_to_cursor = true;
    }

    pub fn move_cursor_to(&mut self, position: usize, extend: bool) {
        if self.order.is_empty() {
            return;
        }
        let position = position.min(self.order.len() - 1);
        if extend {
            self.select_range_to(position);
        } else {
            self.select_only(position);
        }
        self.scroll_to_cursor = true;
    }

    /// Open or shut the folder the cursor is on, for the `Left` and `Right` keys.
    ///
    /// Answers whether the tree changed, which is what tells `Left` apart from "shut already":
    /// there is nothing to close, so it means go out to the parent instead. Every other listing
    /// answers `false` for every row, so both keys are inert in one without being tested for.
    pub fn set_collapsed_at_cursor(&mut self, shut: bool) -> bool {
        let Some(at) = self.cursor else {
            return false;
        };
        // Whether it *is* already what is being asked for, taken first: `set_collapsed` answers
        // `false` for both "not a folder" and "already shut", and `Left` on a shut folder should
        // step out rather than sit there.
        self.set_collapsed(at, shut)
    }

    /// Put the cursor on the row of the folder the cursor's row is *in*, in a tree.
    ///
    /// `Left` on a file, or on a folder that is already shut — the way out of a branch. Answers
    /// whether it moved: `false` at the top level of the tree, where the folder the row is in is
    /// the folder being flattened and has no row of its own.
    ///
    /// Found by walking *up* the rows on screen for the first one drawn a level shallower, which is
    /// where the parent is in pre-order and costs the depth of the branch rather than a lookup
    /// table. A tree is at most a few dozen levels and the rows are `u32`s in a `Vec`.
    ///
    /// The depth is the one the row is **drawn** at — [`Tab::row_depth`] — and not the one its path
    /// has. Two merged chains side by side at the top of the tree are siblings however deep their
    /// paths run, and a search by path depth would have called the one above the parent of the one
    /// below.
    pub fn move_cursor_to_parent(&mut self) -> bool {
        if !self.is_tree() {
            return false;
        }
        let Some(at) = self.cursor else {
            return false;
        };
        let depth = self.row_depth(at);
        if depth == 0 || at >= self.order.len() {
            return false;
        }
        let Some(parent) = (0..at).rev().find(|&above| self.row_depth(above) < depth) else {
            return false;
        };
        self.move_cursor_to(parent, false);
        true
    }

    /// Jump to the next row whose name starts with what has been typed.
    ///
    /// `now` is the frame time; a gap longer than a second starts a fresh word,
    /// which is what makes typing `re` find `readme` but typing `r` a minute later
    /// start again from `r`.
    pub fn type_ahead(&mut self, ch: char, now: f64) {
        if now - self.typeahead_at > 1.0 {
            self.typeahead.clear();
        }
        self.typeahead_at = now;
        self.typeahead.extend(ch.to_lowercase());

        let Some(dir) = self.dir.clone() else { return };
        let needle = self.typeahead.as_str();
        // Start from the row after the cursor, so repeating a letter walks through
        // the matches instead of sticking on the first.
        let start = self.cursor.map(|c| c + 1).unwrap_or(0);
        let found = (0..self.order.len())
            .map(|offset| (start + offset) % self.order.len())
            .find(|&at| {
                let name = dir.name(self.order[at] as usize);
                name.len() >= needle.len()
                    && name
                        .chars()
                        .zip(needle.chars())
                        .all(|(a, b)| a.to_ascii_lowercase() == b)
            });
        if let Some(at) = found {
            self.select_only(at);
            self.scroll_to_cursor = true;
        }
    }
}

/// A stack of tabs, one of them showing.
pub struct Pane {
    pub id: PaneId,
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// Where this pane was last drawn. Needed because the tab strip is drawn
    /// before the panes are, so a drop target is resolved against the previous
    /// frame's geometry.
    pub rect: egui::Rect,
    /// Where the folder rows were, so files can be dropped into one of them. Written by the
    /// listing each frame and read by the next, for the same reason `rect` is.
    pub drop_rows: Vec<(egui::Rect, std::path::PathBuf)>,
    /// Where the rows were, which is where a drop into this pane's own folder goes. Written and
    /// read the same way, and for the same reason, as `drop_rows`.
    pub drop_area: egui::Rect,

    /// This pane's shell, once one has been asked for.
    ///
    /// Per pane rather than per tab, and started lazily: a pane that never opens the console costs
    /// nothing, and a pane that does keeps the same shell while you switch tabs in it — which is
    /// what a console beside a file manager is for. The shell is where you are working; the tabs are
    /// what you are looking at.
    pub console: Option<crate::console::Session>,
    /// What the panel remembers: the line, the history, which shell, what is selected.
    ///
    /// Kept while the panel is shut as well, so closing it does not throw away a half-typed
    /// command.
    pub console_state: crate::ui::console::State,
    pub console_open: bool,
    /// Commands waiting for a shell to exist to run in.
    ///
    /// Only `--console` fills this: a capture run has to ask for a command before there is anything
    /// to ask, and the session is not started until the panel has a rect. Drained in one go by
    /// [`crate::app::App::console_panel`], since the session queues for itself from there on.
    pub console_queue: Vec<String>,
    /// A shell that would not start.
    ///
    /// Remembered so it is not tried again on every frame of a panel that is showing why it
    /// failed — spawning a process sixty times a second is the one wrong answer here. Cleared by
    /// picking a different shell, which is the only thing that could change the outcome.
    pub console_failed: Option<crate::console::Kind>,
}

impl Pane {
    pub fn new(id: PaneId, tab: Tab) -> Self {
        Self {
            id,
            tabs: vec![tab],
            active: 0,
            rect: egui::Rect::NOTHING,
            drop_rows: Vec::new(),
            drop_area: egui::Rect::NOTHING,
            console: None,
            console_state: crate::ui::console::State::default(),
            console_open: false,
            console_queue: Vec::new(),
            console_failed: None,
        }
    }

    /// Show tab `index`, bringing its own scroll position with it.
    ///
    /// **Every change of active tab goes through here.** The listing's scroll offset lives in
    /// the pane's one scroll area, which egui keys by where it is drawn rather than by what it
    /// shows — so a tab that comes to the front inherits wherever the tab before it had got to
    /// unless it asks for its own place back. [`Tab::scroll_y`] is where the tab keeps it, and
    /// [`Tab::scroll_to`] is what moves the listing there.
    pub fn show_tab(&mut self, index: usize) {
        self.active = index.min(self.tabs.len().saturating_sub(1));
        let tab = self.tab_mut();
        tab.scroll_to = Some(tab.scroll_y);
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active.min(self.tabs.len() - 1)]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        let at = self.active.min(self.tabs.len() - 1);
        &mut self.tabs[at]
    }

    /// Close a tab and keep the selection somewhere sensible: the tab to the
    /// right, as every browser does. Returns whether the pane still has tabs.
    pub fn close_tab(&mut self, at: usize) -> bool {
        if at >= self.tabs.len() {
            return true;
        }
        self.tabs.remove(at);
        if self.tabs.is_empty() {
            return false;
        }
        if self.active > at || self.active >= self.tabs.len() {
            self.show_tab(self.active.saturating_sub(1));
        }
        true
    }
}

/// Which side of a pane something is being dropped on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// Whether a split on this side arranges its children side by side.
    pub fn is_horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// Whether the dropped pane becomes the *first* child.
    pub fn is_first(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty folder standing beside a merged chain does not wear a twisty.
    ///
    /// **This is the one thing a chain breaks if the tree is read by path depth.** `empty` is at the
    /// top level with nothing in it; the row after it is the chain `zip > inner`, whose *path* is one
    /// level deeper than the place it is drawn. "The next row is deeper, so this folder has something
    /// in it" — which is how the twisty is decided, and rightly — would have given `empty` a chevron
    /// that opens nothing. So the comparison is between the depths the rows are drawn at, and this is
    /// the listing that tells the two apart.
    ///
    /// It also pins the rest of the shape a chain has to have from the tab's side: the merged row
    /// stands where `zip` stood, and the file inside it is one level in from *there* rather than at
    /// the two its path would say.
    #[test]
    fn an_empty_folder_beside_a_merged_chain_has_nothing_below_it() {
        use crate::fs::dir::{DirBuilder, FLAG_DIR};

        // Level by level, the way a deep walk hands a listing over.
        let mut builder = DirBuilder::new(r"C:\x");
        builder.push("empty", 0, 0, FLAG_DIR);
        builder.push("zip", 0, 0, FLAG_DIR);
        builder.push(r"zip\inner", 0, 0, FLAG_DIR);
        builder.push(r"zip\inner\f.txt", 1, 0, 0);

        let mut tab = Tab::new(r"C:\x");
        tab.flat = true;
        tab.flat_mode = FlatMode::Tree;
        tab.regroup = true;
        tab.apply(Arc::new(builder.finish(0)));

        let dir = tab.dir.clone().expect("a listing");
        let names: Vec<&str> = tab.order.iter().map(|&i| dir.name(i as usize)).collect();
        assert_eq!(names, ["empty", r"zip\inner", r"zip\inner\f.txt"]);
        assert_eq!(
            (tab.row_depth(0), tab.row_depth(1), tab.row_depth(2)),
            (0, 0, 1),
            "the chain stands where `zip` stood, and its file is one in from there"
        );
        assert_eq!(tab.row_merged(1), 1, "one folder merged into the chain");

        assert!(
            !tab.has_children_below(0),
            "`empty` was given a twisty by the chain next to it"
        );
        assert!(
            tab.has_children_below(1),
            "the chain has a file in it and no twisty to open it with"
        );
        assert!(!tab.has_children_below(2), "a file is not a folder");
    }

    /// The size of the selection is kept in step by every gesture that can change it.
    ///
    /// It is maintained rather than measured — see [`Tab::selected_size`] — so the thing worth testing
    /// is that no gesture forgets. Every assertion below compares the running figure against a fresh
    /// walk over the whole folder, which is the answer the status line would otherwise have to compute
    /// on every frame.
    #[test]
    fn a_selection_keeps_its_own_size() {
        use crate::fs::dir::{DirBuilder, FLAG_DIR};

        // Three files and a folder. The folder carries a size the way the find data does, and it must
        // not be counted: a directory's byte count is noise.
        let mut builder = DirBuilder::new(r"C:\here");
        builder.push("a.txt", 100, 0, 0);
        builder.push("b.txt", 20, 0, 0);
        builder.push("c.txt", 3, 0, 0);
        builder.push("sub", 4096, 0, FLAG_DIR);
        let mut tab = Tab::new(r"C:\here");
        tab.apply(Arc::new(builder.finish(0)));
        assert_eq!(tab.order.len(), 4, "everything is on show");

        // What a walk would say, over the same convention `selected_count` follows: every entry that
        // is selected, whether or not the filter is showing it.
        let walked = |tab: &Tab| -> u64 {
            let dir = tab.dir.as_ref().expect("a listing");
            (0..dir.len())
                .filter(|&entry| tab.selected[entry])
                .map(|entry| Tab::size_at(Some(dir), entry))
                .sum()
        };
        let check = |tab: &Tab, what: &str| {
            assert_eq!(tab.selected_size, walked(tab), "after {what}");
        };

        // One row at a time, and a folder among them.
        let row = |tab: &Tab, name: &str| {
            let dir = tab.dir.as_ref().expect("a listing");
            (0..tab.order.len())
                .find(|&at| tab.entry_at(at).is_some_and(|e| dir.name(e) == name))
                .expect("the row")
        };
        let (a, b, c, sub) = (
            row(&tab, "a.txt"),
            row(&tab, "b.txt"),
            row(&tab, "c.txt"),
            row(&tab, "sub"),
        );

        tab.select_only(a);
        assert_eq!(tab.selected_size, 100);
        check(&tab, "one click");

        tab.toggle(b);
        assert_eq!(tab.selected_size, 120, "and the second one added");
        check(&tab, "ctrl-click");
        tab.toggle(b);
        assert_eq!(tab.selected_size, 100, "and taken away again");
        check(&tab, "ctrl-click off");

        tab.select_only(sub);
        assert_eq!(tab.selected_size, 0, "a folder's own size is not a size");
        check(&tab, "a folder clicked");

        tab.select_only(a);
        tab.anchor = Some(a);
        tab.select_range_to(c);
        assert_eq!(tab.selected_size, 123, "a shift-click covers three files");
        check(&tab, "shift-click");

        tab.select_all();
        assert_eq!(tab.selected_size, 123, "and so does everything");
        check(&tab, "select all");

        tab.clear_selection();
        assert_eq!(tab.selected_size, 0);
        check(&tab, "clearing");

        // A rubber band, which recomputes from its own base rather than accumulating.
        tab.select_only(a);
        tab.band = Some(Band {
            base: tab.selected.clone(),
            anchor: egui::pos2(0.0, 0.0),
            current: egui::pos2(0.0, ROW_HEIGHT * 2.5),
        });
        tab.apply_band();
        check(&tab, "a band");
        assert!(
            tab.selected_size >= 100,
            "the band kept what it started with"
        );

        // And the listing being read again keeps the selection by name — and its size with it.
        let mut again = DirBuilder::new(r"C:\here");
        again.push("a.txt", 100, 0, 0);
        again.push("b.txt", 20, 0, 0);
        tab.band = None;
        tab.select_only(row(&tab, "a.txt"));
        tab.refresh();
        tab.apply(Arc::new(again.finish(0)));
        assert_eq!(tab.selected_count, 1, "a.txt came back selected");
        assert_eq!(tab.selected_size, 100);
        check(&tab, "a re-read");

        // Somewhere else is nothing selected at all.
        tab.go_to(PathBuf::from(r"C:\elsewhere"));
        assert_eq!(tab.selected_size, 0);
    }

    /// `@git` has three answers, and the one that matters is the middle one.
    ///
    /// Git answers a frame or two after the listing, and in between the filter is a question that
    /// cannot be evaluated. Excluding everything until then would empty the listing on every refresh —
    /// on every file operation, every `F5`, every time the watcher notices something — and then fill it
    /// again, which reads as the folder having been wiped. So an unanswered question excludes nothing,
    /// and `App::collect_git` rebuilds the order when the answer lands.
    ///
    /// The third answer is a folder outside a repository: **asked, and there is nothing here**, which
    /// keeps no rows. It is the one case that needs `git_answered` rather than `git` to tell it from the
    /// second.
    #[test]
    fn a_filter_on_git_waits_for_git_rather_than_emptying_the_listing() {
        use crate::fs::dir::DirBuilder;

        let mut builder = DirBuilder::new(r"C:\repo");
        for name in ["touched.rs", "untouched.rs"] {
            builder.push(name, 10, 0, 0);
        }
        let mut tab = Tab::new(r"C:\repo");
        tab.apply(Arc::new(builder.finish(0)));
        tab.filter = crate::fs::sort::CHANGED.to_owned();
        assert!(tab.filters_on_git(), "the line asks about git");

        // Asked and not answered: everything, because nothing here can say otherwise yet.
        tab.rebuild_order();
        assert_eq!(tab.order.len(), 2, "the wait emptied the listing");

        // Answered, and there is no repository: nothing has changed because nothing could have.
        tab.git_answered = true;
        tab.rebuild_order();
        assert_eq!(
            tab.order.len(),
            0,
            "a folder with no repository has no changes"
        );

        // Answered with a repository: the rows git has something to say about, and no others.
        tab.git = Some(Arc::new(crate::git::Repo::of([(
            "touched.rs",
            crate::git::State::Modified,
        )])));
        tab.rebuild_order();
        let names: Vec<String> = tab
            .order
            .iter()
            .map(|&i| {
                tab.dir
                    .as_ref()
                    .expect("a listing")
                    .name(i as usize)
                    .to_owned()
            })
            .collect();
        assert_eq!(names, ["touched.rs"]);

        // And the name half still applies on top of it.
        tab.filter = format!("{} .txt$", crate::fs::sort::CHANGED);
        tab.rebuild_order();
        assert_eq!(tab.order.len(), 0, "changed, but not a .txt");

        // With the word gone, the folder comes back whole.
        tab.filter.clear();
        tab.rebuild_order();
        assert_eq!(tab.order.len(), 2);
    }

    /// Arriving somewhere the breadcrumb still runs past selects the segment below it.
    #[test]
    fn arriving_at_a_folder_selects_the_child_the_breadcrumb_shows() {
        // One step up: `c` is on the bar and is where you just came from.
        let mut tab = Tab::new(r"C:\a\b\c");
        tab.navigate(r"C:\a\b");
        assert_eq!(tab.reveal.as_deref(), Some("c"));

        // Two at once, which is what clicking a segment does. The old rule only ever managed a
        // single step, because it asked whether the place it landed was the parent.
        let mut tab = Tab::new(r"C:\a\b\c");
        tab.navigate(r"C:\a");
        assert_eq!(tab.reveal.as_deref(), Some("b"));

        // Going *down* leaves nothing behind on the bar to point at.
        let mut tab = Tab::new(r"C:\a");
        tab.navigate(r"C:\a\b");
        assert_eq!(tab.reveal, None);

        // Sideways: the trail is replaced, so again there is nothing.
        let mut tab = Tab::new(r"C:\a\b\c");
        tab.navigate(r"D:\elsewhere");
        assert_eq!(tab.reveal, None);
    }

    /// The three ways of arriving all get it, because they all go through `go_to`.
    #[test]
    fn up_back_and_forward_all_highlight_off_the_trail() {
        let mut tab = Tab::new(r"C:\a\b\c");
        tab.go_up();
        assert_eq!(tab.path, PathBuf::from(r"C:\a\b"));
        assert_eq!(tab.reveal.as_deref(), Some("c"), "up left `c` behind");

        tab.go_up();
        assert_eq!(tab.reveal.as_deref(), Some("b"), "and `b` above that");
        // Going up *records* the arrival, so there is nothing forward of here to go to — the
        // history reads `c`, `b`, `a` and the cursor is on its last entry.
        assert!(!tab.can_go_forward());

        // Back down it, which is where `go_back` used to do this for itself.
        tab.go_back();
        assert_eq!(tab.path, PathBuf::from(r"C:\a\b"));
        assert_eq!(tab.reveal.as_deref(), Some("c"));
        tab.go_back();
        assert_eq!(tab.path, PathBuf::from(r"C:\a\b\c"));
        assert_eq!(tab.reveal, None, "arriving at the end of the trail");

        // And forward, which never highlighted anything before and now does.
        tab.go_forward();
        assert_eq!(tab.path, PathBuf::from(r"C:\a\b"));
        assert_eq!(tab.reveal.as_deref(), Some("c"));
    }

    /// A re-read of the same folder keeps what was selected; going somewhere else does not.
    ///
    /// Every file operation ends in a re-read, so a selection that does not survive one means
    /// copying a file and then having nothing selected to copy again. That is what made Ctrl+C
    /// then Ctrl+V work exactly once: the second Ctrl+C had an empty selection and copied nothing.
    #[test]
    fn a_re_read_keeps_the_selection_and_a_new_folder_does_not() {
        use crate::fs::dir::DirBuilder;

        let folder = PathBuf::from(r"C:\somewhere");
        let listing = |at: &std::path::Path, names: &[&str]| {
            let mut build = DirBuilder::new(at.to_path_buf());
            for name in names {
                build.push(name, 1, 0, 0);
            }
            Arc::new(build.finish(0))
        };
        /// The display position of a name, which is what the selection is indexed by.
        fn position_of(tab: &Tab, name: &str) -> usize {
            (0..tab.order.len())
                .find(|at| {
                    tab.entry_at(*at)
                        .and_then(|e| tab.dir.as_ref().map(|d| d.name(e) == name))
                        .unwrap_or(false)
                })
                .unwrap_or_else(|| panic!("`{name}` is not in the listing"))
        }

        let mut tab = Tab::new(folder.clone());
        tab.apply(listing(&folder, &["one.txt", "two.txt", "three.txt"]));
        tab.select_only(position_of(&tab, "two.txt"));
        assert_eq!(tab.selection_paths().len(), 1);

        // The same folder read again -- a refresh, or the tail of a file operation.
        tab.apply(listing(&folder, &["one.txt", "two.txt", "three.txt", "four.txt"]));
        let still: Vec<String> = tab
            .selection_paths()
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert_eq!(still, ["two.txt"], "the selection has to survive a re-read");
        assert!(tab.cursor.is_some(), "and the cursor has to go with it");

        // Something that has gone is simply not selected any more.
        tab.apply(listing(&folder, &["one.txt", "three.txt"]));
        assert_eq!(tab.selection_paths().len(), 0);

        // A different folder is a different set of files, even when a name matches.
        tab.apply(listing(&folder, &["one.txt", "two.txt"]));
        tab.select_only(position_of(&tab, "two.txt"));
        let elsewhere = PathBuf::from(r"C:\elsewhere");
        tab.go_to(elsewhere.clone());
        tab.apply(listing(&elsewhere, &["two.txt"]));
        assert_eq!(
            tab.selection_paths().len(),
            0,
            "a name that happens to match somewhere else is not the same file"
        );
    }

    /// A temp-free listing of whatever names you name, for the two tests below.
    #[cfg(test)]
    fn listing(folder: &std::path::Path, names: &[&str]) -> Arc<crate::fs::dir::Dir> {
        use crate::fs::dir::DirBuilder;
        let mut build = DirBuilder::new(folder.to_path_buf());
        for name in names {
            build.push(name, 1, 0, 0);
        }
        Arc::new(build.finish(0))
    }

    /// Where a leaf is in the display order, which is what the selection is indexed by.
    #[cfg(test)]
    fn leaf_at(tab: &Tab, leaf: &str) -> usize {
        (0..tab.order.len())
            .find(|&at| {
                tab.entry_at(at)
                    .and_then(|e| tab.dir.as_ref().map(|d| d.leaf(e) == leaf))
                    .unwrap_or(false)
            })
            .unwrap_or_else(|| panic!("`{leaf}` is not in the listing"))
    }

    /// The leaf the open rename field is attached to.
    #[cfg(test)]
    fn renaming_leaf(tab: &Tab) -> String {
        let (entry, _) = tab.renaming.clone().expect("a rename in progress");
        tab.dir.as_ref().expect("a listing").leaf(entry).to_owned()
    }

    /// A rename in progress survives the folder being re-read, and stays on the same *file*.
    ///
    /// The two halves of naming a newly made file collide by construction: the rename starts when
    /// a re-read brings the file in, and making a file is exactly what has [`crate::watch`] asking
    /// for another re-read. `renaming` holds an *entry index*, so left alone across that second one
    /// the field is still on screen, still committing, and commits the typed name onto whichever
    /// file has landed at that index.
    #[test]
    fn an_open_rename_follows_its_file_across_a_re_read() {
        let folder = PathBuf::from(r"C:\somewhere");
        let mut tab = Tab::new(folder.clone());
        tab.apply(listing(&folder, &["a.txt", "target.txt", "z.txt"]));
        tab.select_only(leaf_at(&tab, "target.txt"));
        tab.begin_rename();
        // Half-typed, which is the state that has something to lose.
        tab.renaming.as_mut().expect("a rename").1 = "half-typ".to_owned();
        tab.rename_fresh = false;

        // A re-read with a new file in front of it, which is what moves the index.
        tab.apply(listing(&folder, &["new.txt", "a.txt", "target.txt", "z.txt"]));
        assert_eq!(
            renaming_leaf(&tab),
            "target.txt",
            "the rename field ended up on a different file"
        );
        assert_eq!(
            tab.renaming.as_ref().expect("a rename").1,
            "half-typ",
            "what was typed into the field was lost"
        );
        assert!(
            tab.rename_fresh,
            "the field moved to another row -- a different egui id, with none of the old focus -- \
             and nothing asked for the caret back"
        );

        // A re-read that leaves it where it was asks for nothing, so a caret mid-name stays put.
        tab.rename_fresh = false;
        tab.apply(listing(&folder, &["new.txt", "a.txt", "target.txt", "z.txt"]));
        assert_eq!(renaming_leaf(&tab), "target.txt");
        assert!(
            !tab.rename_fresh,
            "the row did not move and the caret was thrown to the end anyway"
        );

        // And the file going takes the field with it: there is nothing left to commit onto.
        tab.apply(listing(&folder, &["new.txt", "a.txt", "z.txt"]));
        assert!(
            tab.renaming.is_none(),
            "a rename field left open over a file that has gone"
        );
    }

    /// The file the shell's `New >` just made arrives selected with its name open for editing.
    ///
    /// Found by elimination, because the shell will not say what it made and the name it picks is
    /// localised and may already have been taken. See [`Tab::name_the_new`].
    #[test]
    fn the_file_the_shell_just_made_opens_for_renaming() {
        let folder = PathBuf::from(r"C:\somewhere");
        let mut tab = Tab::new(folder.clone());
        tab.apply(listing(&folder, &["a.txt", "b.txt"]));
        tab.select_only(leaf_at(&tab, "a.txt"));

        // What `App::draw_menu` writes down before handing the verb to the shell.
        tab.name_the_new = Some(tab.names());

        // The watcher's re-read, with whatever the shell decided to call it.
        let made = "Nouveau document texte (2).txt";
        tab.apply(listing(&folder, &["a.txt", "b.txt", made]));
        assert_eq!(
            renaming_leaf(&tab),
            made,
            "the row that was not there before is the one to name"
        );
        assert_eq!(
            tab.renaming.as_ref().expect("a rename").1,
            made,
            "the field starts from the name the file actually has"
        );
        assert_eq!(
            tab.selected_count, 1,
            "and it is the only thing selected, whatever was selected before"
        );
        assert!(tab.scroll_to_cursor, "a new row can be row 500");
        assert!(
            tab.name_the_new.is_none(),
            "the snapshot has to be consumed, or the next thing any program writes here opens a \
             rename field of its own"
        );

        // A listing with nothing new in it consumes the snapshot too, rather than lying in wait.
        tab.name_the_new = Some(tab.names());
        tab.renaming = None;
        tab.apply(listing(&folder, &["a.txt", "b.txt", made]));
        assert!(tab.renaming.is_none(), "nothing was created and a rename opened");
        assert!(tab.name_the_new.is_none());
    }

    /// A snapshot means nothing once the tab is looking at something else.
    #[test]
    fn leaving_the_folder_drops_the_snapshot() {
        let folder = PathBuf::from(r"C:\somewhere");
        let mut tab = Tab::new(folder.clone());
        tab.apply(listing(&folder, &["a.txt"]));

        tab.name_the_new = Some(tab.names());
        let elsewhere = PathBuf::from(r"C:\elsewhere");
        tab.go_to(elsewhere.clone());
        assert!(tab.name_the_new.is_none(), "carried to another folder");
        // Where every row would otherwise have looked new.
        tab.apply(listing(&elsewhere, &["x.txt", "y.txt"]));
        assert!(tab.renaming.is_none());

        // The flat toggle is the other one: a name means `file.txt` on one side of it and
        // `sub\file.txt` on the other, so nothing would match and every row would look new.
        tab.name_the_new = Some(tab.names());
        tab.toggle_flat(FlatMode::Tree, false);
        assert!(tab.name_the_new.is_none(), "carried across the flat toggle");
    }

    #[test]
    fn navigating_after_back_drops_the_future() {
        let mut tab = Tab::new("/a");
        tab.navigate("/a/b");
        tab.navigate("/a/b/c");
        assert!(tab.can_go_back());

        tab.go_back();
        assert_eq!(tab.path, PathBuf::from("/a/b"));
        assert!(tab.can_go_forward());

        tab.navigate("/a/b/d");
        assert!(
            !tab.can_go_forward(),
            "going somewhere new has to replace the forward trail"
        );
        assert_eq!(tab.history, ["/a", "/a/b", "/a/b/d"].map(PathBuf::from));
    }

    #[test]
    fn walking_up_keeps_the_trail_on_the_breadcrumb() {
        let mut tab = Tab::new("/a/b/c");
        assert_eq!(tab.trail, PathBuf::from("/a/b/c"));

        // Up: the folder just left is still on the bar, which is the point.
        tab.go_up();
        assert_eq!(tab.path, PathBuf::from("/a/b"));
        assert_eq!(tab.trail, PathBuf::from("/a/b/c"));

        // And again -- two levels up still shows all three.
        tab.go_up();
        assert_eq!(tab.path, PathBuf::from("/a"));
        assert_eq!(tab.trail, PathBuf::from("/a/b/c"));

        // Back down onto the trail: the trail is unchanged, so nothing on the bar moves
        // while the bold segment walks along it.
        tab.navigate("/a/b");
        assert_eq!(tab.trail, PathBuf::from("/a/b/c"));

        // Deeper than the trail extends it.
        tab.navigate("/a/b/c/d");
        assert_eq!(tab.trail, PathBuf::from("/a/b/c/d"));
    }

    #[test]
    fn stepping_off_the_trail_replaces_it() {
        let mut tab = Tab::new("/a/b/c");
        tab.go_up();

        // A sibling is not an ancestor, however much of the path it shares.
        tab.navigate("/a/b/x");
        assert_eq!(tab.trail, PathBuf::from("/a/b/x"));

        // Nor is a folder whose name merely starts the same way: `starts_with` compares
        // components, so `/a/bb` is not under `/a/b`.
        tab.navigate("/a/bb");
        assert_eq!(tab.trail, PathBuf::from("/a/bb"));
    }

    #[test]
    fn this_pc_is_up_from_everywhere() {
        // The empty path is This PC, and the breadcrumb always starts there -- so going to
        // it is walking up, and the trail stays.
        let mut tab = Tab::new("/a/b");
        tab.navigate(PathBuf::new());
        assert!(tab.path.as_os_str().is_empty());
        assert_eq!(tab.trail, PathBuf::from("/a/b"));

        // Out of This PC to a different root: nothing shared, so the trail goes.
        tab.navigate("/z");
        assert_eq!(tab.trail, PathBuf::from("/z"));
    }

    #[test]
    fn a_duplicate_shows_the_same_bar() {
        let mut tab = Tab::new("/a/b/c");
        tab.go_up();
        let copy = tab.duplicate();
        assert_eq!(copy.path, PathBuf::from("/a/b"));
        assert_eq!(
            copy.trail,
            PathBuf::from("/a/b/c"),
            "a duplicate that lost the trail would show a different bar from the tab it \
             was copied from"
        );
    }

    #[test]
    fn navigating_to_where_you_are_is_not_history() {
        let mut tab = Tab::new("/a");
        tab.navigate("/a");
        assert_eq!(tab.history.len(), 1);
    }

    #[test]
    fn history_stops_growing() {
        let mut tab = Tab::new("/0");
        for i in 1..400 {
            tab.navigate(format!("/{i}"));
        }
        assert_eq!(tab.history.len(), 256);
        assert_eq!(tab.at, 255, "the cursor has to follow the truncation");
        assert_eq!(tab.history[tab.at], tab.path);
    }

    #[test]
    fn closing_the_last_tab_reports_the_pane_is_empty() {
        let mut pane = Pane::new(0, Tab::new("/a"));
        assert!(!pane.close_tab(0));
    }

    #[test]
    fn closing_a_tab_keeps_the_active_one_active() {
        let mut pane = Pane::new(0, Tab::new("/a"));
        pane.tabs.push(Tab::new("/b"));
        pane.tabs.push(Tab::new("/c"));
        pane.active = 2;

        assert!(pane.close_tab(0));
        assert_eq!(pane.active, 1, "still looking at /c");
        assert_eq!(pane.tab().path, PathBuf::from("/c"));

        assert!(pane.close_tab(1));
        assert_eq!(pane.tab().path, PathBuf::from("/b"));
    }
}
