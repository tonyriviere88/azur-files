use super::*;

/// The protocol's own line, read back.
///
/// The path is the part worth pinning: a Windows path has a colon in it, so the split has to stop
/// after two fields and keep the rest whole. Splitting on every colon put `D` in the code field
/// and every command reported success.
#[test]
fn a_sentinel_gives_up_its_three_fields() {
    assert_eq!(
        sentinel("[AZUR:7:0:D:/Sources/MyTools]"),
        Some((7, 0, "D:/Sources/MyTools".to_owned()))
    );
    assert_eq!(
        sentinel("[AZUR:12:101:C:\\Program Files\\Git]"),
        Some((12, 101, r"C:\Program Files\Git".to_owned()))
    );
    // Trailing whitespace is what `cmd`'s `echo` adds.
    assert_eq!(sentinel("[AZUR:1:0:D:\\x]  \n"), Some((1, 0, r"D:\x".to_owned())));

    // Not sentinels.
    assert_eq!(sentinel("cargo test"), None);
    assert_eq!(sentinel("[AZUR:x:0:D:\\x]"), None);
    assert_eq!(sentinel("[AZUR:1:0]"), None);
    assert_eq!(sentinel("see [AZUR:1:0:D:\\x] in the log"), None);
    // A marker with text only *before* it is the protocol arriving on the end of a line that had
    // no newline — `printf 'a'` and then the sentinel. Split, rather than lost.
    assert_eq!(
        split_sentinel("a[AZUR:2:0:D:/src]").map(|(text, found)| (text, found.0)),
        Some(("a", 2))
    );
    // And a marker with text after it is still a program talking about the protocol.
    assert_eq!(split_sentinel("see [AZUR:1:0:D:\\x] in the log"), None);
    assert_eq!(split_sentinel("[AZUR:1:0:D:/x]"), None, "already its own line");
}

/// A carriage return redraws the line rather than starting one.
///
/// This is `cargo build` and `npm install`, which redraw a progress line several times a second.
/// Appending instead of overwriting turns one bar into several hundred lines of near-identical
/// text, which is the difference between a readable build log and an unusable one.
#[test]
fn a_progress_bar_overwrites_its_own_line() {
    let mut session = offline();
    session.blocks.push(Block::default());
    session.absorb("Building [==>   ] 2/9\rBuilding [====> ] 5/9\r", false);
    assert_eq!(shown(&session), vec!["Building [====> ] 5/9"]);

    // And a newline afterwards commits it, so the next line is a new one.
    session.absorb("Building [======] 9/9\nFinished\n", false);
    assert_eq!(shown(&session), vec!["Building [======] 9/9", "Finished"]);
}

/// `\r\n` is one line ending, not a redraw followed by a line.
#[test]
fn windows_line_endings_are_one_ending() {
    let mut session = offline();
    session.blocks.push(Block::default());
    session.absorb("one\r\ntwo\r\n", false);
    assert_eq!(shown(&session), vec!["one", "two"]);
}

/// A line split across two reads is still one line.
///
/// Which is not a hypothetical: a pipe hands over 8 KiB at a time and a build log's lines do not
/// line up with that.
#[test]
fn a_line_split_across_reads_is_rejoined() {
    let mut session = offline();
    session.blocks.push(Block::default());
    session.absorb("error[E0", false);
    assert!(shown(&session).is_empty(), "an unfinished line is not a line yet");
    session.absorb("308]: mismatched types\n", false);
    assert_eq!(shown(&session), vec!["error[E0308]: mismatched types"]);
}

/// The two pipes are assembled separately, and stderr keeps its own identity.
///
/// One place this beats a terminal, which interleaves them into a stream you cannot separate
/// again. It also has to be right per-pipe: a half-line on stdout must not be completed by a
/// newline that arrived on stderr.
#[test]
fn the_two_pipes_do_not_run_into_each_other() {
    let mut session = offline();
    session.blocks.push(Block::default());
    session.absorb("compiling", false);
    session.absorb("warning: unused\n", true);
    session.absorb(" azur\n", false);

    let block = &session.blocks[0];
    assert_eq!(
        block.lines,
        vec![
            Line { text: "warning: unused".to_owned(), err: true },
            Line { text: "compiling azur".to_owned(), err: false },
        ]
    );
}

