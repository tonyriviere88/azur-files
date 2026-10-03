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

/// The listings the filter box's funnel offers — **the questions a name cannot answer**.
///
/// A filter box narrows by what a row is *called*, and that is the right question nearly always:
/// see `azur_egui_theme::filter`, whose syntax the box speaks and which this program does not
/// extend. These two are not about the name at all. They are about what git says about a file and
/// what kind of file it is, so neither is something anybody can type — and each is worth two more
/// gestures besides, because a listing of what changed or of every picture is a *view* rather than
/// a filter: both want the folder's whole tree, and pictures want to be looked at as pictures.
///
/// **One at a time, and the box still composes on top.** A lens is a different question from the
/// name, not a second answer to it, so `Show images only` with `swatch` typed in the box is every
/// picture whose path says `swatch` — which is what the old `@git .rs$` did, without the word
/// having to be typed or remembered.
///
/// Per tab, and dropped by [`Tab::go_to`] exactly as the filter and [`ViewMode`] are: what has
/// changed *here* is not a question about the folder you open next.
///
/// It was a **word in the box** — `@git`, taken off the line before the name test saw it. What that
/// bought was composition, and this keeps that; what it cost was every reader who never found it,
/// since a filter field looks exactly the same whether or not it has a private syntax. A menu on the
/// control that filters is where somebody looking for "show me what changed" looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lens {
    /// Every row git has something to say about: changed, staged, untracked, conflicted — the same
    /// set the status line counts as `N changed`, and a folder is in it when anything under it is,
    /// because a folder filtered out is a folder you cannot open to reach what is inside it.
    ///
    /// One thing about it is not obvious and is deliberate: git answers a frame or two *after* the
    /// listing, and until then the question cannot be evaluated. See [`Tab::git_answered`].
    Git,
    /// Every picture under the folder, by the type its extension names — `Kind::Image` in
    /// [`crate::fs::fmt`], which is the same answer the Type column prints and so cannot drift from
    /// what the listing says a row *is*.
    ///
    /// Folders are kept when a picture is somewhere under them, for the reason [`Self::Git`]'s are:
    /// in a tree the folder is the only way to what it holds. Nothing else is — a gallery with
    /// `.txt` in it is not a gallery.
    Images,
}

impl Lens {
    /// The two, in the order the menu lists them.
    pub const ALL: [Self; 2] = [Self::Git, Self::Images];

    /// What the menu entry says. **A sentence about the listing, not a name for the filter**: the
    /// entry does something rather than being a setting whose value is "git".
    pub fn label(self) -> &'static str {
        match self {
            Self::Git => "Show git changes",
            Self::Images => "Show images only",
        }
    }

    /// What the listing says when this lens leaves nothing at all.
    ///
    /// **A lens that keeps no rows is the one case where an empty listing is not a dead end**, and it
    /// is worth a sentence of its own: "everything here is hidden" is the answer to a different
    /// question, and it sends a reader to `Ctrl+H` for a folder that has plenty in it. Beside
    /// [`Self::label`] so that what the menu asked for and what the pane reports back cannot come to
    /// disagree — see [`crate::ui::filelist`], which is the only caller.
    pub fn nothing_found(self) -> &'static str {
        match self {
            Self::Git => "Nothing here has changed",
            Self::Images => "No pictures here",
        }
    }

    /// For `--lens=<word>` and nothing else, so a capture run can open showing one of these — it is
    /// behind a menu, and a capture run has no pointer. **Not** for the settings file, unlike
    /// [`FlatMode::as_str`]: a lens is a question about the folder in front of you and is not
    /// remembered anywhere.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Images => "images",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|lens| lens.as_str().eq_ignore_ascii_case(text))
    }

    /// Whether this lens wants the tiles rather than the rows.
    ///
    /// Pictures do, and it is the one place in this program that turns the large-icon view on
    /// without the switch being pressed: a listing of photographs whose only column of interest is
    /// the name is the case [`ViewMode::Icons`] exists for, and asking for it and then having to
    /// find the switch would be the same two-gestures-too-many the `@git` word was.
    pub fn wants_tiles(self) -> bool {
        matches!(self, Self::Images)
    }
}

