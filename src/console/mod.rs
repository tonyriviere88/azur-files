//! Running commands in a shell that stays alive, and collecting what they print.
//!
//! Not a terminal. There is no pseudoconsole, no escape-sequence interpreter and no grid — a shell
//! is started on plain pipes, commands are written to its standard input, and what comes back is
//! text in blocks. Which is the right shape for what this is actually for: running a build, a
//! `git status`, a test, and reading the answer. Drawing those blocks is somebody else's job; this
//! file is the shell and the protocol, and it knows nothing about how any of it looks.
//!
//! What that trade buys is the whole of this file being four hundred lines instead of a terminal
//! emulator. What it costs is everything interactive: no `git rebase -i`, no password prompt, no
//! pager. See [What it cannot do](#what-it-cannot-do).
//!
//! # The shell stays alive, so a command has to say when it is done
//!
//! One shell per [`Kind`], living across commands, because that is what makes `cd`, `export` and an
//! activated virtualenv still be true for the next command. The cost of a live shell on a pipe is
//! that **nothing marks the end of a command's output**: with no terminal there is no prompt to
//! recognise, so the pipe just goes quiet and quiet is indistinguishable from slow.
//!
//! So each command is followed down the same pipe by a line this program wrote:
//!
//! ```text
//! cargo test
//! [AZUR:7:0:D:/Sources/MyTools]
//! ```
//!
//! Reading until that line arrives gives four things at once — the command finished, its exit
//! status, where the shell now is, and which command it belonged to. Everything the panel does is
//! built on it: the running spinner, the red exit code, and the block you can collapse, because a
//! block is exactly "the output between one sentinel and the next".
//!
//! The `id` is checked rather than assumed. A sentinel for a command that was stopped arrives after
//! the shell has been replaced, and applying it would close a block belonging to a different one.
//!
//! # Why the marker is printable
//!
//! `\x1e` — the ASCII record separator — is the tempting choice and no program prints it. It is not
//! used here for one reason: `cmd.exe` has no way to emit a control character from an `echo`, and a
//! protocol that is two protocols is twice as much to get wrong. `[AZUR:…]` at the start of a line
//! is a collision nobody will have, and it can be read by eye when a shell misbehaves — which is
//! worth more than the theoretical purity.
//!
//! # What it cannot do
//!
//! A pipe is not a terminal and every program can tell:
//!
//! - **Nothing interactive.** `git rebase -i` opens an editor that is not there; `ssh` asks for a
//!   password nothing can type. Both hang until [`Session::stop`]. The environment turns off what
//!   can be turned off — `PAGER=cat` and `GIT_TERMINAL_PROMPT=0` — because a `git log` that hangs in
//!   `less` is the first thing anybody would hit.
//! - **No colour**, and that is a feature here: every one of these tools tests `isatty` and prints
//!   plain text to a pipe. What arrives coloured anyway is stripped — see [`strip_ansi`] — because a
//!   panel full of `[32m` is worse than a panel with no colour in it.
//! - **`Ctrl+C` is a kill, not a signal.** See [`Session::stop`].

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};

/// The most lines one block keeps.
///
/// A stray `find /` prints for as long as you let it, and the panel must not become the reason this
/// process ran out of memory. The oldest lines go and the count of them is kept, so the block says
/// what it dropped rather than quietly being a lie about what the command printed.
const LINES: usize = 5_000;

/// And the most blocks a session keeps, for the same reason.
const BLOCKS: usize = 200;

/// How much text one read hands over. 8 KiB is a pipe buffer's worth.
const CHUNK: usize = 8 << 10;

/// Which shell a session is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    /// Git Bash — the default, and the one this is written against.
    #[default]
    Bash,
    PowerShell,
    /// Best effort. `cmd` has no way to report a working directory that survives `cd /d`, no useful
    /// quoting, and an `echo` that cannot emit a control character; it is here because it is the one
    /// shell that is always installed.
    Cmd,
}

