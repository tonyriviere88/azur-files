//! Settings that outlive a session.
//!
//! A hand-rolled `key=value` file rather than serde and a format crate. There are a
//! dozen settings; a derive, a dependency and a schema would all be larger than the
//! sixty lines of parsing below, and a file a user can open and fix by hand is a
//! feature rather than a compromise.
//!
//! Anything unparseable is skipped rather than fatal: a settings file is not worth
//! refusing to start over.
//!
//! # What the window comes back as
//!
//! Where it was on the desktop, how big it was, which panes it was divided into, and
//! which tabs were in each of them. Three of those four are single values; the panes
//! are a tree, and [`crate::dock::Node`] writes its own one-line form of it — the
//! `layout=` key — over pane *numbers* that the `pane=` lines below supply in the same
//! order.
//!
//! ```text
//! layout=h0.500(0,1)
//! focus=1
//! pane=0            ← pane 0's tabs, with the first of them in front
//! path=C:\src
//! path=C:\src\ui
//! pane=0            ← pane 1's
//! path=D:\
//! ```
//!
//! A `path` before any `pane` line is a file from the version that remembered tabs and
//! not panes, and every tab in it goes into one pane — which is what that version did.
//!
//! # What is deliberately not here
//!
//! **Whether a listing is showing rows or tiles.** Everything in this file is a *habit* — where the
//! preview panel goes, which shell a console opens on, which way a tree is drawn — and that one is
//! not: it is a question asked of the folder in front of you, so every tab opens in the details view
//! and going anywhere puts it back. See [`crate::pane::ViewMode`], which carries the argument.
//! `the_window_comes_back_the_way_it_was_left` checks no `view=` key appears, because a setting
//! added back by reflex is how that reasoning would be undone without anybody noticing.

use std::path::{Path, PathBuf};

use crate::ui::sidebar::Sections;

/// One pane's worth of tabs, as it was left.
#[derive(Clone, Debug, Default)]
pub struct PaneTabs {
    /// The folders, in the order their tabs sat in the strip.
    pub paths: Vec<PathBuf>,
    /// Which of them was in front.
    pub active: usize,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Folders pinned in the sidebar.
    pub bookmarks: Vec<PathBuf>,
    /// Every tab that was open, grouped by the pane it was in — so the window reopens
    /// divided the way it was left and not merely pointing at the same folders.
    pub panes: Vec<PaneTabs>,
    /// How the panes divided the window: [`crate::dock::Node::encode`]'s form, over
    /// positions in [`Self::panes`]. `None` on a first run, and on a file that predates
    /// panes being remembered.
    pub layout: Option<String>,
    /// Which pane had the keyboard, as a position in [`Self::panes`].
    pub focus: usize,
    pub sidebar_width: f32,
    /// Where every pane's preview panel goes, and how much of the pane it takes.
    ///
    /// A preference rather than per-folder state, which is why it is here and the *open* flag is
    /// not: "where the preview goes" is a habit worth keeping across sessions, and "is one showing
    /// for this folder" is a thing you decide when you are looking at the folder. See
    /// [`crate::ui::preview::Layout`].
    pub preview: crate::ui::preview::Layout,
    /// How much of a pane the console panel takes. The same kind of preference as the preview's
    /// share, and remembered for the same reason: how tall you like a console is a habit, and
    /// whether one is *open* is a decision about what you are doing right now.
    pub console_share: f32,
    /// The shell the console was last pointed at, so it opens on the same one next time.
    ///
    /// A preference in the same sense as the preview's position: which shell you work in is a habit,
    /// and being put back in `bash` every launch is a `Shift+Tab` somebody has to remember to press.
    pub console_shell: crate::console::Kind,
    /// Which way the flatten button shows a folder's tree: as one list, or as the hierarchy.
    ///
    /// The same kind of preference again, and remembered for the same reason — and *whether* a tab
    /// is flattened is not here, for the same reason its preview being open is not: that is a
    /// question you ask about the folder in front of you. See [`crate::pane::FlatMode`].
    pub flat_mode: crate::pane::FlatMode,
    /// Whether a tree merges a chain of folders with nothing in them but each other into one row —
    /// `src > main > java`. See [`crate::fs::sort::build_tree_order`].
    ///
    /// **On**, which is the one preference here whose default is not the quieter option, and
    /// deliberately: the rows it takes away are rows that never had anything to say, and somebody who
    /// wants the strict hierarchy is somebody who will go and ask for it. It is in the flatten
    /// button's own menu beside the two modes.
    pub regroup: bool,
    /// Whether the path field writes `/` between the parts of a path instead of `\`.
    ///
    /// **Off**, because `\` is what Windows shows everywhere else and a path bar that disagreed
    /// with the rest of the desktop by default would be this program being clever. On, it is for
    /// the paths that are on their way somewhere else — a shell, a URL, a source file — where the
    /// conversion is otherwise done by hand every time.
    ///
    /// It changes what the field *shows* and nothing else: the field has always taken either
    /// slash, and [`crate::fs::normalize`] is what sees to that at the door. Ticked in the
    /// field's own context menu, which is the one control the setting is about.
    pub forward_slashes: bool,
    pub sections: Sections,
    /// Window size in points, as last seen.
    pub window: Option<[f32; 2]>,
    /// Where the window was: the outer top-left corner, **in physical pixels**.
    ///
    /// Physical rather than points, which every other measurement here is in, because a
    /// desktop of two monitors at different scale factors has no single coordinate space
    /// in points — a logical position means something different depending on which
    /// monitor resolves it, and the platform resolves it against the monitor the window
    /// happens to be created on rather than the one it is being sent to. Pixels are the
    /// one space where "1920, -8" is a place. See `main::restore_position`.
    pub position: Option<[f32; 2]>,
    pub maximized: bool,
    /// Azur ships both sides of the palette; this one opens on the dark side.
    pub dark: bool,
}

