use super::header::what_fits;
use super::text::coloured;
use super::*;
use std::path::PathBuf;

/// The three hunks of the diff in [`crate::git`]'s own tests, against the file they describe.
///
/// `a b c d e f g h` was committed; the working tree now has `a B c NEW1 NEW2 d e h`.
fn changed() -> (String, crate::git::Changes) {
    let body = "a\nB\nc\nNEW1\nNEW2\nd\ne\nh\n".to_owned();
    let changes = crate::git::Changes {
        hunks: vec![
            crate::git::Hunk {
                added: 2,
                added_count: 1,
                after: 1,
                removed: vec!["b".to_owned()],
                removed_at: 2,
            },
            crate::git::Hunk {
                added: 4,
                added_count: 2,
                after: 3,
                removed: Vec::new(),
                removed_at: 3,
            },
            crate::git::Hunk {
                added: 0,
                added_count: 0,
                after: 7,
                removed: vec!["f".to_owned(), "g".to_owned()],
                removed_at: 6,
            },
        ],
    };
    (body, changes)
}

/// **The whole of the diff view's arithmetic**: every row, what it is, and which line it is.
///
/// A removed line goes back in front of whatever replaced it and keeps its number in `HEAD`; an
/// added line keeps the file's; and the file's own lines are untouched and in order. One row out
/// of step here is a gutter that is wrong for the rest of the file.
#[test]
fn a_diff_view_puts_the_removed_lines_back_where_they_were() {
    let (body, changes) = changed();
    let view = Diffed::build(&body, &changes, false, syntax::Lang::None);
    let rows: Vec<(&str, Mark, Option<u32>)> = view
        .body
        .split('\n')
        .zip(&view.lines)
        .map(|(text, line)| (text, line.mark, line.number))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("a", Mark::Same, Some(1)),
            ("b", Mark::Removed, Some(2)),
            ("B", Mark::Added, Some(2)),
            ("c", Mark::Same, Some(3)),
            ("NEW1", Mark::Added, Some(4)),
            ("NEW2", Mark::Added, Some(5)),
            ("d", Mark::Same, Some(6)),
            ("e", Mark::Same, Some(7)),
            ("f", Mark::Removed, Some(6)),
            ("g", Mark::Removed, Some(7)),
            ("h", Mark::Same, Some(8)),
            // The row a body ending in a newline has, which is why this is built by splitting on
            // `\n` rather than by `lines()`: the galley has it too, and the two have to agree.
            ("", Mark::Same, Some(9)),
        ]
    );
}

/// Collapsed, the same file keeps every change and [`CONTEXT`] lines around it, and says how much
/// it left out.
#[test]
fn collapsing_keeps_the_changes_and_counts_what_it_hides() {
    // Twenty lines, with one changed in the middle: `line 10` was replaced.
    let mut body = String::new();
    for at in 1..=20 {
        body.push_str(&format!("line {at}\n"));
    }
    let changes = crate::git::Changes {
        hunks: vec![crate::git::Hunk {
            added: 10,
            added_count: 1,
            after: 9,
            removed: vec!["old 10".to_owned()],
            removed_at: 10,
        }],
    };
    let view = Diffed::build(&body, &changes, true, syntax::Lang::None);
    let shown: Vec<(Mark, Option<u32>)> = view
        .lines
        .iter()
        .map(|line| (line.mark, line.number))
        .collect();
    assert_eq!(
        shown,
        vec![
            (Mark::Skipped(6), None),
            (Mark::Same, Some(7)),
            (Mark::Same, Some(8)),
            (Mark::Same, Some(9)),
            (Mark::Removed, Some(10)),
            (Mark::Added, Some(10)),
            (Mark::Same, Some(11)),
            (Mark::Same, Some(12)),
            (Mark::Same, Some(13)),
            (Mark::Skipped(8), None),
        ]
    );
    // The count is what is *not* shown, and the two of them plus the rows account for the file.
    let hidden: u32 = view
        .lines
        .iter()
        .filter_map(|line| match line.mark {
            Mark::Skipped(count) => Some(count),
            _ => None,
        })
        .sum();
    let kept = view.lines.len() as u32 - 2 - 1; // the two seams, and the removed line
    assert_eq!(hidden + kept, 21, "twenty lines and the empty last row");
}

/// **The find bar's offsets are offsets into the string on screen, and that is not the file.**
///
/// [`Text::shown`] is what the search runs over, and while a diff is up that is the diff's own body —
/// which is *longer* than the file wherever a hunk took a line away, because the removed lines are put
/// back into it. So a hit in the last part of a diffed file is a byte index past the end of the file, and
/// the canvas used to count the characters up to it in `text.body`: "byte index out of bounds" on a late
/// match, or "not a char boundary" as soon as anything above the hit was not ASCII. Preview any file in
/// this repository with a removed line in it, `Ctrl+F`, step to the end.
#[test]
fn a_hit_in_a_diff_is_an_offset_into_the_diff() {
    let (body, changes) = changed();
    let mut text = Text {
        body,
        truncated: false,
        code: true,
        lang: syntax::Lang::None,
        spans: Vec::new(),
        doc: None,
        changes: Some(changes),
        view: None,
        view_for: None,
    };
    assert!(text.follow(true, false), "no diff view to search over");
    let shown = text.shown(false);
    assert!(
        shown.len() > text.body.len(),
        "the fixture has no removed lines in it, so there is nothing to be out of step"
    );

    // `h` is the file's last line and the view's, with three put-back lines above it.
    let hit = preview::hits(shown, &searching("h", false, false, false))
        .at
        .pop()
        .expect("a hit");
    assert!(
        shown.get(..hit.start).is_some(),
        "the hit is not an offset into what was searched"
    );
    assert!(
        text.body.get(..hit.start).is_none(),
        "the hit still lands inside the file, so this fixture no longer pins the panic"
    );
}