/// How a folder is drawn: as a table of rows, or as a grid of large icons.
///
/// **Both are the same listing**, exactly as [`FlatMode`]'s two are: the order, the selection, the
/// cursor and the filter are all the view's and none of them changes with this. What changes is the
/// arithmetic that puts a row on screen — [`crate::ui::filelist`] for one, [`crate::ui::grid`] for
/// the other — so switching costs a frame and never a re-read.
///
/// **Per tab, and a question asked of the folder in front of you** — which is why it is neither
/// remembered between sessions nor carried to the next folder. [`Tab::go_to`] puts it back to
/// [`Self::Details`], exactly as it does [`Tab::flat`] and the filter, and for the same reason: a
/// folder of photographs and a folder of source want opposite answers, so an answer about one is not
/// an answer about the other. `Details` is therefore what every tab opens as, every time.
///
/// The other way round — a window preference, saved, applied everywhere, which is what [`FlatMode`]
/// is — was tried first and is wrong here for that reason. Which way you want a *tree* shown is a
/// habit; whether a folder is worth looking at as pictures is a fact about the folder.
///
/// **[`AutoTiles`] is the one thing that reads that fact for you**, and it is not a setting for this
/// value: it is a rule applied to the listing as it lands, so what is remembered between sessions is
/// the rule and never the answer. See [`Tab::choose_view`].
///
/// A refresh keeps it, because a refresh is the same folder read again rather than a different one,
/// and so does [`Tab::toggle_flat`] — flattening is another question about the folder you are already
/// looking at.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ViewMode {
    /// The details view: a sticky header over virtualised rows. See [`crate::ui::filelist`].
    #[default]
    Details,
    /// Large icons: a grid of tiles, each with the shell's own thumbnail on it. See
    /// [`crate::ui::grid`].
    Icons,
}

impl ViewMode {
    /// The other one — what the switch on the status line does.
    pub fn toggled(self) -> Self {
        match self {
            Self::Details => Self::Icons,
            Self::Icons => Self::Details,
        }
    }

    #[inline]
    pub fn is_icons(self) -> bool {
        matches!(self, Self::Icons)
    }
}

/// When a folder opens as tiles without anybody pressing the switch: **a rule, not a remembered
/// mode.**
///
/// [`ViewMode`] argues that whether a folder is worth looking at as pictures is a fact about *that
/// folder* and so is never carried anywhere — and that argument is what leaves the switch to be
/// pressed by hand in every folder of photographs somebody opens. This is the other way to honour
/// it: the fact is *read off the folder* as its listing lands. A folder whose rows are mostly
/// pictures opens as tiles; the folder you open out of it is judged for itself, from nothing.
///
/// So what the settings file remembers is this rule, and never a `view=`. See
/// [`crate::config::Config::auto_tiles`], which is where that distinction is spelled out again from
/// the file's side.
///
/// # Four properties worth stating, because each was a choice
///
/// **On arrival only.** Ticking the box or dragging the slider changes nothing that is already on
/// screen — see [`crate::app::Action::SetAutoTiles`]. A rule about how folders *open* that reached
/// back and re-arranged the folders already open would be a setting that moves the thing you are
/// reading, and the switch on the status line would then be arguing with it.
///
/// **A picture is whatever would show as one**, which is two questions rather than one:
/// [`crate::fs::fmt::shows_a_picture`] for the types that are pictures by name, and
/// [`crate::shell::providers`] for the ones only this machine can answer for — a `.pdf` where a reader
/// is installed, a `.psd`, a `.3dr` where Cyclone 3DR is. Both, always. A rule that counted only the
/// first would call a folder of documents 0% on a machine that draws every one of them, which is a
/// rule about this program's own table rather than about the folder.
///
/// **On by default**, which is not this file's usual answer and is the same exception
/// [`crate::config::Config::regroup`] is: the rule earns its default by what it does when it is
/// *wrong*, and what it does when it is wrong is show a folder as tiles that you wanted as rows —
/// one click on the switch four points to the left of the menu, in the folder you are already looking
/// at. Against that, off by default means every folder of photographs anybody opens needs that same
/// click, forever, and most people never find out it could have been otherwise. A default that costs
/// one click when it guesses wrong is not the same kind of default as one that changes what a path
/// bar writes.
///
/// **Something has to be a picture.** At a threshold of 0 the rule is "as soon as there is one",
/// which is what the bottom of the slider should mean — not "every folder, including the empty
/// ones", which is what a bare `share >= 0` would give.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AutoTiles {
    /// Whether a folder is judged at all when it opens.
    pub on: bool,
    /// How much of the folder has to be pictures, as a **percentage** of the rows on show.
    ///
    /// A percentage rather than a fraction because it is what the slider shows and what the
    /// settings file carries — `tiles_threshold=60` is a line somebody can read and edit, and
    /// `0.6` is a line they have to decode.
    pub threshold: f32,
}