impl Kind {
    /// In the order the dropdown offers them.
    pub const ALL: [Kind; 3] = [Kind::Bash, Kind::PowerShell, Kind::Cmd];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Bash => "bash",
            Kind::PowerShell => "pwsh",
            Kind::Cmd => "cmd",
        }
    }

    /// The next one round, for `Shift+Tab`.
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// Read back what [`Kind::label`] wrote, for the settings file.
    ///
    /// The label rather than a second spelling of the same three names: a settings key and a dropdown
    /// entry that can drift apart is a settings key that silently stops matching.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.label() == text)
    }

    /// The line that closes a command: prints the marker, this command's status, and where the shell
    /// now is.
    ///
    /// **The status is captured into a variable first.** Writing `$?` directly into the `printf`
    /// works, but only by an argument-evaluation-order argument that is easy to break later — and
    /// the failure mode is every command reporting the status of the `pwd` inside its own sentinel.
    fn sentinel(self, id: u64) -> String {
        match self {
            // `pwd -W` is a builtin in Git Bash and gives the Windows spelling, so the panel gets a
            // path the rest of this program can navigate to without translation.
            Kind::Bash => format!(
                "__azur=$?; printf '[AZUR:{id}:%d:%s]\\n' \"$__azur\" \"$(pwd -W 2>/dev/null || pwd)\"\n"
            ),
            // Two PowerShell rules, both of which this got wrong first time.
            //
            // `$LASTEXITCODE` is only set by native programs; a failing *cmdlet* leaves it alone and
            // sets `$?` instead. Both are consulted, or `Get-ChildItem nowhere` reports success.
            //
            // And `${__azur}` **must** wear its braces. Inside a double-quoted string a `$name:` is
            // read as a scope-qualified variable — the same syntax as `$env:PATH` — so
            // `"$__azur:$(…)"` is a parse error rather than a value followed by a colon. It fails at
            // the shell, on stderr, while the command itself succeeds: the block simply never closes
            // and the panel appears to hang.
            Kind::PowerShell => format!(
                "$__ok=$?; $__azur=$LASTEXITCODE; if ($null -eq $__azur) {{ $__azur = if ($__ok) {{ 0 }} else {{ 1 }} }}; \
                 Write-Output \"[AZUR:{id}:${{__azur}}:$($PWD.Path)]\"\n"
            ),
            Kind::Cmd => format!("echo [AZUR:{id}:%ERRORLEVEL%:%CD%]\n"),
        }
    }

    /// Move the shell to a folder, in this shell's own spelling.
    ///
    /// Quoted in all three, because a path with a space in it is the normal case on Windows — and
    /// `cd /d` in `cmd` rather than plain `cd`, which changes directory *within* a drive and silently
    /// does nothing at all when handed another one.
    fn cd(self, dir: &Path) -> String {
        let path = dir.to_string_lossy();
        match self {
            // Forward slashes: a backslash inside a double-quoted bash string is an escape.
            Kind::Bash => format!("cd \"{}\"\n", path.replace('\\', "/")),
            Kind::PowerShell => format!("Set-Location -LiteralPath \"{path}\"\n"),
            Kind::Cmd => format!("cd /d \"{path}\"\n"),
        }
    }
}

/// One line of output, and which pipe it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// Standard error. Kept separate rather than merged, which is one place this beats a terminal:
    /// a real one interleaves the two into a single stream and you can no longer tell which was
    /// which. The panel colours it.
    pub err: bool,
}

/// One command and everything it printed.
#[derive(Clone, Debug, Default)]
pub struct Block {
    pub command: String,
    pub lines: Vec<Line>,
    /// `None` while it is still running.
    pub code: Option<i32>,
    /// Lines the cap threw away, so the block can say so.
    pub dropped: usize,
    /// Whether the panel is hiding its output.
    pub collapsed: bool,
    /// Which sentinel closes it — and, because it outlives the block's position in the list, the
    /// name the panel knows it by.
    ///
    /// A block's index is not a name: the cap drops blocks off the front, `Del` removes one from
    /// the middle, and either of those would silently move a selection onto its neighbour.
    pub id: u64,
}

impl Block {
    pub fn running(&self) -> bool {
        self.code.is_none()
    }

    /// Whether it ended in anything other than success.
    pub fn failed(&self) -> bool {
        self.code.is_some_and(|code| code != 0)
    }

