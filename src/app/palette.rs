//! The command palette's half that knows what the window can do: the catalogue, what narrows it,
//! and what a picked entry turns into. `Ctrl+Shift+P`.
//!
//! The drawing is [`crate::ui::palette`]. This half builds the rows it draws, from the same state the
//! menus and the shortcuts read, and turns the one picked into an ordinary [`Action`] — so a command
//! run from here is the same code path as its shortcut and its menu entry, and the journal the click
//! tests read cannot tell them apart.
//!
//! # What is on it
//!
//! **Every command that makes sense without a gesture to go with it**: the tabs and panes, getting
//! about, both kinds of view setting (the pane's and the window's), what can be done to the selection,
//! and the window itself. Not the gesture internals — a tab drag, a drop, a rename being committed —
//! which mean nothing without the pointer that started them, and not the shell's own context-menu
//! verbs: asking for those takes up to a second per selection, and running an arbitrary one of them
//! because two words matched it is what `probe_invokable` did to this repository. `Show context menu`
//! is the way to those, and it puts up the real menu.
//!
//! **Which is which is decided once, in [`listed`]**, an exhaustive `match` over [`Action`]: a variant
//! added to the enum does not compile until it has been put on the palette or said not to belong.
//! `every_listed_action_is_on_the_palette` holds the two to each other — see [`listed`].
//!
//! **An entry that cannot apply is left off rather than greyed**: Paste with nothing on the
//! clipboard, Rename with nothing selected, the diff's switches outside a diff. The list is built when
//! the palette opens, so what it says is what is true of the pane the keyboard was in at that moment.
//!
//! # Matching
//!
//! `azur_egui_theme::filter::Query`, the filter box's own: every word has to appear, in any order,
//! in the entry's label, its heading or its aliases. No fuzzy matcher and no ranking — the catalogue's
//! order *is* the order, so an entry is always found in the same place, and the commands picked most
//! recently go at the head of an empty query. See [`RECENT`].
//!
//! After two characters the places come too, under the commands: the open tabs, the folders
//! [`crate::history`] and the bookmarks offer the path field, the sidebar's places and the drives.
//! Nothing for them is read from the disk.

use super::*;

use crate::ui::palette::{Outcome, Row};

/// How many recently picked commands are remembered, and listed first when nothing is typed.
const RECENT: usize = 6;

/// How many folders out of the history and the bookmarks are offered.
const RECALLED: usize = 8;

/// The headings, in the order the catalogue lists them.
const TABS: &str = "Tabs and panes";
const NAVIGATION: &str = "Navigation";
const VIEW: &str = "View";
const PREFERENCES: &str = "Preferences";
const FILES: &str = "Files";
const WINDOW: &str = "Window";
const RECENTLY: &str = "Recently used";
const GO_TO: &str = "Go to";

/// One thing the palette can do.
///
/// Its own type rather than an [`Action`], for two reasons. An action names a pane and a path, and
/// those are resolved when the entry is *picked* — see [`App::palette_action`] — not when the list
/// was built. And a command has to have a name that outlives the session, for [`RECENT`]: see
/// [`Command::key`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    // Tabs and panes
    NewTab,
    ReopenTab,
    CloseTab,
    NextTab,
    PreviousTab,
    SplitRight,
    SplitBelow,
    FolderDiff,
    DiffShow(crate::diff::Show),
    // Navigation
    Back,
    Forward,
    Up,
    Refresh,
    EditPath,
    Filter,
    Bookmark,
    BookmarkGroup,
    DiscoverNetwork,
    // View: this pane
    Tiles,
    Sizes,
    Console,
    Preview,
    Flatten,
    Lens(Option<crate::pane::Lens>),
    Sort(Column),
    // View: the window's
    Hidden,
    Sidebar,
    FlatMode(crate::pane::FlatMode),
    Regroup,
    AutoTiles,
    SearchSubfolders,
    ForwardSlashes,
    SuggestBookmarks,
    SuggestHistory,
    ShowColumn(Column),
    ResetColumns,
    ShowSection(crate::ui::sidebar::Section),
    ResetSections,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    // Files
    Open,
    OpenNewTab,
    OpenRight,
    OpenBelow,
    Cut,
    Copy,
    Paste,
    PasteLinks(crate::shell::links::Make),
    CopyPaths,
    Rename,
    Batch(crate::pane::batch::Mode),
    NewFolder,
    Delete,
    DeletePermanently,
    Undo,
    Redo,
    SelectAll,
    ClearSelection,
    InvertSelection,
    SelectSameExtension,
    Compare,
    OpenTerminal,
    Reveal,
    ContextMenu,
    // Window
    SpanScreens,
    ResetSize,
    Minimize,
    Maximize,
    Theme(crate::theme::Palette),
    CheckForUpdate,
    UpdateEvery(crate::update::Every),
    InstallUpdate,
    ReleasePage,
    SkipUpdate,
    WinKey,
    CloseWindow,
    // Go to
    Folder { path: PathBuf, recalled: bool },
    Tab { pane: PaneId, tab: usize },
}