/// How much of a folder has to be pictures before it opens as tiles, until somebody moves the
/// slider.
///
/// A folder is not a gallery because a third of it happens to be screenshots, and it plainly is one
/// at nine tenths. Sixty is above "a fair few" and below "nearly all": it catches a camera's
/// download folder with its stray `.txt` in it, and leaves a source folder holding a dozen icons in
/// the details view where its names and dates are.
pub const TILES_THRESHOLD: f32 = 60.0;

impl Default for AutoTiles {
    fn default() -> Self {
        Self {
            on: true,
            threshold: TILES_THRESHOLD,
        }
    }
}

impl AutoTiles {
    /// Whether a folder of `pictures` picture files out of `rows` rows is one to open as tiles.
    ///
    /// `false` for a folder with nothing in it and for one with no picture at all, whatever the
    /// threshold says — see the type's own doc for why 0% cannot mean "always".
    pub fn reached(self, pictures: usize, rows: usize) -> bool {
        if !self.on || pictures == 0 || rows == 0 {
            return false;
        }
        pictures as f32 / rows as f32 * 100.0 >= self.threshold
    }

    /// The threshold as it may be stored: anything outside the slider's own range is not a
    /// percentage, and a hand-edited settings file is allowed to be wrong without changing what the
    /// rule means.
    pub fn clamped(threshold: f32) -> f32 {
        if threshold.is_finite() {
            threshold.clamp(0.0, 100.0)
        } else {
            TILES_THRESHOLD
        }
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
    /// Where a shortcut row points, and what it runs it with, by entry index — see
    /// [`crate::shell::links`].
    ///
    /// A map rather than a column, because unlike an icon this is only ever wanted for a
    /// handful of rows: a `.lnk` or a reparse point. A `Vec` would be an `Option<Target>` per
    /// row, which on a flattened tree of 200,000 is megabytes to say "not a shortcut" 199,990
    /// times.
    ///
    /// **A key that is present means asked.** `None` is an answer as much as `Some` is — an
    /// unreadable shortcut, or one pointing at something with no path — and both stop the row
    /// asking again. Dropped with the folder, exactly like [`Tab::file_icons`].
    pub links: std::collections::HashMap<u32, Option<crate::shell::links::Target>>,

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
    /// no furniture — but [`Lens::Git`] has to, and its two answers are opposite. Not answered
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
    /// A number no build of [`Tab::order`] has had before.
    ///
    /// What [`crate::ui::grid::Layout`] validates its cached geometry against. The length of the
    /// order is not enough on its own — a click on a column header leaves it exactly as long and
    /// every row somewhere else — and a grid that trusted the length would go on drawing the old
    /// arrangement with the new rows in it, which is the sort of wrongness that looks like a
    /// scrolling bug.
    pub order_gen: u64,
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
    /// The other half of the filter: which of the funnel menu's listings is on show, if any.
    ///
    /// Beside [`Tab::filter`] and not part of it, because the two are different questions and a row
    /// has to pass both — see [`Lens`]. No settling delay, unlike the text: it arrives whole, from a
    /// menu, so there are no keystrokes to wait for.
    pub lens: Option<Lens>,
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

    /// Whether this tab is showing its folder as rows or as tiles. See [`ViewMode`].
    pub view_mode: ViewMode,
    /// Whether the next listing to arrive is the folder being **opened**, and so may still choose
    /// which of the two views it opens in. See [`AutoTiles`] and [`Tab::choose_view`].
    ///
    /// **The whole of "only when opening a folder" is this one flag.** A listing arrives for four
    /// different reasons and only one of them is an opening: a navigation, an `F5`, a file operation
    /// that ended in a re-read, and [`crate::watch`] noticing somebody else write into the folder.
    /// The last three are *the same folder again*, and a rule that fired on those would take the
    /// view back off anybody who had pressed the switch — silently, and at the moment a build
    /// happened to touch the folder they were reading.
    ///
    /// So it is set by [`Tab::new`] and by [`Tab::go_to`], which are the two ways a tab comes to be
    /// pointing at a folder it has not shown yet, and consumed by the first listing that lands.
    pub opening: bool,
    /// Where the tiles go, when it is showing tiles.
    ///
    /// Cached rather than worked out per frame, because in a tree it is not arithmetic: a folder's
    /// files are gathered into a grid of their own and the blocks have to be walked to say how tall
    /// the whole thing is. Rebuilt when the order or the width changes and not otherwise — see
    /// [`crate::ui::grid::Layout::ensure`].
    pub grid: crate::ui::grid::Layout,

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

/// A number no build of any tab's display order has had before. See [`Tab::order_gen`].
///
/// Its own counter rather than a share of [`next_view`]'s, because the two are not the same clock:
/// one order is built many times per view — every sort, every filter, every twisty — and a reader
/// comparing generations must not be able to mistake one for the other.
fn next_order_gen() -> u64 {
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
            order_gen: next_order_gen(),
            tree: Vec::new(),
            // Type, not Name: a folder read by type comes up grouped — every source
            // file together, every image together — and within a group it is still in
            // name order, so nothing is harder to find than it would have been.
            sort_by: Column::Type,
            ascending: true,
            filter: String::new(),
            filter_at: None,
            lens: None,
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
            // Details, always: nothing is remembered and nothing is inherited. See [`ViewMode`].
            view_mode: ViewMode::default(),
            // And this folder has not been looked at yet, so the listing that lands may still make
            // it the tiles. See [`AutoTiles`].
            opening: true,
            grid: crate::ui::grid::Layout::default(),
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
        // Rows or tiles, because "as it currently looks" is the whole of what a duplicate is — and
        // this is the *same folder*, which is the one thing [`ViewMode`] is a question about. The
        // moment the copy navigates anywhere it goes back to the details view, like any other tab.
        //
        // The grid's own geometry is deliberately not carried across: it is a cache over an order
        // this tab is about to build for itself, and one frame of arithmetic beats a stale copy.
        tab.view_mode = self.view_mode;
        // And it is not opening: the view above is the one the original is showing, chosen or judged
        // already, and a copy of a folder somebody has switched to rows must not be judged back into
        // tiles. See [`Tab::opening`].
        tab.opening = false;
        // And which listing the funnel is showing, for the same reason as the flatten it comes with:
        // a duplicate of a pane showing what changed is a pane showing what changed. See [`Lens`].
        tab.lens = self.lens;
        // Open the same way, but reading for itself — see `Preview::duplicate`, which explains
        // why the decoded content is deliberately not carried across.
        tab.preview = self.preview.duplicate();
        if let Some(dir) = &self.dir {
            tab.apply(dir.clone());
        }
        tab
    }

    // ---- Navigation ----------------------------------------------------

    // ---- The listing ---------------------------------------------------

    // ---- Selection -----------------------------------------------------

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
mod tests;

// One `impl Tab` per concern, along the banners this file used to carry. The types and the two
// constructors stay here; what a tab *does* is next door.
mod listing;
mod navigate;
mod select;