/// **The build is a walk over the body and a walk over the hunks, not one of each per line.**
///
/// `git diff -U0` gives every isolated changed line a hunk of its own and runs over the *whole* file,
/// while the body it is matched against here is the first [`preview::TEXT_CAP`] of that file. A large
/// mostly-rewritten file is therefore tens of thousands of lines against tens of thousands of hunks, and
/// the nested walk this used to do was their product: 10^9 iterations on the UI thread, inside
/// [`Text::follow`], on the frame the diff toggle was pressed.
#[test]
fn a_diff_of_a_rewritten_file_is_not_the_product_of_its_two_lengths() {
    // Forty thousand lines, every one of them replaced — one hunk each, which is what `-U0` gives.
    const ROWS: u32 = 40_000;
    let mut body = String::new();
    let mut hunks = Vec::new();
    for at in 1..=ROWS {
        body.push_str(&format!("new {at}\n"));
        hunks.push(crate::git::Hunk {
            added: at,
            added_count: 1,
            after: at - 1,
            removed: vec![format!("old {at}")],
            removed_at: at,
        });
    }
    let changes = crate::git::Changes { hunks };

    let started = std::time::Instant::now();
    let view = Diffed::build(&body, &changes, false, syntax::Lang::None);
    let took = started.elapsed();

    // The output is the same output: every removed line in front of the line that replaced it, and the
    // row a body ending in a newline has.
    assert_eq!(view.lines.len() as u32, ROWS * 2 + 1);
    let head: Vec<(&str, Mark, Option<u32>)> = view
        .body
        .split('\n')
        .zip(&view.lines)
        .take(4)
        .map(|(text, line)| (text, line.mark, line.number))
        .collect();
    assert_eq!(
        head,
        vec![
            ("old 1", Mark::Removed, Some(1)),
            ("new 1", Mark::Added, Some(1)),
            ("old 2", Mark::Removed, Some(2)),
            ("new 2", Mark::Added, Some(2)),
        ]
    );
    assert_eq!(view.lines[view.lines.len() - 2].number, Some(ROWS));
    // Generous by a wide margin — the walk itself is milliseconds — and still an order of magnitude
    // under what the nested version took for this file.
    assert!(
        took < std::time::Duration::from_secs(5),
        "{ROWS} lines against {ROWS} hunks took {took:?}"
    );
}

/// A file with nothing changed in it has no diff view at all, whatever the toggles say — which is
/// what makes the default costless for most files.
#[test]
fn nothing_changed_means_nothing_to_show() {
    let mut text = Text {
        body: "a\nb\n".to_owned(),
        truncated: false,
        code: true,
        lang: syntax::Lang::None,
        spans: Vec::new(),
        doc: None,
        changes: None,
        view: None,
        view_for: None,
    };
    assert!(!text.follow(true, false), "nothing to build");
    assert!(text.view.is_none());
    assert_eq!(text.shown(false), "a\nb\n");

    // And with changes, the toggles decide — each move rebuilding once and once only.
    text.changes = Some(changed().1);
    assert!(text.follow(true, false));
    assert!(!text.follow(true, false), "already built for these");
    assert!(text.view.is_some());
    assert!(text.follow(true, true), "collapsing is a different body");
    assert!(text.follow(false, true), "and off is no body");
    assert!(text.view.is_none());
    assert_eq!(text.shown(false), "a\nb\n");
}

/// A probe: the metrics of the two faces the preview lays text out in.
#[test]
#[ignore = "a probe"]
fn probe_font_metrics() {
    let ctx = egui::Context::default();
    azur_egui_theme::fonts::install(&ctx);
    let t = Theme::dark();
    let _ = ctx.run_ui(Default::default(), |ui| {
        let p = ui.painter();
        for (name, font) in [
            ("mono", t.fonts.mono.clone()),
            ("body", t.fonts.body.clone()),
            ("caption", t.fonts.caption.clone()),
        ] {
            let probe = p.layout_no_wrap("Ay".to_owned(), font.clone(), Color32::WHITE);
            let rows = p.layout_no_wrap("Ay\nBg".to_owned(), font.clone(), Color32::WHITE);
            let baseline = azur_egui_theme::components::galley_baseline(&probe);
            let lift = azur_egui_theme::components::ink_lift(p, &font);
            let top = probe.rows[0]
                .glyphs
                .iter()
                .filter(|g| !g.uv_rect.is_nothing())
                .map(|g| g.pos.y + g.uv_rect.offset.y)
                .fold(f32::INFINITY, f32::min);
            let bottom = probe.rows[0]
                .glyphs
                .iter()
                .filter(|g| !g.uv_rect.is_nothing())
                .map(|g| g.pos.y + g.uv_rect.offset.y + g.uv_rect.size.y)
                .fold(f32::NEG_INFINITY, f32::max);
            println!(
                "{name:8} size {:.1}  row {:.2}  pitch {:.2}  baseline(ascent) {:.2}  ink {:.2}..{:.2}  ink_ascent {:.2}  lift {:+.2}",
                font.size,
                probe.rows[0].size.y,
                rows.rows[1].pos.y - rows.rows[0].pos.y,
                baseline,
                top,
                bottom,
                baseline - top,
                lift,
            );
        }
    });
}

/// A pane wider than it is tall gets the panel down the side; anything squarer gets it along
/// the bottom. Which is the whole of `Auto`, and it is the reason it exists: two panes side by
/// side are each half as wide, and a preview taking 40% of *that* leaves no listing at all.
#[test]
fn auto_puts_the_panel_where_the_pane_has_room_for_it() {
    let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(820.0, 540.0));
    let narrow = Rect::from_min_size(pos2(0.0, 0.0), vec2(410.0, 540.0));
    assert_eq!(Where::Auto.side(wide), Side::Right, "one pane in a window");
    assert_eq!(Where::Auto.side(narrow), Side::Bottom, "two side by side");
    // A square pane goes to the bottom, because width is the scarcer thing in a listing.
    let square = Rect::from_min_size(pos2(0.0, 0.0), vec2(500.0, 500.0));
    assert_eq!(Where::Auto.side(square), Side::Bottom);
    // And the two fixed ones do not care what shape anything is.
    for pane in [wide, narrow, square] {
        assert_eq!(Where::Right.side(pane), Side::Right);
        assert_eq!(Where::Bottom.side(pane), Side::Bottom);
    }
}

#[test]
fn a_position_survives_a_trip_through_the_settings_file() {
    for at in Where::ALL {
        assert_eq!(Where::parse(at.as_str()), Some(at));
        assert_eq!(Where::parse(&at.as_str().to_uppercase()), Some(at));
    }
    assert_eq!(Where::parse("sideways"), None);
    assert_eq!(Where::parse(""), None);
}

