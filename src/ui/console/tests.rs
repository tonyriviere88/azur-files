use super::*;

/// A block with an id and some output, without a shell.
fn block(id: u64, command: &str, lines: &[&str]) -> Block {
    Block {
        command: command.to_owned(),
        lines: lines
            .iter()
            .map(|text| crate::console::Line {
                text: (*text).to_owned(),
                err: false,
            })
            .collect(),
        code: Some(0),
        dropped: 0,
        collapsed: false,
        id,
    }
}

fn press(key: Key, m: Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: m,
    }
}

/// Push a frame's worth of events through, and say what came out.
///
/// The modifiers are taken from the last key in the batch, which is what a real frame's state
/// amounts to for a single keystroke.
fn feed(state: &mut State, blocks: &mut Vec<Block>, events: Vec<egui::Event>) -> Outcome {
    let mut out = Outcome::default();
    let mut events = events;
    state.rebuild(blocks, state.cols());
    let held = events
        .iter()
        .rev()
        .find_map(|event| match event {
            egui::Event::Key { modifiers, .. } => Some(*modifiers),
            _ => None,
        })
        .unwrap_or(Modifiers::NONE);
    state.keys(&mut events, held, blocks, &mut out);
    out
}

const SHIFT: Modifiers = Modifiers::SHIFT;
const CTRL: Modifiers = Modifiers::COMMAND;

// -- the line ----------------------------------------------------------

#[test]
fn typing_goes_in_and_enter_sends_it() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    let out = feed(
        &mut state,
        &mut blocks,
        vec![
            egui::Event::Text("git ".to_owned()),
            egui::Event::Text("status".to_owned()),
            press(Key::Enter, Modifiers::NONE),
        ],
    );
    assert_eq!(out.send.as_deref(), Some("git status"));
    // And the line is empty again, ready for the next one.
    assert_eq!(state.line.text(), "");
}

#[test]
fn an_empty_line_is_not_a_command() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    let out = feed(
        &mut state,
        &mut blocks,
        vec![
            egui::Event::Text("   ".to_owned()),
            press(Key::Enter, Modifiers::NONE),
        ],
    );
    assert_eq!(out.send, None);
    assert!(state.history.is_empty());
}

#[test]
fn the_caret_walks_words_and_characters() {
    let mut editor = Editor::default();
    editor.insert("git commit --amend");
    editor.left(false, true);
    assert_eq!(editor.caret, "git commit ".len(), "one word back");
    editor.left(false, true);
    assert_eq!(editor.caret, "git ".len(), "and another");
    editor.right(false, false);
    assert_eq!(editor.caret, "gitc".len() + 1, "one character forward");
}

#[test]
fn a_selection_is_replaced_by_what_is_typed() {
    let mut editor = Editor::default();
    editor.insert("cargo build");
    editor.home(false);
    editor.right(true, true);
    assert_eq!(editor.selected(), "cargo ");
    editor.insert("rustc ");
    assert_eq!(editor.text(), "rustc build");
}

#[test]
fn a_caret_at_a_selection_collapses_to_its_edge() {
    let mut editor = Editor::default();
    editor.insert("abcdef");
    editor.all();
    editor.left(false, false);
    assert_eq!(editor.caret, 0, "to the near end, not one back from it");
    editor.all();
    editor.right(false, false);
    assert_eq!(editor.caret, 6, "and the far one going the other way");
}

/// Byte offsets and character columns are not the same number, and a caret that confuses them
/// panics on the next edit rather than merely looking wrong.
#[test]
fn the_line_survives_characters_wider_than_a_byte() {
    let mut editor = Editor::default();
    editor.insert("écho café");
    assert_eq!(editor.col(), 9, "nine characters, eleven bytes");
    editor.backspace();
    assert_eq!(editor.text(), "écho caf");
    editor.home(false);
    editor.right(false, false);
    assert_eq!(editor.caret, 2, "past a two-byte character");
    editor.delete();
    assert_eq!(editor.text(), "ého caf");
}

// -- the history -------------------------------------------------------