impl Command {
    /// The name [`RECENT`] is written down under, or `None` for a place — a folder is the history's
    /// to remember, and a tab is gone with the session.
    ///
    /// The variant as `Debug` spells it, which is a name and not a position: a command added later
    /// cannot reinterpret what an older file remembered, and one renamed is simply forgotten.
    pub(crate) fn key(&self) -> Option<String> {
        match self {
            Self::Folder { .. } | Self::Tab { .. } => None,
            command => Some(format!("{command:?}")),
        }
    }

    /// Words an entry is also found by, beyond its label and its heading. The few where the label is
    /// this program's word and somebody typing will reach for another.
    fn aliases(&self) -> &'static str {
        match self {
            Self::Refresh => "reload",
            Self::EditPath => "address location",
            Self::Filter => "find search",
            Self::Bookmark | Self::BookmarkGroup => "favourite favorite pin",
            Self::Tiles => "icons thumbnails grid details",
            Self::Sizes => "measure space",
            Self::Console => "terminal shell",
            Self::Preview => "quick look",
            Self::Flatten => "tree recursive",
            Self::Sidebar => "sidebar",
            Self::FolderDiff | Self::Compare => "compare",
            Self::Delete | Self::DeletePermanently => "remove",
            Self::Theme(_) => "palette colour color dark light",
            Self::WinKey => "default win+e",
            _ => "",
        }
    }
}

/// Whether an action is something the palette offers.
///
/// **Exhaustive on purpose, and the reason this function exists.** A new [`Action`] variant does not
/// compile until it is in one of the two lists — so a new command cannot be added to the window and
/// quietly left off the palette. `true` is a promise that some [`Command`] comes to it;
/// `every_listed_action_is_on_the_palette` in the click tests checks the promise, so putting a variant
/// in the first list also means giving it an entry in [`App::catalogue`] and an arm in
/// [`App::palette_action`].
///
/// The second list is everything that is not a command somebody would look for by name: the halves
/// of a gesture, a field being committed or cancelled, an answer to a question the window asked, and
/// the variants that do what a listed one does for a caller with something else in hand.
pub(crate) fn listed(action: &Action) -> bool {
    match action {
        Action::NewTab { .. }
        | Action::ReopenTab
        | Action::CloseTab { .. }
        | Action::NextTab { .. }
        | Action::SplitFocused { .. }
        | Action::FolderDiff
        | Action::SetDiffShow { .. }
        | Action::ActivateTab { .. }
        | Action::OpenInSplit { .. }
        | Action::Navigate { .. }
        | Action::NavigateNewTab { .. }
        | Action::Back(_)
        | Action::Forward(_)
        | Action::Up(_)
        | Action::Refresh(_)
        | Action::EditPath(_)
        | Action::FocusFilter(_)
        | Action::ToggleBookmark(_)
        | Action::AddBookmarkGroup
        | Action::DiscoverNetwork
        | Action::SetView { .. }
        | Action::ToggleSizes(_)
        | Action::ToggleConsole(_)
        | Action::TogglePreview(_)
        | Action::ToggleFlat(_)
        | Action::SetLens { .. }
        | Action::Sort { .. }
        | Action::ToggleHidden
        | Action::ToggleSidebar
        | Action::SetFlatMode(_)
        | Action::SetRegroup(_)
        | Action::SetAutoTiles(_)
        | Action::SetSearchSubfolders(_)
        | Action::SetForwardSlashes(_)
        | Action::SetSuggestBookmarks(_)
        | Action::SetSuggestHistory(_)
        | Action::ShowColumn { .. }
        | Action::ResetColumns
        | Action::ShowSection { .. }
        | Action::ResetSectionOrder
        | Action::Zoom(_)
        | Action::Open(_)
        | Action::Cut(_)
        | Action::Copy(_)
        | Action::Paste(_)
        | Action::PasteLinks { .. }
        | Action::CopyPaths(_)
        | Action::BeginRename(_)
        | Action::BeginBatch { .. }
        | Action::NewFolder(_)
        | Action::Delete { .. }
        | Action::Undo { .. }
        | Action::SelectAll(_)
        | Action::ClearSelection(_)
        | Action::InvertSelection(_)
        | Action::SelectSameExtension(_)
        | Action::DiffFolders { .. }
        | Action::OpenTerminal(_)
        | Action::Reveal(_)
        | Action::ShellMenu { .. }
        | Action::Window(_)
        | Action::SetTheme(_)
        | Action::CheckForUpdate
        | Action::SetUpdateEvery(_)
        | Action::InstallUpdate
        | Action::OpenReleasePage
        | Action::SkipUpdate
        | Action::SetWinKey(_) => true,

        // What a listed action does, for a caller with something else in hand: the global menu,
        // which has no pane; the shell's own verbs, redirected over named paths; a shortcut file;
        // the Back button's menu; one half of a toggle the palette has whole.
        Action::NewTabFocused
        | Action::OpenNewTab(_)
        | Action::GoToHistory { .. }
        | Action::CutItems(_)
        | Action::CopyItems(_)
        | Action::PasteIntoFolder(_)
        | Action::AddBookmark(_)
        | Action::AddBookmarkIn { .. }
        | Action::RemoveBookmark(_)
        | Action::ClosePreview(_)
        | Action::SetBatchMode { .. }
        // A gesture, or a half of one: it needs the pointer, the row or the drag that started it.
        | Action::Focus(_)
        | Action::BeginTabDrag { .. }
        | Action::MoveTab { .. }
        | Action::SplitTab { .. }
        | Action::DragOut { .. }
        | Action::DropHere { .. }
        | Action::MakeLinks { .. }
        | Action::DriveMenu { .. }
        | Action::MoveBookmark { .. }
        | Action::MoveSection { .. }
        | Action::MoveColumn { .. }
        | Action::ToggleCollapsed { .. }
        | Action::ToggleBookmarkGroup(_)
        | Action::UngroupBookmarks(_)
        | Action::RemoveBookmarkGroup(_)
        | Action::SetTilesThreshold(_)
        | Action::ForgetVisit(_)
        | Action::Disconnect(_)
        | Action::ToggleVideoFullscreen(_)
        // A field being typed into, committed or let go.
        | Action::FilterFlat { .. }
        | Action::FilterDown(_)
        | Action::FilterEnter(_)
        | Action::FilterSelect(_)
        | Action::SearchSubfoldersHere(_)
        | Action::BatchApply(_)
        | Action::BatchCancel(_)
        | Action::CommitRename { .. }
        | Action::CancelRename(_)
        | Action::BeginKeywords { .. }
        | Action::CommitKeywords { .. }
        | Action::CancelKeywords(_)
        | Action::BeginRenameBookmarkGroup(_)
        | Action::CommitRenameBookmarkGroup { .. }
        | Action::CancelRenameBookmarkGroup
        // An answer to something the window asked, and the window's own bookkeeping.
        | Action::Steer { .. }
        | Action::Leave(_)
        | Action::RememberLayout
        // The palette itself.
        | Action::TogglePalette => false,
    }
}