/// A shut panel takes nothing, an open one leaves the listing usable, and the seam between
/// them belongs to neither.
#[test]
fn the_panel_leaves_the_listing_usable_on_both_sides() {
    let body = Rect::from_min_size(pos2(0.0, 32.0), vec2(900.0, 620.0));
    let shut = Layout::default();
    assert_eq!(split(body, false, shut), (body, None));

    type Span = fn(Rect) -> f32;
    for (at, span, least) in [
        (Where::Right, (|r: Rect| r.width()) as Span, MIN_PANEL_W),
        (Where::Bottom, |r: Rect| r.height(), MIN_PANEL_H),
    ] {
        let layout = Layout {
            at,
            ..Default::default()
        };
        let (list, panel) = split(body, true, layout);
        let panel = panel.expect("an open panel is somewhere");
        assert!(
            span(list) >= MIN_LIST,
            "{at:?}: the listing got {} of {}",
            span(list),
            span(body)
        );
        assert!(
            span(panel) >= least + SEAM,
            "{at:?}: the panel is too small"
        );
        // They abut, with the seam inside the panel's first point.
        match at.side(body) {
            Side::Right => assert_eq!(list.right(), panel.left()),
            Side::Bottom => assert_eq!(list.bottom(), panel.top()),
        }
        assert_eq!(
            span(list) + span(panel),
            span(body),
            "{at:?}: room went missing"
        );

        // Dragged past either stop, both sides keep their minimum.
        for share in [0.001, 0.999] {
            let (list, panel) = split(
                body,
                true,
                Layout {
                    at,
                    share,
                    ..Default::default()
                },
            );
            let panel = panel.expect("still open");
            assert!(span(list) >= MIN_LIST, "{at:?} at {share}: the listing");
            assert!(span(panel) >= least, "{at:?} at {share}: the panel");
        }

        // And a pane with no room for it does not get one, rather than getting one over the
        // top of the listing it is about.
        let cramped = match at.side(body) {
            Side::Right => Rect::from_min_size(body.min, vec2(MIN_LIST + 8.0, 400.0)),
            Side::Bottom => Rect::from_min_size(body.min, vec2(400.0, MIN_LIST + 8.0)),
        };
        assert_eq!(split(cramped, true, layout), (cramped, None), "{at:?}");
    }
}

/// **The comment goes first, then the size, and only then does the name crop.**
///
/// The bar's one non-obvious rule, stated as a table. What it is really guarding is the third
/// case: a name long enough to need cropping has already cost both details, so nothing is ever
/// abbreviated while something droppable is still on the bar.
#[test]
fn the_details_give_way_before_the_name_does() {
    // Everything fits.
    assert_eq!(what_fits(100.0, 80.0, 60.0, 300.0), (true, true));
    // Exactly enough is enough.
    assert_eq!(what_fits(100.0, 80.0, 60.0, 240.0), (true, true));
    // One point short: the comment goes, and the name is untouched.
    assert_eq!(what_fits(100.0, 80.0, 60.0, 239.0), (false, true));
    // Shorter still: the size goes too.
    assert_eq!(what_fits(100.0, 80.0, 60.0, 159.0), (false, false));
    // And a name that cannot fit even alone still keeps the controls — it crops instead, which
    // is what the `(false, false)` answer leaves the caller to do.
    assert_eq!(what_fits(400.0, 80.0, 60.0, 120.0), (false, false));
    // A view with no details of its own: whichever way the flags fall there is nothing to
    // draw, and a name that does fit is reported as fitting.
    assert_eq!(what_fits(40.0, 0.0, 0.0, 50.0), (true, true));
}

/// **A panel down the side is never too narrow for its own controls.**
///
/// The bar's zoom field and four buttons are a fixed cost, and a panel that cannot show them is
/// a panel whose only way back to 100% has gone. `MIN_PANEL_W` is what guarantees it, so this
/// is the assertion that ties the two constants together — change either and it says so.
#[test]
fn the_narrowest_panel_still_has_room_for_its_controls() {
    // A compile-time assertion, because both sides are constants: this is a statement about the
    // source rather than about a run, and `const` is where clippy rightly insists it goes.
    const _: () = assert!(MIN_PANEL_W >= ACTIONS + GLYPH + PAD * 3.0);
    // And there is something left over for a name, or the bar is controls and nothing else.
    const _: () = assert!(MIN_PANEL_W - ACTIONS - GLYPH - PAD * 3.0 >= 24.0);
}

/// The panel follows the keyboard, but only once it stops moving — and it *does* let go when
/// the keyboard moves onto something with no preview, which is the half of [`Preview::follow`]
/// that being inside the pane makes necessary.
#[test]
fn the_panel_waits_for_the_selection_to_stop_moving() {
    let mut it = Preview {
        open: true,
        ..Default::default()
    };
    let one = Ask::One(PathBuf::from(r"C:\pics\one.png"), preview::Kind::Picture);
    let two = Ask::One(PathBuf::from(r"C:\pics\two.png"), preview::Kind::Picture);

    it.follow(Some(one.clone()), 10.0);
    let (ready, left) = it.settle(10.0 + FOLLOW_DELAY * 0.6);
    assert!(ready.is_none(), "it read before the wait was up");
    assert!((left.expect("waiting") - FOLLOW_DELAY * 0.4).abs() < 1e-6);

    // Moved on before the wait was up: the clock restarts, and the first one is never read
    // at all — which is the point of the wait rather than a side effect of it.
    it.follow(Some(two.clone()), 10.0 + FOLLOW_DELAY * 0.6);
    assert!(
        it.settle(10.0 + FOLLOW_DELAY * 1.2).0.is_none(),
        "the wait did not restart"
    );
    let (ready, left) = it.settle(10.0 + FOLLOW_DELAY * 2.0);
    assert_eq!(ready.as_ref(), Some(&two));
    assert_eq!(left, None);

    // The file already on show is never asked for again, or the answer arriving would queue
    // another read of it for ever.
    it.asked(0, two.clone(), 1);
    it.follow(Some(two.clone()), 40.0);
    assert_eq!(it.settle(41.0), (None, None));
    assert_eq!(it.showing(), Some(two.first()));

    // Selecting a *second* picture is a different question, so it is asked afresh.
    let pair = Ask::Pair(
        PathBuf::from(r"C:\pics\one.png"),
        PathBuf::from(r"C:\pics\two.png"),
    );
    it.follow(Some(pair.clone()), 50.0);
    assert_eq!(it.settle(50.0 + FOLLOW_DELAY * 2.0).0.as_ref(), Some(&pair));

    // And the keyboard moving onto something with no preview clears it: this panel is beside
    // the row it is about, so a stale picture next to a different selection would be a lie.
    it.asked(0, pair, 2);
    it.follow(None, 60.0);
    assert_eq!(it.showing(), None);
    assert!(matches!(it.focused().content, Content::Unsupported(_)));

    // The keyboard asking for the panel itself does not wait at all.
    it.ask_for(one.clone());
    assert_eq!(it.settle(70.0).0, Some(one));
}

