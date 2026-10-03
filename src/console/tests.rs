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
        let line = kind.closing(42);
        assert!(line.contains("[AZUR:42:"), "{kind:?}: {line}");
        assert!(
            !line.ends_with('\n'),
            "{kind:?} brought its own ending — `dispatch_with` owns that, because whether this goes \
             on the command's line is what decides if the command can be typed at"
        );
        if kind != Kind::Cmd {
            assert!(
                line.starts_with("__azur=$?") || line.starts_with("$__ok=$?"),
                "{kind:?} reads the status after running something else: {line}"
            );
        }
    }
    // **`cmd` has to read its status late.** `%ERRORLEVEL%` is expanded when the line is parsed, and
    // the line is now the command's own — so the naming spelling would report the status of the
    // command before this one. Measured: `cmd /c exit 3 & echo %ERRORLEVEL%` prints `0`.
    let cmd = Kind::Cmd.closing(42);
    assert!(cmd.contains("!ERRORLEVEL!"), "{cmd}");
    assert!(
        program(Kind::Cmd).is_some_and(|(_, args)| args.contains(&"/V:ON")),
        "`!ERRORLEVEL!` means nothing without delayed expansion turned on"
    );
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
        // **The protocol is never shown.** Worth asserting per shell rather than trusting it: the
        // closing statement now rides on the command's own line, so `cmd` — which reads every line it
        // is given back down stdout — is echoing a longer line than it used to, and
        // [`Session::expect_echo`] has to still recognise it.
        assert!(
            !block.lines.iter().any(|line| line.text.contains("AZUR:")),
            "{kind:?} left the protocol in the log: {:?}",
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
fn finish(session: &mut Session) -> bool {
    until(session, |session| !session.running())
}

/// Pump until something is true of the session. False on timeout.
///
/// A real `bash --login` takes a few hundred milliseconds to become useful, so the wait is
/// generous — and it is a wait for the thing being waited on rather than a fixed sleep, for the
/// same reason `Harness::settle` is.
fn until(session: &mut Session, done: impl Fn(&Session) -> bool) -> bool {
    for _ in 0..600 {
        session.poll();
        if done(session) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// **A REPL, against a real shell** — which is the whole point of the closing statement moving.
///
/// `python -i` and not bare `python`: bare, it reads its standard input as a *script*, prompts for
/// nothing and would swallow anything typed as more source. `-i` is what makes it a REPL, and
/// [`needs_a_terminal`] says so rather than leaving anybody to find that out.
///
/// Skipped where bash or python is not installed, as the other live tests are.
#[test]
fn a_real_repl_answers_what_is_typed_at_it() {
    let ctx = egui::Context::default();
    let here = std::env::current_dir().expect("a working directory");
    let Ok(mut session) = Session::start(Kind::Bash, &here, &ctx) else {
        return;
    };
    session.send(None, "python --version");
    if !finish(&mut session) || session.blocks.last().is_some_and(Block::failed) {
        return;
    }

    session.send(None, "python -i");
    assert!(
        until(&mut session, |session| session.takes_input()),
        "the REPL never came up"
    );
    // And it stays open: a REPL runs until it is told to stop, so nothing closes this block but the
    // word that ends the program.
    session.feed("print(6 * 7)");
    assert!(
        until(&mut session, |session| shown(session)
            .iter()
            .any(|line| line.trim() == "42")),
        "the REPL did not answer: {:?}",
        shown(&session)
    );
    assert!(
        shown(&session).iter().any(|line| line.contains("6 * 7")),
        "what was typed is not in the log: {:?}",
        shown(&session)
    );

    // `exit()`, because there is no way to send an end of file down a pipe the session owns.
    session.feed("exit()");
    assert!(finish(&mut session), "the REPL never ended");
}

/// A command that ends without a newline still closes its block.
///
/// The reported shape, found by driving the real thing: `printf 'a'` prints one character and no
/// line ending, so the shell writes the sentinel onto the same line. Before this, the block sat
/// there saying "running" about a command that had already finished.
#[test]
fn output_with_no_newline_on_it_does_not_swallow_the_sentinel() {
    let mut session = offline();
    session.dispatch_with(None, "printf a".to_owned());
    let id = session.blocks[0].id;
    session.absorb(&format!("a[AZUR:{id}:0:D:/x]\n"), false);
    let block = session.blocks.last().expect("a block");
    assert_eq!(block.code, Some(0), "it closed");
    assert_eq!(shown(&session), vec!["a".to_owned()], "and kept the output");
}

/// A program that never ends a line has its line ended for it, and the sentinel still closes the block.
///
/// `cat` of a file with no line endings in it, or one long JSON document: nothing capped how much a
/// partial line could hold, so it grew by every read for as long as the command ran and the panel became
/// the reason this process ran out of memory. The half that must survive the fix is the close — the
/// sentinel arrives stuck to the end of whatever line was open, so a cap that *dropped* the excess would
/// eventually drop the sentinel and leave a block saying "running" for ever.
#[test]
fn a_line_that_never_ends_is_broken_rather_than_held_for_ever() {
    let mut session = offline();
    session.dispatch_with(None, "cat one-long-line.json".to_owned());
    let id = session.blocks[0].id;
    for _ in 0..3 {
        session.absorb(&"x".repeat(CHUNK), false);
    }
    let lines = shown(&session);
    assert_eq!(lines.len(), 3, "the partial line was never broken");
    assert!(
        lines.iter().all(|line| line.len() == LINE_CAP),
        "broken somewhere other than the cap: {:?}",
        lines.iter().map(String::len).collect::<Vec<_>>()
    );

    // And the tail of it, with the sentinel behind it on the same line.
    session.absorb(&format!("tail[AZUR:{id}:0:D:/x]\n"), false);
    assert_eq!(
        session.blocks.last().expect("a block").code,
        Some(0),
        "the block never closed"
    );
    assert_eq!(shown(&session).last().map(String::as_str), Some("tail"));
}

/// A line carrying the marker thousands of times is thousands of lines, not thousands of stack frames.
///
/// [`split_sentinel`] finds the *last* marker in a line, so splitting the front off and handing it back
/// to [`Session::line`] recursed once per marker — on the UI thread, with nothing bounding how long a
/// line could be. That is a stack overflow, which is not a panic anybody can catch and takes the window
/// with it. The count here is well past what a 1 MB stack holds at a frame apiece.
#[test]
fn a_line_full_of_markers_does_not_recurse() {
    let mut session = offline();
    session.dispatch_with(None, "printf x".to_owned());
    let id = session.blocks[0].id;
    let mut line = "printed".to_owned();
    for _ in 0..20_000 {
        line.push_str(&format!("[AZUR:{id}:0:D:/x]"));
    }
    session.line(line, false, false);
    assert_eq!(shown(&session), vec!["printed".to_owned()]);
    assert_eq!(
        session.blocks.last().expect("a block").code,
        Some(0),
        "the sentinels went unread"
    );
}

/// The OEM code page on the same pipe as UTF-8, which is every `net use` on a French machine.
///
/// **The regression is the hang, not the accents.** `\x82` is a continuation byte, so it can never
/// begin a UTF-8 sequence: `valid_up_to` stayed 0, the byte sat at the front of the buffer for ever
/// and nothing behind it was ever forwarded — the sentinel included — so the block span for the rest
/// of the session. Which is why the assertions here are mostly about bytes simply getting *through*.
///
/// The bytes are the real ones, off `net use | od -tx1`.
#[test]
fn the_oem_code_page_does_not_wedge_the_decoder() {
    let mut decoder = Decoder::default();
    let text = decoder.feed(b"Les connexions seront m\x82moris\x82es.\r\n");
    assert!(text.ends_with(".\r\n"), "the rest of the line never arrived: {text:?}");

    // The one that actually hung: the sentinel is written *behind* that output, and before this it
    // never reached the UI at all.
    assert_eq!(decoder.feed(b"[AZUR:1:0:D:/x]\n"), "[AZUR:1:0:D:/x]\n");

    // And the accents are the accents — asked of the machine's own code page rather than asserted as
    // a spelling that is only right in western Europe. 850 and 437 both make `\x82` an `é`; 932 makes
    // it the lead byte of something else entirely, and there the question does not apply.
    #[cfg(windows)]
    if !oem::double_byte(0x82) {
        let e = oem::decode(b"\x82");
        // Checked separately, or this test passes on a decoder that answers `U+FFFD` to everything:
        // the expectation is built from the same function it is testing the wiring of.
        assert!(!e.contains('\u{fffd}'), "the code page did not decode its own byte");
        assert_eq!(text.trim_end(), format!("Les connexions seront m{e}moris{e}es."));
    }
}

/// A UTF-8 character split by a read boundary is still one character.
///
/// The half the old code got right, and the half a fix that merely skipped invalid bytes would break:
/// `\xc3` alone is not wrong, it is unfinished, and two reads must not make it two replacement
/// characters. `error_len` is what tells the two cases apart.
#[test]
fn half_a_character_waits_for_its_other_half() {
    let mut decoder = Decoder::default();
    assert_eq!(decoder.feed(b"caf\xc3"), "caf", "half a character is not a character");
    assert_eq!(decoder.feed(b"\xa9\n"), "é\n", "and the other half completes it");
}

/// One code page byte does not take the good bytes with it.
///
/// The shortcut is to hand the code page every byte from `0x80` up in one run. `bash` writes UTF-8
/// into the same pipe, so a `\x82` from `net` with a `\xc3\xa9` from `echo` behind it would take both,
/// and an `é` that arrived perfectly well comes out as two other letters.
#[test]
fn one_bad_byte_does_not_swallow_the_utf8_behind_it() {
    let mut decoder = Decoder::default();
    let text = decoder.feed(b"\x82\xc3\xa9\n");
    // On a double-byte code page `\x82` really does claim the byte after it, and that is correct
    // there; the claim being tested is that a single-byte one stops at itself.
    #[cfg(windows)]
    if oem::double_byte(0x82) {
        return;
    }
    assert!(text.ends_with("é\n"), "the UTF-8 was read as the code page: {text:?}");
}

/// A real command whose output is not UTF-8 closes its block.
///
/// The end-to-end version of the three above, driving the real shell. `net use` is what it was found
/// with, but that answer depends on the machine's language for its accents and on the network for its
/// speed — so the byte is printed directly instead. `\x82` is the one `net` sent, and `printf` sends
/// exactly it on any machine in any locale. Skipped where the shell is not installed, as the other
/// live tests are.
#[test]
fn a_command_that_prints_the_code_page_still_closes_its_block() {
    let ctx = egui::Context::default();
    let here = std::env::current_dir().expect("a working directory");
    let Ok(mut session) = Session::start(Kind::Bash, &here, &ctx) else {
        return;
    };
    session.send(None, r"printf 'm\x82moris\x82es\n'");
    assert!(finish(&mut session), "the block never closed — the decoder is wedged again");
    assert!(
        shown(&session).iter().any(|line| line.contains("moris")),
        "it closed but the line was eaten: {:?}",
        shown(&session)
    );

    // And the session is still usable afterwards, which is the part that made this more than one
    // stuck command: the reader thread never recovered, so everything behind it was dead too.
    session.send(None, "echo after");
    assert!(finish(&mut session), "the next command never closed");
    assert!(
        shown(&session).iter().any(|line| line.trim() == "after"),
        "{:?}",
        shown(&session)
    );
}

/// The closing statement rides on the command's own line, so the pipe is the command's to read.
///
/// The measured failure this is about: a shell reading commands from a pipe reads exactly one line at
/// a time, so a closing statement on the *next* line is sitting in the pipe when the command starts
/// and the command reads it. `head -1` printed `__a=$?; printf …` as its own output, the closing
/// statement never reached the shell, and the block never closed.
#[test]
fn the_closing_statement_rides_on_the_commands_line() {
    for kind in Kind::ALL {
        let mut session = offline();
        session.kind = kind;
        session.dispatch_with(None, "head -1".to_owned());
        let written = std::mem::take(&mut session.echoes);
        // `echoes` is only armed for `cmd`, so the written text is checked through the block instead
        // for the other two — what matters here is the flag the block carries.
        assert!(
            session.blocks[0].takes_input,
            "{kind:?}: a plain command must be typeable at"
        );
        if kind == Kind::Cmd {
            assert_eq!(
                written.len(),
                1,
                "{kind:?} wrote {written:?} — the closing statement is on a second line"
            );
            assert!(written[0].starts_with("head -1"), "{written:?}");
        }
    }
}

/// And goes back to its own line where the command's text would swallow it.
///
/// Each of these would otherwise take the closing statement with it — a comment runs to the end of the
/// line, and `&&` would make closing the block conditional on the command succeeding, so a *failing*
/// command would hang for ever. Those give up being typeable at, which is exactly the old behaviour,
/// and keep the thing that must never fail.
#[test]
fn a_command_that_would_swallow_the_closing_statement_does_not_get_the_chance() {
    for (kind, command) in [
        (Kind::Bash, "ls # have a look"),
        (Kind::Bash, "cargo build &&"),
        (Kind::Bash, "echo one |"),
        (Kind::Bash, r"echo long \"),
        (Kind::PowerShell, "Get-ChildItem # here"),
        (Kind::Cmd, "rem nothing to do"),
        (Kind::Cmd, "dir ^"),
    ] {
        assert!(
            kind.needs_its_own_line(command),
            "{kind:?} would have let `{command}` eat the closing statement"
        );
        let mut session = offline();
        session.kind = kind;
        session.dispatch_with(None, command.to_owned());
        assert!(
            !session.blocks[0].takes_input,
            "{kind:?}: `{command}` cannot be typed at, and must not claim to be"
        );
    }
    // A `&` on its own is a separator rather than a continuation: the command goes to the background
    // and the closing statement runs straight after it, on the same line, with no `;` in between.
    assert!(!Kind::Bash.needs_its_own_line("sleep 5 &"));
    assert_eq!(Kind::Bash.joiner("sleep 5 &"), " ", "a second separator is a syntax error");

    // **The `rem` test used to be `text[..3]`, and byte 3 is not always a character boundary.** `ab°`
    // is four bytes with the third of them inside the `°`, so typing it with `cmd` selected panicked
    // in the middle of deciding how to send it. Every one of these is a command somebody can type.
    for command in ["ab°", "é", "°°°", "剖", "cd ..", ""] {
        assert!(
            !Kind::Cmd.needs_its_own_line(command),
            "{command:?} is not a comment"
        );
    }
    // And it still means what it meant for anything ASCII, in either case.
    assert!(Kind::Cmd.needs_its_own_line("REM off"));
    assert!(Kind::Cmd.needs_its_own_line("rem"));
}

/// Typing at a running command, which is the whole point of the line above.
#[test]
fn a_line_typed_at_a_running_command_goes_down_the_pipe_and_into_the_log() {
    let mut session = offline();
    session.dispatch_with(None, "python -i".to_owned());
    // `offline` has no pipe behind it, so writing the command down it marked the shell gone. That flag
    // is about the pipe; what is being checked here is the protocol above it.
    session.gone = false;
    assert!(session.takes_input(), "a running one-line command takes input");

    session.feed("print(6*7)");
    assert_eq!(
        shown(&session),
        vec!["print(6*7)".to_owned()],
        "a pipe echoes nothing, so the panel has to show what was typed"
    );

    // **The prompt just answered is gone rather than glued to the next thing.** A `>>> ` has no line
    // ending, so it waits in the standard error partial — and the next stderr line was a traceback,
    // which arrived wearing every prompt since answered: `>>> >>> Traceback (most recent call last):`.
    session.absorb(">>> ", true);
    session.gone = false; // again: writing down a pipe that is not there marks the shell gone.
    session.feed("1/0");
    session.absorb("Traceback (most recent call last):\n", true);
    assert_eq!(
        shown(&session),
        vec![
            "print(6*7)".to_owned(),
            "1/0".to_owned(),
            "Traceback (most recent call last):".to_owned()
        ],
        "the answered prompt is stuck to the traceback"
    );

    // Not once it has closed: that pipe belongs to the shell again, and a line written to it then
    // would be run as a command nobody typed.
    let id = session.blocks[0].id;
    session.absorb(&format!("[AZUR:{id}:0:D:/x]\n"), false);
    assert!(!session.takes_input(), "the command has finished");
    let before = shown(&session);
    session.gone = false;
    session.feed("print(1)");
    assert_eq!(shown(&session), before, "nothing was added");
}

/// A program a pipe cannot carry is refused, and told where it *can* be run.
///
/// The failure being replaced is the silent one: `claude` on a pipe printed nothing at all and waited
/// for ever, so the only way out was Stop. Every message names the way to run the thing.
#[test]
fn a_program_that_needs_a_terminal_is_told_so_rather_than_left_hanging() {
    for command in ["vim", "htop", "less log.txt", r"C:\tools\nvim.exe x", "claude", "python"] {
        let why = needs_a_terminal(command)
            .unwrap_or_else(|| panic!("`{command}` would have hung with nothing on screen"));
        assert!(
            why.contains("Ctrl+Enter") || why.contains("-i"),
            "`{command}` was refused without saying what to do instead: {why}"
        );
    }
    // The forms that do work here are not refused — `claude -p` prints one answer down the pipe, and
    // `python -i` is a REPL the line above can be typed at.
    for command in ["claude -p \"what is this repo\"", "python -i", "python script.py", "git status"] {
        assert_eq!(needs_a_terminal(command), None, "`{command}` works here and was refused");
    }

    // And the refusal is a block in the log, closed, next to the command it is about.
    let mut session = offline();
    session.send(None, "vim");
    assert_eq!(session.blocks.len(), 1);
    assert!(!session.blocks[0].running(), "there is nothing to wait for");
    assert!(session.blocks[0].lines.iter().all(|line| line.err), "it is a complaint");
    assert!(session.echoes.is_empty(), "nothing was written to the shell");
}

/// One typed while something else is running is refused too, when its turn comes.
///
/// **The queue was the hole.** `send` checks for a running command and queues before anything looks at
/// what was typed, so a `vim` typed during a build reached `dispatch` with no check at all and hung
/// exactly as before. And it cannot be refused any *earlier* than its turn: a refusal is a closed
/// block, and one pushed on top of the running block would make the closed one last — so nothing would
/// look running any more, and the sentinel for the real command would no longer match the last block.
#[test]
fn a_queued_command_that_needs_a_terminal_is_refused_when_its_turn_comes() {
    let mut session = offline();
    session.kind = Kind::Bash;
    session.dispatch_with(None, "sleep 5".to_owned());
    session.gone = false;
    assert!(session.running());

    session.send(None, "htop");
    assert_eq!(session.blocks.len(), 1, "nothing may be pushed over a running block");
    assert!(session.running(), "the running block is still the last one");

    // The build finishes, and the queued command gets its answer rather than hanging.
    let id = session.blocks[0].id;
    session.absorb(&format!("[AZUR:{id}:0:D:/x]\n"), false);
    session.poll();
    assert_eq!(session.blocks.len(), 2);
    let refused = session.blocks.last().expect("a block");
    assert_eq!(refused.command, "htop");
    assert!(!refused.running(), "it was dispatched instead of being answered");
    assert!(refused.lines.iter().any(|line| line.text.contains("Ctrl+Enter")));
}

/// The shell that a hand-off starts, per shell, with the command as one argument.
#[test]
fn a_hand_off_passes_the_command_as_one_argument() {
    for kind in Kind::ALL {
        let Some((program, args)) = interactive(kind, "claude --resume") else {
            continue;
        };
        assert!(
            program.file_name().is_some_and(|name| !name.is_empty()),
            "{kind:?}: {program:?}"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("claude --resume"),
            "{kind:?} split the command up: {args:?}"
        );
        let takes = match kind {
            Kind::Bash => "-c",
            Kind::PowerShell => "-Command",
            Kind::Cmd => "/C",
        };
        assert_eq!(args.get(args.len() - 2).map(String::as_str), Some(takes), "{args:?}");
        // `-i`, or a program looking for a terminal will not find one even in a terminal.
        if kind == Kind::Bash {
            assert!(args.contains(&"-i".to_owned()), "{args:?}");
        }
    }
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