    fn push(&mut self, text: String, err: bool) {
        if self.lines.len() >= LINES {
            self.lines.remove(0);
            self.dropped += 1;
        }
        self.lines.push(Line { text, err });
    }

    /// Overwrite the last line, which is what a `\r` means.
    fn overwrite(&mut self, text: String, err: bool) {
        match self.lines.last_mut() {
            Some(last) if last.err == err => last.text = text,
            _ => self.push(text, err),
        }
    }
}

/// What a reader thread has to say.
enum Chunk {
    Out(String),
    Err(String),
    /// The pipe closed, which means the shell has gone.
    Closed,
}

/// A shell, its blocks, and the pipes between them.
pub struct Session {
    pub kind: Kind,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    from: Receiver<Chunk>,
    /// Everything spawned under the shell, so [`Session::stop`] can end all of it.
    job: Job,
    /// The shell exited on its own — `exit` typed at it, or a crash.
    gone: bool,

    pub blocks: Vec<Block>,
    /// Where the shell is, as of the last sentinel.
    cwd: Option<PathBuf>,
    /// The folder it was started in, and where a replacement is put back.
    home: PathBuf,
    next_id: u64,
    /// Partial lines, one per pipe, waiting for the newline that completes them.
    partial: [String; 2],
    /// Whether the last line of the block is the line this pipe is still writing.
    ///
    /// A carriage return leaves the cursor at the start of the line it is on, so it is **what comes
    /// next** that overwrites — and it overwrites whether it ends in another carriage return or in a
    /// newline. Attaching the overwrite to the carriage return itself instead looks right and leaves
    /// one stale line behind per progress bar, which is every `cargo build`.
    redrawing: [bool; 2],
    /// Commands typed while one was running.
    ///
    /// The panel turns Send into Stop while something is running, so this only fills from a shell
    /// that prints slowly — but a queue is four lines and losing what somebody typed is not.
    queued: VecDeque<String>,
    /// Lines written down the pipe that `cmd` is going to read back to us. See [`Session::expect_echo`].
    echoes: VecDeque<String>,
}

impl Session {
    /// Start a shell in a folder.
    pub fn start(kind: Kind, dir: &Path, ctx: &egui::Context) -> Result<Self, String> {
        let (child, from, job) = spawn(kind, dir, ctx)?;
        let mut session = Self {
            kind,
            stdin: None,
            child: Some(child),
            from,
            job,
            gone: false,
            blocks: Vec::new(),
            cwd: (!dir.as_os_str().is_empty()).then(|| dir.to_path_buf()),
            home: dir.to_path_buf(),
            next_id: 1,
            partial: [String::new(), String::new()],
            redrawing: [false, false],
            queued: VecDeque::new(),
            echoes: VecDeque::new(),
        };
        session.stdin = session.child.as_mut().and_then(|child| child.stdin.take());
        session.prepare();
        Ok(session)
    }

    /// The shell's own first commands, whose output nobody sees.
    ///
    /// `chcp` is the one that matters: `cmd` writes its output in the OEM code page, so without it
    /// every accented file name in a listing comes back as mojibake. It is sent as a command rather
    /// than set in the environment because the code page is a property of the console, not of the
    /// process.
    ///
    /// **`@echo off` is the one that stopped `cmd` working at all.** `cmd` writes its prompt even
    /// when it is reading from a pipe, and a prompt has no newline after it — so the next thing down
    /// the pipe continues that line, and the next thing is the sentinel.
    /// `D:\src>[AZUR:1:0:D:\src]` is not a line beginning with the marker, so nothing recognised it,
    /// no block ever closed, and every command in `cmd` sat there saying "running" for ever.
    ///
    /// `@echo off` turns the prompt off outright. `prompt $_` also works — `$_` is `cmd`'s escape for
    /// a newline, so the prompt ends its own line — but it leaves two blank lines per command in the
    /// log where this leaves none. What neither of them turns off is `cmd` reading each command back
    /// to us; see [`Session::expect_echo`] for that half.
    fn prepare(&mut self) {
        let setup = match self.kind {
            Kind::Bash => "",
            // `UTF8Encoding::new($false)` rather than `[Text.Encoding]::UTF8`, which is the same
            // encoding constructed *with* a byte-order mark. Windows PowerShell 5.1 — the fallback
            // when `pwsh` is absent — will emit that mark, and `U+FEFF` is not whitespace, so a
            // sentinel wearing one silently stops being a sentinel. `strip_ansi` drops it as well,
            // because one line of defence for a hang with no error message is not enough.
            Kind::PowerShell => "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)\n",
            // `echo.` prints an empty line, and it is there to end one. `cmd` opens by writing a
            // prompt, a prompt has no newline behind it, and a part-line waits for the next newline
            // to arrive — which would be the first output line of the first command, so the block
            // opened with `D:\src>hello`. One newline of the shell's own, while nothing is listening,
            // finishes that line where it can still be thrown away.
            Kind::Cmd => "@echo off\nchcp 65001>nul\necho.\n",
        };
        if !setup.is_empty() {
            self.write(setup);
        }
    }

