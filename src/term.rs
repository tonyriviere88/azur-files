//! A shell in a pane: the pseudoconsole, the thread that reads it, and the grid it writes into.
//!
//! This half is the model. [`crate::ui::term`] is the only thing that draws, at explicit rects and
//! in the design system's palette, exactly as the rest of the interface does.
//!
//! # What is not written here, and why
//!
//! Everything below the grid comes from `alacritty_terminal` — the ConPTY, the escape-sequence
//! parser, the cell grid, the scrollback and the selection. That is the largest dependency in
//! [`Cargo.toml`](../Cargo.toml) and its justification is there in full; the short version is that
//! the parser is not the hard part. Scroll regions that are off by one after `DECSTBM`, the wrap
//! flag that decides whether a re-size rejoins a soft-wrapped line, the alternate screen handing
//! back exactly the grid `vim` left, `\r` overwriting a progress bar in place — each of those is a
//! bug found weeks later in somebody else's program.
//!
//! # One thread per shell, and it is not this program's
//!
//! `EventLoop::spawn` starts a thread that owns the pseudoconsole and holds the grid behind a
//! `FairMutex`. It parses into the grid without this program's involvement and pokes
//! [`Proxy::send_event`] when there is something new — where the only thing that happens is
//! [`egui::Context::request_repaint`], because a window that paints on demand has to be told.
//!
//! The mutex is *fair* deliberately: the renderer takes it every frame it draws, and an unfair
//! lock would let sixty frames a second starve a shell producing output as fast as it can.
//!
//! # Answering the terminal back
//!
//! Some escape sequences are questions — the device attributes, the cursor position, the text area
//! size — and a program that asks one and is not answered **hangs**. Those arrive as
//! [`alacritty_terminal::event::Event::PtyWrite`] and friends, and [`Proxy`] writes the answer
//! straight back down the pseudoconsole from the thread they arrive on. It is the one place this
//! program speaks to the shell without being asked to.
//!
//! # OSC 52 is off
//!
//! The escape sequence that lets a program set the system clipboard. `vte` supports it and this
//! turns it off: any program that can print can then rewrite what is on your clipboard, which in a
//! file manager is a paste target for real files. `Ctrl+Shift+C` and `Ctrl+Shift+V` in the panel go
//! through this program's own clipboard, where the user asked for it.
//!
//! # Where the folder comes from
//!
//! Nowhere here. OSC 7 — the sequence a shell uses to announce its directory — is not surfaced by
//! `vte` at all, so there is nothing to read even though this program owns the stream. It does not
//! matter: an embedded shell reads the same profile as any other, so if the prompt hook is
//! installed it publishes exactly like a shell in Windows Terminal does, and [`crate::cwd`] moves
//! the pane. One mechanism for both, and the panel needed no part of it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use alacritty_terminal::event::{Event as TermEvent, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Osc52, Term};
use alacritty_terminal::tty;

pub mod keys;

pub use alacritty_terminal::vte::ansi::Rgb;

/// The sixteen colours a terminal is allowed to name, plus the two it draws with by default.
///
/// Held here rather than in [`crate::theme`] because two very different things need the same
/// answer: [`crate::ui::term`] draws with it, and [`Proxy`] answers `OSC 4` with it from the
/// pseudoconsole's thread, where there is no theme to ask. One value, so a program that asks what
/// colour 4 is gets the colour it is about to see.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the same eight bright.
    pub ansi: [Rgb; 16],
    pub fg: Rgb,
    pub bg: Rgb,
}