/// How wide the sidebar is until somebody drags it, and what a double click on its splitter
/// puts it back to.
///
/// Enough for the longest place name and a drive's label and letter, now that nothing has to
/// share the row with a free-space caption.
pub const SIDEBAR_WIDTH: f32 = 200.0;

/// How many tabs are worth reopening, across every pane.
///
/// Reopening more than this is slower than the folder the user actually wanted, so a session
/// that ended with fifty tabs open does not become a slow start. Applied when the file is read
/// as well as when it is written, because the cost is in the reading either way.
const TABS: usize = 24;

/// And how many panes. Nothing can produce this many by dragging tabs to edges; it is here so
/// that a settings file which has been edited, truncated or filled with nonsense costs a
/// bounded amount to read.
const PANES: usize = 24;

/// The window's size on a first run, and what `Reset window size` puts it back to.
///
/// 1024×600 fits every laptop this is likely to run on, and the layout is built to be usable at
/// it: four columns, a sidebar and a status line all fit, which is the point of the density
/// choices throughout. Comfortably above the 720×420 minimum the window refuses to go below.
pub const WINDOW_SIZE: [f32; 2] = [1024.0, 600.0];

impl Default for Config {
    fn default() -> Self {
        Self {
            bookmarks: Vec::new(),
            panes: Vec::new(),
            layout: None,
            focus: 0,
            sidebar_width: SIDEBAR_WIDTH,
            preview: crate::ui::preview::Layout::default(),
            console_share: crate::ui::console::SHARE,
            console_shell: crate::console::Kind::default(),
            flat_mode: crate::pane::FlatMode::default(),
            regroup: true,
            forward_slashes: false,
            sections: Sections::default(),
            window: None,
            position: None,
            maximized: false,
            dark: true,
        }
    }
}

impl Config {
    /// Read the settings, falling back to the defaults for anything missing.
    pub fn load() -> Self {
        let text = file()
            .and_then(|path| std::fs::read_to_string(path).ok())
            // Nothing under the current name: either a first run or a rename, and the two
            // are told apart by whether the old file is there. See [`previous_file`].
            .or_else(|| previous_file().and_then(|path| std::fs::read_to_string(path).ok()));
        match text {
            Some(text) => Self::parse(&text),
            None => Self::default(),
        }
    }

