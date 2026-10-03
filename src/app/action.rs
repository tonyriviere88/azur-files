//! Everything the interface can ask for, and nothing that carries it out.
//!
//! The drawing code never mutates structure — it pushes one of these, and [`super::App::apply`]
//! performs it after the frame, when nothing is borrowed. See the module header on [`super`] for
//! why that is what makes "drag this tab into a split that does not exist yet" expressible.

use super::*;

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
    /// Count what is inside every folder on show, and draw each row's share of the total — or stop.
    ///
    /// Unlike [`Self::ToggleFlat`] beside it, nothing is re-read: the listing is the same listing and
    /// this adds a figure to a cell that was blank. What it does start is a tree walk per folder, off
    /// the UI thread — see [`crate::sizes`]. Per tab, and it survives a navigation, which is the one
    /// view setting that does; [`crate::pane::Tab::sizes`] is where that exception is argued.
    ToggleSizes(PaneId),
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
    /// Judge a folder as it opens and show it as tiles if it is mostly pictures, or stop.
    ///
    /// The window's preference, and the one on this list that deliberately **does nothing to what is
    /// on screen** — see [`crate::pane::AutoTiles`]. `SetFlatMode` and `SetRegroup` above take effect
    /// at once on every pane, because what they change is how a listing that is already here is
    /// arranged; this changes how the *next* folder opens. A rule about opening that re-arranged the
    /// folders already open would move the thing somebody is reading, and would then be arguing with
    /// the switch four points to the left of it on the same bar.
    ///
    /// Ticked in the view switch's own context menu — see [`crate::ui::filelist::tiles_menu`], which
    /// is also where [`Self::SetTilesThreshold`] is dragged.
    SetAutoTiles(bool),
    /// And how much of a folder has to be pictures for that, as a percentage. The other half of
    /// [`Self::SetAutoTiles`], with everything said there applying — including that it changes
    /// nothing on screen.
    SetTilesThreshold(f32),
    /// Write `/` rather than `\` between the parts of a path in the path field, or stop. The
    /// window's preference again, and the path field's own context menu is where it is ticked —
    /// see [`crate::ui::breadcrumb::slash_menu`].
    SetForwardSlashes(bool),
    /// Show one of the listings a *name* cannot ask for, or go back to the whole folder.
    ///
    /// What the filter box's funnel menu offers, and what the status line's `N changed` asks for —
    /// see [`crate::pane::Lens`]. **More than the filter, because either half alone answers half the
    /// question**: the lens over a folder's own children finds only what is in *that* folder, so the
    /// flatten comes with it, and pictures come with the tiles as well.
    SetLens {
        pane: PaneId,
        lens: Option<crate::pane::Lens>,
    },
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
            Self::ToggleSizes(_) => "ToggleSizes",
            Self::SetFlatMode(_) => "SetFlatMode",
            Self::SetView { .. } => "SetView",
            Self::SetRegroup(_) => "SetRegroup",
            Self::SetAutoTiles(_) => "SetAutoTiles",
            Self::SetTilesThreshold(_) => "SetTilesThreshold",
            Self::SetForwardSlashes(_) => "SetForwardSlashes",
            Self::SetLens { .. } => "SetLens",
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
pub(super) struct Asking {
    pub(super) token: u64,
    pub(super) pane: PaneId,
    pub(super) at: egui::Pos2,
    pub(super) items: Vec<PathBuf>,
    pub(super) folder: PathBuf,
    /// How much of a menu was asked for, so that a slow one can be asked for again with less.
    pub(super) depth: crate::shell::menu::Depth,
    /// The pass this was asked for in, so the click that asked for it is not also the click
    /// that cancels it — see [`App::pump_asking`].
    pub(super) since: u64,
    /// When it was asked for, for the deadline in [`App::pump_asking`].
    pub(super) asked: std::time::Instant,
}