/// One command in the catalogue, with everything it is found by.
struct Entry {
    command: Command,
    row: Row,
    found_by: String,
}

/// The palette while it is up: the catalogue it was opened with, and what the field has narrowed it
/// to.
pub(crate) struct Palette {
    pub(crate) state: crate::ui::palette::State,
    /// Every command that applied when it opened, in catalogue order.
    catalogue: Vec<Entry>,
    /// What is on screen: the rows, and the command behind each.
    pub(super) rows: Vec<Row>,
    pub(super) commands: Vec<Command>,
    /// The text the rows were narrowed for, so they are only narrowed again when it changes.
    narrowed: Option<String>,
}

/// A row for the catalogue. Its shortcut is filled in by [`App::catalogue`], from
/// [`super::shortcuts`] — never typed here.
fn row(heading: &'static str, label: impl Into<String>) -> Row {
    Row {
        label: label.into(),
        heading,
        shortcut: String::new(),
        checked: false,
        danger: false,
        icon: None,
    }
}

/// The same, ticked when `on`.
fn switch(heading: &'static str, label: impl Into<String>, on: bool) -> Row {
    Row {
        checked: on,
        ..row(heading, label)
    }
}

/// The same, in the danger ink.
fn danger(heading: &'static str, label: impl Into<String>) -> Row {
    Row {
        danger: true,
        ..row(heading, label)
    }
}

impl App {
    /// Put the palette up over the window, built from what is true now — or take it down.
    pub(super) fn toggle_palette(&mut self) {
        if self.palette.take().is_some() {
            return;
        }
        let catalogue = self
            .catalogue()
            .into_iter()
            .map(|(command, row)| Entry {
                found_by: format!("{} {} {}", row.label, row.heading, command.aliases()),
                command,
                row,
            })
            .collect();
        self.palette = Some(Palette {
            state: crate::ui::palette::State::default(),
            catalogue,
            rows: Vec::new(),
            commands: Vec::new(),
            narrowed: None,
        });
    }