/// A sentinel closes its own block and is never shown.
#[test]
fn a_sentinel_closes_its_block_and_only_its_block() {
    let mut session = offline();
    session.blocks.push(Block { id: 4, ..Block::default() });
    session.absorb("done\n[AZUR:4:2:D:/x]\n", false);

    assert_eq!(shown(&session), vec!["done"], "the protocol line was shown");
    assert_eq!(session.blocks[0].code, Some(2));
    assert!(session.blocks[0].failed());
    assert_eq!(session.cwd(), Some(Path::new(r"D:\x")), "the folder came with it");
    assert!(!session.running());
    assert!(
        !session.blocks[0].collapsed,
        "a block that printed something must stay open"
    );

    // A command that succeeded silently folds itself; one that failed silently does not, because
    // that is the one worth looking at.
    session.blocks.push(Block { id: 5, ..Block::default() });
    session.absorb("[AZUR:5:0:D:/x]\n", false);
    assert!(session.blocks[1].collapsed, "a silent `cd` left a blank body behind");
    session.blocks.push(Block { id: 6, ..Block::default() });
    session.absorb("[AZUR:6:1:D:/x]\n", false);
    assert!(!session.blocks[2].collapsed, "a silent failure folded itself away");

    // A sentinel for a command that was stopped arrives after its block has gone, and must not
    // close whatever is open now.
    session.blocks.push(Block { id: 9, ..Block::default() });
    session.absorb("[AZUR:4:0:D:/x]\n", false);
    assert!(
        session.blocks.last().is_some_and(Block::running),
        "a stale sentinel closed the wrong block"
    );
}

/// Escape sequences are dropped, and the text around them survives intact.
#[test]
fn colour_a_program_printed_anyway_is_stripped() {
    assert_eq!(strip_ansi("\x1b[32mok\x1b[0m"), "ok");
    assert_eq!(strip_ansi("\x1b[1;31merror\x1b[0m: bad"), "error: bad");
    // An OSC, ended either way.
    assert_eq!(strip_ansi("\x1b]0;title\x07after"), "after");
    assert_eq!(strip_ansi("\x1b]0;title\x1b\\after"), "after");
    // Nothing to do, and the fast path out.
    assert_eq!(strip_ansi("plain text"), "plain text");
    assert_eq!(strip_ansi("100% [====]"), "100% [====]");
    // A byte-order mark, which Windows PowerShell 5.1 can put in front of its output. It is not
    // whitespace, so nothing else would remove it, and a sentinel wearing one stops parsing.
    assert_eq!(strip_ansi("\u{feff}[AZUR:1:0:D:/x]"), "[AZUR:1:0:D:/x]");
    assert_eq!(
        sentinel(&strip_ansi("\u{feff}[AZUR:1:0:D:/x]")),
        Some((1, 0, "D:/x".to_owned()))
    );
}

/// The cap holds, and the block says what it dropped.
#[test]
fn a_runaway_command_is_bounded_and_says_so() {
    let mut block = Block::default();
    for line in 0..LINES + 250 {
        block.push(format!("line {line}"), false);
    }
    assert_eq!(block.lines.len(), LINES);
    assert_eq!(block.dropped, 250);
    assert_eq!(block.lines[0].text, "line 250", "the oldest went, not the newest");
}