/// **A view somebody picked for a file beats the classifier, and only for that file.**
///
/// The button under `No preview for a .zip` is one call to [`Preview::force`], and everything that
/// makes it behave is in the *lifetime* of what it sets rather than in the click: the panel is offered
/// the classifier's own answer again on every frame, and the file's name has not changed, so an
/// override that did not know which file it was about would silently follow the keyboard down the
/// folder. That is the assertion here, along with the two ways it ends.
#[test]
fn a_view_picked_for_one_file_does_not_follow_the_keyboard() {
    let mut it = Preview {
        open: true,
        ..Default::default()
    };
    let path = PathBuf::from(r"C:\stuff\thing.nosuchthing");
    // What the classifier says about it, offered every frame: on Windows anything with an extension
    // nobody has heard of is the shell's to draw, and the shell had nothing.
    let named = Ask::One(path.clone(), preview::Kind::Shell);

    it.follow(Some(named.clone()), 10.0);
    let (ready, _) = it.settle(10.0 + FOLLOW_DELAY * 2.0);
    assert_eq!(ready.as_ref(), Some(&named));
    it.asked(0, named.clone(), 1);
    it.focused_mut().content = Content::Unsupported("nosuchthing".to_owned());

    // The button. Asked for at once — a click is not a selection and has nothing to debounce.
    it.focused_mut().force(preview::Kind::Text);
    assert_eq!(it.focused().forced_kind(), Some(preview::Kind::Text));
    let (ready, left) = it.settle(10.0);
    assert_eq!(
        ready,
        Some(Ask::One(path.clone(), preview::Kind::Text)),
        "the pick did not become a read"
    );
    assert_eq!(left, None, "the pick waited for the debounce");
    it.asked(0, Ask::One(path.clone(), preview::Kind::Text), 2);

    // And it holds while the keyboard stays put, against the classifier saying `Shell` every frame.
    // Without this the panel would ask for the file again the moment the answer landed, for ever.
    it.follow(Some(named.clone()), 20.0);
    assert_eq!(it.settle(21.0), (None, None), "it asked for the file again");
    assert_eq!(it.focused().forced_kind(), Some(preview::Kind::Text));

    // **Pressing it again takes it off**, back to the kind the name asks for.
    it.focused_mut().force(preview::Kind::Text);
    assert_eq!(it.focused().forced_kind(), None);
    assert_eq!(it.settle(22.0).0, Some(named.clone()));
    it.asked(0, named.clone(), 3);

    // A different view is a different pick rather than a toggle.
    it.focused_mut().force(preview::Kind::Picture);
    it.focused_mut().force(preview::Kind::Binary);
    assert_eq!(it.focused().forced_kind(), Some(preview::Kind::Binary));
    assert_eq!(
        it.settle(23.0).0,
        Some(Ask::One(path.clone(), preview::Kind::Binary))
    );
    it.asked(0, Ask::One(path.clone(), preview::Kind::Binary), 4);

    // **And the keyboard moving on drops it.** The next file is a fresh question: a folder of `.dat`
    // is exactly as likely to be a folder of something else, and a panel that read the second one as
    // a binary because the first one was would be a panel that had learnt the wrong thing.
    let next = Ask::One(
        PathBuf::from(r"C:\stuff\other.nosuchthing"),
        preview::Kind::Shell,
    );
    it.follow(Some(next.clone()), 30.0);
    assert_eq!(it.focused().forced_kind(), None, "the pick followed the keyboard");
    assert_eq!(it.settle(30.0 + FOLLOW_DELAY * 2.0).0, Some(next.clone()));
    it.asked(0, next, 5);

    // As does the panel letting go of the file altogether — a folder selected, or `Ctrl+P`.
    it.focused_mut().force(preview::Kind::Text);
    it.follow(None, 40.0);
    assert!(it.focused().forced_kind().is_none() && it.focused().forced.is_none());
}

/// **A view picked for an extension nobody has heard of, once it works, is the view for the next file
/// with that extension** — and only once it works, and only until its button is pressed again.
#[test]
fn a_view_that_worked_is_remembered_for_the_extension() {
    let mut it = Preview {
        open: true,
        ..Default::default()
    };
    let mut remembered = Remembered::default();
    let one = PathBuf::from(r"C:\stuff\one.toto");
    let two = PathBuf::from(r"C:\stuff\two.toto");
    let shown = |path: &PathBuf| Ask::One(path.clone(), preview::Kind::Shell);
    let body = || {
        Content::Text(Text {
            body: "hello".to_owned(),
            spans: Vec::new(),
            doc: None,
            truncated: false,
            code: true,
            lang: syntax::Lang::None,
            changes: None,
            view: None,
            view_for: None,
        })
    };
    // What [`Slot::arrived`] does with an answer, minus the answer having to be a real one.
    let land = |it: &mut Preview, content: Content| {
        it.focused_mut().awaiting = None;
        it.focused_mut().content = content;
    };

    // The first file, which the shell had nothing for.
    it.follow_all(vec![shown(&one)], 10.0, &mut remembered);
    it.asked(0, shown(&one), 1);
    land(&mut it, Content::Unsupported("toto".to_owned()));

    // Picked, and it failed: nothing learnt. The next file is still a fresh question.
    it.focused_mut().force(preview::Kind::Picture);
    it.settle(10.0);
    it.asked(0, Ask::One(one.clone(), preview::Kind::Picture), 2);
    land(&mut it, Content::Failed("Format error".to_owned()));
    it.follow_all(vec![shown(&one)], 11.0, &mut remembered);
    it.follow_all(vec![shown(&two)], 12.0, &mut remembered);
    assert_eq!(it.focused().forced_kind(), None, "a failed pick was remembered");

    // Back on the first, picked as text, and it worked.
    it.follow_all(vec![shown(&one)], 13.0, &mut remembered);
    it.asked(0, shown(&one), 3);
    it.focused_mut().force(preview::Kind::Text);
    it.settle(13.0);
    it.asked(0, Ask::One(one.clone(), preview::Kind::Text), 4);
    land(&mut it, body());
    it.follow_all(vec![shown(&one)], 14.0, &mut remembered);

    // **The next `.toto` is asked for as text** without anybody pressing anything, and the chooser
    // would show Text pressed.
    it.follow_all(vec![shown(&two)], 15.0, &mut remembered);
    assert_eq!(it.focused().forced_kind(), None, "not on show yet");
    assert_eq!(
        it.settle(15.0 + FOLLOW_DELAY * 2.0).0,
        Some(Ask::One(two.clone(), preview::Kind::Text))
    );
    it.asked(0, Ask::One(two.clone(), preview::Kind::Text), 5);
    assert_eq!(it.focused().forced_kind(), Some(preview::Kind::Text));

    // A different panel — another pane, another tab — gets the same answer from the same window.
    let mut other = Preview {
        open: true,
        ..Default::default()
    };
    other.follow_all(vec![shown(&one)], 16.0, &mut remembered);
    assert_eq!(
        other.settle(16.0 + FOLLOW_DELAY * 2.0).0,
        Some(Ask::One(one.clone(), preview::Kind::Text))
    );

    // A different extension is not affected.
    let elsewhere = PathBuf::from(r"C:\stuff\three.titi");
    it.follow_all(vec![shown(&elsewhere)], 17.0, &mut remembered);
    assert_eq!(it.settle(17.0 + FOLLOW_DELAY * 2.0).0, Some(shown(&elsewhere)));

    // **Pressing it again forgets it**, for this file and for the extension — otherwise the next frame
    // would put it straight back and there would be no way to the shell's answer.
    it.follow_all(vec![shown(&two)], 20.0, &mut remembered);
    it.settle(20.0 + FOLLOW_DELAY * 2.0);
    it.asked(0, Ask::One(two.clone(), preview::Kind::Text), 6);
    land(&mut it, body());
    it.focused_mut().force(preview::Kind::Text);
    assert_eq!(it.settle(21.0).0, Some(shown(&two)));
    it.asked(0, shown(&two), 7);
    it.follow_all(vec![shown(&two)], 22.0, &mut remembered);
    assert_eq!(it.settle(23.0), (None, None), "the forgotten view came back");
    assert_eq!(it.focused().forced_kind(), None);
    other.follow_all(vec![shown(&two)], 30.0, &mut remembered);
    assert_eq!(other.settle(30.0 + FOLLOW_DELAY * 2.0).0, Some(shown(&two)));
}

