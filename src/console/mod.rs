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

/// How long one line may get before it is broken in two.
///
/// [`LINES`] caps how many lines a block keeps and nothing capped how long one of them could be, which
/// left the same hole one level down: a program that writes without ever ending a line — `cat` of a
/// file with no line endings, one long JSON document, a tool drawing its own progress with neither
/// `\n` nor `\r` — grew [`Session::partial`] by every read for as long as it ran, until this process
/// was the reason the machine ran out of memory.
///
/// **Broken and not dropped**, which is the part that matters: the sentinel that closes a block arrives
/// on the end of whatever line was open when the command finished, so a cap that threw the excess away
/// would eventually throw away the sentinel — and a block that never closes is the failure this whole
/// file is arranged to prevent. A forced break is also what a terminal does with a line longer than its
/// window, and it keeps every byte.
///
/// One pipe read's worth ([`CHUNK`]) — longer than any line anybody reads, and small enough that the
/// number it really decides stays reasonable: a block can hold [`LINES`] of these, so this is what puts
/// a ceiling of 40 MB on one runaway command. Only one [`Piece::Part`] can arrive per read, so a broken
/// line is at most a chunk over the cap.
const LINE_CAP: usize = CHUNK;

/// Which of the two slots in [`Session::partial`] and [`Session::redrawing`] is standard error.
///
/// `usize::from(err)` is how [`Session::absorb`] picks between them, so this is `usize::from(true)`
/// spelled out for the one place that names the pipe rather than being handed it.
const ERR: usize = 1;

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

    /// The statement that closes a command: prints the marker, this command's status, and where the
    /// shell now is.
    ///
    /// **No separator and no line ending** — [`Session::dispatch_with`] owns both, because whether
    /// this goes on the command's own line is the difference between a command that can be typed at
    /// and one that cannot. See [`Kind::needs_its_own_line`].
    ///
    /// **The status is captured into a variable first.** Writing `$?` directly into the `printf`
    /// works, but only by an argument-evaluation-order argument that is easy to break later — and
    /// the failure mode is every command reporting the status of the `pwd` inside its own sentinel.
    fn closing(self, id: u64) -> String {
        match self {
            // `pwd -W` is a builtin in Git Bash and gives the Windows spelling, so the panel gets a
            // path the rest of this program can navigate to without translation.
            Kind::Bash => format!(
                "__azur=$?; printf '[AZUR:{id}:%d:%s]\\n' \"$__azur\" \"$(pwd -W 2>/dev/null || pwd)\""
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
                 Write-Output \"[AZUR:{id}:${{__azur}}:$($PWD.Path)]\""
            ),
            // **`!…!` and not `%…%` for both of them**, which is why `cmd` is started `/V:ON`. On a
            // line of its own either spelling works. On the *command's* line `%ERRORLEVEL%` and `%CD%`
            // are expanded when the line is parsed — before the command has run — so the block
            // reported the status *and the folder* of the command before it. Measured both ways:
            // `cmd /c exit 3 & echo %ERRORLEVEL%` prints `0` where two lines print `3`, and `cd .. &
            // echo %CD%` names the folder it started in. Delayed expansion reads them when the `echo`
            // runs, which is the whole point of the sentinel.
            Kind::Cmd => format!("echo [AZUR:{id}:!ERRORLEVEL!:!CD!]"),
        }
    }

    /// What puts [`Kind::closing`] after a command on the same line.
    ///
    /// Nothing but a space after a `&` or a `;` the command already ends with: those are separators
    /// themselves, and a second one is a syntax error rather than a no-op.
    fn joiner(self, command: &str) -> &'static str {
        let text = command.trim_end();
        if text.ends_with('&') || text.ends_with(';') {
            return " ";
        }
        match self {
            Kind::Bash | Kind::PowerShell => "; ",
            Kind::Cmd => " & ",
        }
    }

    /// Whether the closing statement has to go on a line of its own.
    ///
    /// **One line is what lets a command read what you type.** A shell reading commands from a pipe
    /// reads exactly one line at a time — measured, by giving `head -1` a sentinel on the next line
    /// and watching it read the sentinel — so a closing statement on its own line is sitting in the
    /// pipe when the command starts, and the command eats it. The block then never closes, because the
    /// line that closes it has been consumed. On one line the shell has taken both before the command
    /// runs, and the pipe is empty for the command to read from.
    ///
    /// So one line is the default and this is the list of commands whose own text would eat, comment
    /// out or condition whatever is put after it. Those go back to two lines: they cannot be typed at,
    /// which is exactly the old behaviour, and their block still closes — which is the part that must
    /// never fail.
    fn needs_its_own_line(self, command: &str) -> bool {
        let text = command.trim_end();
        // Ordered before the bare `&` in `joiner`: `&&` and `||` would make the closing statement
        // conditional on the command succeeding, so a failing command would never close its block.
        // A trailing `|` or a continuation character means the command is unfinished either way.
        let continues = ["&&", "||", "|"]
            .iter()
            .chain(match self {
                Kind::Cmd => ["^"].iter(),
                _ => ["\\"].iter(),
            })
            .any(|tail| text.ends_with(tail));
        if continues {
            return true;
        }
        match self {
            // A `#` runs to the end of the line and takes the closing statement with it. Tested for
            // anywhere rather than only outside quotes: `git commit -m "fix #12"` is then sent over two
            // lines for no reason, which costs it nothing — it is not a command anybody types at.
            Kind::Bash | Kind::PowerShell => text.contains('#'),
            // `rem` is `cmd`'s comment and swallows the `&` as well.
            //
            // Compared as **bytes**, because `text[..3]` panics when byte 3 falls inside a character:
            // `ab°` is four bytes and the third of them is half of the `°`, so typing that into the
            // console with `cmd` selected took the window out. The answer is the same for anything
            // ASCII — if the first three bytes *are* `rem` then byte 3 is a boundary anyway.
            Kind::Cmd => text
                .as_bytes()
                .get(..3)
                .is_some_and(|head| head.eq_ignore_ascii_case(b"rem")),
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
    /// Whether this command can be typed at while it runs.
    ///
    /// True when its closing statement went on its own line — see [`Kind::needs_its_own_line`] — and
    /// so the pipe is the command's to read rather than the shell's. Recorded per block rather than
    /// asked again later, because the answer depends on the text of the command that is running and
    /// the panel is drawing long after that text was decided about.
    pub takes_input: bool,
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
            cwd: (!crate::fs::is_synthetic(dir)).then(|| dir.to_path_buf()),
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
            .filter(|dir| !crate::fs::is_synthetic(dir) && Some(*dir) != self.cwd.as_deref())
            .map(|dir| self.kind.cd(dir));
        self.dispatch_or_refuse(moved, command.to_owned());
    }

    /// Run a command, or say why it cannot be. **Only with nothing already running.**
    ///
    /// Both ways in go through here — typed, and taken off the queue a command ahead of it filled —
    /// because the queue was the hole: a `vim` typed while a build was going was queued before anything
    /// looked at it, and then dispatched with no check at all.
    ///
    /// The reason it cannot be done while something runs is [`Session::refuse`]'s block. A refusal is a
    /// *closed* block, and pushing one on top of a running block makes the closed one last — so
    /// [`Session::running`] says nothing is going, and the sentinel that arrives for the real command
    /// no longer matches the last block's id and never closes it.
    fn dispatch_or_refuse(&mut self, before: Option<String>, command: String) {
        match needs_a_terminal(&command) {
            Some(why) => self.refuse(&command, why),
            None => self.dispatch_with(before, command),
        }
    }

    /// A command that was not run, and why, as a block of its own.
    ///
    /// A block rather than a notice over the window: it belongs in the log next to the command it is
    /// about, it can be scrolled back to, and it is the same shape as every other answer the panel
    /// gives. Closed on arrival, because there is nothing to wait for.
    fn refuse(&mut self, command: &str, why: String) {
        let id = self.next_id;
        self.next_id += 1;
        if self.blocks.len() >= BLOCKS {
            self.blocks.remove(0);
        }
        self.blocks.push(Block {
            command: command.to_owned(),
            id,
            code: Some(1),
            lines: vec![Line { text: why, err: true }],
            ..Block::default()
        });
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
        // The closing statement goes down the pipe in the same write as the command, so nothing can be
        // interleaved between them by a second `send` — and, wherever it can, on the same *line*, so
        // the pipe is empty for the command to read from. See [`Kind::needs_its_own_line`].
        let own_line = self.kind.needs_its_own_line(&command);
        let mut text = before.unwrap_or_default();
        text.push_str(&command);
        if own_line {
            text.push('\n');
        } else {
            text.push_str(self.kind.joiner(&command));
        }
        text.push_str(&self.kind.closing(id));
        text.push('\n');
        self.blocks.push(Block {
            command,
            id,
            takes_input: !own_line,
            ..Block::default()
        });
        self.write(&text);
    }

    /// Lines this program wrote that the shell is going to read back to us, oldest first.
    ///
    /// **Only `cmd`, and only because it cannot be told not to.** A `cmd` reading commands from a
    /// pipe writes each line it reads back down stdout, and `@echo off` does not stop it — that
    /// setting is about the prompt, and the read-back happens because stdin is not a console. So
    /// every block came out with the command repeated under its own header — with the literal text of
    /// the closing statement on the end of it, since the two now share a line — which reads as though
    /// the shell had printed the protocol.
    ///
    /// Since this program is the one that wrote those lines it knows exactly what they will be, so
    /// they are matched and dropped rather than guessed at by shape. A line the *command* prints that
    /// happens to be identical is dropped too, which is the one thing this can get wrong, and it
    /// costs one duplicate line in `cmd` alone.
    ///
    /// It matches on the line as written, before `cmd` expands anything: delayed expansion happens when
    /// the line is *run*, and `cmd` reads it back first. Asserted per shell by
    /// `a_real_shell_reports_its_own_status_and_folder`, which fails if any of the protocol reaches the
    /// log.
    fn expect_echo(&mut self, text: &str) {
        if self.kind != Kind::Cmd {
            return;
        }
        self.echoes
            .extend(text.lines().map(|line| line.trim_end().to_owned()));
    }

    fn write(&mut self, text: &str) {
        self.expect_echo(text);
        self.write_raw(text);
    }

    /// Down the pipe without registering an echo.
    ///
    /// For [`Session::feed`], whose text is read by the *command* rather than by the shell — so `cmd`
    /// never reads it back, and registering it would arm [`Session::expect_echo`] to drop the next
    /// output line that happened to match what was typed.
    fn write_raw(&mut self, text: &str) {
        let failed = match self.stdin.as_mut() {
            Some(stdin) => stdin.write_all(text.as_bytes()).and_then(|()| stdin.flush()).is_err(),
            None => true,
        };
        if failed {
            self.gone = true;
        }
    }

    /// Type at the command that is running.
    ///
    /// **Not a command.** The text goes down the same pipe, but no block is opened and no closing
    /// statement follows it: it is standard input for whatever is reading it, which is how `python -i`
    /// gets a line to evaluate and how a `[y/N]` gets answered.
    ///
    /// Echoed into the block on the way past, because a pipe is not a terminal — nothing gives back
    /// what was typed, so without this the answers are invisible and the log reads as though the
    /// program talked to itself.
    ///
    /// **There is no way to send an end of file.** Closing the pipe is what `Ctrl+D` means and this
    /// pipe is the session's, so closing it would take the shell with it; a `cat` with nothing to end
    /// it needs [`Session::stop`]. A REPL wants its own word — `exit()`, `quit`, `\q`.
    /// **The prompt that has just been answered is thrown away.** A prompt is written without a line
    /// ending behind it — that is what makes it a prompt — so `>>> ` sits in the standard error partial
    /// line waiting for a newline that is never coming. The next thing on stderr was a traceback, which
    /// then arrived as `>>> >>> Traceback (most recent call last):` with every prompt since answered
    /// stuck to the front of it. Answering a prompt consumes it, which is what a terminal's own echo
    /// would have made obvious, and dropping it here is what makes the log read as a transcript.
    pub fn feed(&mut self, text: &str) {
        if self.gone || !self.takes_input() {
            return;
        }
        self.partial[ERR] = String::new();
        self.redrawing[ERR] = false;
        if let Some(block) = self.blocks.last_mut() {
            block.push(text.to_owned(), false);
        }
        self.write_raw(&format!("{text}\n"));
    }

    /// Whether what is running now can be typed at. False when nothing is running.
    pub fn takes_input(&self) -> bool {
        self.blocks
            .last()
            .is_some_and(|block| block.running() && block.takes_input)
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
                self.dispatch_or_refuse(None, next);
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
                //
                // Unless it has grown past [`LINE_CAP`], in which case it is ended here as though a
                // newline had arrived — the flag going with it, exactly as it would have.
                Piece::Part(part) => {
                    self.partial[slot].push_str(part);
                    if self.partial[slot].len() >= LINE_CAP {
                        let line = std::mem::take(&mut self.partial[slot]);
                        let over = std::mem::replace(&mut self.redrawing[slot], false);
                        self.line(line, err, over);
                    }
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
        // newline on it. Split into the lines it should have been and handled as such.
        //
        // **Iteratively, and that is a fix rather than a preference.** This used to hand the front of
        // the line back to itself, which is one stack frame per marker in it — and [`split_sentinel`]
        // takes the *last* marker, so a line carrying it a thousand times recursed a thousand deep on
        // the UI thread. With nothing capping how long a line could get (see [`LINE_CAP`], which now
        // does) that was a stack overflow, and a stack overflow is not a panic anybody can catch.
        //
        // The cuts come out right to left, which is the order they are found in, and are then walked
        // left to right, which is the order the lines were printed in. The echo check above is over
        // the whole line and is not repeated per piece: an echo is a line `cmd` read back to us and it
        // arrives with its own ending, so it is never one of these pieces.
        if !err {
            let mut cuts: Vec<usize> = Vec::new();
            let mut head = line.as_str();
            while let Some((printed, _)) = split_sentinel(head) {
                cuts.push(printed.len());
                head = printed;
            }
            if !cuts.is_empty() {
                let mut from = 0;
                for to in cuts.iter().rev().copied().chain(std::iter::once(line.len())) {
                    // Only the first of them can overwrite the block's last line. The rest are lines
                    // that follow it, and a `\r` does not reach past the line it was on.
                    self.one(line[from..to].to_owned(), false, over && from == 0);
                    from = to;
                }
                return;
            }
        }
        self.one(line, err, over);
    }

    /// One line with nothing stuck to the end of it: the sentinel handled if that is what it is, and
    /// otherwise the line put in the open block.
    ///
    /// Split out from [`Session::line`] so that the splitting up there can be a loop over the pieces
    /// rather than a call back into itself — see the note on it.
    fn one(&mut self, line: String, err: bool, over: bool) {
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

// ---------------------------------------------------------------------------
// What a pipe cannot carry
// ---------------------------------------------------------------------------

/// Programs that draw over the whole screen, and so can never work on a pipe.
///
/// Not "programs that are interactive" — a great many of those are fine here now that a command can
/// be typed at. These are the ones that put the terminal in raw mode, switch to the alternate screen
/// and address the cursor: there is no partial version of that on a pipe, and no amount of forwarding
/// keystrokes makes one.
const FULL_SCREEN: [&str; 15] = [
    "vim", "vi", "nvim", "nano", "pico", "emacs", "htop", "btop", "top", "less", "more", "man",
    "fzf", "tig", "lazygit",
];

/// Why a command cannot work down a pipe, if it cannot — as a sentence for the panel to show.
///
/// **A short list on purpose.** The failure this exists to prevent is the silent one: a full-screen
/// program on a pipe prints nothing at all and waits for ever, and before this the only way out was
/// Stop. Saying so costs nothing and names the way to run it.
///
/// What is deliberately *not* guessed at is anything whose behaviour depends on configuration this
/// program does not read. `git commit` and `git rebase -i` open whatever `core.editor` is, which may
/// as easily be `code --wait` and work perfectly — so refusing them would break a working command to
/// warn about a broken one. They are left to hang, and Stop, as before.
pub fn needs_a_terminal(command: &str) -> Option<String> {
    let mut words = command.split_whitespace();
    let name = bare_name(words.next()?);
    let rest: Vec<&str> = words.collect();
    let flagged = |flags: [&str; 2]| rest.iter().any(|arg| flags.contains(arg));

    if FULL_SCREEN.contains(&name.as_str()) {
        return Some(format!(
            "`{name}` draws over the whole screen, which a pipe cannot carry. \
             Ctrl+Enter runs it in a terminal of its own."
        ));
    }
    match name.as_str() {
        "claude" if !flagged(["-p", "--print"]) => Some(
            "`claude` is a full-screen program. `claude -p \"…\"` prints one answer and works here; \
             Ctrl+Enter opens the interactive one in a terminal."
                .to_owned(),
        ),
        // Bare, these read their standard input as a *script* rather than prompting — so the panel
        // would show nothing, and anything typed would be swallowed as more source. `-i` is the flag
        // that makes each of them a REPL that works here, which is worth saying rather than leaving
        // somebody to find out.
        "python" | "python3" | "py" | "node" if rest.is_empty() => Some(format!(
            "bare `{name}` reads its standard input as a script and never prompts. \
             `{name} -i` is a REPL you can type at here."
        )),
        _ => None,
    }
}

/// The bare name of a program: no folder, no `.exe`, lower case.
fn bare_name(word: &str) -> String {
    word.rsplit(['/', '\\'])
        .next()
        .unwrap_or(word)
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .to_ascii_lowercase()
}

/// Run a command in a terminal of its own, for the things a pipe cannot carry.
///
/// **Outside this program's session and outside its job object.** Nothing about it is the panel's: it
/// does not appear as a block, Stop does not reach it, and closing the window it came from leaves it
/// running. Which is the point — it is a real terminal, and the reason to go there is that this panel
/// is not one.
///
/// Windows Terminal when it is installed, because that is the terminal somebody who wants one has,
/// and a console of this program's own making otherwise. No new dependency either way: the whole cost
/// of this is picking the right arguments.
pub fn in_terminal(kind: Kind, dir: &Path, command: &str) -> Result<(), String> {
    let (program, args) =
        interactive(kind, command).ok_or_else(|| format!("{} is not installed", kind.label()))?;

    // **A `;` is Windows Terminal's own argument separator**, for splitting a window into panes — so a
    // command with one in it would be cut in half at it and half of it run somewhere unexpected.
    // Those go the other way rather than being launched wrong.
    if !command.contains(';') && !crate::fs::is_synthetic(dir) {
        if let Some(wt) = which("wt.exe") {
            let mut launch = Command::new(wt);
            launch.arg("-d").arg(dir).arg(&program).args(&args);
            if launch.spawn().is_ok() {
                return Ok(());
            }
        }
    }
    // `CREATE_NEW_CONSOLE` rather than merely leaving `CREATE_NO_WINDOW` off: this process is a
    // `windows` subsystem binary with no console of its own, and asking for one outright is clearer
    // than relying on what an absent flag falls back to.
    let mut launch = Command::new(&program);
    launch.args(&args);
    if !crate::fs::is_synthetic(dir) {
        launch.current_dir(dir);
    }
    crate::shell::new_console(&mut launch);
    launch
        .spawn()
        .map(|_| ())
        .map_err(|why| format!("Cannot start {}: {why}", kind.label()))
}

/// A shell told to run one command interactively.
///
/// **The window closes when the command ends**, which is right for what comes through here: `claude`,
/// `vim` and `htop` are quit deliberately, and a window left over afterwards would need a second
/// gesture to be rid of. It also keeps the command out of a `;`-joined argument, which is what lets
/// Windows Terminal be used at all.
fn interactive(kind: Kind, command: &str) -> Option<(PathBuf, Vec<String>)> {
    // The same flags [`program`] starts the panel's own shell with, plus the one that takes a command
    // and the `-i` that a program looking for a terminal wants to find.
    let (path, flags): (PathBuf, Vec<&str>) = match kind {
        Kind::Bash => (bash()?, vec!["--login", "-i", "-c"]),
        Kind::PowerShell => (
            which("pwsh.exe").or_else(|| which("powershell.exe"))?,
            vec!["-NoLogo", "-Command"],
        ),
        Kind::Cmd => (PathBuf::from("cmd.exe"), vec!["/C"]),
    };
    let mut args: Vec<String> = flags.into_iter().map(str::to_owned).collect();
    args.push(command.to_owned());
    Some((path, args))
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
    if !crate::fs::is_synthetic(dir) {
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

/// Bytes off a pipe as text, and the one place two encodings meet.
///
/// **A console pipe carries UTF-8 and the OEM code page at the same time.** The shell writes UTF-8;
/// `net`, `ipconfig`, `sc`, `tasklist` and every other Win32 console tool write the code page
/// [`GetOEMCP`](oem) reports, because that is what console output *is* and redirecting the handle to
/// a pipe does not change it. So on a French machine `net use` sends `m\x82moris\x82es` down the same
/// pipe that `echo é` sent `\xc3\xa9` down, and nothing in either says which it is.
///
/// Valid UTF-8 wins, and what is not UTF-8 goes to [`oem::decode`] **one broken sequence at a time**,
/// with UTF-8 tried again immediately after. Handing the code page every byte from `0x80` up instead
/// is the shortcut that looks equivalent and is not: a stray `\x82` from `net` would swallow the
/// perfectly good `\xc3\xa9` printed behind it and turn one `é` into two other letters.
///
/// # Why `valid_up_to` on its own was a hang
///
/// This was three lines and a `continue`, on the reasoning that a sequence which does not parse is a
/// sequence still arriving. `\x82` is not: it is a *continuation* byte, so it can never begin one.
/// `valid_up_to` was 0 for ever, every read appended to a buffer nothing would drain, and **nothing
/// was forwarded again** — the sentinel included. So the block stayed open, the panel span for ever
/// about a command that had finished in a second, and every later command in that session was dead
/// behind it. Only stderr kept working, because it is a second thread with its own buffer.
///
/// The discriminator is [`std::str::Utf8Error::error_len`], which the old code did not consult:
/// `None` is a sequence truncated by the read boundary and waits, `Some` is a byte that will never be
/// valid and has to be consumed.
#[derive(Default)]
struct Decoder {
    /// Bytes held back until the rest of their character arrives: the start of a UTF-8 sequence split
    /// by a read boundary, or the lead byte of a double-byte OEM character split by the same.
    tail: Vec<u8>,
}

impl Decoder {
    /// One read's worth of bytes, holding back only a character the boundary cut in half.
    fn feed(&mut self, bytes: &[u8]) -> String {
        self.tail.extend_from_slice(bytes);
        let mut out = String::with_capacity(self.tail.len());
        while !self.tail.is_empty() {
            // `None` for the length means there is nothing invalid: either all of it parsed, or what
            // did not is a sequence still on its way.
            let (good, bad) = match std::str::from_utf8(&self.tail) {
                Ok(_) => (self.tail.len(), None),
                Err(why) => (why.valid_up_to(), why.error_len()),
            };
            if good > 0 {
                // Valid by construction: `valid_up_to` is exactly where it stopped being.
                if let Ok(text) = std::str::from_utf8(&self.tail[..good]) {
                    out.push_str(text);
                }
                self.tail.drain(..good);
            }
            let Some(bad) = bad else {
                break;
            };
            // Not UTF-8 at all, so the code page. The whole of the broken sequence goes, since those
            // bytes are a code page character and its neighbours rather than a prefix of anything.
            let run = if oem::double_byte(self.tail[0]) { 2 } else { bad };
            if run > self.tail.len() {
                // A lead byte whose trail byte is in the next read.
                break;
            }
            out.push_str(&oem::decode(&self.tail[..run]));
            self.tail.drain(..run);
        }
        out
    }

    /// Whatever is held back when the pipe closes.
    ///
    /// The rest of it is never coming, so the code page's reading of those bytes is the better guess:
    /// a sequence that looked like truncated UTF-8 at the very end of a stream which also carried
    /// code page bytes most likely never was UTF-8. Either way it is a character the panel shows
    /// rather than one it silently drops.
    fn flush(&mut self) -> String {
        oem::decode(&std::mem::take(&mut self.tail))
    }
}

/// One thread per pipe, forwarding decoded text.
///
/// The thread does as little as possible: read bytes, [`Decoder::feed`], send. It deliberately does
/// not know about lines, escape sequences or the protocol — all of that is [`Session::absorb`], on the
/// UI thread, where it can be tested without a process.
fn pipe(mut source: impl Read + Send + 'static, to_ui: Sender<Chunk>, err: bool, ctx: egui::Context) {
    let _ = std::thread::Builder::new()
        .name(format!("console-{}", if err { "err" } else { "out" }))
        .spawn(move || {
            let wrap = |text| if err { Chunk::Err(text) } else { Chunk::Out(text) };
            let mut buffer = [0u8; CHUNK];
            let mut decoder = Decoder::default();
            loop {
                let read = match source.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                // Empty only when the read was a character's first bytes and nothing else.
                let text = decoder.feed(&buffer[..read]);
                if text.is_empty() {
                    continue;
                }
                if to_ui.send(wrap(text)).is_err() {
                    return;
                }
                // Nothing else would wake a window that paints on demand.
                ctx.request_repaint();
            }
            let left = decoder.flush();
            if !left.is_empty() {
                let _ = to_ui.send(wrap(left));
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
        // `/Q` is echo off, without which every command appears twice. `/V:ON` turns on delayed
        // expansion, which is what makes `!ERRORLEVEL!` in [`Kind::closing`] mean anything — without
        // it every block reports the status of the command before it. See the comment there.
        Kind::Cmd => Some((PathBuf::from("cmd.exe"), vec!["/V:ON", "/Q", "/K"])),
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

/// The other encoding on the pipe. See [`Decoder`] for which bytes reach it and why.
#[cfg(windows)]
#[path = "../windows/oem.rs"]
mod oem;

#[cfg(not(windows))]
mod oem {
    /// Nowhere but Windows has a second encoding on a pipe, so there is nothing to decode with: a
    /// byte that is not UTF-8 here is a byte with no code page behind it.
    pub fn decode(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// No code page, so no lead bytes.
    pub fn double_byte(_first: u8) -> bool {
        false
    }
}

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