#[test]
fn up_and_down_walk_the_history_and_come_back() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    for command in ["one", "two"] {
        feed(
            &mut state,
            &mut blocks,
            vec![
                egui::Event::Text(command.to_owned()),
                press(Key::Enter, Modifiers::NONE),
            ],
        );
    }
    feed(&mut state, &mut blocks, vec![egui::Event::Text("half".to_owned())]);
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    assert_eq!(state.line.text(), "two");
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    assert_eq!(state.line.text(), "one");
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    assert_eq!(state.line.text(), "one", "and it stops at the oldest");
    feed(&mut state, &mut blocks, vec![press(Key::ArrowDown, Modifiers::NONE)]);
    assert_eq!(state.line.text(), "two");
    feed(&mut state, &mut blocks, vec![press(Key::ArrowDown, Modifiers::NONE)]);
    assert_eq!(state.line.text(), "half", "the half-typed line comes back");
}

#[test]
fn the_same_command_twice_is_one_entry() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    for _ in 0..3 {
        feed(
            &mut state,
            &mut blocks,
            vec![
                egui::Event::Text("make".to_owned()),
                press(Key::Enter, Modifiers::NONE),
            ],
        );
    }
    assert_eq!(state.history, vec!["make".to_owned()]);
}

// -- the shell ---------------------------------------------------------

/// The one that has been reported broken three times. It is a plain `Shift+Tab` press with no
/// exact-modifier match and no `consume_key` anywhere near it.
#[test]
fn shift_tab_walks_the_shells_round() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    assert_eq!(state.kind(), Kind::Bash);
    for expected in [Kind::PowerShell, Kind::Cmd, Kind::Bash] {
        let out = feed(&mut state, &mut blocks, vec![press(Key::Tab, SHIFT)]);
        assert_eq!(state.kind(), expected);
        assert_eq!(out.swap, Some(expected), "and the caller is told to swap");
    }
}

/// Both halves of the reason the focus filter exists: a `Tab` of either kind is *ours*, so
/// neither can reach egui's focus navigation and take the keyboard out of the panel.
#[test]
fn no_tab_of_any_kind_escapes_the_panel() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    let mut events = vec![press(Key::Tab, Modifiers::NONE), press(Key::Tab, SHIFT)];
    let mut out = Outcome::default();
    state.keys(&mut events, Modifiers::NONE, &mut blocks, &mut out);
    assert!(events.is_empty(), "both were consumed");
}

// -- the blocks --------------------------------------------------------

#[test]
fn shift_up_picks_blocks_from_the_newest_back() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "one", &["a"]), block(2, "two", &["b"])];
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
    assert_eq!(state.aim, Aim::Block(2), "the newest first");
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
    assert_eq!(state.aim, Aim::Block(1));
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
    assert_eq!(state.aim, Aim::Block(1), "and stops at the oldest");
    feed(&mut state, &mut blocks, vec![press(Key::ArrowDown, SHIFT)]);
    assert_eq!(state.aim, Aim::Block(2));
    feed(&mut state, &mut blocks, vec![press(Key::ArrowDown, SHIFT)]);
    assert_eq!(state.aim, Aim::Prompt, "walking off the end is the way out");
}

#[test]
fn a_picked_block_folds_with_left_and_right() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "ls", &["a", "b"])];
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
    feed(&mut state, &mut blocks, vec![press(Key::ArrowLeft, SHIFT)]);
    assert!(blocks[0].collapsed, "shift+left folds it");
    feed(&mut state, &mut blocks, vec![press(Key::ArrowRight, SHIFT)]);
    assert!(!blocks[0].collapsed, "shift+right opens it");
    // And the plain arrows too, because with a block picked there is no caret to move.
    feed(&mut state, &mut blocks, vec![press(Key::ArrowLeft, Modifiers::NONE)]);
    assert!(blocks[0].collapsed);
}

/// The arrows have to be about the caret again the moment the prompt is.
#[test]
fn the_arrows_go_back_to_the_caret_when_typing_resumes() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "ls", &["a"])];
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
    assert_eq!(state.aim, Aim::Block(1));
    feed(&mut state, &mut blocks, vec![egui::Event::Text("x".to_owned())]);
    assert_eq!(state.aim, Aim::Prompt, "typing takes the aim back");
    feed(&mut state, &mut blocks, vec![press(Key::ArrowLeft, Modifiers::NONE)]);
    assert!(!blocks[0].collapsed, "and the arrow moved the caret");
    assert_eq!(state.line.caret, 0);
}

#[test]
fn del_forgets_a_picked_block_but_never_a_running_one() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "one", &["a"]), block(2, "two", &["b"])];
    blocks[1].code = None; // still going
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
    assert_eq!(state.aim, Aim::Block(2));
    feed(&mut state, &mut blocks, vec![press(Key::Delete, Modifiers::NONE)]);
    assert_eq!(blocks.len(), 2, "a running block's output has to land somewhere");

    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
    assert_eq!(state.aim, Aim::Block(1));
    feed(&mut state, &mut blocks, vec![press(Key::Delete, Modifiers::NONE)]);
    assert_eq!(blocks.len(), 1);
    assert_eq!(state.aim, Aim::Block(2), "and the selection lands on the next");
}