impl Palette {
    /// The colour at an index of the 256-colour cube, which is what `OSC 4` asks about.
    ///
    /// 0–15 are [`Self::ansi`]; 16–231 are the 6×6×6 cube; 232–255 are the greyscale ramp. The two
    /// arithmetic ranges are xterm's, and they are arithmetic rather than a table for a reason —
    /// a 240-entry table in a file manager would be 240 chances to typo a colour nobody checks.
    pub fn at(&self, index: usize) -> Rgb {
        const STEPS: [u8; 6] = [0, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
        match index {
            0..=15 => self.ansi[index],
            16..=231 => {
                let index = index - 16;
                Rgb {
                    r: STEPS[index / 36],
                    g: STEPS[(index / 6) % 6],
                    b: STEPS[index % 6],
                }
            }
            232..=255 => {
                let grey = 8 + 10 * (index as u8 - 232);
                Rgb { r: grey, g: grey, b: grey }
            }
            // 256 and 257 are the foreground and the background; anything past that is a slot
            // `vte` keeps for the cursor and the dim variants, and the foreground is the safe
            // answer for all of them.
            257 => self.bg,
            _ => self.fg,
        }
    }
}

/// How many lines of scrollback a panel keeps.
///
/// `alacritty_terminal` defaults to 10,000, which is a terminal's answer. This is a panel inside a
/// file manager, opened to run a build and read what it said: 5,000 lines is more than anybody
/// scrolls back through and a bounded amount of memory per pane, which matters because there is one
/// of these per pane rather than one per window.
const SCROLLBACK: usize = 5_000;

/// The smallest grid a shell is given, in cells.
///
/// A pseudoconsole of zero columns is not a thing, and a panel dragged almost shut would otherwise
/// ask for one. Programs also lay themselves out against the width they are told, so a shell
/// briefly told it is one column wide reflows everything it has printed.
const MIN: Size = Size { cols: 2, rows: 1 };

/// A grid's size in cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Size {
    /// The size that fits in a rect, given what one cell measures.
    pub fn fitting(rect: egui::Vec2, cell: egui::Vec2) -> Self {
        let cols = (rect.x / cell.x.max(1.0)).floor() as usize;
        let rows = (rect.y / cell.y.max(1.0)).floor() as usize;
        Self {
            cols: cols.max(MIN.cols),
            rows: rows.max(MIN.rows),
        }
    }
}

/// Only the three the trait actually requires; the rest are provided.
///
/// `total_lines` is the screen and no more, because this describes the size a grid is being *told*
/// to be. The scrollback is [`SCROLLBACK`] and belongs to the grid itself.
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// What the pseudoconsole's thread learns and the window has to be told.
#[derive(Default)]
struct Shared {
    /// The title the shell set, which is what a terminal tab would show.
    title: Option<String>,
    /// The shell has gone. The panel says so rather than closing itself — a build that ended in a
    /// panic is a thing to read, and a panel that vanished took the reason with it.
    exited: bool,
    /// The bell rang. Not sounded; the panel flashes its own edge, which is what a terminal inside
    /// another program should do rather than making a noise the user did not ask any program for.
    bell: bool,
}

/// The terminal's way of telling the window something happened.
///
/// Every method here runs on the pseudoconsole's thread, so it does two kinds of thing and nothing
/// else: set a flag behind a mutex, and answer questions the shell will otherwise wait forever for.
#[derive(Clone)]
pub struct Proxy {
    ctx: egui::Context,
    shared: Arc<Mutex<Shared>>,
    /// The way back down the pseudoconsole.
    ///
    /// A `OnceLock` because of a chicken and egg in the construction order: the grid needs a
    /// listener before it exists, the event loop needs the grid, and the sender only exists once
    /// the loop does. Filled immediately after, and before the thread that reads it is started.
    back: Arc<OnceLock<EventLoopSender>>,
    /// The size, for the one sequence that asks about it.
    size: Arc<Mutex<(Size, egui::Vec2)>>,
    /// What the panel is drawn with, for the sequence that asks about that. Shared rather than
    /// copied because the palette changes when the window's does.
    palette: Arc<Mutex<Palette>>,
}

impl Proxy {
    fn answer(&self, text: String) {
        if let Some(back) = self.back.get() {
            Notifier(back.clone()).notify(text.into_bytes());
        }
    }

    fn with<R>(&self, edit: impl FnOnce(&mut Shared) -> R) -> Option<R> {
        self.shared.lock().ok().map(|mut shared| edit(&mut shared))
    }
}