    /// Put the palette up with `text` already typed — `--palette=<text>`, for a capture.
    #[cfg(any(test, feature = "devtools"))]
    pub fn open_palette_with(&mut self, text: &str) {
        if self.palette.is_none() {
            self.toggle_palette();
        }
        if let Some(palette) = self.palette.as_mut() {
            palette.state.text = text.to_owned();
        }
    }

    /// Every command that applies to the window as it is, in the order the palette lists them.
    pub(crate) fn catalogue(&self) -> Vec<(Command, Row)> {
        let mut out: Vec<(Command, Row)> = Vec::new();
        let Some(pane) = self.focused_pane() else {
            return out;
        };
        let tab = pane.tab();
        let this_pc = tab.path.as_os_str().is_empty();
        let synthetic = fs::is_synthetic(&tab.path);
        let virtual_place = crate::archive::is_virtual_location(&tab.path);
        let selected = tab.selected_count;
        let cursor_dir = tab.cursor.is_some_and(|at| tab.is_dir_at(at));
        // Every row's key comes out of the one table the keyboard reads, so what is printed is what
        // the key does.
        let mut add = |command: Command, row: Row| {
            let shortcut = super::shortcuts::label(&command);
            out.push((command, Row { shortcut, ..row }));
        };

        // ---- Tabs and panes -------------------------------------------------
        add(Command::NewTab, row(TABS, "New tab"));
        if !self.closed.is_empty() {
            add(Command::ReopenTab, row(TABS, "Reopen closed tab"));
        }
        add(Command::CloseTab, row(TABS, "Close tab"));
        if pane.tabs.len() > 1 {
            add(Command::NextTab, row(TABS, "Next tab"));
            add(Command::PreviousTab, row(TABS, "Previous tab"));
        }
        add(Command::SplitRight, row(TABS, "Split to the right"));
        add(Command::SplitBelow, row(TABS, "Split below"));
        add(Command::FolderDiff, row(TABS, "Folder diff..."));
        if let Some(diff) = &tab.diff {
            for show in crate::diff::Show::ALL {
                let label = format!("Diff: {}", show.label().to_lowercase());
                add(Command::DiffShow(show), switch(TABS, label, diff.show == show));
            }
        }

        // ---- Navigation -----------------------------------------------------
        if tab.can_go_back() {
            add(Command::Back, row(NAVIGATION, "Back"));
        }
        if tab.can_go_forward() {
            add(Command::Forward, row(NAVIGATION, "Forward"));
        }
        if !this_pc {
            add(Command::Up, row(NAVIGATION, "Up one folder"));
        }
        add(Command::Refresh, row(NAVIGATION, "Refresh"));
        add(Command::EditPath, row(NAVIGATION, "Edit path"));
        if tab.diff.is_none() {
            add(Command::Filter, row(NAVIGATION, "Filter this folder"));
        }
        if !synthetic {
            let label = if self.is_bookmarked(&tab.path) {
                "Remove bookmark"
            } else {
                "Bookmark this folder"
            };
            add(Command::Bookmark, row(NAVIGATION, label));
        }
        // The group appears in the left panel with its name open for typing, so not without one.
        if self.sidebar_shown {
            add(Command::BookmarkGroup, row(NAVIGATION, "New bookmark group"));
        }
        add(Command::DiscoverNetwork, row(NAVIGATION, "Look for computers on the network"));

        // ---- View: this pane ------------------------------------------------
        if tab.diff.is_none() {
            let label = if tab.view_mode.is_icons() {
                "Show as details"
            } else {
                "Show as tiles"
            };
            add(Command::Tiles, row(VIEW, label));
            if !this_pc {
                add(Command::Sizes, switch(VIEW, "Folder sizes", tab.sizes.on));
            }
            add(Command::Console, switch(VIEW, "Console", pane.console_open));
            add(Command::Preview, switch(VIEW, "Preview panel", tab.preview.open));
            add(Command::Flatten, switch(VIEW, "Flatten subfolders", tab.flat));
            for lens in crate::pane::Lens::ALL {
                let on = tab.lens == Some(lens);
                add(Command::Lens(Some(lens)), switch(VIEW, lens.label(), on));
            }
            if tab.lens.is_some() {
                add(Command::Lens(None), row(VIEW, "Show the whole folder"));
            }
            for column in Column::ALL {
                if column == Column::Name || self.columns.shown(column) {
                    let label = format!("Sort by {}", column.header().to_lowercase());
                    add(Command::Sort(column), switch(VIEW, label, tab.sort_by == column));
                }
            }
        }

        // ---- View: the window's ---------------------------------------------
        let prefs = [
            (Command::Hidden, "Show hidden files", self.show_hidden),
            (Command::Sidebar, "Left panel", self.sidebar_shown),
            (Command::Regroup, "Merge chains of single folders", self.regroup),
            (Command::AutoTiles, "Open picture folders as tiles", self.auto_tiles.on),
            (Command::SearchSubfolders, "Filter searches subfolders", self.search_subfolders),
            (Command::ForwardSlashes, "Write paths with forward slashes", self.forward_slashes),
            (Command::SuggestBookmarks, "Path suggestions from bookmarks", self.suggest_bookmarks),
            (Command::SuggestHistory, "Path suggestions from history", !self.visits.paused()),
        ];
        for (command, label, on) in prefs {
            add(command, switch(PREFERENCES, label, on));
        }
        for mode in crate::pane::FlatMode::ALL {
            let label = format!("Flattened as a {}", mode.label().to_lowercase());
            add(Command::FlatMode(mode), switch(PREFERENCES, label, self.flat_mode == mode));
        }
        // Name is refused at the other end, and a switch that does nothing is not offered.
        for column in Column::ALL.into_iter().filter(|&column| column != Column::Name) {
            let label = format!("Column: {}", column.header().to_lowercase());
            let on = self.columns.shown(column);
            add(Command::ShowColumn(column), switch(PREFERENCES, label, on));
        }
        add(Command::ResetColumns, row(PREFERENCES, "Reset columns"));
        for section in crate::ui::sidebar::Section::ALL {
            let label = format!("Left panel: {}", section.label());
            let on = self.sections.shown(section);
            add(Command::ShowSection(section), switch(PREFERENCES, label, on));
        }
        add(Command::ResetSections, row(PREFERENCES, "Reset left panel order"));
        add(Command::ZoomIn, row(PREFERENCES, "Zoom in"));
        add(Command::ZoomOut, row(PREFERENCES, "Zoom out"));
        add(Command::ZoomReset, row(PREFERENCES, "Reset zoom"));

        // ---- Files ----------------------------------------------------------
        if tab.cursor.is_some() {
            add(Command::Open, row(FILES, "Open"));
        }
        if cursor_dir {
            add(Command::OpenNewTab, row(FILES, "Open in new tab"));
            add(Command::OpenRight, row(FILES, "Open in a pane to the right"));
            add(Command::OpenBelow, row(FILES, "Open in a pane below"));
        }
        if selected > 0 {
            add(Command::Cut, row(FILES, "Cut"));
            add(Command::Copy, row(FILES, "Copy"));
        }
        if self.clipboard_has_files && !synthetic && !virtual_place {
            use crate::shell::links::Make;
            add(Command::Paste, row(FILES, "Paste"));
            for kind in [Make::Shortcut, Make::Symbolic, Make::Hard, Make::Junction] {
                let label = crate::shell::menu::Own::PasteLinks(kind).label();
                add(Command::PasteLinks(kind), row(FILES, label));
            }
        }
        add(Command::CopyPaths, row(FILES, "Copy path(s)"));
        if selected == 1 {
            add(Command::Rename, row(FILES, "Rename"));
        }
        if selected > 1 {
            for mode in crate::pane::batch::Mode::ALL {
                let label = format!("Batch rename: {}", mode.label().to_lowercase());
                add(Command::Batch(mode), row(FILES, label));
            }
        }
        if !synthetic && !virtual_place {
            add(Command::NewFolder, row(FILES, "New folder"));
        }
        if selected > 0 {
            add(Command::Delete, row(FILES, "Delete"));
            add(Command::DeletePermanently, danger(FILES, "Delete permanently"));
        }
        if self.history.can_undo() {
            add(Command::Undo, row(FILES, "Undo"));
        }
        if self.history.can_redo() {
            add(Command::Redo, row(FILES, "Redo"));
        }
        add(Command::SelectAll, row(FILES, "Select all"));
        if selected > 0 {
            add(Command::ClearSelection, row(FILES, "Clear selection"));
        }
        add(Command::InvertSelection, row(FILES, "Invert selection"));
        // Only on a file: on a folder there is no type to match, and the command would do nothing.
        if tab.cursor.is_some() && !cursor_dir {
            add(Command::SelectSameExtension, row(FILES, "Select files of the same type"));
        }
        if self.two_folders_selected().is_some() {
            add(Command::Compare, row(FILES, "Compare the two selected folders"));
        }
        if !synthetic {
            add(Command::OpenTerminal, row(FILES, "Open terminal here"));
        }
        add(Command::Reveal, row(FILES, "Show in Explorer"));
        if !(this_pc && selected == 0) && !virtual_place {
            add(Command::ContextMenu, row(FILES, "Show context menu"));
        }

        // ---- Window ---------------------------------------------------------
        add(Command::SpanScreens, row(WINDOW, "Across every screen"));
        add(Command::ResetSize, row(WINDOW, "Reset window size"));
        add(Command::Minimize, row(WINDOW, "Minimize"));
        let label = if self.maximized { "Restore" } else { "Maximize" };
        add(Command::Maximize, row(WINDOW, label));
        for palette in crate::theme::Palette::ALL {
            let label = format!("Theme: {}", palette.label());
            let on = self.theme.palette == palette;
            add(Command::Theme(palette), switch(WINDOW, label, on));
        }
        add(Command::CheckForUpdate, row(WINDOW, "Check for updates"));
        for every in crate::update::Every::ALL {
            let label = format!("Auto update: {}", every.label().to_lowercase());
            let on = self.update_every == every;
            add(Command::UpdateEvery(every), switch(WINDOW, label, on));
        }
        if let Some(crate::update::Badge::Available { version }) =
            self.updater.badge(&self.update_skip)
        {
            add(Command::InstallUpdate, row(WINDOW, format!("Update to {version} and restart")));
            add(Command::ReleasePage, row(WINDOW, format!("What's new in {version}")));
            add(Command::SkipUpdate, row(WINDOW, format!("Skip version {version}")));
        }
        let label = "Set as default explorer (changes the registry)";
        add(Command::WinKey, switch(WINDOW, label, self.win_key.ours()));
        add(Command::CloseWindow, danger(WINDOW, "Close window"));
        out
    }