// -- copying -----------------------------------------------------------

#[test]
fn ctrl_c_copies_the_picked_block_whole() {
    let mut state = State::default();
    let mut blocks = vec![block(7, "ls -l", &["one", "two"])];
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
    let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
    assert_eq!(out.copy.as_deref(), Some("> ls -l\r\none\r\ntwo"));
}

#[test]
fn ctrl_c_copies_a_dragged_span_across_rows() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "ls", &["alpha", "beta"])];
    state.rebuild(&blocks, None);
    // Row 0 is the header, 1 and 2 the output. From "ph" in alpha to "be" in beta.
    state.aim = Aim::Text(Span::new(
        Spot { row: 1, col: 2 },
        Spot { row: 2, col: 2 },
    ));
    let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
    assert_eq!(out.copy.as_deref(), Some("pha\r\nbe"));
}

/// The blank row between two blocks is a row, so a drag across it copies the blank line it looks
/// like — what you see is what you get, air included.
#[test]
fn a_span_across_two_blocks_keeps_the_line_between_them() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "one", &["a"]), block(2, "two", &["b"])];
    state.rebuild(&blocks, None);
    // 0 header, 1 output, 2 the blank row, 3 the next header, 4 its output.
    state.aim = Aim::Text(Span::new(
        Spot { row: 1, col: 0 },
        Spot { row: 4, col: 1 },
    ));
    let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
    assert_eq!(out.copy.as_deref(), Some("a\r\n\r\ntwo\r\nb"));
}

#[test]
fn ctrl_c_copies_the_prompts_own_selection() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    feed(
        &mut state,
        &mut blocks,
        vec![egui::Event::Text("cargo build".to_owned())],
    );
    feed(&mut state, &mut blocks, vec![press(Key::A, CTRL)]);
    let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
    assert_eq!(out.copy.as_deref(), Some("cargo build"));
}

/// With nothing selected anywhere it is the Ctrl+C somebody meant.
#[test]
fn ctrl_c_with_nothing_selected_is_a_stop() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
    assert!(out.stop);
    assert_eq!(out.copy, None);
}

/// egui-winit may send `Event::Copy` as well as the keystroke, and a cut that ran twice would
/// take the text out and then take out whatever was next to it.
#[test]
fn a_copy_that_arrives_twice_only_happens_once() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    feed(&mut state, &mut blocks, vec![egui::Event::Text("abcdef".to_owned())]);
    feed(&mut state, &mut blocks, vec![press(Key::A, CTRL)]);
    let out = feed(
        &mut state,
        &mut blocks,
        vec![egui::Event::Cut, press(Key::X, CTRL)],
    );
    assert_eq!(out.copy.as_deref(), Some("abcdef"));
    assert_eq!(state.line.text(), "", "cut once, not twice");
}

// -- selecting by word -------------------------------------------------

/// The reported case: in `source.cpp`, `cpp` has to be selectable on its own.
#[test]
fn a_dot_is_its_own_run_so_either_side_of_it_is_a_word() {
    let chars: Vec<char> = "source.cpp".chars().collect();
    assert_eq!(word_at(&chars, 0), 0..6, "source");
    assert_eq!(word_at(&chars, 5), 0..6);
    assert_eq!(word_at(&chars, 6), 6..7, "the dot alone");
    assert_eq!(word_at(&chars, 7), 7..10, "cpp");
    assert_eq!(word_at(&chars, 9), 7..10);
}

#[test]
fn runs_of_space_and_punctuation_are_words_too() {
    let chars: Vec<char> = " M  src/ui/console.rs".chars().collect();
    assert_eq!(word_at(&chars, 0), 0..1, "the leading space");
    assert_eq!(word_at(&chars, 1), 1..2, "M");
    assert_eq!(word_at(&chars, 2), 2..4, "both spaces, as one run");
    assert_eq!(word_at(&chars, 4), 4..7, "src");
    assert_eq!(word_at(&chars, 7), 7..8, "the slash");
}

#[test]
fn a_word_at_the_end_of_a_line_still_ends_at_the_line() {
    let chars: Vec<char> = "abc".chars().collect();
    assert_eq!(word_at(&chars, 3), 0..3, "a column past the end clamps back in");
    assert_eq!(word_at(&chars, 99), 0..3);
    assert_eq!(word_at(&[], 0), 0..0, "and an empty line has no word");
}