impl EventListener for Proxy {
    fn send_event(&self, event: TermEvent) {
        match event {
            // There is new content in the grid. A window that paints on demand has to be told, and
            // this is the only thing that tells it.
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {
                self.ctx.request_repaint();
                return;
            }

            // Questions. A program that asks one and is not answered waits forever — this is why
            // `PtyWrite` cannot simply be ignored, and it is the difference between a terminal and
            // something that looks like one until you run `tput`.
            TermEvent::PtyWrite(text) => self.answer(text),
            TermEvent::TextAreaSizeRequest(format) => {
                let size = self.size.lock().ok().map(|size| *size);
                if let Some((size, cell)) = size {
                    self.answer(format(window_size(size, cell)));
                }
            }
            TermEvent::ColorRequest(index, format) => {
                // Answered from the palette the panel is actually drawn with, so a program that
                // asks what colour 4 is gets the colour it is about to see.
                let palette = self.palette.lock().ok().map(|palette| *palette);
                if let Some(palette) = palette {
                    self.answer(format(palette.at(index)));
                }
            }

            TermEvent::Title(title) => {
                self.with(|shared| shared.title = Some(title));
            }
            TermEvent::ResetTitle => {
                self.with(|shared| shared.title = None);
            }
            TermEvent::Bell => {
                self.with(|shared| shared.bell = true);
            }
            TermEvent::Exit | TermEvent::ChildExit(_) => {
                self.with(|shared| shared.exited = true);
            }
            // OSC 52 is off — see the module header — so neither of these can arrive.
            TermEvent::ClipboardStore(..) | TermEvent::ClipboardLoad(..) => {}
        }
        self.ctx.request_repaint();
    }
}

fn window_size(size: Size, cell: egui::Vec2) -> WindowSize {
    WindowSize {
        num_lines: size.rows as u16,
        num_cols: size.cols as u16,
        cell_width: cell.x.max(1.0) as u16,
        cell_height: cell.y.max(1.0) as u16,
    }
}

/// One shell, its grid, and the thread between them.
pub struct Terminal {
    /// The grid. Locked by the renderer every frame it draws and by the parser whenever the shell
    /// says anything, which is why it is the *fair* mutex and not a `std` one.
    term: Arc<FairMutex<Term<Proxy>>>,
    to_pty: EventLoopSender,
    shared: Arc<Mutex<Shared>>,
    size: Arc<Mutex<(Size, egui::Vec2)>>,
    /// Shared with [`Proxy`], which answers `OSC 4` out of it.
    palette: Arc<Mutex<Palette>>,
    /// Joined in [`Drop`], after the loop has been told to stop.
    thread: Option<std::thread::JoinHandle<()>>,
    /// What the shell was started as, for the panel to name it.
    pub shell: String,
}

impl Terminal {
    /// Start a shell in a folder.
    ///
    /// The error is a sentence for the panel to show. A shell that cannot be started is worth
    /// saying out loud — the usual cause is a `terminal_shell=` naming something that is not
    /// installed, and a panel that opened empty would look like this program's fault.
    pub fn spawn(
        ctx: &egui::Context,
        dir: &Path,
        size: Size,
        cell: egui::Vec2,
        palette: Palette,
    ) -> Result<Self, String> {
        let shell = Shell::find().ok_or_else(|| "No shell found to run".to_owned())?;
        Self::spawn_shell(ctx, dir, size, cell, palette, shell)
    }

    fn spawn_shell(
        ctx: &egui::Context,
        dir: &Path,
        size: Size,
        cell: egui::Vec2,
        palette: Palette,
        shell: Shell,
    ) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let sizes = Arc::new(Mutex::new((size, cell)));
        let palette = Arc::new(Mutex::new(palette));
        let back: Arc<OnceLock<EventLoopSender>> = Arc::new(OnceLock::new());
        let proxy = Proxy {
            ctx: ctx.clone(),
            shared: Arc::clone(&shared),
            back: Arc::clone(&back),
            size: Arc::clone(&sizes),
            palette: Arc::clone(&palette),
        };