    /// Run a command in a folder, changing to it first if the shell is not already there.
    ///
    /// **The folder is the pane's, and that is the direction that matters.** The explorer is the thing
    /// you steer — with the breadcrumb, the sidebar, a double click — and a command runs wherever you
    /// are looking. The shell follows the window, not the other way round; the only time the window
    /// follows the shell is when a command itself moved it, which is what the sentinel reports.
    ///
    /// The `cd` goes down the pipe in the same write as the command and the sentinel, so nothing can
    /// interleave between them — and the block still shows only what was typed, because a `cd` this
    /// program inserted is not something the user should have to read.
    pub fn send(&mut self, dir: Option<&Path>, command: &str) {
        let command = command.trim_end();
        if command.is_empty() || self.gone {
            return;
        }
        if self.running() {
            self.queued.push_back(command.to_owned());
            return;
        }
        let moved = dir
            .filter(|dir| !dir.as_os_str().is_empty() && Some(*dir) != self.cwd.as_deref())
            .map(|dir| self.kind.cd(dir));
        self.dispatch_with(moved, command.to_owned());
    }

    fn dispatch(&mut self, command: String) {
        self.dispatch_with(None, command);
    }

    fn dispatch_with(&mut self, before: Option<String>, command: String) {
        let id = self.next_id;
        self.next_id += 1;
        // **Anything written without a newline behind it belongs to nobody.** Nothing is running or
        // this would have been queued, so a part-line still waiting for its ending was written by the
        // shell itself rather than by a command — and the next newline to arrive will be the new
        // block's, which would take the orphan with it.
        //
        // That is not hypothetical: `cmd` opens by writing a prompt, a prompt has no newline, and so
        // the first line of the first block came out as `D:\src>hello`. Cleared for every shell,
        // because a part-line outliving the command that started it is wrong in all of them.
        self.partial = [String::new(), String::new()];
        self.redrawing = [false, false];
        if self.blocks.len() >= BLOCKS {
            self.blocks.remove(0);
        }
        // The sentinel goes down the pipe in the same write as the command, so nothing can be
        // interleaved between them by a second `send`.
        let mut text = before.unwrap_or_default();
        text.push_str(&command);
        text.push('\n');
        text.push_str(&self.kind.sentinel(id));
        self.blocks.push(Block {
            command,
            id,
            ..Block::default()
        });
        self.write(&text);
    }

    /// Lines this program wrote that the shell is going to read back to us, oldest first.
    ///
    /// **Only `cmd`, and only because it cannot be told not to.** A `cmd` reading commands from a
    /// pipe writes each line it reads back down stdout, and `@echo off` does not stop it — that
    /// setting is about the prompt, and the read-back happens because stdin is not a console. So
    /// every block came out with the command repeated under its own header, followed by the literal
    /// text of the sentinel command, which reads as though the shell had printed the protocol.
    ///
    /// Since this program is the one that wrote those lines it knows exactly what they will be, so
    /// they are matched and dropped rather than guessed at by shape. A line the *command* prints that
    /// happens to be identical is dropped too, which is the one thing this can get wrong, and it
    /// costs one duplicate line in `cmd` alone.
    fn expect_echo(&mut self, text: &str) {
        if self.kind != Kind::Cmd {
            return;
        }
        self.echoes
            .extend(text.lines().map(|line| line.trim_end().to_owned()));
    }