/// Every spelling of a folder a shell can report.
#[test]
fn a_reported_folder_is_understood_in_every_spelling() {
    for (text, want) in [
        (r"D:\Sources", Some(r"D:\Sources")),
        ("D:/Sources", Some(r"D:\Sources")),
        ("/d/Sources", Some(r"D:\Sources")),
        ("/mnt/d/Sources", Some(r"D:\Sources")),
        ("/d", Some(r"D:\")),
        (r"\\server\share", Some(r"\\server\share")),
        // Inside the shell's own root: real, and not derivable from here.
        ("/usr/bin", None),
        ("/tmp", None),
        ("", None),
    ] {
        assert_eq!(windows_path(text).as_deref(), want.map(Path::new), "{text:?}");
    }
}

/// Each shell's closing line refers to the command that just ran, not to itself.
///
/// The trap is real and silent: `printf '...' "$?" "$(pwd)"` reports the status of the `pwd`
/// unless the status is captured into a variable first, so every command comes back successful.
#[test]
fn every_shell_captures_the_status_before_it_asks_anything_else() {
    for kind in Kind::ALL {
        let line = kind.sentinel(42);
        assert!(line.contains("[AZUR:42:"), "{kind:?}: {line}");
        assert!(line.ends_with('\n'), "{kind:?} must be one complete line");
        if kind != Kind::Cmd {
            assert!(
                line.starts_with("__azur=$?") || line.starts_with("$__ok=$?"),
                "{kind:?} reads the status after running something else: {line}"
            );
        }
    }
    assert_eq!(Kind::Bash.next(), Kind::PowerShell);
    assert_eq!(Kind::Cmd.next(), Kind::Bash, "Shift+Tab wraps");
}

/// **The protocol, against a real shell.**
///
/// Everything above this is the assembler, checked without a process. This is the part that is
/// either right or not: whether a live shell on a pipe, given a command and a sentinel, comes
/// back with the command's own exit status and the folder it is now in.
///
/// Read-only commands only — `echo`, `pwd`, `cd` inside the repository. Nothing here writes,
/// moves or deletes anything, which is the standing rule for a test in this suite that reaches a
/// real shell at all.
///
/// Skipped rather than failed where the shell is not installed, since that is a fact about the
/// machine and not about this code.
#[test]
fn a_real_shell_reports_its_own_status_and_folder() {
    // **`Cmd` is in the list**, and it was not — which is exactly why `cmd` shipped unable to
    // close a single block: it writes a prompt with no newline after it, so the sentinel
    // continued that line and stopped being a line beginning with the marker. The one shell this
    // loop did not cover was the one that did not work.
    for kind in Kind::ALL {
        let ctx = egui::Context::default();
        let here = std::env::current_dir().expect("a working directory");
        let Ok(mut session) = Session::start(kind, &here, &ctx) else {
            continue;
        };

        // A command that succeeds and prints one line.
        session.send(None, "echo hello");
        assert!(finish(&mut session), "{kind:?} never closed the block");
        let block = session.blocks.last().expect("a block");
        assert_eq!(block.code, Some(0), "{kind:?}: {:?}", block.lines);
        assert!(
            block.lines.iter().any(|line| line.text.trim() == "hello" && !line.err),
            "{kind:?} lost the output: {:?}",
            block.lines
        );
        assert_eq!(
            session.cwd(),
            Some(here.as_path()),
            "{kind:?} reported the wrong folder"
        );

        // A command that fails. The status has to be the command's own — the trap this whole
        // arrangement exists to avoid is the sentinel reporting the status of its own `pwd`.
        let fails = match kind {
            Kind::Bash => "ls /definitely-not-here-a4f1c9",
            _ => "cmd /c exit 3",
        };
        session.send(None, fails);
        assert!(finish(&mut session), "{kind:?} never closed the failing block");
        let block = session.blocks.last().expect("a block");
        assert!(
            block.failed(),
            "{kind:?} reported success for a command that failed: {:?}",
            block.code
        );

        // And `cd` is still true afterwards, which is the whole reason the shell is kept alive.
        let up = here.parent().expect("a parent").to_path_buf();
        session.send(None, "cd ..");
        assert!(finish(&mut session), "{kind:?} never closed the `cd`");
        assert_eq!(session.cwd(), Some(up.as_path()), "{kind:?} did not follow the `cd`");
    }
}

/// Standard error arrives, and arrives marked as such.
#[test]
fn a_real_shell_keeps_stderr_apart_from_stdout() {
    let ctx = egui::Context::default();
    let here = std::env::current_dir().expect("a working directory");
    let Ok(mut session) = Session::start(Kind::Bash, &here, &ctx) else {
        return;
    };
    session.send(None, "echo out; echo oops 1>&2");
    assert!(finish(&mut session), "the block never closed");

    let block = session.blocks.last().expect("a block");
    assert!(
        block.lines.iter().any(|l| l.text.trim() == "out" && !l.err),
        "{:?}",
        block.lines
    );
    assert!(
        block.lines.iter().any(|l| l.text.trim() == "oops" && l.err),
        "stderr was not marked, or did not arrive: {:?}",
        block.lines
    );
}

/// Pump until the open block closes. False on timeout.
///
/// A real `bash --login` takes a few hundred milliseconds to become useful, so the wait is
/// generous — and it is a wait for the thing being waited on rather than a fixed sleep, for the
/// same reason `Harness::settle` is.
fn finish(session: &mut Session) -> bool {
    for _ in 0..600 {
        session.poll();
        if !session.running() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// A command that ends without a newline still closes its block.
///
/// The reported shape, found by driving the real thing: `printf 'a'` prints one character and no
/// line ending, so the shell writes the sentinel onto the same line. Before this, the block sat
/// there saying "running" about a command that had already finished.
#[test]
fn output_with_no_newline_on_it_does_not_swallow_the_sentinel() {
    let mut session = offline();
    session.dispatch("printf a".to_owned());
    let id = session.blocks[0].id;
    session.absorb(&format!("a[AZUR:{id}:0:D:/x]\n"), false);
    let block = session.blocks.last().expect("a block");
    assert_eq!(block.code, Some(0), "it closed");
    assert_eq!(shown(&session), vec!["a".to_owned()], "and kept the output");
}

/// A session with no process behind it, for the assembler tests.
///
/// The pieces being checked — line assembly, the sentinel, the cap — are all on the UI thread's
/// side of the channel, so none of them need a shell. The ones that do are further down.
fn offline() -> Session {
    let (_, from) = std::sync::mpsc::channel();
    Session {
        kind: Kind::Bash,
        child: None,
        stdin: None,
        from,
        job: Job::holding_nothing(),
        gone: false,
        blocks: Vec::new(),
        cwd: None,
        home: PathBuf::from(r"D:\"),
        next_id: 1,
        partial: [String::new(), String::new()],
        redrawing: [false, false],
        queued: VecDeque::new(),
        echoes: VecDeque::new(),
    }
}

fn shown(session: &Session) -> Vec<String> {
    session
        .blocks
        .last()
        .map(|block| block.lines.iter().map(|line| line.text.clone()).collect())
        .unwrap_or_default()
}

impl Job {
    /// A job holding nothing, so [`offline`] has something to put in the field.
    fn holding_nothing() -> Self {
        #[cfg(windows)]
        {
            Self(0)
        }
        #[cfg(not(windows))]
        {
            Self()
        }
    }
}