    /// The file, without the file: separated from [`Self::load`] so that what the format
    /// means can be checked without a disk or a profile directory anywhere in it.
    ///
    /// Reachable from the rest of the crate for one reason — `app::layout_tests` puts a real
    /// window's settings through this and opens a window from what comes back, which is the
    /// only place the two halves of the layout meet: the tree's pane *numbers* here and the
    /// pane order in [`crate::dock::Node::panes`] there.
    pub(crate) fn parse(text: &str) -> Self {
        let mut config = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "bookmark" => config.bookmarks.push(PathBuf::from(value)),
                // Opens a pane, and everything down to the next one belongs to it. The
                // value is which of its tabs was in front.
                "pane" => {
                    if config.panes.len() < PANES {
                        config.panes.push(PaneTabs {
                            paths: Vec::new(),
                            active: value.parse().unwrap_or(0),
                        });
                    }
                }
                "path" => {
                    // See the module header: a file from before panes were remembered has
                    // no `pane` line at all, and one pane is what it meant.
                    let pane = match config.panes.last_mut() {
                        Some(pane) => pane,
                        None => {
                            config.panes.push(PaneTabs::default());
                            config.panes.last_mut().expect("just pushed")
                        }
                    };
                    if pane.paths.len() < TABS {
                        pane.paths.push(PathBuf::from(value));
                    }
                }
                "layout" => config.layout = Some(value.to_owned()),
                "focus" => config.focus = value.parse().unwrap_or(0),
                "preview" => {
                    // `right`, `bottom` or `auto`, and then optionally the share — a word and a
                    // number rather than two keys, since neither means much without the other.
                    let (at, share) = value.split_once(',').unwrap_or((value, ""));
                    if let Some(at) = crate::ui::preview::Where::parse(at.trim()) {
                        config.preview.at = at;
                    }
                    if let Ok(share) = share.trim().parse::<f32>() {
                        config.preview.share = share.clamp(0.1, 0.9);
                    }
                }
                "console_share" => {
                    if let Ok(share) = value.trim().parse::<f32>() {
                        config.console_share = share.clamp(0.1, 0.9);
                    }
                }
                "console_shell" => {
                    if let Some(kind) = crate::console::Kind::parse(value.trim()) {
                        config.console_shell = kind;
                    }
                }
                // `list` or `tree`. Anything else leaves the default, which is the list — the
                // view the button produced before there was a choice.
                "flatten" => {
                    if let Some(mode) = crate::pane::FlatMode::parse(value.trim()) {
                        config.flat_mode = mode;
                    }
                }
                // Read as "anything but 0", like `diff` below and for the same reason: its default is
                // *on*, so a settings file written by an older build has no line for it and the
                // default has to stand.
                "regroup" => config.regroup = value != "0",
                // Read as "only 1", like the preview's two below: its default is *off*, so a
                // missing line and a `0` mean the same thing and both have to leave it alone.
                "forward_slashes" => config.forward_slashes = value == "1",
                "line_numbers" => config.preview.numbers = value == "1",
                "markdown_source" => config.preview.markup = value == "1",
                // The one preview flag whose default is *on*, so it is read as "anything but 0":
                // a settings file written by an older build has no line for it, and the default
                // stands. See [`crate::ui::preview::Layout::diff`].
                "diff" => config.preview.diff = value != "0",
                "diff_collapse" => config.preview.collapse = value == "1",
                "sidebar_width" => {
                    if let Ok(width) = value.parse::<f32>() {
                        config.sidebar_width = width.clamp(140.0, 520.0);
                    }
                }
                "sections" => {
                    let flags: Vec<bool> = value.split(',').map(|f| f.trim() == "1").collect();
                    if flags.len() == 3 {
                        config.sections = Sections {
                            drives: flags[0],
                            bookmarks: flags[1],
                            places: flags[2],
                        };
                    }
                }
                "window" => {
                    if let Some((w, h)) = value.split_once(',') {
                        if let (Ok(w), Ok(h)) = (w.trim().parse::<f32>(), h.trim().parse::<f32>()) {
                            // A window smaller than this would have no room for the
                            // chrome, and a saved size can come from another monitor.
                            if w >= 640.0 && h >= 400.0 && w < 20_000.0 && h < 20_000.0 {
                                config.window = Some([w, h]);
                            }
                        }
                    }
                }
                "position" => {
                    if let Some((x, y)) = value.split_once(',') {
                        if let (Ok(x), Ok(y)) = (x.trim().parse::<f32>(), y.trim().parse::<f32>()) {
                            // A desktop is bounded, and a position outside these is either
                            // corrupt or from a machine with a wall of monitors this one
                            // does not have. Whether it is on a monitor *now* is a question
                            // only the platform can answer — see `main::restore_position`.
                            if x.abs() < 60_000.0 && y.abs() < 60_000.0 {
                                config.position = Some([x, y]);
                            }
                        }
                    }
                }
                "maximized" => config.maximized = value == "1",
                "theme" => config.dark = !value.eq_ignore_ascii_case("light"),
                _ => {}
            }
        }
        config
    }

    /// Write the settings back, best effort.
    ///
    /// **Never from a test.** This is not caution, it is a bug that was shipped and found: a
    /// test that adds a bookmark, changes the theme or drags the sidebar marks the settings
    /// dirty, and the frame it runs in writes them — so `cargo test` replaced the real
    /// `config.ini` with a test fixture's, and the bookmarks in it were simply gone. It happened
    /// repeatedly and looked like nothing, because a test that passes is a test nobody examines.
    /// A test process has no business writing a user's settings under any circumstances.
    pub fn save(&self) {
        if cfg!(test) {
            return;
        }
        let Some(path) = file() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = self.to_text();

        // One level of undo, and the reason it is here: settings are written whole, so
        // anything wrong with the in-memory copy at the moment of a save — a launch that read
        // no config, a second instance, a test run against the wrong profile — replaces the
        // file with it and the bookmarks are simply gone. The previous contents cost one write.
        if let Ok(existing) = std::fs::read(&path) {
            if !existing.is_empty() && existing != text.as_bytes() {
                let _ = std::fs::write(path.with_extension("ini.bak"), &existing);
            }
        }
        let _ = std::fs::write(path, text);
    }

    /// What [`Self::save`] would write. Separated from it for the same reason
    /// [`Self::parse`] is: the format is worth checking, and a test must never go near the
    /// file.
    pub(crate) fn to_text(&self) -> String {
        let mut text = format!("# {}\n", crate::brand::NAME);
        text.push_str(&format!("sidebar_width={:.0}\n", self.sidebar_width));
        // The preview panel: where it goes and how much room it takes, as a word and a number,
        // since neither means much without the other.
        text.push_str(&format!(
            "preview={},{:.3}\n",
            self.preview.at.as_str(),
            self.preview.share
        ));
        text.push_str(&format!("console_share={:.3}\n", self.console_share));
        text.push_str(&format!("console_shell={}\n", self.console_shell.label()));
        text.push_str(&format!("flatten={}\n", self.flat_mode.as_str()));
        text.push_str(&format!("regroup={}\n", flag(self.regroup)));
        text.push_str(&format!(
            "forward_slashes={}\n",
            flag(self.forward_slashes)
        ));
        text.push_str(&format!("line_numbers={}\n", flag(self.preview.numbers)));
        text.push_str(&format!("markdown_source={}\n", flag(self.preview.markup)));
        text.push_str(&format!("diff={}\n", flag(self.preview.diff)));
        text.push_str(&format!("diff_collapse={}\n", flag(self.preview.collapse)));
        text.push_str(&format!(
            "sections={},{},{}\n",
            flag(self.sections.drives),
            flag(self.sections.bookmarks),
            flag(self.sections.places),
        ));
        if let Some([w, h]) = self.window {
            text.push_str(&format!("window={w:.0},{h:.0}\n"));
        }
        if let Some([x, y]) = self.position {
            text.push_str(&format!("position={x:.0},{y:.0}\n"));
        }
        text.push_str(&format!("maximized={}\n", flag(self.maximized)));
        text.push_str(if self.dark {
            "theme=dark\n"
        } else {
            "theme=light\n"
        });
        for bookmark in &self.bookmarks {
            text.push_str(&format!("bookmark={}\n", bookmark.display()));
        }
        // The layout, then the panes it is written over, in the order it numbers them.
        if let Some(layout) = &self.layout {
            text.push_str(&format!("layout={layout}\n"));
        }
        text.push_str(&format!("focus={}\n", self.focus));
        // Reopening more than this is slower than the folder the user actually wanted, so a
        // runaway session does not become a slow start. Counted across every pane, and never
        // at the cost of a pane: the first tab of each is what the layout is a tree over, so
        // dropping one would leave `layout=` describing a window that cannot be built.
        let mut room = TABS;
        for pane in self.panes.iter().take(PANES) {
            let keep = pane.paths.len().min(room.max(1));
            room = room.saturating_sub(keep);
            text.push_str(&format!("pane={}\n", pane.active.min(keep.saturating_sub(1))));
            for path in pane.paths.iter().take(keep) {
                text.push_str(&format!("path={}\n", path.display()));
            }
        }
        text
    }
}