        let config = Config {
            scrolling_history: SCROLLBACK,
            // See the module header: a program that can print must not be able to rewrite the
            // clipboard of a file manager.
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, proxy.clone())));

        // `working_directory` is only half of opening in the pane's folder; `Shell::options` is
        // where the other half is, and it is not obvious.
        let options = shell.options(dir);
        let pty = tty::new(&options, window_size(size, cell), 0)
            .map_err(|why| format!("Cannot start {}: {why}", shell.name))?;

        let loop_ = EventLoop::new(Arc::clone(&term), proxy, pty, false, false)
            .map_err(|why| format!("Cannot watch {}: {why}", shell.name))?;
        let to_pty = loop_.channel();
        // Before the thread starts, so no event can arrive with nowhere to send its answer.
        let _ = back.set(to_pty.clone());
        let thread = std::thread::Builder::new()
            .name("terminal".to_owned())
            .spawn(move || {
                let _ = loop_.spawn().join();
            })
            .map_err(|why| format!("Cannot start a thread for {}: {why}", shell.name))?;

        Ok(Self {
            term,
            to_pty,
            shared,
            size: sizes,
            palette,
            thread: Some(thread),
            shell: shell.name,
        })
    }

    /// Follow the window's palette when it changes.
    ///
    /// Called every frame and cheap when nothing moved. It exists because the theme can be switched
    /// with a shell already running, and a panel left drawing the other palette's colours is the
    /// most obvious possible wrongness.
    pub fn set_palette(&self, palette: Palette) {
        if let Ok(mut held) = self.palette.lock() {
            *held = palette;
        }
    }

    /// Tell the shell the panel is a different shape.
    ///
    /// Cheap when nothing moved, because it is called every frame: a `SIGWINCH` per frame would
    /// have every program that reflows on resize reflowing sixty times a second.
    pub fn resize(&mut self, size: Size, cell: egui::Vec2) {
        let changed = match self.size.lock() {
            Ok(mut held) if *held != (size, cell) => {
                *held = (size, cell);
                true
            }
            _ => false,
        };
        if !changed {
            return;
        }
        self.term.lock().resize(size);
        Notifier(self.to_pty.clone()).on_resize(window_size(size, cell));
    }

    /// Type at the shell.
    pub fn send(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        // Anything typed is a reason to look at the bottom of the output, which is what every
        // terminal does and what makes a scrolled-back panel usable rather than confusing.
        self.term.lock().scroll_display(Scroll::Bottom);
        Notifier(self.to_pty.clone()).notify(bytes);
    }

    /// Scroll the view without touching the shell.
    pub fn scroll(&self, lines: i32) {
        if lines != 0 {
            self.term.lock().scroll_display(Scroll::Delta(lines));
        }
    }

    /// Where a row and column on screen is in the grid.
    ///
    /// The two differ by the scrollback: display row 0 is grid line `-display_offset`, so a click
    /// on a panel scrolled back five hundred lines has to name the line that is *shown* there
    /// rather than the one at the top of the screen.
    pub fn point_at(&self, row: usize, col: usize) -> alacritty_terminal::index::Point {
        use alacritty_terminal::index::{Column, Line, Point};
        let offset = self.term.lock().grid().display_offset() as i32;
        Point::new(Line(row as i32 - offset), Column(col))
    }

    /// Begin, replace or clear the selection.
    pub fn select(&self, selection: Option<alacritty_terminal::selection::Selection>) {
        self.term.lock().selection = selection;
    }

    /// Drag the selection out to a point, if there is one being dragged.
    pub fn extend_selection(&self, to: alacritty_terminal::index::Point, side: alacritty_terminal::index::Side) {
        let mut term = self.term.lock();
        if let Some(selection) = term.selection.as_mut() {
            selection.update(to, side);
        }
    }

    /// The selected text, if any is selected.
    pub fn selected(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// Read the grid — the renderer's one way in.
    ///
    /// Takes the lock for the length of the closure, so the closure draws and does nothing else.
    pub fn read<R>(&self, look: impl FnOnce(&Term<Proxy>) -> R) -> R {
        look(&self.term.lock())
    }

    /// The title the shell asked for, if it asked.
    pub fn title(&self) -> Option<String> {
        self.shared.lock().ok().and_then(|shared| shared.title.clone())
    }

    /// Whether the shell has gone.
    pub fn exited(&self) -> bool {
        self.shared.lock().is_ok_and(|shared| shared.exited)
    }

    /// Whether the bell has rung since this was last asked. Reading it clears it.
    pub fn bell(&mut self) -> bool {
        self.shared
            .lock()
            .ok()
            .map(|mut shared| std::mem::take(&mut shared.bell))
            .unwrap_or(false)
    }
}

