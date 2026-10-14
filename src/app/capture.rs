//! What the developer flags drive, and the instruments that watch them.
//!
//! `--shot`, `--walk`, `--scroll`, `--trace`, `--rename`, `--path`, `--tiles`, `--preview`,
//! `--find`, `--compare`, `--console`, `--reveal`, `--filter`, `--lens`, `--flat` and `--copy`. None of it
//! is reachable by a person using the program; all of it is how the screenshots in `docs/` are
//! taken and how "does browsing let go of what it read?" is answered by a number.
//!
//! **Not in a shipped build.** This module is compiled for tests and for the `devtools` feature
//! (`cargo devtools`), and for nothing else — a release build has no flag that reaches it. See
//! `devtools` in `Cargo.toml` for why.
//!
//! Two kinds of thing live here. The **instruments** — [`Walk`], [`Scrolling`], `--trace` and
//! [`process_memory`] — drive the window and watch it from inside. The **entry points** are the
//! other half: one method per flag, each doing what a click or a keystroke would have done.
use super::*;

#[cfg(windows)]
#[path = "../windows/process.rs"]
mod win;
#[cfg(windows)]
pub(super) use win::process_memory;

/// This process's private bytes, and its GDI and USER handle counts.
///
/// The three numbers a memory report is actually about. Private bytes is Task Manager's
/// "Memory" column, and it counts COM's allocator and every loaded shell extension's own
/// heap as well as Rust's. The handle counts are here because a leaked `HICON`, `HBITMAP` or
/// `HDC` costs memory without a single Rust allocation, and this program asks the shell for
/// an icon per file type it meets.