/// **The header's view-as button is offered over a view that worked**, for a file whose name had no
/// answer, and nowhere it would do nothing. Pressing it puts the chooser up; a pick, or the tile moving
/// on, takes it down again.
#[test]
fn a_view_that_worked_can_be_changed_from_the_header() {
    let mut it = Preview {
        open: true,
        ..Default::default()
    };
    let odd = PathBuf::from(r"C:\stuff\notes.toto");
    let named = Ask::One(odd.clone(), preview::Kind::Shell);
    let text = || {
        Content::Text(Text {
            body: "words".to_owned(),
            spans: Vec::new(),
            doc: None,
            truncated: false,
            code: true,
            lang: syntax::Lang::None,
            changes: None,
            view: None,
            view_for: None,
        })
    };

    // The sniffer read it as text. The name had no answer, so the button is there.
    it.asked(0, named.clone(), 1);
    it.focused_mut().awaiting = None;
    it.focused_mut().content = text();
    assert!(it.focused().choosable(), "no way to show it as anything else");

    it.focused_mut().choose();
    assert!(it.focused().choosing);
    assert!(it.focused().choosable(), "the button has to stay to put the file back");
    it.focused_mut().choose();
    assert!(!it.focused().choosing, "the button again did not put the file back");

    // A pick takes the chooser down, and becomes the read.
    it.focused_mut().choose();
    it.focused_mut().force(preview::Kind::Picture);
    assert!(!it.focused().choosing, "a pick left the chooser up");
    assert_eq!(it.settle(1.0).0, Some(Ask::One(odd.clone(), preview::Kind::Picture)));

    // As does the tile getting an answer about anything at all — the keyboard moving on included.
    it.focused_mut().choose();
    it.asked(0, Ask::One(odd.clone(), preview::Kind::Picture), 2);
    assert!(!it.focused().choosing, "the chooser outlived the file it was about");

    // **Not where the canvas is the chooser already**: a pick that failed offers it in place.
    it.focused_mut().awaiting = None;
    it.focused_mut().content = Content::Failed("Format error".to_owned());
    assert!(!it.focused().choosable(), "a second way to open what is already open");
    // Nor over `No preview for a .toto`, for the same reason.
    it.asked(0, named.clone(), 3);
    it.focused_mut().forced = None;
    it.focused_mut().content = Content::Unsupported("toto".to_owned());
    assert!(!it.focused().choosable());

    // **And not for a file whose name is the answer.** A `.png` is a picture because it is one.
    let png = Ask::One(PathBuf::from(r"C:\pics\a.png"), preview::Kind::Picture);
    it.asked(0, png, 4);
    it.focused_mut().awaiting = None;
    it.focused_mut().content = text();
    assert!(!it.focused().choosable(), "offered over a file its name already answers");
}

/// Closing lets go of what the panel was holding, and a duplicated tab does not inherit it.
#[test]
fn a_shut_panel_holds_nothing() {
    let mut it = Preview {
        open: true,
        ..Default::default()
    };
    it.asked(0, 
        Ask::One(PathBuf::from(r"C:\a.png"), preview::Kind::Picture),
        3,
    );
    assert!(it.busy());
    // A duplicate opens the same way and reads for itself: a copy of three 16 MB textures per
    // `Ctrl+T` would make duplicating a tab the most expensive thing in the window.
    let copy = it.duplicate();
    assert!(copy.open && !copy.busy() && copy.showing().is_none());

    it.close();
    assert!(!it.open && !it.busy() && it.showing().is_none());
    assert!(matches!(it.focused().content, Content::Nothing));
}

/// The bar's title says what is on show, and a comparison says both names.
#[test]
fn a_comparison_is_titled_with_both_names() {
    let one = Ask::One(PathBuf::from(r"C:\pics\a.png"), preview::Kind::Picture);
    assert_eq!(one.title(), "a.png");
    let pair = Ask::Pair(
        PathBuf::from(r"C:\pics\a.png"),
        PathBuf::from(r"D:\other\b.png"),
    );
    assert_eq!(pair.title(), "a.png ↔ b.png");
    // The first is what anything needing one path uses — the folder for the tooltip, the
    // staleness test in `follow`.
    assert_eq!(pair.first(), Path::new(r"C:\pics\a.png"));
}

/// The checkerboard reads as a checkerboard: its two squares are far enough apart to see and
/// close enough together not to compete with the picture on top of them.
///
/// Both halves matter. A board whose squares are the same colour says nothing about an alpha
/// channel, and one in black and white would be the loudest thing in the window — the point
/// of it is to be recognisably *absence*.
#[test]
fn the_checkerboard_reads_as_a_checkerboard() {
    use azur_egui_theme::contrast::apart;

    for t in Theme::all() {
        let name = t.palette.key();
        let got = apart(t.bg.layer, t.bg.control_active);
        assert!(
            (8.0..=18.0).contains(&got),
            "{name}: the checkerboard's squares are {got:.1} ΔL* apart"
        );
    }
    // And the palettes have to agree about it, which is what ruled out the first pair this used:
    // a board that is plain in one palette and invisible in another is not one rule, it is two.
    //
    // **A ratio, and it used to be a difference of 5 ΔL\*.** That figure was written when there
    // were exactly two palettes whose neutral ladders happened to be near-identical in depth, and
    // it was never consistent with the band above it: two palettes at 8 and 18 both satisfy "is a
    // checkerboard" and are 10 apart, so the difference was quietly a much stricter rule than the
    // one this test says it is enforcing. The light palette's own surfaces put its board at 9.3
    // against the dark palette's 15.4 — both well inside the band, 6.1 apart — and the honest
    // reading is that the old figure expired rather than that the board broke.
    //
    // Twice over is the loosest this can be and still mean something: at 2.2× a palette could sit
    // at the bottom of the band while another sits above the top of it, which is the failure the
    // sentence above describes.
    let depth = |t: &Theme| apart(t.bg.layer, t.bg.control_active);
    let (weakest, strongest) = Theme::all().map(|t| depth(&t)).fold(
        (f32::INFINITY, 0.0f32),
        |(lo, hi), d| (lo.min(d), hi.max(d)),
    );
    assert!(
        strongest <= weakest * 2.0,
        "the board is {:.1}× stronger in one palette than another ({weakest:.1} to {strongest:.1} \
         ΔL*), so it is two rules rather than one",
        strongest / weakest
    );
}