    fn write(&mut self, text: &str) {
        self.expect_echo(text);
        let failed = match self.stdin.as_mut() {
            Some(stdin) => stdin.write_all(text.as_bytes()).and_then(|()| stdin.flush()).is_err(),
            None => true,
        };
        if failed {
            self.gone = true;
        }
    }

    /// Stop whatever is running.
    ///
    /// **A kill, not an interrupt, and the shell goes with it.** `GenerateConsoleCtrlEvent` signals a
    /// console *process group*, and this program is a `windows` subsystem process with no console to
    /// have a group in — so there is no way to deliver a real `Ctrl+C` from here. What there is is a
    /// job object holding the shell and everything it started, and ending that is reliable, immediate
    /// and total.
    ///
    /// The cost is the session: a killed program does not get to clean up, and the shell's
    /// environment goes with it. The working directory does not, because the sentinel has been
    /// reporting it all along — so the replacement starts where the old one was, and the only thing
    /// actually lost is whatever was `export`ed by hand.
    pub fn stop(&mut self, ctx: &egui::Context) {
        self.job.end();
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdin = None;
        self.child = None;
        self.queued.clear();

        // Close whatever was running, so it reads as stopped rather than as still going.
        if let Some(block) = self.blocks.last_mut().filter(|block| block.running()) {
            block.code = Some(130);
            block.push("(stopped)".to_owned(), true);
        }

        let where_ = self.cwd.clone().unwrap_or_else(|| self.home.clone());
        match spawn(self.kind, &where_, ctx) {
            Ok((child, from, job)) => {
                self.child = Some(child);
                self.from = from;
                self.job = job;
                self.gone = false;
                self.stdin = self.child.as_mut().and_then(|child| child.stdin.take());
                self.partial = [String::new(), String::new()];
                self.redrawing = [false, false];
                self.prepare();
            }
            Err(why) => {
                self.gone = true;
                if let Some(block) = self.blocks.last_mut() {
                    block.push(why, true);
                }
            }
        }
    }

    /// Take everything the shell has said since the last frame.
    pub fn poll(&mut self) {
        while let Ok(chunk) = self.from.try_recv() {
            match chunk {
                Chunk::Out(text) => self.absorb(&text, false),
                Chunk::Err(text) => self.absorb(&text, true),
                Chunk::Closed => {
                    self.gone = true;
                    // A shell that has gone leaves nothing to finish the block, so it is closed
                    // here or it would spin for ever.
                    if let Some(block) = self.blocks.last_mut().filter(|b| b.running()) {
                        block.code = Some(-1);
                    }
                }
            }
        }
        // Anything queued while that was running.
        if !self.running() && !self.gone {
            if let Some(next) = self.queued.pop_front() {
                self.dispatch(next);
            }
        }
    }

    /// Feed one chunk of one pipe through the line assembler.
    ///
    /// Split out from the threads on purpose: `\r` handling, escape stripping and the sentinel are
    /// where the bugs are, and none of them need a process to be checked.
    fn absorb(&mut self, text: &str, err: bool) {
        let text = strip_ansi(text);
        let slot = usize::from(err);
        for piece in split(&text) {
            let (rest, redraw) = match piece {
                // No line ending yet: hold it and leave the redraw flag where it is, so a partial
                // arriving after a `carriage return` still overwrites when it is finally completed.
                Piece::Part(part) => {
                    self.partial[slot].push_str(part);
                    continue;
                }
                Piece::Line(rest) => (rest, false),
                // A carriage return with no newline behind it. `cargo` and `npm` redraw a progress
                // line several times a second this way.
                Piece::Redraw(rest) => (rest, true),
            };
            let mut line = std::mem::take(&mut self.partial[slot]);
            line.push_str(rest);
            let over = std::mem::replace(&mut self.redrawing[slot], redraw);
            self.line(line, err, over);
        }
    }