    /// The two folders selected in the focused pane, when exactly two are — what `Folder diff` on the
    /// context menu is offered on, and the same test. See [`App::shell_menu`].
    fn two_folders_selected(&self) -> Option<(PathBuf, PathBuf)> {
        let tab = self.focused_pane()?.tab();
        if tab.selected_count != 2 || fs::is_synthetic(&tab.path) {
            return None;
        }
        match <[PathBuf; 2]>::try_from(tab.selected_folders()) {
            Ok([left, right]) => Some((left, right)),
            Err(_) => None,
        }
    }

    /// Narrow the catalogue to what the field says, when what it says has changed.
    fn narrow_palette(&mut self) {
        let Some(palette) = self.palette.as_ref() else { return };
        if palette.narrowed.as_deref() == Some(palette.state.text.as_str()) {
            return;
        }
        let text = palette.state.text.clone();
        let query = azur_egui_theme::filter::Query::parse(&text);

        let mut rows: Vec<Row> = Vec::new();
        let mut commands: Vec<Command> = Vec::new();
        // **The last few picked, first, when nothing has been typed** — under a heading of their
        // own, and listed again in their place below, because "where it always is" is what the
        // catalogue's order is for.
        if query.is_empty() {
            for key in &self.palette_recent {
                let recent = palette
                    .catalogue
                    .iter()
                    .find(|entry| entry.command.key().as_deref() == Some(key.as_str()));
                if let Some(entry) = recent {
                    rows.push(Row {
                        heading: RECENTLY,
                        ..entry.row.clone()
                    });
                    commands.push(entry.command.clone());
                }
            }
        }
        for entry in palette.catalogue.iter().filter(|entry| query.matches(&entry.found_by)) {
            rows.push(entry.row.clone());
            commands.push(entry.command.clone());
        }
        // The places, once there is enough typed for them to be a choice rather than a flood.
        if text.trim().chars().count() >= 2 && !query.is_empty() {
            self.places_for(&text, &query, &mut rows, &mut commands);
        }

        let Some(palette) = self.palette.as_mut() else { return };
        palette.rows = rows;
        palette.commands = commands;
        palette.narrowed = Some(text);
    }