// -----------------------------------------------------------------------
// The find bar
// -----------------------------------------------------------------------

fn searching(text: &str, case: bool, word: bool, regex: bool) -> preview::Search {
    preview::Search {
        text: text.to_owned(),
        case,
        word,
        regex,
    }
}

/// **A highlight you cannot read the text through hides the thing it is pointing at**, which is
/// worse than no highlight at all. Both fills, in both themes, against the ink each one is paired
/// with — and the pairing is the point: the strong fill comes with `on_accent` and the quiet one
/// with `primary`, and swapping either is what the measurement catches.
#[test]
fn every_ink_in_the_find_bar_can_be_read() {
    use azur_egui_theme::contrast::{ratio, TEXT};

    for t in Theme::all() {
        let name = t.palette.key();
        for (what, ink, fill) in [
            ("the current hit", t.text.on_accent, t.accent.default),
            ("the other hits", t.text.primary, t.accent.subtle),
        ] {
            let got = ratio(ink, fill);
            assert!(
                got >= TEXT,
                "{name}: {what} is {got:.2}:1, under the {TEXT}:1 floor for text"
            );
        }
    }
}

/// **The bar floats over the surface its own fill is a step from**, so the step is the thing to
/// measure — and measuring it is what turned this bar from a hole in the text into a bar.
///
/// `SAME` and not WCAG's `SHAPE`, deliberately. A one-point line looks like ink, but what it is
/// doing here is dividing a surface from the surface behind it, and that is a *seam* — the case
/// the design system's own ruler exists for, because WCAG's floors are about ink on a fill and
/// have nothing to say about two greys meeting at an edge. Held to `SHAPE` instead, nothing in
/// the dark neutral ramp would pass: `stroke.default` on `bg.layer` is **1.51:1**, since a ratio
/// between two near-blacks is dominated by the 0.05 in its own denominator.
#[test]
fn the_find_bar_has_an_edge_you_can_see() {
    use azur_egui_theme::contrast::{apart, SAME};

    for t in Theme::all() {
        let name = t.palette.key();
        let got = apart(t.stroke.default, t.bg.layer);
        assert!(
            got >= SAME,
            "{name}: the bar's outline is {got:.1} ΔL* from the panel it floats on"
        );
    }
    // And the reason the outline is doing the work rather than the fill: the fill on its own
    // reads in one theme and not the other, which is two rules and not one. The same thing that
    // was wrong with the first checkerboard — see the test above.
    let dark = apart(Theme::dark().bg.layer, Theme::dark().bg.layer_alt);
    let light = apart(Theme::light().bg.layer, Theme::light().bg.layer_alt);
    assert!(
        dark < SAME && light > SAME,
        "the fill has stopped being the asymmetric one ({dark:.1} dark, {light:.1} light); if it \
         now reads on both sides, the outline can go back to `subtle`"
    );
}

/// epaint asserts that a job's sections are ordered and *contiguous* rather than tolerating a
/// gap, so this is the shape of the one thing that would turn a highlight into a panic.
///
/// Run over **two bases**, and the second is the one syntax colouring added: when the body is
/// already cut into coloured parts, a hit can begin in one and end in the next, and the two
/// layers have to be intersected rather than concatenated. What is asserted about the fills is
/// therefore the *ground they cover* and not how many there are — one hit across two parts is
/// two sections, and that is correct.
#[test]
fn a_highlighted_body_is_one_contiguous_run_of_sections() {
    let t = Theme::dark();
    let plain = egui::TextFormat {
        font_id: egui::FontId::monospace(12.0),
        color: t.text.primary,
        ..Default::default()
    };
    // A hit at the very start, one in the middle, and one at the very end: the three places an
    // empty section would be pushed if they were not dropped.
    let body = "foo bar foo baz foo";
    let hits = preview::hits(body, &searching("foo", false, false, false)).at;
    assert_eq!(hits, vec![0..3, 8..11, 16..19]);

    /// Touching ranges joined, so a hit split across two parts counts once.
    fn merged(ranges: &[Range<usize>]) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for range in ranges {
            match out.last_mut() {
                Some(last) if last.end == range.start => last.end = range.end,
                _ => out.push(range.clone()),
            }
        }
        out
    }
    let fills = |job: &egui::text::LayoutJob, want: Color32| -> Vec<Range<usize>> {
        merged(
            &job.sections
                .iter()
                .filter(|s| {
                    if want == Color32::TRANSPARENT {
                        s.format.background != want
                    } else {
                        s.format.background == want
                    }
                })
                .map(|s| s.byte_range.start.0..s.byte_range.end.0)
                .collect::<Vec<_>>(),
        )
    };

    // The second base's parts are picked to straddle: `9..14` starts inside the middle hit and
    // ends outside it.
    let straddling = coloured(
        body.len(),
        &[
            syntax::Span {
                at: 0..5,
                tok: syntax::Tok::Keyword,
            },
            syntax::Span {
                at: 9..14,
                tok: syntax::Tok::Str,
            },
        ],
        &plain,
        &t,
    );
    for (what, base) in [
        ("a plain body", vec![(0..body.len(), plain.clone())]),
        ("a coloured one", straddling),
    ] {
        for at in 0..hits.len() {
            let job = overlay(body, &(0..body.len()), &base, &hits, at, &t);
            assert_eq!(job.text, body);
            let first = job.sections.first().expect("sections");
            assert_eq!(
                first.byte_range.start.0, 0,
                "{what}: does not start at the beginning"
            );
            let last = job.sections.last().expect("sections");
            assert_eq!(
                last.byte_range.end.0,
                body.len(),
                "{what}: does not reach the end"
            );
            for pair in job.sections.windows(2) {
                assert_eq!(
                    pair[0].byte_range.end.0, pair[1].byte_range.start.0,
                    "{what}: a gap between sections"
                );
            }
            assert_eq!(
                fills(&job, Color32::TRANSPARENT),
                hits,
                "{what}: the fills do not cover exactly the hits"
            );
            assert_eq!(
                fills(&job, t.accent.default),
                vec![hits[at].clone()],
                "{what}: the strong fill is not exactly the current hit"
            );
        }
    }
}