#[cfg(not(windows))]
pub(super) fn process_memory() -> (usize, u32, u32) {
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
pub(super) struct Walk {
    pub(super) dirs: Vec<PathBuf>,
    pub(super) at: usize,
    pub(super) stepped_at: f64,
}

impl Walk {
    /// A step every this often. Faster than a person browses, slow enough that each folder
    /// is actually read and drawn rather than superseded before its scan lands.
    pub(super) const STEP: f64 = 0.25;

    pub(super) fn collect(from: &Path) -> Self {
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
    pub(super) fn step(&mut self, time: f64) -> Option<PathBuf> {
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
pub(super) struct Scrolling {
    pub(super) on: bool,
    /// Where in the sweep, 0..1 and back.
    pub(super) phase: f32,
    pub(super) up: bool,
}

impl Scrolling {
    /// A step every frame, as fast as the window will paint.
    ///
    /// `span` is how tall the listing is — which is not `rows × ROW_HEIGHT` in the large-icon view, so
    /// the caller works it out from whichever view is showing. Handed the row figure, a grid of tiles
    /// would sweep to an offset several times its own height and spend nearly the whole cycle clamped
    /// against the bottom, which measures the one position that is not moving.
    pub(super) fn step(&mut self, span: f32) -> Option<f32> {
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

/// `--walk`, `--scroll` and `--trace` together: the state a measuring run drives the window with.
#[derive(Default)]
pub(super) struct Devtools {
    pub(super) walk: Option<Walk>,
    pub(super) scrolling: Scrolling,
    /// `--trace`: report each change of the window's shape, and the memory figures, on stdout.
    pub(super) trace: bool,
    /// When `--trace` last reported the memory figures.
    pub(super) reported_at: f64,
}

impl Devtools {
    /// How often `--trace` reports the memory figures. Often enough to see a curve over a
    /// minute of browsing, rare enough that the log stays readable.
    pub(super) const REPORT: f64 = 3.0;
}

impl App {
    /// One frame's worth of `--walk`, `--scroll` and `--trace`, called from [`App::frame`] with the
    /// shape it is about to settle against.
    pub(super) fn drive_devtools(&mut self, ctx: &egui::Context, shape: super::settling::Shape, now: f64) {
        // `--trace`, the first half: one line per change of shape, which says whether it was the
        // size, the scale, the focus or the minimised flag that moved — the difference between a fix
        // and a guess for text that goes soft after a window gesture.
        if self.devtools.trace && shape != self.settling.was {
            println!("{now:8.3}  {:?} -> {shape:?}", self.settling.was);
        }
        // `--walk`: step to the next folder by itself, so a real window can be measured
        // browsing rather than sitting still.
        if let Some(next) = self.devtools.walk.as_mut().and_then(|walk| walk.step(now)) {
            let pane = self.focused;
            self.perform(ctx, Action::Navigate { pane, path: next });
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(Walk::STEP));
        }
        // `--scroll`: keep the listing moving, so the painting is measured and not just the
        // reading.
        if self.devtools.scrolling.on {
            let span = self.focused_pane().map_or(0.0, |p| {
                let tab = p.tab();
                if tab.view_mode.is_icons() {
                    tab.grid.height()
                } else {
                    tab.order.len() as f32 * crate::pane::ROW_HEIGHT
                }
            });
            if let Some(to) = self.devtools.scrolling.step(span) {
                if let Some(pane) = self.pane_mut(self.focused) {
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
        if self.devtools.trace {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(Devtools::REPORT));
            if now - self.devtools.reported_at > Devtools::REPORT {
                self.devtools.reported_at = now;
                let (private, gdi, user) = process_memory();
                let (dirs, entries) = self.loader.held();
                // Beside the folder cache and separate from it, because they answer the two halves
                // of "memory grows as I browse" that look identical from the outside: a session that
                // walked through a few tarballs is holding their indexes, which are not folders.
                let (archives, in_archives) = crate::archive::held();
                let (kinds, paths, textures) = self.icons.held();
                let (known, pictures, pages, spare) = self.thumbs.held();
                let (gaveup, queued) = self.thumbs.stuck();
                println!(
                    "{now:8.1}  private {:>7} KB   gdi {gdi:>5}   user {user:>4}   \
                     cache {dirs:>3} folders / {entries:>8} entries   \
                     archives {archives:>2} / {in_archives:>7} entries   \
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
    }

    /// `--walk=<dir>`: browse subfolder after subfolder by itself. See [`Walk`].
    pub fn walking(mut self, from: Option<PathBuf>) -> Self {
        self.devtools.walk = from.as_deref().map(Walk::collect);
        self
    }

    /// `--flat`: open with every pane flattened, for looking at that view without having to
    /// press the button first. The same family as `--menu` and `--rename`.
    pub fn flattened(mut self, on: bool) -> Self {
        if on {
            for tab in super::tabs_mut(&mut self.panes) {
                tab.flat = true;
                // In whichever mode the settings ask for, because that is what pressing the
                // button would have done — a flag that opened the list while the settings said
                // tree would be a capture of a view nobody can reach.
                tab.flat_mode = self.flat_mode;
                tab.regroup = self.regroup;
            }
        }
        self
    }

    /// `--sizes`: open with every folder being counted and every row wearing its share, for looking
    /// at that column without having to press the button first. The same family as [`Self::flattened`]
    /// — and it composes with it, since the two are different questions and a flattened tree is
    /// counted off its own listing.
    ///
    /// The flag alone is enough: [`crate::pane::Tab::apply`] sizes the column when the listing lands,
    /// and `App::collect_sizes` starts the counting on the frame after. What a capture then has to do
    /// is *wait* for it — see [`App::sizes_pending`].
    pub fn measuring(mut self, on: bool) -> Self {
        if on {
            for tab in super::tabs_mut(&mut self.panes) {
                // Not `Tab::set_sizes`: there is no listing yet, so there is nothing to size, put
                // back or re-measure. This is the flag pressing the button before the window opens.
                tab.sizes.on = true;
            }
        }
        self
    }

    /// `--scroll`: run the listing up and down for ever. See [`Scrolling`].
    pub fn scrolling(mut self, on: bool) -> Self {
        self.devtools.scrolling.on = on;
        self
    }

    /// `--trace`: report every change to the window's shape on stdout.
    ///
    /// For the one class of bug this program cannot see from the inside — text that goes
    /// soft for a few seconds after a window gesture. One line per change says whether it
    /// was the size, the scale, the focus or the minimised flag that moved, which is the
    /// difference between a fix and a guess.
    pub fn tracing(mut self, on: bool) -> Self {
        self.devtools.trace = on;
        self
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

    /// `--lens=<word>`: open the first pane showing one of the funnel's listings.
    ///
    /// The other half of [`Self::filtering`], and it exists for a sharper version of the same reason:
    /// a lens is behind a menu, and a capture run has no pointer to open one with. Set on the tab for
    /// the same reason the filter is — it is in force on the frame the listing lands.
    ///
    /// **What comes with it does not come from here.** The flatten and, for pictures, the tiles are
    /// what the menu entry sets alongside the lens — see [`Action::SetLens`] — and `main` asks for
    /// those through the flags that already exist for them, so this flag composes with `--flat=tree`
    /// instead of arguing with it.
    pub fn with_lens(mut self, lens: Option<crate::pane::Lens>) -> Self {
        if let Some(lens) = lens {
            if let Some(pane) = self.panes.first_mut() {
                pane.tab_mut().lens = Some(lens);
            }
        }
        self
    }

    /// Whether the focused pane has a listing with anything in it.
    pub fn has_rows(&self) -> bool {
        self.focused_pane().is_some_and(|p| !p.tab().order.is_empty())
    }

    /// Select every file in the focused pane and open the batch rename panel over them.
    ///
    /// For `--batch`, which is how a capture run gets the panel on screen. Files only, so the shot
    /// shows names with extensions for a rule to leave alone; in Pattern mode when a pattern is
    /// given, so the shot can show the tokens filled in.
    pub fn begin_batch_here(&mut self, pattern: Option<String>) {
        let pane = self.focused;
        let Some(p) = self.pane_mut(pane) else { return };
        let tab = p.tab_mut();
        tab.clear_selection();
        let files: Vec<usize> = (0..tab.order.len()).filter(|&at| !tab.is_dir_at(at)).collect();
        for at in files {
            tab.toggle(at);
        }
        let mode = match pattern {
            Some(_) => crate::pane::batch::Mode::Pattern,
            None => crate::pane::batch::Mode::default(),
        };
        if let (true, Some(pattern), Some(batch)) = (tab.begin_batch(mode), pattern, tab.batch.as_mut()) {
            batch.rule.pattern = pattern;
        }
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

    /// Copy `from` into the focused pane's folder.
    ///
    /// For `--copy`, which is how a capture run gets the transfer panel on screen: it is only ever
    /// up while a copy is, and a capture has no clipboard gesture to start one with. Copy onto a name
    /// that is already there, and the panel holds still on its question for as long as the capture
    /// needs it.
    pub fn copy_here(&mut self, from: PathBuf, ctx: &egui::Context) {
        let Some(into) = self.path_of(self.focused) else {
            return;
        };
        let owner = self.owner;
        self.ops.start(
            crate::shell::ops::Job::Copy {
                items: vec![from],
                into,
            },
            owner,
            ctx,
        );
    }

    /// Whether a copy started by [`Self::copy_here`] has yet to put its panel on screen: still
    /// deciding, or running but not yet long enough to be shown and with nothing to ask.
    pub fn copy_pending(&self) -> bool {
        use crate::shell::ops::fast::Phase;
        self.ops.transfers().iter().any(|s| match s.phase {
            Phase::Starting => true,
            Phase::Running => {
                s.running_for < crate::ui::transfers::DELAY
                    && s.question.is_none()
                    && s.short.is_none()
                    && s.snag.is_none()
            }
            Phase::Shell | Phase::Finished => false,
        })
    }

    /// Stop every fast copy, for a capture run on its way out: the window will not close on one
    /// that is still running — see [`App::mind_the_close`].
    pub fn cancel_copies(&self) {
        self.ops.cancel_copies();
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

    /// Whether a capture should keep waiting for the folders to be counted.
    ///
    /// The same reason as [`App::git_pending`], and the longest wait of the family: a tree walk per
    /// folder is seconds where the shell is milliseconds, so without this `--sizes` photographs a
    /// column of blank cells — the loading state rather than the view. The caller's own patience is
    /// what bounds it; a folder big enough to outlast that is one whose screenshot is honestly
    /// half-counted.
    pub fn sizes_pending(&self) -> bool {
        self.tabs().any(|tab| tab.sizes.waiting() > 0)
    }

    /// Whether a capture should keep waiting for the tiles' pictures.
    ///
    /// The same reason as [`App::git_pending`]: the shell answers in tens of milliseconds and a
    /// capture settles in twelve frames, so without this a screenshot of the large-icon view is a grid
    /// of painted placeholder glyphs — a photograph of the loading state rather than of the view.
    pub fn thumbs_pending(&self) -> bool {
        self.thumbs.pending()
    }

    /// Whether a capture should keep waiting for the sync provider, so a shot of a synced folder
    /// shows its Status column filled in rather than its folders blank.
    pub fn cloud_pending(&self) -> bool {
        self.cloud.pending()
    }

    /// The same, for the link counts, so a shot of a folder of hard links shows its column.
    pub fn hard_links_pending(&self) -> bool {
        self.hard_links.pending()
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

    /// `--diff`: open a folder diff in the first pane, of its folder against `against` or else the
    /// next pane's — the application menu's `Folder diff...`, which a capture run has no pointer to
    /// reach.
    pub fn folder_diff_here(&mut self, show: Option<crate::diff::Show>, against: Option<PathBuf>) {
        let mut order = Vec::new();
        self.layout.panes(&mut order);
        if let Some(first) = order.first() {
            self.focused = *first;
        }
        match against {
            Some(right) => {
                let host = self.focused;
                if let Some(left) = self.path_of(host) {
                    self.diff_folders(host, left, Some(right));
                }
            }
            None => self.open_folder_diff(),
        }
        if let Some(show) = show {
            let pane = self.outer(self.focused);
            self.show_in_diff(pane, show);
        }
    }

    /// Whether a capture should keep waiting for a folder diff's comparison to land.
    pub fn diff_pending(&self) -> bool {
        self.tabs()
            .filter_map(|tab| tab.diff.as_ref())
            .any(|side| side.is_left() && side.comparison.is_none())
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
        let kind = Self::preview_kind_at;
        let previewable = |tab: &Tab, at: usize| kind(tab, at).is_some();
        // **A kind this program decodes, in preference to one the shell would draw.** Nearly every
        // file is previewable now — see [`crate::preview::kind_of`] — so "the first previewable row"
        // in a folder of mixed content is whatever happens to sort first, and a capture run has no
        // way to say it meant the source file. The picture, text and binary views are the ones worth
        // photographing; a shell render is the fallback here as it is everywhere else.
        let decoded = |tab: &Tab, at: usize| {
            !matches!(kind(tab, at), None | Some(crate::preview::Kind::Shell))
        };
        // Whatever `--reveal=` already selected, if that can be previewed — so the two flags
        // compose and a capture can name the file it wants. Failing that the first row this program
        // reads itself, and failing *that* the first row with any preview at all.
        let row = tab
            .cursor
            .filter(|&at| previewable(tab, at))
            .or_else(|| (0..tab.order.len()).find(|&at| decoded(tab, at)))
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

    /// `--deps=<module>`: pick that row of the dependency tree, so a capture shows the two symbol
    /// panels beside it.
    ///
    /// They are behind a click on a row and a capture run has no mouse, which is the same argument
    /// [`App::begin_rename_here`] and [`App::open_preview_here`] make. Whether the row was there,
    /// because the caller is a command line and a name that matched nothing is worth saying rather
    /// than a screenshot that quietly shows the tree on its own.
    pub fn pick_dependency(&mut self, name: &str) -> bool {
        let pane = self.focused;
        self.pane_mut(pane)
            .is_some_and(|p| p.tab_mut().preview.pick_dependency(name))
    }

    /// Select the *second* previewable file as well, so `--shot --preview --compare` photographs a
    /// comparison rather than one picture.
    ///
    /// For the same reason [`App::open_preview_here`] exists: two selected rows is a mouse gesture
    /// and a capture run has no mouse.
    /// **And ask for the blend**, which is the whole of what this flag means: selecting two pictures
    /// tiles them now, so a `--compare` that only selected them would photograph two tiles and the
    /// flag would quietly no longer be about a comparison at all. See
    /// [`crate::ui::preview::Preview::set_compare`].
    ///
    /// Two pictures if the folder has them, and otherwise two text files, which are compared too —
    /// see [`App::comparable`].
    pub fn compare_here(&mut self) {
        use crate::preview::Kind;
        if !self.select_previewable(2, Some(Kind::Picture)) {
            self.select_previewable(2, Some(Kind::Text));
        }
        let pane = self.focused;
        if let Some(p) = self.pane_mut(pane) {
            p.tab_mut().preview.set_compare(true);
        }
    }

    /// Select the first `how_many` previewable files, so `--shot --previews=3` photographs the panel
    /// tiled rather than showing one file.
    ///
    /// The same argument [`App::compare_here`] makes and the general case of it: two, three or four
    /// selected rows is a mouse gesture, and a capture run has no mouse.
    pub fn preview_here(&mut self, how_many: usize) {
        self.select_previewable(how_many, None);
    }

    /// Select the first `how_many` rows that have a preview, or that have one of `only` in particular.
    ///
    /// What both of the above are: the row-picking rule differs by one predicate and the selecting is
    /// identical, and it was written twice before this. Clamped to [`crate::ui::preview::MOST`],
    /// because that is as many tiles as the panel has shapes for.
    ///
    /// Whether it did: `false` is a folder without that many, and nothing selected.
    fn select_previewable(&mut self, how_many: usize, only: Option<crate::preview::Kind>) -> bool {
        let pane = self.focused;
        let Some(p) = self.pane_mut(pane) else {
            return false;
        };
        let tab = p.tab_mut();
        let rows: Vec<usize> = (0..tab.order.len())
            .filter(|&at| match (Self::preview_kind_at(tab, at), only) {
                // A kind was asked for: only that one counts.
                (Some(kind), Some(want)) => kind == want,
                // Otherwise anything with a preview at all.
                (Some(_), None) => true,
                (None, _) => false,
            })
            .take(how_many.clamp(1, crate::ui::preview::MOST))
            .collect();
        // **All of them or none**, which matters for `--compare`: one picture selected where two were
        // asked for is a capture of the wrong thing, and silently narrowing it would look like the
        // feature failing rather than the folder not having two.
        if rows.len() < how_many.min(crate::ui::preview::MOST) {
            return false;
        }
        let Some((&first, rest)) = rows.split_first() else {
            return false;
        };
        tab.select_only(first);
        for &row in rest {
            tab.toggle(row);
        }
        true
    }
}