    /// One completed line. `over` replaces the block's last line instead of adding one.
    fn line(&mut self, line: String, err: bool, over: bool) {
        // A line `cmd` is reading back to us rather than one anything printed. See [`expect_echo`].
        // Checked before the sentinel, because the echo of the sentinel *command* is one of them and
        // it contains the marker — mid-line, so `sentinel` would reject it anyway, but leaving it to
        // be rejected is leaving it in the log.
        if !err && self.echoes.front().is_some_and(|echo| *echo == line.trim_end()) {
            self.echoes.pop_front();
            return;
        }
        // Output with the sentinel stuck to the end of it, because the command's last line had no
        // newline on it. Split into the two lines it should have been and handled as such.
        if !err {
            if let Some((printed, _)) = split_sentinel(&line) {
                let (printed, rest) = (printed.to_owned(), line[printed.len()..].to_owned());
                self.line(printed, false, over);
                self.line(rest, false, false);
                return;
            }
        }
        // Only stdout carries sentinels, and only for the block that is actually open.
        if !err {
            if let Some((id, code, cwd)) = sentinel(&line) {
                let open = self.blocks.last().is_some_and(|block| block.id == id);
                if open {
                    if let Some(path) = windows_path(&cwd) {
                        self.cwd = Some(path);
                    }
                    if let Some(block) = self.blocks.last_mut() {
                        block.code = Some(code);
                        // A command that succeeded and printed nothing folds itself. `cd`, `git add`,
                        // `mkdir` — the header is the whole story, and a blank body under each of
                        // them is how a log of twenty commands stops being readable. A *failed*
                        // silent command stays open, because that one you want to look at.
                        block.collapsed = code == 0 && block.lines.is_empty();
                    }
                }
                // Either way the line itself is protocol and is never shown.
                return;
            }
        }
        if let Some(block) = self.blocks.last_mut() {
            if over {
                block.overwrite(line, err);
            } else {
                block.push(line, err);
            }
        }
    }

    /// Whether a command is in flight.
    pub fn running(&self) -> bool {
        self.blocks.last().is_some_and(Block::running)
    }

    /// Where the shell is, as of its last command.
    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Whether the shell itself has gone.
    pub fn gone(&self) -> bool {
        self.gone
    }

    /// Throw the history away, keeping the shell.
    pub fn clear(&mut self) {
        self.blocks.retain(Block::running);
    }
}