fn flag(value: bool) -> u8 {
    u8::from(value)
}

/// Where the settings live.
fn file() -> Option<PathBuf> {
    // A folder with spaces in it is the Windows convention and a nuisance to type
    // everywhere else, so the two platforms get the two spellings of the same name.
    let name = if cfg!(windows) {
        crate::brand::NAME
    } else {
        "azur-file-explorer"
    };
    base_dir().map(|base| base.join(name).join("config.ini"))
}

/// What the settings folder was called before the program had a name.
///
/// Read only when there is nothing under the current name, and never written: a user who
/// had bookmarks, open tabs and a window size should not lose them to a rename, and the
/// first save afterwards puts them in the new place. One migration, nothing remembered,
/// and it can go once nobody is coming from that version.
fn previous_file() -> Option<PathBuf> {
    base_dir().map(|base| base.join("yet-another-file-explorer").join("config.ini"))
}

/// Where per-user settings go on this platform.
///
/// **`YAFE_PROFILE` replaces it outright.** Not a developer convenience — a safety rail. A
/// test that drives this program with real mouse and keyboard input has to launch it for real,
/// and a real launch writes real settings on the way out: the window size, the open tabs, the
/// bookmarks. Point it somewhere disposable and a test run cannot touch anybody's own.
fn base_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("YAFE_PROFILE") {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(active: usize, paths: &[&str]) -> PaneTabs {
        PaneTabs {
            paths: paths.iter().map(PathBuf::from).collect(),
            active,
        }
    }

    /// Everything about the window's shape survives a write and a read.
    ///
    /// Round-tripped rather than compared against a fixture: the file is something a user may
    /// edit, but what it has to *do* is bring the window back, and that is the claim worth
    /// pinning. The position is the awkward one — it is the only value here in physical
    /// pixels, and it can be negative, which is what a monitor to the left of the primary one
    /// means.
    #[test]
    fn the_window_comes_back_the_way_it_was_left() {
        let saved = Config {
            panes: vec![pane(1, &[r"C:\src", r"C:\src\ui"]), pane(0, &[r"D:\"])],
            layout: Some("h0.400(0,1)".to_owned()),
            focus: 1,
            window: Some([1380.0, 840.0]),
            position: Some([-1920.0, -8.0]),
            sidebar_width: 260.0,
            dark: false,
            preview: crate::ui::preview::Layout {
                at: crate::ui::preview::Where::Bottom,
                share: 0.615,
                numbers: true,
                markup: true,
                // Both away from their defaults, which is the only way a round trip proves anything:
                // a value that is never written still comes back right if the default happens to
                // match what was asked for.
                diff: false,
                collapse: true,
            },
            console_share: 0.28,
            console_shell: crate::console::Kind::PowerShell,
            // Away from their defaults for the same reason the two above are.
            flat_mode: crate::pane::FlatMode::Tree,
            regroup: false,
            forward_slashes: true,
            ..Config::default()
        };

        let back = Config::parse(&saved.to_text());
        assert_eq!(back.window, saved.window);
        assert_eq!(back.position, saved.position);
        assert_eq!(back.layout, saved.layout);
        assert_eq!(back.focus, 1);
        assert_eq!(back.sidebar_width, 260.0);
        assert!(!back.dark);
        // The preview panel's four preferences. Worth pinning together with the window's shape,
        // because they are the same kind of thing — how the window comes back — and because a
        // value that is written and not read is the failure this round trip is for: `preview=` was
        // parsed for a while before anything wrote it, and the panel silently forgot its position
        // every session.
        assert_eq!(back.preview.at, crate::ui::preview::Where::Bottom);
        assert!((back.preview.share - 0.615).abs() < 1e-3);
        assert!(back.preview.numbers);
        assert!(back.preview.markup);
        assert!(!back.preview.diff, "the one flag whose default is on");
        assert!(back.preview.collapse);
        // A settings file from the build before this feature has no line for either, and the diff
        // has to come back *on* — its default — rather than off because the key was missing.
        let older = Config::parse("theme=dark\nline_numbers=1\n");
        assert!(older.preview.diff, "on by default");
        assert!(!older.preview.collapse);
        // Which flatten mode the button produces, written as a word for the same reason the shell
        // is — and a file without the line comes back as the list, which is the view the button
        // produced before there was a choice.
        assert_eq!(back.flat_mode, crate::pane::FlatMode::Tree);
        assert!(saved.to_text().contains("flatten=tree"));
        assert_eq!(older.flat_mode, crate::pane::FlatMode::List);
        // And whether that tree merges its chains of single folders, which is the other flag whose
        // default is *on* — so the older file has to come back with it set, and the saved one, which
        // turned it off, has to come back off.
        assert!(!back.regroup);
        assert!(older.regroup, "on by default");
        // And **nothing at all about rows or tiles**, which is the one listing setting this file
        // deliberately does not carry: a tab always opens in the details view. A `view=` key written
        // by hand is ignored, and one appearing here again would mean the argument on
        // `crate::pane::ViewMode` had been undone without anybody noticing.
        // Matched with the newline in front of it, or `preview=` would satisfy it and the assertion
        // would pass whatever this file wrote.
        assert!(
            !saved.to_text().contains("\nview="),
            "a `view=` key is being written: {}",
            saved.to_text()
        );
        // Which slash the path field writes. The other way round from the two above — its default is
        // *off*, so it is the missing line that has to come back false and the `1` that has to
        // survive. A preference nobody can keep is worse than no preference: the whole point of it is
        // that the field opens the same way every time.
        assert!(back.forward_slashes);
        assert!(!older.forward_slashes, "off by default");
        // And the console's two, which are the same kind of thing again. The shell is written as the
        // word the dropdown shows, so a settings file stays something you can read and edit.
        assert!((back.console_share - 0.28).abs() < 1e-3);
        assert_eq!(back.console_shell, crate::console::Kind::PowerShell);
        assert!(saved.to_text().contains("console_shell=pwsh"));

        assert_eq!(back.panes.len(), 2, "{:?}", back.panes);
        assert_eq!(back.panes[0].paths, saved.panes[0].paths);
        assert_eq!(back.panes[0].active, 1, "which tab was in front is part of it");
        assert_eq!(back.panes[1].paths, saved.panes[1].paths);
    }

    /// A settings file from the version that remembered tabs but not panes.
    ///
    /// Its `path` lines have no `pane` line above them, and what they meant was one pane
    /// holding all of them. Somebody upgrading has that file and no other, so reading it as
    /// nothing at all would lose every folder they had open.
    #[test]
    fn a_file_from_before_panes_opens_as_one_pane() {
        let back = Config::parse("path=C:\\a\npath=C:\\b\ntheme=dark\n");
        assert_eq!(back.panes.len(), 1);
        assert_eq!(back.panes[0].paths.len(), 2);
        assert_eq!(back.layout, None, "and no layout to try to build");
    }

    /// The caps hold, and never by dropping a pane.
    ///
    /// A pane with no tabs cannot be drawn, and `layout=` is a tree over exactly the panes
    /// that follow it — so a cap that emptied one would describe a window this program then
    /// refuses to build, and the whole layout would be thrown away over a tab limit.
    #[test]
    fn the_tab_cap_never_costs_a_pane() {
        let many: Vec<String> = (0..40).map(|i| format!("C:\\{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let saved = Config {
            // The first pane alone wants more than the cap allows, and there are two more
            // behind it.
            panes: vec![pane(30, &refs), pane(0, &[r"D:\one"]), pane(0, &[r"D:\two"])],
            layout: Some("h0.500(0,v0.500(1,2))".to_owned()),
            ..Config::default()
        };

        let back = Config::parse(&saved.to_text());
        assert_eq!(back.panes.len(), 3, "every pane has to be written");
        assert!(back.panes.iter().all(|p| !p.paths.is_empty()));
        let total: usize = back.panes.iter().map(|p| p.paths.len()).sum();
        assert!(total <= TABS + 2, "{total} tabs got through the cap");
        assert!(
            back.panes[0].active < back.panes[0].paths.len(),
            "the tab in front has to be one of the ones that survived"
        );
    }

    /// Nonsense is skipped, not fatal — and the rest of the file still lands.
    #[test]
    fn a_broken_line_costs_only_itself() {
        let back = Config::parse(
            "# a comment\n\
             \n\
             sidebar_width=lots\n\
             window=wide,tall\n\
             position=\n\
             focus=first\n\
             pane=x\n\
             path=C:\\a\n\
             theme=light\n",
        );
        assert_eq!(back.sidebar_width, SIDEBAR_WIDTH);
        assert_eq!(back.window, None);
        assert_eq!(back.position, None);
        assert_eq!(back.focus, 0);
        assert_eq!(back.panes.len(), 1);
        assert_eq!(back.panes[0].active, 0);
        assert!(!back.dark, "and the line after the mess still applies");
    }
}