impl Drop for Terminal {
    /// Stop the loop, then wait for its thread.
    ///
    /// Both halves, and in that order. The thread is parked in a poll on the pseudoconsole's pipes;
    /// `Msg::Shutdown` is what wakes it, and joining without sending it would wait for a shell that
    /// has not been asked to stop. Closing a tab must not take the window with it.
    fn drop(&mut self) {
        let _ = self.to_pty.send(Msg::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A shell this program knows how to start.
struct Shell {
    /// What to show the user, and what to put in an error.
    name: String,
    program: String,
    args: Vec<String>,
    /// Variables the shell needs beyond what this process inherits.
    env: Vec<(String, String)>,
}

impl Shell {
    /// The shell to run, in the order this program prefers them.
    ///
    /// `terminal_shell=` in the settings overrides it outright and may be a full path, for a shell
    /// installed somewhere this does not look.
    fn find() -> Option<Self> {
        if let Some(asked) = std::env::var("AZUR_SHELL").ok().filter(|text| !text.is_empty()) {
            return Some(Self::named(&asked));
        }
        Self::bash().or_else(|| Self::pwsh()).or_else(|| Some(Self::cmd()))
    }

    /// One of the three names, or a path to something else.
    fn named(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "bash" | "git" | "gitbash" | "git-bash" => Self::bash().unwrap_or_else(Self::cmd),
            "pwsh" | "powershell" => Self::pwsh().unwrap_or_else(Self::cmd),
            "cmd" => Self::cmd(),
            // A path, or a program on `PATH`. No arguments: a shell nobody here has heard of is a
            // shell whose flags nobody here can guess.
            _ => Self {
                name: text.to_owned(),
                program: text.to_owned(),
                args: Vec::new(),
                env: Vec::new(),
            },
        }
    }

    /// Git Bash, and **the environment variable is not optional**.
    ///
    /// `--login` is what a Git Bash shortcut passes and what makes the MSYS environment — `PATH`,
    /// `HOME`, the whole of `/usr/bin` — exist at all. It also sources `/etc/profile`, which ends by
    /// changing to the home directory unless `CHERE_INVOKING` is set. So without that variable the
    /// panel opens a shell in `~` while the pane beside it shows something else, and
    /// `working_directory` looks like it was ignored.
    fn bash() -> Option<Self> {
        let mut tried = Vec::new();
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                tried.push(base.join(r"Git\bin\bash.exe"));
                tried.push(base.join(r"Programs\Git\bin\bash.exe"));
            }
        }
        // And wherever `git` itself is: `…\Git\cmd\git.exe` puts bash two doors along.
        if let Some(git) = which("git.exe") {
            if let Some(root) = git.parent().and_then(Path::parent) {
                tried.push(root.join(r"bin\bash.exe"));
            }
        }
        tried.push(PathBuf::from("bash.exe"));

        let found = tried
            .into_iter()
            .find(|path| path.file_name().is_some_and(|name| name == "bash.exe") && path.is_file())
            .or_else(|| which("bash.exe"))?;
        Some(Self {
            name: "Git Bash".to_owned(),
            program: found.to_string_lossy().into_owned(),
            args: vec!["--login".to_owned(), "-i".to_owned()],
            env: vec![("CHERE_INVOKING".to_owned(), "1".to_owned())],
        })
    }

    fn pwsh() -> Option<Self> {
        let (name, program) = match which("pwsh.exe") {
            Some(path) => ("PowerShell", path),
            None => ("Windows PowerShell", which("powershell.exe")?),
        };
        Some(Self {
            name: name.to_owned(),
            program: program.to_string_lossy().into_owned(),
            args: vec!["-NoLogo".to_owned()],
            env: Vec::new(),
        })
    }