// -- one character, one column -----------------------------------------

#[test]
fn a_tab_becomes_spaces_up_to_the_next_stop() {
    assert_eq!(columns("\tnew file:"), "        new file:");
    assert_eq!(columns("ab\tc"), "ab      c", "from column two to column eight");
    assert_eq!(
        columns("12345678\tx"),
        "12345678        x",
        "a tab on a stop still moves a whole one"
    );
    assert_eq!(columns("plain text"), "plain text", "and nothing else is touched");
}

/// The reported case, to the character.
///
/// A `git status` line begins with a tab, so every character after it sat four columns along
/// while the arithmetic counted one — and a selection that *looked* right came out three
/// characters along, being the tab's other three columns.
#[test]
fn a_selection_on_a_tabbed_line_copies_what_was_highlighted() {
    let line = "\tnew file:   ../Plugin/Src/View/LgsxDetailsWidget.h";
    let mut state = State::default();
    let mut blocks = vec![block(1, "git status", &[line])];
    state.rebuild(&blocks, None);

    // Row 1 is the output. Find `LgsxDetailsWidget.h` by the column it is *drawn* at, which is
    // what the pointer would have picked.
    let shown = super::shown(&blocks, Row::Text(0, 0, 0), None).into_owned();
    let at = shown.find("LgsxDetailsWidget.h").expect("the name is on the row");
    let from = shown[..at].chars().count();
    let to = from + "LgsxDetailsWidget.h".chars().count();
    state.aim = Aim::Text(Span::new(
        Spot { row: 1, col: from },
        Spot { row: 1, col: to },
    ));

    let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
    assert_eq!(out.copy.as_deref(), Some("LgsxDetailsWidget.h"));
}

// -- clearing ----------------------------------------------------------

#[test]
fn clear_and_cls_are_the_panels_own_and_never_reach_a_shell() {
    for word in ["clear", "cls", "CLS"] {
        let mut state = State::default();
        let mut blocks = vec![block(1, "ls", &["a"])];
        let out = feed(
            &mut state,
            &mut blocks,
            vec![
                egui::Event::Text(word.to_owned()),
                press(Key::Enter, Modifiers::NONE),
            ],
        );
        assert!(out.clear, "{word} clears the log");
        assert_eq!(out.send, None, "{word} is not handed to the shell");
        // And it is still in the history, so `Up` finds it like anything else typed.
        assert_eq!(state.history, vec![word.to_owned()]);
    }
}

// -- wrapping ----------------------------------------------------------

#[test]
fn alt_z_turns_wrapping_on_and_off() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    state.width = 10;
    assert_eq!(state.cols(), Some(10), "on to begin with");
    feed(&mut state, &mut blocks, vec![press(Key::Z, Modifiers::ALT)]);
    assert_eq!(state.cols(), None);
    feed(&mut state, &mut blocks, vec![press(Key::Z, Modifiers::ALT)]);
    assert_eq!(state.cols(), Some(10));
}

#[test]
fn a_wrapped_line_is_as_many_rows_as_it_has_slices() {
    assert_eq!(slices("", 10), 1, "a blank line is still a line");
    assert_eq!(slices("0123456789", 10), 1, "exactly one row");
    assert_eq!(slices("0123456789a", 10), 2);
    assert_eq!(slices("\tab", 10), 1, "measured after the tab is expanded");
    assert_eq!(slices("\tabc", 10), 2, "eight columns of tab and three of text");
}

#[test]
fn wrapping_slices_a_line_across_rows_and_copies_it_back_whole() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "ls", &["abcdefghij"])];
    state.width = 4;
    state.wrap = true;
    state.rebuild(&blocks, state.cols());
    // The header is one row at four columns wide (`ls`), then the line in three.
    assert_eq!(state.rows(), 4);
    assert_eq!(state.row(&blocks, 0), Some(Row::Head(0, 0)));
    assert_eq!(state.row(&blocks, 1), Some(Row::Text(0, 0, 0)));
    assert_eq!(state.row(&blocks, 3), Some(Row::Text(0, 0, 2)));
    assert_eq!(shown(&blocks, Row::Text(0, 0, 1), state.cols()), "efgh");

    // **Copied back without the wrap in it.** Three rows on screen, one line on the clipboard —
    // a line ending dropped into the middle of a wrapped path is worse than not wrapping at all.
    state.aim = Aim::Text(Span::new(
        Spot { row: 1, col: 0 },
        Spot { row: 3, col: 2 },
    ));
    let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
    assert_eq!(out.copy.as_deref(), Some("abcdefghij"));
}