/// A slice of a body comes out as a job whose own offsets start at nought.
///
/// Which is what makes a rendered document possible: the hits are offsets into the whole of
/// [`markdown::Doc::text`] and each block is laid out as its own galley, so every section has to
/// be shifted back by where the block began. Getting that wrong is not a wrong colour, it is a
/// panic inside epaint or a highlight in the wrong paragraph.
#[test]
fn a_block_of_a_document_is_offset_back_to_its_own_start() {
    let t = Theme::dark();
    let plain = egui::TextFormat {
        font_id: egui::FontId::monospace(12.0),
        color: t.text.primary,
        ..Default::default()
    };
    let text = "first block\nsecond has foo in it";
    let block = 12..text.len();
    let hits = preview::hits(text, &searching("foo", false, false, false)).at;
    assert_eq!(hits, vec![23..26], "the fixture moved");

    let job = overlay(
        text,
        &block,
        &[(block.clone(), plain.clone())],
        &hits,
        0,
        &t,
    );
    assert_eq!(job.text, "second has foo in it");
    assert_eq!(
        job.sections.first().expect("sections").byte_range.start.0,
        0
    );
    assert_eq!(
        job.sections.last().expect("sections").byte_range.end.0,
        job.text.len()
    );
    // And the fill lands on the word rather than eleven characters along from it.
    let filled = job
        .sections
        .iter()
        .find(|s| s.format.background != Color32::TRANSPARENT)
        .expect("a fill");
    assert_eq!(
        &job.text[filled.byte_range.start.0..filled.byte_range.end.0],
        "foo"
    );

    // A hit in a *different* block contributes nothing to this one, rather than being clamped
    // to its edge — which would put a highlight on a character nobody searched for.
    let other = overlay(text, &(0..11), &[(0..11, plain)], &hits, 0, &t);
    assert!(
        other
            .sections
            .iter()
            .all(|s| s.format.background == Color32::TRANSPARENT),
        "a hit outside the block was drawn inside it"
    );
}

#[test]
fn the_counter_says_which_of_how_many() {
    let mut find = Find::default();
    assert_eq!(find.counter(), "", "an empty field has not failed to find");

    find.search = searching("foo", false, false, false);
    find.against("foo bar foo");
    assert_eq!(find.counter(), "1 of 2");
    find.step(1);
    assert_eq!(find.counter(), "2 of 2");

    find.search = searching("nowhere", false, false, false);
    find.against("foo bar foo");
    assert_eq!(find.counter(), "No results");
    // A search that found nothing has nowhere to be scrolled to either.
    assert!(!find.reveal);

    find.search = searching("(unclosed", false, false, true);
    find.against("foo");
    assert_eq!(find.counter(), "Bad pattern");

    // A count that is really a floor has to look like one.
    find.search = searching("a", false, false, false);
    find.against(&"a".repeat(preview::search::HITS + 10));
    assert_eq!(find.counter(), format!("1 of {}+", preview::search::HITS));

    // And a search that gave up before it found anything cannot claim there is nothing there. Set by
    // hand rather than provoked: the walk's budget is ten million comparisons, and the shape that
    // reaches it belongs in the search's own tests.
    find.hits.clear();
    find.capped = true;
    assert_eq!(find.counter(), "Stopped");
}

#[test]
fn stepping_through_the_hits_wraps_at_both_ends() {
    let mut find = Find {
        search: searching("a", false, false, false),
        ..Find::default()
    };
    find.against("a a a");
    assert_eq!(find.at, 0);
    find.step(-1);
    assert_eq!(find.at, 2, "back from the first goes round to the last");
    find.step(1);
    assert_eq!(find.at, 0, "and on from the last comes round again");

    // **Standing still asks for nothing.** The bar calls `step` once a frame with whatever its
    // buttons came to, which is nought on almost every frame, and a `reveal` set then is a
    // `scroll_to_rect` in every frame the panel draws — text that springs back to the current hit
    // under the wheel and cannot be read around.
    find.reveal = false;
    find.step(0);
    assert!(!find.reveal, "a step of nothing asked to be scrolled to");
    find.step(1);
    assert!(find.reveal, "a step did not ask to be scrolled to");

    // And it is safe with nothing found, which is the state the arrows are disabled in — but
    // `Enter` reaches this too.
    find.search = searching("z", false, false, false);
    find.against("a a a");
    find.step(1);
    assert_eq!(find.at, 0);
}

/// Typing `foo` is three searches, and the first two are `f` and `fo`. Landing back at the top of
/// the file for each of them is the difference between typing a word and typing a word while
/// reading — so the hit chosen is the first one at or after where you already were.
#[test]
fn a_longer_query_keeps_your_place_in_the_file() {
    let body = "fizz\nfoo\nfizz\nfoo\nfizz\nfoo";
    let mut find = Find {
        search: searching("f", false, false, false),
        ..Find::default()
    };
    find.against(body);
    // Down to the `f` of the third `foo`, which is the eighth hit.
    for _ in 0..7 {
        find.step(1);
    }
    let was = find.hits[find.at].start;
    assert_eq!(&body[was..was + 3], "foo");

    find.search = searching("foo", false, false, false);
    find.against(body);
    assert_eq!(
        find.hits[find.at].start, was,
        "the longer query went back to the top instead of staying put"
    );

    // And a query with nothing at or after where you were comes back to the first hit rather
    // than off the end of the vector.
    find.search = searching("fizz", false, false, false);
    find.against("fizz\nnothing after");
    assert_eq!(find.at, 0);
}

/// The hits are offsets into one body, so a different body has to invalidate them: they end up as
/// layout sections over the *new* text, where an offset past the end is a panic.
#[test]
fn a_new_file_forgets_what_was_found_in_the_last_one() {
    let mut find = Find {
        search: searching("aaaa", false, false, false),
        ..Find::default()
    };
    find.against("aaaa aaaa aaaa");
    assert_eq!(find.hits.len(), 3);

    find.forget();
    assert!(find.hits.is_empty());
    assert!(find.done.is_none(), "the next frame would not re-run it");
    // And the query itself survives, which is the half that should.
    assert_eq!(find.search.text, "aaaa");
    find.against("x");
    assert!(find.hits.is_empty());
}

