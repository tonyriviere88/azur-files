//! What the developer flags drive, and the instruments that watch them.
//!
//! `--shot`, `--walk`, `--scroll`, `--trace`, `--rename`, `--path`, `--tiles`, `--preview`,
//! `--find`, `--compare`, `--console`, `--reveal`, `--filter`, `--lens` and `--flat`. None of it
//! is reachable by a person using the program; all of it is how the screenshots in `docs/` are
//! taken and how "does browsing let go of what it read?" is answered by a number.
//!
//! Two kinds of thing live here. The **instruments** — [`Shape`], [`Settling`], [`Walk`],
//! [`Scrolling`], [`process_memory`] — watch the window from inside and say when it has stopped
//! changing, which is what a capture has to wait for. The **entry points** are the other half:
//! one method per flag, each doing what a click or a keystroke would have done.

use super::*;

#[cfg(windows)]
#[path = "../windows/process.rs"]
mod win;
#[cfg(windows)]
pub(super) use win::process_memory;

/// What the window looks like to the platform, as far as rendering crisply is concerned.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(super) struct Shape {
    /// In *physical pixels*, which is what the framebuffer is sized in.
    pub(super) pixels: (u32, u32),
    /// Rounded, because a scale factor arrives as a float and comparing floats for
    /// equality every frame is a way to request repaints for ever.
    pub(super) scale: u32,
    /// What the *platform* says the scale is, against `scale`, which is what egui rasterised
    /// the glyph atlas for.
    ///
    /// Tracked separately because the two disagreeing is the one thing that would make text
    /// soft without anything else moving: an atlas built for one scale, drawn at another.
    /// Zero when the platform has not said.
    pub(super) native_scale: u32,
    pub(super) focused: bool,
    pub(super) minimized: bool,
}

impl Shape {
    pub(super) fn of(ctx: &egui::Context) -> Self {
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
pub(super) struct Settling {
    pub(super) was: Shape,
    /// When the shape last changed, in `InputState::time`.
    pub(super) changed_at: Option<f64>,
    /// `--trace`: report each change on stdout.
    pub(super) trace: bool,
    /// When `--trace` last reported the memory figures.
    pub(super) reported_at: f64,
}

impl Settling {
    /// How long after a change to keep painting. Long enough to cover a restore animation
    /// and a scale change, short enough that nobody notices the frames.
    pub(super) const QUIET: f64 = 0.75;

    /// How often `--trace` reports the memory figures. Often enough to see a curve over a
    /// minute of browsing, rare enough that the log stays readable.
    pub(super) const REPORT: f64 = 3.0;

    /// Whether a frame should be asked for.
    pub(super) fn observe(&mut self, now: Shape, time: f64) -> bool {
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

impl App {
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
}