    /// The places that match: the open tabs, the folders the path field would recall, the sidebar's
    /// places and the drives — in that order, and each folder once.
    fn places_for(
        &self,
        text: &str,
        query: &azur_egui_theme::filter::Query,
        rows: &mut Vec<Row>,
        commands: &mut Vec<Command>,
    ) {
        use azur_egui_theme::icons::Icon;
        let mut push = |command: Command, label: String, icon: Icon<'static>| {
            rows.push(Row {
                icon: Some(icon),
                ..row(GO_TO, label)
            });
            commands.push(command);
        };

        for pane in &self.panes {
            for (index, tab) in pane.tabs.iter().enumerate() {
                let path = tab.path.display().to_string();
                if !query.matches(&format!("{} {path}", tab.title)) {
                    continue;
                }
                let label = if path.is_empty() || tab.title == path {
                    format!("Tab: {}", tab.title)
                } else {
                    format!("Tab: {} — {path}", tab.title)
                };
                push(Command::Tab { pane: pane.id, tab: index }, label, &crate::icons::folder);
            }
        }

        let here = self.path_of(self.focused).unwrap_or_default();
        let recalled = self.visits.recall(
            text,
            self.bookmarks.paths(),
            &here,
            crate::history::now(),
            RECALLED,
        );
        let mut seen: Vec<PathBuf> = Vec::new();
        for offer in recalled {
            let icon: Icon<'static> = if offer.bookmarked {
                &crate::icons::star_filled
            } else {
                &crate::icons::history
            };
            let label = offer.path.display().to_string();
            seen.push(offer.path.clone());
            let command = Command::Folder {
                path: offer.path,
                recalled: !offer.bookmarked,
            };
            push(command, label, icon);
        }

        let places = self
            .places
            .iter()
            .map(|place| (place.label.clone(), place.path.clone()))
            .chain(
                self.volumes
                    .all()
                    .iter()
                    .map(|drive| (drive.display_name(), drive.path.clone())),
            );
        for (label, path) in places {
            if seen.contains(&path) || !query.matches(&format!("{label} {}", path.display())) {
                continue;
            }
            seen.push(path.clone());
            push(Command::Folder { path, recalled: false }, label, &crate::icons::folder);
        }
    }

    /// Draw the palette, if it is up, and do what it says.
    pub(super) fn draw_palette(&mut self, ctx: &egui::Context, t: &Theme, screen: Rect) {
        self.narrow_palette();
        let Some(palette) = self.palette.as_mut() else { return };
        match crate::ui::palette::show(ctx, t, screen, &mut palette.state, &palette.rows) {
            Outcome::Open => {}
            Outcome::Closed => self.palette = None,
            Outcome::Forget(at) => {
                if let Some(Command::Folder { path, recalled: true }) = palette.commands.get(at) {
                    self.actions.push(Action::ForgetVisit(path.clone()));
                    // The row has to go now, not when the text next changes.
                    palette.narrowed = None;
                }
            }
            Outcome::Chose { at, aside } => {
                let Some(command) = self.palette.take().and_then(|p| p.commands.get(at).cloned())
                else {
                    return;
                };
                if let Some(key) = command.key() {
                    self.palette_recent.retain(|k| *k != key);
                    self.palette_recent.insert(0, key);
                    self.palette_recent.truncate(RECENT);
                    self.config_dirty = true;
                }
                // Back to the listing, as a menu gives the keyboard back when it closes.
                ctx.memory_mut(|m| m.stop_text_input());
                if let Some(action) = self.palette_action(ctx, command, aside) {
                    debug_assert!(listed(&action), "{} is on the palette but not in `listed`", action.name());
                    self.actions.push(action);
                }
            }
        }
    }

    /// What a command comes to, for the pane the keyboard is in. Asks and changes nothing — the
    /// action is performed after the frame like any other.
    ///
    /// `aside` is `Alt+Enter`: a folder in a tab of its own rather than in this one. `None` only where
    /// what the entry was listed for has gone since, such as the row under the cursor.
    pub(crate) fn palette_action(
        &self,
        ctx: &egui::Context,
        command: Command,
        aside: bool,
    ) -> Option<Action> {
        use crate::pane::Side;
        let p = self.focused_pane()?;
        let pane = p.id;
        let tab = p.tab();
        let here = tab.path.clone();
        let under_cursor = tab
            .cursor
            .and_then(|at| Some((tab.target_at(at)?, tab.is_dir_at(at))));
        let folder_under_cursor = || under_cursor.clone().filter(|&(_, dir)| dir).map(|(path, _)| path);

        Some(match command {
            Command::NewTab => Action::NewTab { pane },
            Command::ReopenTab => Action::ReopenTab,
            Command::CloseTab => Action::CloseTab { pane, tab: p.active },
            Command::NextTab => Action::NextTab { pane, delta: 1 },
            Command::PreviousTab => Action::NextTab { pane, delta: -1 },
            Command::SplitRight => Action::SplitFocused { side: Side::Right },
            Command::SplitBelow => Action::SplitFocused { side: Side::Bottom },
            Command::FolderDiff => Action::FolderDiff,
            Command::DiffShow(show) => Action::SetDiffShow { pane, show },

            Command::Back => Action::Back(pane),
            Command::Forward => Action::Forward(pane),
            Command::Up => Action::Up(pane),
            Command::Refresh => Action::Refresh(pane),
            Command::EditPath => Action::EditPath(pane),
            Command::Filter => Action::FocusFilter(pane),
            Command::Bookmark => Action::ToggleBookmark(here),
            Command::BookmarkGroup => Action::AddBookmarkGroup,
            Command::DiscoverNetwork => Action::DiscoverNetwork,

            Command::Tiles => Action::SetView {
                pane,
                mode: tab.view_mode.toggled(),
            },
            Command::Sizes => Action::ToggleSizes(pane),
            Command::Console => Action::ToggleConsole(pane),
            Command::Preview => Action::TogglePreview(pane),
            Command::Flatten => Action::ToggleFlat(pane),
            Command::Lens(lens) => Action::SetLens { pane, lens },
            Command::Sort(column) => Action::Sort { pane, column },

            Command::Hidden => Action::ToggleHidden,
            Command::Sidebar => Action::ToggleSidebar,
            Command::FlatMode(mode) => Action::SetFlatMode(mode),
            Command::Regroup => Action::SetRegroup(!self.regroup),
            Command::AutoTiles => Action::SetAutoTiles(!self.auto_tiles.on),
            Command::SearchSubfolders => Action::SetSearchSubfolders(!self.search_subfolders),
            Command::ForwardSlashes => Action::SetForwardSlashes(!self.forward_slashes),
            Command::SuggestBookmarks => Action::SetSuggestBookmarks(!self.suggest_bookmarks),
            Command::SuggestHistory => Action::SetSuggestHistory(self.visits.paused()),
            Command::ShowColumn(column) => Action::ShowColumn {
                column,
                shown: !self.columns.shown(column),
            },
            Command::ResetColumns => Action::ResetColumns,
            Command::ShowSection(section) => Action::ShowSection {
                section,
                shown: !self.sections.shown(section),
            },
            Command::ResetSections => Action::ResetSectionOrder,
            Command::ZoomIn => Action::Zoom(Zoom::In),
            Command::ZoomOut => Action::Zoom(Zoom::Out),
            Command::ZoomReset => Action::Zoom(Zoom::Reset),

            Command::Open => match under_cursor? {
                (path, true) if aside => Action::NavigateNewTab { pane, path },
                _ => Action::open_row(pane, tab, tab.cursor?)?,
            },
            Command::OpenNewTab => Action::NavigateNewTab {
                pane,
                path: folder_under_cursor()?,
            },
            Command::OpenRight => Action::OpenInSplit {
                pane,
                path: folder_under_cursor()?,
                side: Side::Right,
            },
            Command::OpenBelow => Action::OpenInSplit {
                pane,
                path: folder_under_cursor()?,
                side: Side::Bottom,
            },
            Command::Cut => Action::Cut(pane),
            Command::Copy => Action::Copy(pane),
            Command::Paste => Action::Paste(pane),
            Command::PasteLinks(kind) => Action::PasteLinks { into: here, kind },
            Command::CopyPaths => Action::CopyPaths(tab.selection_or_here()),
            Command::Rename => Action::BeginRename(pane),
            Command::Batch(mode) => Action::BeginBatch { pane, mode },
            Command::NewFolder => Action::NewFolder(pane),
            Command::Delete => Action::Delete { pane, permanent: false },
            Command::DeletePermanently => Action::Delete { pane, permanent: true },
            Command::Undo => Action::Undo { redo: false },
            Command::Redo => Action::Undo { redo: true },
            Command::SelectAll => Action::SelectAll(pane),
            Command::ClearSelection => Action::ClearSelection(pane),
            Command::InvertSelection => Action::InvertSelection(pane),
            Command::SelectSameExtension => Action::SelectSameExtension(pane),
            Command::Compare => {
                let (left, right) = self.two_folders_selected()?;
                Action::DiffFolders { pane, left, right }
            }
            Command::OpenTerminal => Action::OpenTerminal(here),
            Command::Reveal => {
                Action::Reveal(tab.selection_paths().into_iter().next().unwrap_or(here))
            }
            // The menu a right click puts up, over the listing — where `--menu` asks for it.
            Command::ContextMenu => self.folder_menu_action(ctx)?,

            Command::SpanScreens => Action::Window(WindowAction::SpanScreens),
            Command::ResetSize => Action::Window(WindowAction::ResetSize),
            Command::Minimize => Action::Window(WindowAction::Minimize),
            Command::Maximize => Action::Window(WindowAction::ToggleMaximize),
            Command::Theme(palette) => Action::SetTheme(palette),
            Command::CheckForUpdate => Action::CheckForUpdate,
            Command::UpdateEvery(every) => Action::SetUpdateEvery(every),
            Command::InstallUpdate => Action::InstallUpdate,
            Command::ReleasePage => Action::OpenReleasePage,
            Command::SkipUpdate => Action::SkipUpdate,
            Command::WinKey => Action::SetWinKey(!self.win_key.ours()),
            Command::CloseWindow => Action::Window(WindowAction::Close),

            Command::Folder { path, .. } if aside => Action::NavigateNewTab { pane, path },
            Command::Folder { path, .. } => Action::Navigate { pane, path },
            Command::Tab { pane, tab } => Action::ActivateTab { pane, tab },
        })
    }
}

/// Which way [`Action::Zoom`] goes. egui's own steps, the ones `Ctrl` `+` and `-` take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zoom {
    In,
    Out,
    Reset,
}