/// Turning it on and off again has to give the same log back.
#[test]
fn the_index_agrees_with_itself_whichever_way_it_is_built() {
    let mut state = State::default();
    let blocks = vec![block(1, "one", &["a", "b"]), block(2, "two", &["c"])];
    state.rebuild(&blocks, None);
    let flat: Vec<_> = (0..state.rows())
        .map(|at| state.row(&blocks, at))
        .collect();
    // Wide enough that nothing wraps, so the wrapped index must come out the same.
    state.width = 80;
    state.wrap = true;
    state.rebuild(&blocks, state.cols());
    let wrapped: Vec<_> = (0..state.rows())
        .map(|at| state.row(&blocks, at))
        .collect();
    assert_eq!(flat, wrapped);
}

// -- the row index -----------------------------------------------------

#[test]
fn the_rows_are_a_header_and_its_lines() {
    let mut state = State::default();
    let blocks = vec![block(1, "one", &["a", "b"]), block(2, "two", &["c"])];
    state.rebuild(&blocks, None);
    assert_eq!(state.rows(), 6);
    assert_eq!(state.row(&blocks, 0), Some(Row::Head(0, 0)), "no air at the top");
    assert_eq!(state.row(&blocks, 1), Some(Row::Text(0, 0, 0)));
    assert_eq!(state.row(&blocks, 2), Some(Row::Text(0, 1, 0)));
    assert_eq!(state.row(&blocks, 3), Some(Row::Gap(1)), "a line before the next");
    assert_eq!(state.row(&blocks, 4), Some(Row::Head(1, 0)));
    assert_eq!(state.row(&blocks, 5), Some(Row::Text(1, 0, 0)));
    assert_eq!(state.row(&blocks, 6), None);
}

/// A block starts at its blank row, so `Shift+Up` brings the air along and the header it lands on
/// is never jammed against the top edge.
#[test]
fn a_block_starts_at_the_blank_row_above_it() {
    let mut state = State::default();
    let blocks = vec![block(1, "one", &["a"]), block(2, "two", &["b"])];
    state.rebuild(&blocks, None);
    assert_eq!(state.starts, vec![0, 2, 5]);
}

#[test]
fn a_folded_block_is_one_row() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "one", &["a", "b"]), block(2, "two", &["c"])];
    blocks[0].collapsed = true;
    state.rebuild(&blocks, None);
    assert_eq!(state.rows(), 4);
    assert_eq!(state.row(&blocks, 0), Some(Row::Head(0, 0)));
    assert_eq!(state.row(&blocks, 1), Some(Row::Gap(1)), "straight to the next");
    assert_eq!(state.row(&blocks, 2), Some(Row::Head(1, 0)));
    assert_eq!(state.row(&blocks, 3), Some(Row::Text(1, 0, 0)));
}

#[test]
fn a_capped_block_says_so_on_a_row_of_its_own() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "find /", &["a"])];
    blocks[0].dropped = 4_000;
    state.rebuild(&blocks, None);
    assert_eq!(state.rows(), 3);
    assert_eq!(state.row(&blocks, 1), Some(Row::Cut(0)));
    assert_eq!(state.row(&blocks, 2), Some(Row::Text(0, 0, 0)));
}

/// The whole point of the index: the cost is the blocks, never their lines.
#[test]
fn the_index_does_not_grow_with_the_output() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "yes", &[])];
    blocks[0].lines = (0..50_000)
        .map(|n| crate::console::Line {
            text: n.to_string(),
            err: false,
        })
        .collect();
    state.rebuild(&blocks, None);
    assert_eq!(state.starts.len(), 2, "one entry per block, and the total");
    assert_eq!(state.rows(), 50_001);
    assert_eq!(state.row(&blocks, 50_000), Some(Row::Text(0, 49_999, 0)));
}