impl Drop for Session {
    /// End the job before the child, or a shell with children outlives the window that owned it.
    fn drop(&mut self) {
        self.job.end();
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A run of text between the characters that end a line.
enum Piece<'a> {
    /// No line ending yet.
    Part(&'a str),
    /// Ends the line.
    Line(&'a str),
    /// Ends the line by redrawing it.
    Redraw(&'a str),
}

/// Break a chunk at `\n` and at a `\r` that is not part of `\r\n`.
fn split(text: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let bytes = text.as_bytes();
    let mut from = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\n' => {
                pieces.push(Piece::Line(&text[from..at]));
                at += 1;
                from = at;
            }
            b'\r' => {
                let crlf = bytes.get(at + 1) == Some(&b'\n');
                pieces.push(if crlf {
                    Piece::Line(&text[from..at])
                } else {
                    Piece::Redraw(&text[from..at])
                });
                at += if crlf { 2 } else { 1 };
                from = at;
            }
            _ => at += 1,
        }
    }
    if from < bytes.len() {
        pieces.push(Piece::Part(&text[from..]));
    }
    pieces
}

/// The three numbers out of a sentinel line, or `None` if it is not one.
fn sentinel(line: &str) -> Option<(u64, i32, String)> {
    let inner = line.trim().strip_prefix("[AZUR:")?.strip_suffix(']')?;
    // Split from the left twice and keep the remainder whole: a Windows path contains colons.
    let (id, rest) = inner.split_once(':')?;
    let (code, cwd) = rest.split_once(':')?;
    Some((id.parse().ok()?, code.trim().parse().ok()?, cwd.to_owned()))
}

/// A sentinel with output stuck to the front of it: what the command printed, and the sentinel.
///
/// **A command whose last line has no newline on it leaves the sentinel sharing that line.** `printf
/// 'a'`, `echo -n`, a tool that ends without one — the shell writes the sentinel next, and the pipe
/// carries `a[AZUR:2:0:D:/src]`. That is not a line *beginning* with the marker, so nothing recognised
/// it and the block stayed open for ever, saying "running" about a command that had finished.
///
/// The discriminator is that the sentinel **ends the line**, which is what keeps
/// `see [AZUR:1:0:D:\x] in the log` from being one: a marker with text after it is a program talking
/// about the protocol, and a marker with text only *before* it is the protocol arriving late.
fn split_sentinel(line: &str) -> Option<(&str, (u64, i32, String))> {
    let at = line.rfind("[AZUR:")?;
    if at == 0 {
        return None;
    }
    let found = sentinel(&line[at..])?;
    Some((&line[..at], found))
}

/// Drop the escape sequences a program printed anyway.
///
/// Only dropped, never interpreted. Everything worth colouring here is already coloured by this
/// program — stderr, an exit code — and a half-honoured `SGR` is worse than none: a stray reset that
/// went missing leaves the rest of a build log the wrong colour with no way to tell why.
///
/// `CSI` and `OSC` cover what a tool with `--color=always` emits. An `OSC` runs to `BEL` or to the
/// two-byte `ST`, and a `CSI` to its first byte in `@`–`~`.
fn strip_ansi(text: &str) -> String {
    // A byte-order mark goes too, because it is the same kind of thing: a character in the stream
    // that is not content. `trim` would not remove it — `U+FEFF` is not whitespace — so a sentinel
    // that picked one up would stop parsing, and a sentinel that does not parse is a block that
    // never closes and a panel that appears to hang with nothing on screen to explain it.
    if !text.contains('\x1b') && !text.contains('\u{feff}') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{feff}' {
            continue;
        }
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // A two-character escape: nothing to keep and nothing to skip past.
            _ => {}
        }
    }
    out
}

/// Whatever the shell reported as its folder, as a path this program can navigate to.
///
/// `cmd` and PowerShell say `D:\x`; `pwd -W` says `D:/x`; a bash without it says `/d/x`, and WSL
/// says `/mnt/d/x`. Anything else — `/usr/bin`, `/tmp` — is a real directory inside the shell's own
/// root whose place on this file system cannot be worked out from the path, so it is refused rather
/// than guessed at.
pub fn windows_path(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let windows = |text: &str| Some(PathBuf::from(text.replace('/', "\\")));
    if text.starts_with("//") || text.starts_with(r"\\") {
        return windows(text);
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return windows(text);
    }
    // `/mnt/` first, or `/mnt/d/x` reads as a drive called `m`.
    let rest = text.strip_prefix("/mnt/").or_else(|| text.strip_prefix('/'))?;
    let mut chars = rest.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    match chars.next() {
        None => Some(PathBuf::from(format!("{letter}:\\"))),
        // Both are ASCII, so byte 2 is a character boundary.
        Some('/') => windows(&format!("{letter}:/{}", &rest[2..])),
        Some(_) => None,
    }
}