    /// The one that is always there, and the reason nothing above has to succeed.
    fn cmd() -> Self {
        Self {
            name: "Command Prompt".to_owned(),
            program: "cmd.exe".to_owned(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    fn options(&self, dir: &Path) -> tty::Options {
        let mut env: std::collections::HashMap<String, String> =
            self.env.iter().cloned().collect();
        // Set here rather than through `tty::setup_env`, which puts them in *this* process's
        // environment with `env::set_var` — a global mutation on behalf of one panel, from a
        // program that also runs shell extensions in-process.
        //
        // `xterm-256color` rather than `alacritty`: this is not Alacritty and cannot promise the
        // terminfo entry is installed, and a `TERM` naming an entry the shell cannot find leaves
        // programs with no capabilities at all.
        env.insert("TERM".to_owned(), "xterm-256color".to_owned());
        env.insert("COLORTERM".to_owned(), "truecolor".to_owned());

        tty::Options {
            shell: Some(tty::Shell::new(self.program.clone(), self.args.clone())),
            // A pane showing This PC has no folder for a shell to be in, and the empty path is how
            // that is spelled everywhere in this program.
            working_directory: (!dir.as_os_str().is_empty()).then(|| dir.to_path_buf()),
            // The shell has gone and the panel says so; there is nothing left to drain into.
            drain_on_exit: false,
            env,
            escape_args: true,
        }
    }
}

/// Where a program is, by the same search the shell would do.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grid is sized by whole cells, and never to nothing.
    ///
    /// The floor matters: a panel 10.5 cells wide is 10 columns, because the eleventh would be
    /// drawn half outside the rect. The minimum matters more — a pseudoconsole of zero columns is
    /// not a thing, and dragging the splitter shut asks for one every frame on the way.
    #[test]
    fn a_grid_is_whole_cells_and_never_empty() {
        let cell = egui::vec2(8.0, 17.0);
        assert_eq!(
            Size::fitting(egui::vec2(800.0, 340.0), cell),
            Size { cols: 100, rows: 20 }
        );
        assert_eq!(
            Size::fitting(egui::vec2(84.0, 20.0), cell),
            Size { cols: 10, rows: 1 },
            "a part-cell is not a cell"
        );
        let shut = Size::fitting(egui::vec2(0.0, 0.0), cell);
        assert!(shut.cols >= MIN.cols && shut.rows >= MIN.rows, "{shut:?}");
        // And a cell of nothing, which is what an unmeasured font would give.
        let unmeasured = Size::fitting(egui::vec2(400.0, 200.0), egui::Vec2::ZERO);
        assert!(unmeasured.cols >= MIN.cols && unmeasured.rows >= MIN.rows);
    }

    /// What a shell is told about its own size, in the shape ConPTY wants.
    #[test]
    fn the_pseudoconsole_is_told_cells_and_pixels() {
        let size = Size { cols: 120, rows: 30 };
        let window = window_size(size, egui::vec2(7.5, 17.0));
        assert_eq!((window.num_cols, window.num_lines), (120, 30));
        // Truncated to whole pixels, and never zero: ConPTY divides by these.
        assert_eq!((window.cell_width, window.cell_height), (7, 17));
        let degenerate = window_size(size, egui::Vec2::ZERO);
        assert!(degenerate.cell_width >= 1 && degenerate.cell_height >= 1);
    }

    /// Git Bash is asked for by name and comes back with the two things that make it open in the
    /// right folder.
    ///
    /// `--login` is what builds the MSYS environment, and `CHERE_INVOKING` is what stops
    /// `/etc/profile` changing to the home directory on the way — which is a bug that looks exactly
    /// like `working_directory` being ignored. Skipped where Git is not installed rather than
    /// failing, since that is a fact about the machine.
    #[test]
    fn git_bash_is_started_where_the_pane_is() {
        let Some(bash) = Shell::bash() else {
            return;
        };
        assert!(bash.program.to_ascii_lowercase().ends_with("bash.exe"), "{}", bash.program);
        assert!(bash.args.iter().any(|arg| arg == "--login"));
        assert_eq!(
            bash.env.iter().find(|(name, _)| name == "CHERE_INVOKING").map(|(_, v)| v.as_str()),
            Some("1"),
            "without this, `--login` changes to the home directory and the folder is lost"
        );

        let here = std::env::current_dir().expect("a working directory");
        let options = bash.options(&here);
        assert_eq!(options.working_directory.as_deref(), Some(here.as_path()));
        assert_eq!(options.env.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert!(
            !options.env.contains_key("PATH"),
            "the shell's own profile builds its PATH; this must not preempt it"
        );

        // This PC is a list of volumes, not a folder a shell can start in.
        assert_eq!(bash.options(Path::new("")).working_directory, None);
    }

    /// A name nobody here knows is taken as a program, and `cmd` is always reachable.
    #[test]
    fn an_unknown_shell_is_taken_at_its_word() {
        let odd = Shell::named(r"C:\tools\nu.exe");
        assert_eq!(odd.program, r"C:\tools\nu.exe");
        assert!(odd.args.is_empty(), "nobody here knows this shell's flags");
        assert_eq!(Shell::named("cmd").program, "cmd.exe");
        // Whatever else is missing, something can always be started.
        assert!(Shell::find().is_some());
    }
}