/// Wrapped, a frame in which nothing moved has to do nothing — the mode the panel now opens in
/// cannot be paying a pass over every line of the log to be told what it already knows.
///
/// Proved by leaving a mark in the index and finding it still there: had the second call rebuilt
/// anything at all, the first thing it does is clear that away.
#[test]
fn a_wrapped_index_is_left_alone_until_something_moves() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "ls", &["abcdefghij"])];
    state.width = 4;
    state.rebuild(&blocks, state.cols());
    let rows = state.rows();

    state.starts.push(usize::MAX);
    state.rebuild(&blocks, state.cols());
    assert_eq!(state.starts.last().copied(), Some(usize::MAX), "untouched");
    assert_eq!(state.rows(), rows);

    // And a line arriving is something moving.
    blocks[0].lines.push(crate::console::Line {
        text: "k".to_owned(),
        err: false,
    });
    state.rebuild(&blocks, state.cols());
    assert_eq!(state.rows(), rows + 1, "built again");
    assert_eq!(state.starts.last().copied(), Some(state.rows()));
}

// -- who has the keyboard ----------------------------------------------

/// The panel's own claim, egui's answer, and the frame in which egui has no answer at all.
#[test]
fn the_keys_are_the_panels_until_something_else_takes_them() {
    let ctx = egui::Context::default();
    let mut state = State::default();
    let pane: PaneId = 1;
    let mut checked = false;
    let _ = ctx.run_ui(Default::default(), |ui| {
        let ctx = ui.ctx();
        assert!(!state.keeps_keys(ctx, pane), "never claimed");

        state.take_keys();
        // Nothing focused: the frame a click *completes* in, where egui has revoked the panel's
        // focus and the panel has not asked for it back yet. Still the panel's — otherwise a key
        // pressed in that frame goes nowhere and the listing lights up for it.
        assert!(state.keeps_keys(ctx, pane), "claimed, and nothing else holds it");

        ctx.memory_mut(|m| m.request_focus(id(pane)));
        assert!(state.keeps_keys(ctx, pane), "claimed, and egui agrees");

        ctx.memory_mut(|m| m.request_focus(Id::new("a-field-somewhere")));
        assert!(
            !state.keeps_keys(ctx, pane),
            "something else holds it: a field, or the other pane's console"
        );

        state.drop_keys();
        ctx.memory_mut(|m| m.request_focus(id(pane)));
        assert!(
            !state.keeps_keys(ctx, pane),
            "given up, whatever egui is still saying"
        );
        checked = true;
    });
    assert!(checked, "the pass ran");
}

// -- the geometry ------------------------------------------------------

#[test]
fn the_console_takes_its_share_off_the_bottom() {
    let above = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
    let (list, panel) = split(above, true, 0.35);
    let panel = panel.expect("there is room in six hundred points");
    assert_eq!(panel.bottom(), above.bottom(), "it is the bottom of the pane");
    assert_eq!(list.bottom(), panel.top(), "and the seam is the panel's");
    assert!((panel.height() - SEAM - 600.0 * 0.35).abs() < 0.5);
}

#[test]
fn a_pane_too_short_keeps_its_listing_instead() {
    let above = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 120.0));
    let (list, panel) = split(above, true, 0.35);
    assert_eq!(panel, None);
    assert_eq!(list, above);
}

#[test]
fn a_shut_console_takes_nothing() {
    let above = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
    assert_eq!(split(above, false, 0.35), (above, None));
}

#[test]
fn escape_lets_go_of_one_thing_at_a_time() {
    let mut state = State::default();
    let mut blocks = vec![block(1, "ls", &["a"])];
    feed(&mut state, &mut blocks, vec![egui::Event::Text("half".to_owned())]);
    feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);

    feed(&mut state, &mut blocks, vec![press(Key::Escape, Modifiers::NONE)]);
    assert_eq!(state.aim, Aim::Prompt, "the block first");
    assert_eq!(state.line.text(), "half", "and the line is still there");

    feed(&mut state, &mut blocks, vec![press(Key::Escape, Modifiers::NONE)]);
    assert_eq!(state.line.text(), "", "then the line");

    // And with nothing left to let go of, it is the window's again.
    let mut events = vec![press(Key::Escape, Modifiers::NONE)];
    let mut out = Outcome::default();
    state.keys(&mut events, Modifiers::NONE, &mut blocks, &mut out);
    assert_eq!(events.len(), 1, "passed through");
}

#[test]
fn a_pasted_block_of_lines_stays_one_command() {
    let mut state = State::default();
    let mut blocks = Vec::new();
    let out = feed(
        &mut state,
        &mut blocks,
        vec![
            egui::Event::Paste("git add .\r\ngit commit\n".to_owned()),
            press(Key::Enter, Modifiers::NONE),
        ],
    );
    assert_eq!(out.send.as_deref(), Some("git add . git commit"));
}