/// **The shapes the panel cuts for two, three and four files.**
///
/// The geometry the whole feature is: one tile fills the canvas, two go side by side, three put the
/// odd one along the full width of the bottom, and four make a 2x2. Checked as *relationships* rather
/// than against numbers, because the numbers are the canvas's and the shape is the claim.
#[test]
fn the_canvas_is_cut_into_the_shape_the_count_asks_for() {
    let canvas = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0));
    let spans = |rects: &[Rect]| -> Vec<(f32, f32, f32, f32)> {
        rects
            .iter()
            .map(|r| (r.left(), r.top(), r.right(), r.bottom()))
            .collect()
    };

    // One file has the lot, and no seam is taken out of it: there is no neighbour to be separated
    // from.
    assert_eq!(tiles(canvas, 1), vec![canvas]);
    assert_eq!(tiles(canvas, 0), vec![canvas], "an empty panel still has a tile");

    // Two, side by side: same height as the canvas, and a gap between them.
    let two = tiles(canvas, 2);
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].top(), canvas.top());
    assert_eq!(two[0].bottom(), canvas.bottom());
    assert_eq!(two[1].top(), canvas.top());
    assert_eq!(two[1].bottom(), canvas.bottom());
    assert!(two[0].right() < two[1].left(), "the two tiles overlap");
    assert!(
        (two[1].left() - two[0].right() - SEAM).abs() < 1e-6,
        "the gap between them is not one seam: {:?}",
        spans(&two)
    );

    // Three: two across the top, the third the *full width* underneath.
    let three = tiles(canvas, 3);
    assert_eq!(three.len(), 3);
    assert_eq!(three[0].top(), canvas.top());
    assert_eq!(three[1].top(), canvas.top());
    assert_eq!(three[0].bottom(), three[1].bottom(), "the top two are one row");
    assert!(three[0].right() < three[1].left(), "the top two overlap");
    // The one that makes this shape what it is.
    assert_eq!(three[2].left(), canvas.left());
    assert_eq!(three[2].right(), canvas.right());
    assert_eq!(three[2].bottom(), canvas.bottom());
    assert!(
        three[2].top() > three[0].bottom(),
        "the bottom tile is not below the top row: {:?}",
        spans(&three)
    );

    // Four: two by two, so the columns line up down the panel and the rows across it.
    let four = tiles(canvas, 4);
    assert_eq!(four.len(), 4);
    assert_eq!(four[0].left(), four[2].left(), "the left column is not a column");
    assert_eq!(four[0].right(), four[2].right());
    assert_eq!(four[1].left(), four[3].left(), "the right column is not a column");
    assert_eq!(four[0].top(), four[1].top(), "the top row is not a row");
    assert_eq!(four[2].bottom(), four[3].bottom(), "the bottom row is not a row");
    assert!(four[0].bottom() < four[2].top(), "the rows overlap");
    // Nothing spills out of the canvas, which is what the seam coming out of the *tiles* buys.
    for tile in &four {
        assert!(canvas.contains_rect(*tile), "{tile:?} is outside {canvas:?}");
    }
}

/// The panel grows and shrinks with the selection, and shrinking lets go of what it was holding.
///
/// Dropping the slot is the whole of the teardown for a video that was playing in it — see
/// [`Preview::fit`] — so the count going down has to actually remove them rather than blank them.
#[test]
fn the_panel_holds_one_slot_per_file_up_to_four() {
    let mut it = Preview {
        open: true,
        ..Default::default()
    };
    let ask = |name: &str| Ask::One(PathBuf::from(format!(r"C:\pics\{name}.png")), preview::Kind::Picture);

    // A panel with nothing selected still has one tile to say so in.
    it.follow_all(Vec::new(), 0.0, &mut Remembered::default());
    assert_eq!(it.count(), 1);

    let four = [ask("a"), ask("b"), ask("c"), ask("d")];
    it.follow_all(four.to_vec(), 10.0, &mut Remembered::default());
    assert_eq!(it.count(), 4);

    // Every tile settles on its own file, and each is a request of its own.
    let (ready, left) = it.settle_all(10.0 + FOLLOW_DELAY * 2.0);
    assert_eq!(left, None);
    assert_eq!(ready.len(), 4, "one request per tile, got {ready:?}");
    for (at, (slot, got)) in ready.iter().enumerate() {
        assert_eq!(*slot, at, "the tile index has to travel with the request");
        assert_eq!(got, &four[at], "tile {at} settled on the wrong file");
    }
    for (at, (slot, got)) in ready.into_iter().enumerate() {
        it.asked(slot, got, at as u64 + 1);
    }
    assert_eq!(it.showing_all().len(), 4);

    // Past four the panel does not grow: a select-all is not a request for forty previews.
    let five: Vec<Ask> = "abcde".chars().map(|c| ask(&c.to_string())).collect();
    it.follow_all(five.clone(), 20.0, &mut Remembered::default());
    assert_eq!(it.count(), MOST);

    // And the focus comes back with the selection when it shrinks under it.
    it.focus = 3;
    it.follow_all(four[..2].to_vec(), 30.0, &mut Remembered::default());
    assert_eq!(it.count(), 2);
    assert_eq!(it.focused_at(), 1, "the focus stayed off the end of the slots");

    // Shut, and it is back to its resting shape rather than four empty tiles.
    it.close();
    assert_eq!(it.count(), 1);
    assert_eq!(it.focused_at(), 0);
    assert!(!it.open);
}

/// A read comes back to the tile that asked for it, and nowhere else.
///
/// The token is the whole of that: four tiles are four requests in flight at once, and a payload is a
/// decoded picture — there is one of it, so handing it to the wrong tile loses it.
#[test]
fn a_payload_lands_in_the_tile_that_asked_for_it() {
    let mut it = Preview {
        open: true,
        ..Default::default()
    };
    let ask = |name: &str| Ask::One(PathBuf::from(format!(r"C:\pics\{name}.txt")), preview::Kind::Text);
    let two = [ask("first"), ask("second")];
    it.follow_all(two.to_vec(), 0.0, &mut Remembered::default());
    let (ready, _) = it.settle_all(FOLLOW_DELAY * 2.0);
    assert_eq!(ready.len(), 2);
    it.asked(0, two[0].clone(), 11);
    it.asked(1, two[1].clone(), 22);

    // Both tokens are this panel's, and neither is any other panel's.
    assert!(it.wants(11) && it.wants(22));
    assert!(!it.wants(33));
    // Still busy while either is outstanding, which is what a capture waits on.
    assert!(it.busy());

    // The second tile's answer is what the second tile shows.
    assert_eq!(it.showing_all(), vec![two[0].first(), two[1].first()]);
    // And a request whose tile has gone is dropped rather than landing on a survivor.
    it.follow_all(two[..1].to_vec(), 100.0, &mut Remembered::default());
    it.asked(1, two[1].clone(), 44);
    assert!(!it.wants(44), "a request for a tile that no longer exists was kept");
}

/// **Two selected pictures tile, unless the diff button asks for the blend.**
///
/// The gesture used to mean the blended comparison and now means two tiles, so the blend has to be
/// reachable and has to be *asked for* — see [`crate::app::App::selected_previews`]. This is the
/// panel's half: the latch, and its dying with the selection it was about.
#[test]
fn the_blend_is_a_latch_that_dies_with_its_two_files() {
    let mut it = Preview {
        open: true,
        ..Default::default()
    };
    assert!(!it.comparing(), "the blend is off until it is asked for");

    // Two pictures selected, so the button is on offer and the latch can be set.
    it.allow_compare(true);
    it.compare = true;
    assert!(it.comparing());
    // Still two pictures: the latch stands.
    it.allow_compare(true);
    assert!(it.comparing());

    // The selection stops being two pictures and the latch goes with it, or the next two picked
    // would be blended by a choice made about a different pair.
    it.allow_compare(false);
    assert!(!it.comparing());
}