/// Start a shell, wire its pipes to threads, and put it in a job.
fn spawn(
    kind: Kind,
    dir: &Path,
    ctx: &egui::Context,
) -> Result<(Child, Receiver<Chunk>, Job), String> {
    let (program, args) = program(kind).ok_or_else(|| format!("{} is not installed", kind.label()))?;

    let mut command = Command::new(&program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !dir.as_os_str().is_empty() {
        command.current_dir(dir);
    }
    // Everything that stops a program waiting for a terminal it has not got.
    //
    // `PAGER` is the one that would be reported as a hang: `git log` with no pager set runs `less`,
    // which waits for a keypress from a terminal that does not exist. `TERM=dumb` is what tells the
    // rest of them not to try, and `GIT_TERMINAL_PROMPT=0` turns a credential prompt into an error
    // message, which is a thing you can read instead of a thing you have to stop.
    command
        .env("TERM", "dumb")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("CHERE_INVOKING", "1");
    crate::shell::no_window(&mut command);

    let mut child = command
        .spawn()
        .map_err(|why| format!("Cannot start {}: {why}", kind.label()))?;

    let job = Job::holding(&child);

    let (to_ui, from) = std::sync::mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        pipe(stdout, to_ui.clone(), false, ctx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        pipe(stderr, to_ui, true, ctx.clone());
    }
    Ok((child, from, job))
}

/// One thread per pipe, forwarding decoded text.
///
/// The thread does as little as possible: read bytes, decode, send. It deliberately does not know
/// about lines, escape sequences or the protocol — all of that is [`Session::absorb`], on the UI
/// thread, where it can be tested without a process.
///
/// The decoding is the one subtle part. A read can end in the middle of a UTF-8 sequence, and
/// `from_utf8_lossy` on each chunk would turn every such split into a replacement character — one
/// per 8 KiB, for ever. So the tail is carried to the next read.
fn pipe(mut source: impl Read + Send + 'static, to_ui: Sender<Chunk>, err: bool, ctx: egui::Context) {
    let _ = std::thread::Builder::new()
        .name(format!("console-{}", if err { "err" } else { "out" }))
        .spawn(move || {
            let mut buffer = [0u8; CHUNK];
            let mut tail: Vec<u8> = Vec::new();
            loop {
                let read = match source.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                tail.extend_from_slice(&buffer[..read]);
                let good = match std::str::from_utf8(&tail) {
                    Ok(_) => tail.len(),
                    Err(why) => why.valid_up_to(),
                };
                if good == 0 {
                    continue;
                }
                let text = String::from_utf8_lossy(&tail[..good]).into_owned();
                tail.drain(..good);
                let chunk = if err { Chunk::Err(text) } else { Chunk::Out(text) };
                if to_ui.send(chunk).is_err() {
                    return;
                }
                // Nothing else would wake a window that paints on demand.
                ctx.request_repaint();
            }
            let _ = to_ui.send(Chunk::Closed);
            ctx.request_repaint();
        });
}

/// Where a shell is and what to pass it.
///
/// The arguments are all "read commands from standard input and do not pretend to be interactive".
/// `bash -i` is the wrong flag here and looks right: on a pipe it turns on job control, warns about
/// it, and prints a prompt into the output.
fn program(kind: Kind) -> Option<(PathBuf, Vec<&'static str>)> {
    match kind {
        // `--login` is what builds the MSYS environment — `PATH`, `HOME`, `/usr/bin`. It also ends
        // by changing to the home directory unless `CHERE_INVOKING` is set, which `spawn` does.
        Kind::Bash => bash().map(|path| (path, vec!["--login", "-s"])),
        Kind::PowerShell => which("pwsh.exe")
            .or_else(|| which("powershell.exe"))
            .map(|path| (path, vec!["-NoLogo", "-Command", "-"])),
        // `/Q` is echo off, without which every command appears twice.
        Kind::Cmd => Some((PathBuf::from("cmd.exe"), vec!["/Q", "/K"])),
    }
}

fn bash() -> Option<PathBuf> {
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        let Some(base) = std::env::var_os(var) else {
            continue;
        };
        for tail in [r"Git\bin\bash.exe", r"Programs\Git\bin\bash.exe"] {
            let candidate = PathBuf::from(&base).join(tail);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Wherever `git` itself is: `…\Git\cmd\git.exe` puts bash two doors along.
    if let Some(root) = which("git.exe").as_deref().and_then(Path::parent).and_then(Path::parent) {
        let candidate = root.join(r"bin\bash.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    which("bash.exe")
}

/// Where a program is, by the search the shell would do.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// A job object holding a shell and everything it starts.
///
/// The only reliable way to stop a command tree from a process with no console. A `git` that started
/// `ssh` that started something else is one call, and none of them can escape it — the kill-on-close
/// limit means even losing this handle takes the tree down rather than leaking it.
struct Job(#[cfg(windows)] isize);

#[cfg(windows)]
#[path = "../windows/job.rs"]
mod win;

#[cfg(not(windows))]
mod win {
    use super::Job;
    use std::process::Child;

    impl Job {
        pub fn holding(_child: &Child) -> Self {
            Self()
        }
        pub fn end(&mut self) {}
    }
}

#[cfg(test)]
mod tests;
