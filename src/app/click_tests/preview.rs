//! The preview panel, driven with real clicks: every view, its bar, and the find bar.

use super::*;

/// Put the focused pane in the folder the test binary is in, select it, and open the preview
/// panel on it — then let the walk land.
///
/// The binary is this test process, which is the one fixture on any machine that is a real PE
/// image, is always there, and imports something. Selected rather than handed to the panel
/// directly, because the panel follows the selection every frame: pointing it at a file that
/// is *not* selected is a state the running program never reaches, and one that would be
/// cleared on the next frame anyway.
fn open_preview(h: &mut Harness) {
    let me = std::env::current_exe().expect("a test process has an executable");
    let folder = me.parent().expect("it is in a folder").to_path_buf();
    let pane = h.app.panes[0].id;
    h.app
        .perform(&h.ctx.clone(), Action::Navigate { pane, path: folder });
    h.settle();

    let name = me
        .file_name()
        .expect("it has a name")
        .to_string_lossy()
        .into_owned();
    let at = {
        let tab = h.app.panes[0].tab();
        let dir = tab.dir.as_ref().expect("the listing arrived");
        tab.order
            .iter()
            .position(|&i| dir.name(i as usize) == name)
            .unwrap_or_else(|| panic!("{name} is not in its own folder's listing"))
    };
    h.app.panes[0].tab_mut().select_only(at);
    h.app.panes[0].tab_mut().preview.open = true;
    h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
    for attempt in 0..400 {
        h.frame(Vec::new());
        if !h.app.preview_pending() && attempt > 2 {
            break;
        }
        // Frames are free here; the walk is on a worker thread sharing a machine with
        // seven other test threads.
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!h.app.preview_pending(), "the read never came back");
    assert_eq!(
        h.app.panes[0].tab().preview.showing(),
        Some(me.as_path()),
        "the panel is not showing the binary that was selected"
    );
    h.take_journal();
}

/// Open a text preview on this program's own `main.rs` — a real file, in the monospace face,
/// with plenty in it to find.
///
/// `main.rs` rather than any other because it is one of the few that stays a single file at the
/// top of `src`: this navigates to the real source tree, so the fixture has to be something a
/// reorganisation will not move.
fn open_text_preview(h: &mut Harness) {
    let pane = h.app.panes[0].id;
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app
        .perform(&h.ctx.clone(), Action::Navigate { pane, path: src });
    h.settle();
    let at = {
        let tab = h.app.panes[0].tab();
        let dir = tab.dir.as_ref().expect("the listing arrived");
        tab.order
            .iter()
            .position(|&i| dir.name(i as usize) == "main.rs")
            .expect("this program's own `src` has a `main.rs` in it")
    };
    h.app.panes[0].tab_mut().select_only(at);
    h.app.panes[0].tab_mut().preview.open = true;
    h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
    for attempt in 0..400 {
        h.frame(Vec::new());
        if !h.app.preview_pending() && attempt > 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!h.app.preview_pending(), "the read never came back");
    h.take_journal();
}

/// Where the panel is, once the layout has looked at the pane it is in.
fn preview_rect(h: &Harness) -> Rect {
    crate::ui::preview::split(pane_body(h), true, h.app.preview)
        .1
        .expect("the panel has room in a harness-sized pane")
}

/// How many rows the focused pane's dependency view is showing.
fn shown_deps(h: &Harness) -> usize {
    h.app.panes[0]
        .tab()
        .preview
        .dependency_rows()
        .expect("the panel is showing a dependency tree")
}

/// **The panel is inside the pane**, on whichever side the layout says, and its close button
/// gives the room back.
///
/// Driven through the real button rather than through `perform`, because the one thing only
/// that can catch is a button nothing can reach — and this one sits on a bar inside a pane,
/// over a listing that also claims every point of it for its own hit-testing.
#[test]
fn the_preview_panel_takes_room_from_the_listing_and_its_close_button_gives_it_back() {
    use crate::ui::preview::{Side, Where};

    for at in [Where::Right, Where::Bottom] {
        let mut h = Harness::new();
        h.app.preview.at = at;
        open_preview(&mut h);
        let body = pane_body(&h);
        let panel = preview_rect(&h);

        // The listing gave up room on one side, and only there.
        match at.side(body) {
            Side::Right => {
                assert!(panel.right() >= body.right() - 0.5, "{at:?}: not at the edge");
                assert_eq!(panel.top(), body.top(), "{at:?}: it is not full height");
                assert!(panel.width() < body.width() * 0.6);
            }
            Side::Bottom => {
                assert!(panel.bottom() >= body.bottom() - 0.5, "{at:?}");
                assert_eq!(panel.left(), body.left(), "{at:?}: it is not full width");
                assert!(panel.height() < body.height() * 0.6);
            }
        }
        // And it is showing the walk rather than the word `Reading…`.
        //
        // **Named modules, not one particular module.** This looked for the words `API set`, which is a
        // row the walk only draws for an api-set import — and *which* imports are in the first ten rows
        // is the order the linker wrote this test binary's import table in. A panel along the bottom of
        // a pane holds about nine rows against the side panel's twenty-five, so adding a Win32 call
        // anywhere in this crate could push that row out of view and fail a test about something else
        // entirely. Which it did. What the assertion is for is that the walk *finished*: rows naming
        // real modules, and not the word it says while it is still reading.
        let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
        assert!(
            texts
                .iter()
                .any(|text| text.to_ascii_lowercase().ends_with(".dll")),
            "{at:?}: the panel names no imported module: {texts:?}"
        );
        assert!(
            !texts.iter().any(|text| text.contains("Reading")),
            "{at:?}: the panel is still reading"
        );

        // **A row picks when it is clicked and folds when it is double-clicked.** Worth driving for
        // real rather than through the view's own methods, because the rows live inside a
        // `ScrollArea` inside a panel inside a pane — and a click landing on any of those instead of
        // on the row is precisely the kind of thing that looks correct in the source and does
        // nothing at all.
        let before = shown_deps(&h);
        assert!(before > 2, "{at:?}: the root's imports are not on show");
        // **The row is asked for rather than assumed.** This used to click the row under the
        // root, on the reasoning that the first import would have imports of its own. That is
        // not a fact about this program: the panel is pointed at the test binary itself, so the
        // order of these rows is the order the linker wrote *its* import table in, and adding
        // anything to this crate can rearrange it. It did — an API set came to the top, an API
        // set has nothing under it, and the click landed on a row that could not unfold. Which
        // says nothing about whether the click reached it, and that is the whole question here.
        // And it has to be a row this panel can *show*, not merely one the tree holds: a click below
        // the last visible row lands on empty canvas, and the panel along the bottom of a pane holds
        // about nine rows where the one down the side holds twenty-five. See
        // `crate::ui::deps::View::foldable`, where all of that is written down.
        //
        // **Clicked where the row was drawn, and not where the constants say it should have been.**
        // Which is the third time this test was caught by the same thing, and the first time it stops
        // being able to happen: the position used to be `panel.top() + HEADER + ROW * row`, and the
        // panel's rows do not start at `HEADER` — there is a seam above the header, so every click
        // landed one row low. It passed anyway for as long as the row below the intended one also
        // happened to be foldable, and stopped the day this crate gained an import that put an api
        // set there.
        //
        // So the drawn text is the authority on both halves: the candidate is the first foldable row
        // whose name is on the panel exactly once, and where it is on the panel is where the click
        // goes. A name drawn twice — a module imported by two things that are both on screen, which
        // the auto-expand made ordinary — would be a guess between two rows, and one drawn not at all
        // is a row this docking cannot show.
        let texts = h.texts();
        let candidates = h.app.panes[0].tab().preview.dependency_foldable(60);
        let (name, drawn) = candidates
            .iter()
            .find_map(|(_, name)| {
                let mut here = texts
                    .iter()
                    .filter(|(pos, text)| text == name && panel.contains(*pos));
                let first = here.next()?.0;
                here.next().is_none().then_some((name, first))
            })
            .unwrap_or_else(|| {
                panic!(
                    "{at:?}: none of the {} foldable rows is drawn exactly once on the panel",
                    candidates.len()
                )
            });
        // A few points into the row, the text being drawn just inside its top edge — and clear of
        // the expander at the left-hand end, which is the one part of a row that folds on a single
        // click.
        let first_import = pos2(panel.left() + 80.0, drawn.y + crate::ui::deps::ROW / 4.0);
        // **Settled first.** The panel has just been filled by a worker thread, and a click on a
        // row of it in the same breath is a click on a view that is still arriving: co-executing
        // with the other two tests in this group, this went from unfolding the row to doing
        // nothing at all, deterministically enough to bisect and racy enough to flip on an
        // unrelated `println!`. One quiet frame is what a person's hand gives it for free.
        h.wait();
        h.click_at(first_import);
        assert_eq!(
            shown_deps(&h),
            before,
            "{at:?}: a single click on the body of a row folded it"
        );
        let picked = h.app.panes[0]
            .tab()
            .preview
            .dependency_picked()
            .unwrap_or_else(|| panic!("{at:?}: clicking `{name}` picked nothing"));
        assert_eq!(picked.from, Some(0), "the row clicked was one of the root's");
        // And the panels about it are on screen — in whichever of the two arrangements this docking
        // has room for, which is the point of asking the panel rather than computing a rect here.
        h.wait();
        assert!(
            h.texts()
                .into_iter()
                .any(|(pos, text)| panel.contains(pos) && text.starts_with("Exports")),
            "{at:?}: a row is picked and its symbols are nowhere on the panel"
        );

        // The double click folds, and folding does not disturb the pick.
        h.wait();
        h.double_click_at(first_import);
        let opened = shown_deps(&h);
        assert!(
            opened > before,
            "{at:?}: double-clicking a row did not unfold it: {before} rows, then {opened}"
        );
        h.wait();
        h.double_click_at(first_import);
        assert_eq!(shown_deps(&h), before, "{at:?}: it did not fold up again");
        // Clicking the picked row again puts it away, which is the only way back to a tree with the
        // whole canvas to itself.
        h.wait();
        h.click_at(first_import);
        assert_eq!(
            h.app.panes[0].tab().preview.dependency_picked(),
            None,
            "{at:?}: the pick could not be put away"
        );

        // The close button, found by hovering rather than by arithmetic.
        let close = egui::Id::new(("preview-close", h.app.panes[0].id));
        let found = h
            .find(
                close,
                panel.right() - 16.0,
                (panel.top() as i32)..(panel.top() as i32 + 34),
            )
            .unwrap_or_else(|| panic!("{at:?}: nothing answers to the close button"));
        assert_eq!(h.click_at(found), vec!["ClosePreview"]);
        assert!(!h.app.panes[0].tab().preview.open);
        // And the listing has the whole body back.
        assert_eq!(
            crate::ui::preview::split(pane_body(&h), false, h.app.preview),
            (pane_body(&h), None),
            "{at:?}: the room did not come back"
        );
    }
}

/// **The find bar over a text preview**, driven the way it is used: the button on the panel's
/// bar, then typing, then the arrows.
///
/// Every part of this is the kind that looks right in the source and does nothing on screen. The
/// button is on a bar inside a panel inside a pane, over a listing that hit-tests every point of
/// it. The bar itself is a floating layer over a `ScrollArea` holding a selectable label, so a
/// click that reached the label instead of the bar would start selecting text, and a keystroke
/// that missed the field would go to the *listing* and move the selection — which would change
/// the file being previewed out from under the search.
#[test]
fn the_find_bar_searches_the_text_on_show() {
    let mut h = Harness::new();
    open_text_preview(&mut h);
    let panel = preview_rect(&h);
    let pane = h.app.panes[0].id;
    assert!(
        !h.app.panes[0].tab().preview.finding().0,
        "the bar starts shut"
    );

    // The button, hovered for rather than measured to: it is the third from the right-hand end
    // of the bar, and the buttons are 24 points at a 32-point pitch.
    let strip = (panel.top() as i32)..(panel.top() as i32 + 34);
    let id = egui::Id::new(("preview-find", pane));
    let button = (0..5)
        .find_map(|step| h.find(id, panel.right() - 20.0 - step as f32 * 32.0, strip.clone()))
        .expect("nothing on the panel's bar answers to the find button");
    h.click_at(button);
    assert!(
        h.app.panes[0].tab().preview.finding().0,
        "the button did not open the bar"
    );

    // And the caret is in the field, because the button that opens it puts it there. If this
    // keystroke went anywhere else it would be moving the selection in the listing.
    h.frame(vec![Event::Text("window".to_owned())]);
    h.frame(Vec::new());
    let (open, at, hits, counter) = h.app.panes[0].tab().preview.finding();
    assert!(open);
    assert!(
        hits >= 4,
        "`window` should be all over `main.rs`, found {hits}"
    );
    assert_eq!(at, 0);
    assert_eq!(counter, format!("1 of {hits}"));

    // `Enter` is the next-match arrow, and `Shift+Enter` the previous one — which is what makes
    // the bar usable without the pointer going near it again.
    h.frame(vec![Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert_eq!(
        h.app.panes[0].tab().preview.finding().1,
        1,
        "Enter did not step to the next hit"
    );
    h.modifiers = Modifiers::SHIFT;
    h.frame(vec![Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::SHIFT,
    }]);
    h.modifiers = Modifiers::NONE;
    assert_eq!(
        h.app.panes[0].tab().preview.finding().1,
        0,
        "Shift+Enter did not step back"
    );

    // **`F3` and `Shift+F3` are the arrows too**, and the interesting half is what they must
    // *not* do: `F3` is also the shortcut that puts the caret in the pane's filter box, the path
    // bar is drawn before this panel, and a keystroke taken there would never reach here. So the
    // filter's text is checked as well as the hit — a `z` appearing in it would mean the path bar
    // had swallowed the key and the caret with it.
    for (shift, want, what) in [
        (Modifiers::NONE, 1, "F3"),
        (Modifiers::SHIFT, 0, "Shift+F3"),
    ] {
        h.modifiers = shift;
        h.frame(vec![Event::Key {
            key: egui::Key::F3,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: shift,
        }]);
        h.modifiers = Modifiers::NONE;
        assert_eq!(
            h.app.panes[0].tab().preview.finding().1,
            want,
            "{what} did not step the match"
        );
        assert!(
            h.tab(0).filter.is_empty(),
            "{what} put the caret in the filter box instead"
        );
    }

    // The regex toggle turns the same text into a pattern. `window\b` finds fewer than
    // `window` does, because `windows` stops counting.
    h.app.panes[0].tab_mut().preview.set_regex(true);
    h.app.panes[0].tab_mut().preview.look_for(r"window\b");
    h.frame(Vec::new());
    let (_, _, whole, _) = h.app.panes[0].tab().preview.finding();
    assert!(
        whole > 0 && whole < hits,
        "`window\\b` found {whole} where `window` found {hits}"
    );

    // And a pattern that will not compile says so rather than emptying the panel.
    h.app.panes[0].tab_mut().preview.look_for("(unclosed");
    h.frame(Vec::new());
    let (_, _, none, complaint) = h.app.panes[0].tab().preview.finding();
    assert_eq!(none, 0);
    assert_eq!(complaint, "Bad pattern");

    // Escape shuts it, from the keyboard, with the caret still in the field.
    h.frame(vec![Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(Vec::new());
    assert!(
        !h.app.panes[0].tab().preview.finding().0,
        "Escape did not shut the bar"
    );
    // The panel is still showing the file, which is the point of a find bar that closes.
    assert!(h.app.panes[0].tab().preview.showing().is_some());
}

/// **A source file reaches the canvas coloured**, which is a claim about the galley rather than
/// about the lexer.
///
/// `crate::syntax` has its own tests and they are about byte ranges. What none of them can say is
/// that those ranges become *layout sections with colours in them* on the shape list — the path
/// from a span to a section runs through `Text::spans`, `coloured` and `overlay`, and a body that
/// arrived on screen in one flat colour would pass every test in that module.
#[test]
fn a_source_file_reaches_the_canvas_with_its_colours() {
    let mut h = Harness::new();
    // This program's own `main.rs`, which is Rust and long enough to hold every token kind.
    open_text_preview(&mut h);
    h.frame(Vec::new());

    // The body's galley: the one on the shape list with the most text in it by a wide margin.
    let body = h
        .texts()
        .into_iter()
        .map(|(_, text)| text)
        .max_by_key(String::len)
        .expect("the panel painted something");
    assert!(
        body.len() > 10_000,
        "that is not the body: {} bytes",
        body.len()
    );

    // Its sections' colours, which is where the answer is. Gathered off the shapes rather than
    // recomputed, so this is what epaint was handed.
    let mut inks = std::collections::BTreeSet::new();
    fn walk(shape: &egui::Shape, into: &mut std::collections::BTreeSet<[u8; 4]>) {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text.len() > 10_000 => {
                for section in &text.galley.job.sections {
                    into.insert(section.format.color.to_array());
                }
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, into)),
            _ => {}
        }
    }
    for shape in &h.shapes {
        walk(shape, &mut inks);
    }

    let t = crate::theme::Theme::dark();
    assert!(
        inks.len() >= 5,
        "the body is set in {} colours, so it is not coloured",
        inks.len()
    );
    for (role, want) in [
        ("the body's own", t.text.primary),
        ("a keyword", t.syntax.keyword),
        ("a comment", t.syntax.comment),
        ("a string", t.syntax.string),
        ("a type", t.syntax.kind),
    ] {
        assert!(
            inks.contains(&want.to_array()),
            "{role} colour is not on the canvas: {inks:?}"
        );
    }
}

/// **A Markdown file is drawn as a document**, and the button beside it goes back to the markup.
///
/// End to end rather than over `markdown::parse`, which has its own tests, because what those
/// cannot say is that the *panel* chose the document — the decision runs from the extension on
/// the worker, through `Text::doc`, to which of two functions `show` calls, and every step of
/// that is somewhere the two views could have been swapped.
///
/// The find bar is here for a reason of its own. Its hits are byte offsets, and there are now two
/// strings they could be offsets into: the markup and the document. `bold word` exists in one of
/// them and not the other, so a search that answers the same in both views is a search running
/// against the wrong one — which would not be a wrong count, it would be a highlight painted over
/// an unrelated word.
#[test]
fn a_markdown_file_is_rendered_and_the_toggle_shows_its_source() {
    let root = crate::sandbox::dir("md");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("a directory in the temp folder");
    std::fs::write(
        root.join("notes.md"),
        "# The heading\n\nA **bold** word and `code`.\n\n- an item\n",
    )
    .expect("a file in the temp folder");

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();
    h.app.panes[0].tab_mut().preview.open = true;
    h.app.panes[0].tab_mut().select_only(0);
    settle_preview(&mut h);

    let shown =
        |h: &Harness| -> Vec<String> { h.texts().into_iter().map(|(_, text)| text).collect() };

    // ---- Rendered: the marks are gone and the words are not ----------------
    let texts = shown(&h);
    assert!(
        texts.iter().any(|text| text == "The heading"),
        "the heading is not on screen as a heading: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text == "A bold word and code."),
        "the paragraph is not on screen with its markup followed: {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|text| text.contains("**") || text.contains("# ")),
        "the markup is still being drawn: {texts:?}"
    );

    // ---- Inline code sits on the paragraph's own baseline --------------------
    //
    // One galley, two faces, and epaint places a glyph at `ascent + valign × (row height −
    // line height)` — so the monospace run comes out three pixels above the prose unless the
    // renderer corrects it, and the tinted fill behind it stays put, which is what made it
    // read as an underline. See `ui::preview::Faces::of`.
    //
    // Asserted on the *painted* galley rather than on the correction: what matters is where
    // the glyphs ended up, and `Glyph::pos.y` is the baseline.
    fn baselines(shape: &egui::Shape, into: &mut Vec<(String, Vec<f32>)>) {
        match shape {
            egui::Shape::Text(text) => {
                for row in &text.galley.rows {
                    let mut ys: Vec<f32> = row.glyphs.iter().map(|g| g.pos.y).collect();
                    ys.dedup();
                    into.push((text.galley.text().to_owned(), ys));
                }
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| baselines(s, into)),
            _ => {}
        }
    }
    let mut rows = Vec::new();
    for shape in &h.shapes {
        baselines(shape, &mut rows);
    }
    let mixed: Vec<&(String, Vec<f32>)> = rows
        .iter()
        .filter(|(text, _)| text.contains("A bold word and code."))
        .collect();
    assert!(
        !mixed.is_empty(),
        "the paragraph with inline code in it was not painted"
    );
    for (text, ys) in mixed {
        assert_eq!(
            ys.len(),
            1,
            "{text:?} is drawn on {} baselines, {ys:?} — the inline code is off the line",
            ys.len()
        );
    }

    // The line-number toggle is not offered over a document, because a document has no lines of
    // the file's. Asked of the *bar*, since a control that exists but is never drawn is exactly
    // what this is checking against.
    let panel = preview_rect(&h);
    let strip = (panel.top() as i32)..(panel.top() as i32 + 34);
    let button_at = |h: &mut Harness, id: egui::Id| {
        (0..6).find_map(|step| {
            h.find(id, panel.right() - 20.0 - step as f32 * 32.0, strip.clone())
        })
    };
    assert!(
        button_at(&mut h, egui::Id::new(("preview-numbers", pane))).is_none(),
        "the line-number toggle is on the bar over a rendered document"
    );

    // ---- The find bar searches what is on the screen -----------------------
    h.app.panes[0].tab_mut().preview.look_for("bold word");
    h.frame(Vec::new());
    let (_, _, rendered_hits, _) = h.app.panes[0].tab().preview.finding();
    assert_eq!(
        rendered_hits, 1,
        "`bold word` is two words in the rendered document and should be found once"
    );

    // ---- And the toggle goes to the markup ---------------------------------
    let markup = button_at(&mut h, egui::Id::new(("preview-markup", pane)))
        .expect("nothing on the panel's bar answers to the markup toggle");
    h.click_at(markup);
    h.frame(Vec::new());
    let texts = shown(&h);
    assert!(
        texts.iter().any(|text| text.contains("# The heading")),
        "the toggle did not show the markup: {texts:?}"
    );
    assert!(
        h.app.preview.markup,
        "the toggle did not record itself as a preference"
    );
    // The same query, against the other string. Nought, because the source has two asterisks
    // between `bold` and `word` — which is the proof that the search followed the view.
    let (_, _, source_hits, counter) = h.app.panes[0].tab().preview.finding();
    assert_eq!(
        source_hits, 0,
        "`bold word` was found in the markup, where those two words are not adjacent"
    );
    assert_eq!(counter, "No results");
    // And the gutter's toggle is back, because there are lines to number again.
    assert!(
        button_at(&mut h, egui::Id::new(("preview-numbers", pane))).is_some(),
        "the line-number toggle did not come back over the markup"
    );

    crate::sandbox::remove(&root);
}

/// **The preview button's context menu**: show or hide, and then the three positions.
///
/// Driven with a real right click, because a menu hung off a button on a bar that a listing
/// also hit-tests is the kind of thing that looks perfectly correct in the source and simply
/// never opens. And it is **sticky** — ticking a position leaves it up, since three radio
/// buttons you have to reopen the menu between are three menus.
#[test]
fn the_preview_button_carries_the_panels_position() {
    use crate::ui::preview::Where;

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let eye = egui::Id::new(("preview", pane));

    // Find the button, which sits between the path and the flatten toggle rather than at a
    // position this test gets to assume.
    let y = h.path_bar_y(0);
    let right = h.pane_rect(0).right();
    let sweep: Vec<Pos2> = (0..40)
        .map(|step| pos2(right - 20.0 - step as f32 * 6.0, y))
        .collect();
    let at = sweep
        .into_iter()
        .find(|&at| h.hovers(eye, at))
        .expect("nothing on the path bar answers to the preview button");

    // A right click puts the menu up, with the position group in it.
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(Vec::new());
    let entries: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
    for want in ["Show preview", "Position", "Right", "Bottom", "Auto"] {
        assert!(
            entries.iter().any(|text| text == want),
            "`{want}` is not in the menu: {entries:?}"
        );
    }

    // Ticking a position changes the window's preference — and the menu stays up, which is
    // what `sticky` is for. Found by its label rather than by arithmetic over menu rows.
    let bottom = h
        .texts()
        .into_iter()
        .find(|(_, text)| text == "Bottom")
        .map(|(at, _)| at + vec2(8.0, 6.0))
        .expect("the entry was drawn a moment ago");
    assert_eq!(h.app.preview.at, Where::Auto, "the default has moved");
    h.click_at(bottom);
    assert_eq!(
        h.app.preview.at,
        Where::Bottom,
        "clicking the entry did not move the panel"
    );
    assert!(
        h.texts().iter().any(|(_, text)| text == "Position"),
        "the menu closed on a tick, so setting two of these means opening it twice"
    );
}

/// **A double click in the text view selects the word under it**, and keeps it.
///
/// egui's own selectable `Label` implements the gesture, so this is a test that the *panel* does
/// not get in its way — and the reason it is worth having is that nothing in the source says the
/// gesture exists, so nothing in the source would say if it stopped. Three things could take it
/// away: the Label losing the pointer to something drawn over it, a `Sense` that stops sensing
/// clicks, and — the subtle one — the Label's auto-generated id changing between frames, which
/// makes egui drop the selection on the frame after it was made. So the assertions are that the
/// band covers **the word and not the line**, and that it is still there three frames later.
///
/// The band is read out of the galley's own row mesh rather than off a `Shape::Rect`, because
/// that is where epaint puts it: a text selection is vertices in the row it belongs to.
#[test]
fn a_double_click_in_the_preview_selects_the_word_under_it() {
    let root = crate::sandbox::dir("sel");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("notes.txt");
    std::fs::write(&path, "alpha beta gamma\ndelta epsilon zeta\n").unwrap();

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();
    h.app.open_preview_here();
    // Waited for by asking whether it is on screen yet, not by spending a fixed 200 ms and
    // hoping. The file is read on a worker thread, so the fixed budget was a race that this
    // test won on its own and lost in the suite, where the rest of it has the machine busy.
    let mut body_at = None;
    for _ in 0..240 {
        h.frame(Vec::new());
        body_at = h
            .texts()
            .into_iter()
            .find(|(_, text)| text.starts_with("alpha beta gamma"));
        if body_at.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Where the body was drawn, found by its own text rather than by arithmetic over the panel.
    let (at, body) = body_at.expect("the body is not on screen");
    let over = at + vec2(20.0, 8.0);
    h.frame(vec![Event::PointerMoved(over)]);
    assert_eq!(
        h.cursor,
        egui::CursorIcon::Text,
        "the text is not what the pointer is over, so nothing below means anything"
    );

    h.double_click_at(over);
    let ink = h.ctx.style_of(egui::Theme::Dark).visuals.selection.bg_fill;
    let selected = |h: &Harness| -> Vec<(usize, Rect, f32)> {
        fn walk(shape: &egui::Shape, ink: egui::Color32, into: &mut Vec<(usize, Rect, f32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    for (which, row) in text.galley.rows.iter().enumerate() {
                        let mut band = Rect::NOTHING;
                        for vertex in row.visuals.mesh.vertices.iter().filter(|v| v.color == ink)
                        {
                            band.extend_with(vertex.pos);
                        }
                        if band.is_finite() {
                            into.push((which, band, row.size.x));
                        }
                    }
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, ink, into)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &h.shapes {
            walk(shape, ink, &mut out);
        }
        out
    };

    let bands = selected(&h);
    assert_eq!(bands.len(), 1, "a double click selected {bands:?}");
    let (row, band, row_width) = bands[0];
    assert_eq!(row, 0, "the word is on the row that was clicked");
    assert!(
        band.left() < 1.0 && band.width() < row_width * 0.5,
        "the band is {:.1} wide of a {row_width:.1} row — a line rather than a word",
        band.width()
    );
    // The word, checked against the text itself rather than against a number: `alpha ` is the
    // first word and a space, so the band has to stop inside that span.
    assert!(
        body.starts_with("alpha "),
        "the fixture is not what this assertion is about"
    );

    // And it is still there afterwards. A selection that lasts one frame is a selection nobody
    // has — this is what a changing widget id would look like.
    for after in 1..=3 {
        h.frame(Vec::new());
        assert_eq!(
            selected(&h).first().map(|(_, band, _)| band.width()),
            Some(band.width()),
            "the selection was gone {after} frame(s) later"
        );
    }
    crate::sandbox::remove(&root);
}

/// `Ctrl+P` opens this folder's preview panel, and closes it again.
///
/// On the pane the keyboard is in and on that pane's tab, which is the whole of what "each
/// folder has one" means: the shortcut is not a window-wide switch.
#[test]
fn ctrl_p_opens_and_shuts_this_folders_preview() {
    let mut h = Harness::with_panes(2);
    let second = h.app.panes[1].id;
    h.app.perform(&h.ctx.clone(), Action::Focus(second));
    h.frame(Vec::new());

    let press = |h: &mut Harness| {
        h.take_journal();
        h.modifiers = Modifiers::COMMAND;
        h.frame(vec![Event::Key {
            key: egui::Key::P,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }]);
        h.modifiers = Modifiers::NONE;
        h.frame(Vec::new());
        h.take_journal()
    };

    assert!(!h.app.panes[1].tab().preview.open);
    assert_eq!(press(&mut h), vec!["TogglePreview"]);
    assert!(h.app.panes[1].tab().preview.open, "it did not open");
    // And only in the pane the keyboard is in.
    assert!(
        !h.app.panes[0].tab().preview.open,
        "it opened in the other pane as well"
    );
    assert_eq!(press(&mut h), vec!["TogglePreview"]);
    assert!(!h.app.panes[1].tab().preview.open, "it did not shut again");
}

/// **`Space` is the other key for the preview panel**, and it steps aside for a name.
///
/// Three things, and the last two are the whole reason the key is read where the characters are
/// rather than up with the shortcuts: a space with nothing being typed is the panel, a space a
/// moment after a letter is part of what is being looked for, and a space that is merely *still*
/// down is a letter too — otherwise a thumb on the bar would flap the panel at the repeat rate.
#[test]
fn space_opens_the_preview_panel_unless_a_name_is_being_typed() {
    let mut h = Harness::with_panes(2);
    let second = h.app.panes[1].id;
    h.app.perform(&h.ctx.clone(), Action::Focus(second));
    h.frame(Vec::new());

    // A space arrives as a key *and* as a character, which is what `egui-winit` sends for it.
    //
    // **And the bar is let go of**, which is the half a harness has to be told about and the half this
    // test was missing. **egui decides for itself whether a press is a repeat** — `repeat =
    // !first_press`, against the keys it has down, in `InputState::begin_pass` — so the flag a caller
    // passes is not the flag the application reads. A test that pressed the bar and never released it
    // had every tap after the first *become* a repeat, which is a keyboard nobody has, and it is what
    // made the held case below pass against a program that flapped the panel at the repeat rate.
    fn space(h: &mut Harness, pressed: bool) {
        let key = |pressed, repeat| Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed,
            repeat,
            modifiers: Modifiers::NONE,
        };
        h.frame(if pressed {
            vec![key(true, false), Event::Text(" ".to_owned())]
        } else {
            vec![key(false, false)]
        });
        h.frame(Vec::new());
    }
    // One tap of the bar — down, up — and what it asked for.
    let tap = |h: &mut Harness| {
        h.take_journal();
        space(h, true);
        space(h, false);
        h.take_journal()
    };

    assert!(!h.app.panes[1].tab().preview.open);
    assert_eq!(tap(&mut h), vec!["TogglePreview"]);
    assert!(h.app.panes[1].tab().preview.open, "it did not open");
    // And only in the pane the keyboard is in, exactly as `Ctrl+P`.
    assert!(
        !h.app.panes[0].tab().preview.open,
        "it opened in the other pane as well"
    );
    assert_eq!(tap(&mut h), vec!["TogglePreview"]);
    assert!(!h.app.panes[1].tab().preview.open, "it did not shut again");

    // **Held down**: the bar goes down once and stays down, so the first press is a press and every
    // one after it is a repeat — the same character over again, and no new press. Only the first is
    // the panel, or a thumb resting on the bar would flap it at the machine's repeat rate.
    h.take_journal();
    space(&mut h, true);
    assert_eq!(
        h.take_journal(),
        vec!["TogglePreview"],
        "the press that starts a hold is still a press"
    );
    space(&mut h, true);
    let held = h.take_journal();
    assert!(
        held.is_empty(),
        "a held space toggled the panel, got {held:?}"
    );
    assert!(h.app.panes[1].tab().preview.open, "the hold shut it again");
    space(&mut h, false);
    // Back to shut, so what follows is about the space rather than about which way the panel was left.
    // The wait is not decoration: every repeat above **typed a space**, which is a letter of a name and
    // puts a word in flight — so without it this tap would be the next case rather than this one.
    h.wait();
    assert_eq!(tap(&mut h), vec!["TogglePreview"]);
    assert!(!h.app.panes[1].tab().preview.open);

    // And with a word in flight the space belongs to the word. The letter goes to the type-ahead
    // in the frame before, which is what puts a word in flight at all — and the wait first is what
    // makes this an assertion about *that* letter, since the held space above typed one too.
    h.wait();
    h.frame(vec![Event::Text("a".to_owned())]);
    assert!(
        tap(&mut h).is_empty(),
        "the space in a name opened the panel"
    );
    assert!(!h.app.panes[1].tab().preview.open);
}

/// **Two selected pictures become a comparison**: three views, and a toggle down to one.
///
/// End to end, because the interesting part is the *decision* — two selected rows rather than
/// one cursor — and that lives in `App::selected_preview` where the panel cannot see it. The
/// fixtures are written here: two PNGs that differ in one corner, which is a diff with a known
/// answer.
#[test]
fn two_selected_pictures_are_compared_in_three_views() {
    let root = crate::sandbox::dir("diff");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("a directory in the temp folder");
    // Three of them, because the last assertion is about what *three* selected pictures do and
    // `select_all` over two would still be a pair.
    for (name, tint) in [("a.png", 40u8), ("b.png", 200), ("c.png", 40)] {
        let mut buffer = image::RgbaImage::from_pixel(60, 40, image::Rgba([9, 9, 9, 255]));
        buffer.put_pixel(50, 30, image::Rgba([tint, 9, 9, 255]));
        buffer
            .save(root.join(name))
            .expect("a PNG in the temp folder");
    }

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();
    h.app.panes[0].tab_mut().preview.open = true;

    // One picture selected is one picture: the comparison is not something a single selection
    // can produce by accident.
    h.app.panes[0].tab_mut().select_only(0);
    settle_preview(&mut h);
    assert_eq!(
        h.app.panes[0].tab().preview.frames(),
        Some((1, 1)),
        "one selected picture is not one view"
    );

    // And the second one **tiles** them: selecting two files is a request for two previews, and each
    // tile holds its own single picture. See [`crate::app::App::selected_previews`].
    h.app.panes[0].tab_mut().toggle(1);
    settle_preview(&mut h);
    assert_eq!(
        h.app.panes[0].tab().preview.count(),
        2,
        "two selected pictures did not become two tiles"
    );
    assert_eq!(
        h.app.panes[0].tab().preview.frames(),
        Some((1, 1)),
        "a tile beside another is still one picture, not a comparison"
    );

    // The blend is what the diff button asks for while exactly those two are selected — and then it
    // is one tile with three views in it, which is what two pictures used to mean by default.
    h.app.panes[0].tab_mut().preview.set_compare(true);
    settle_preview(&mut h);
    assert_eq!(
        h.app.panes[0].tab().preview.count(),
        1,
        "the blend is one tile, not two"
    );
    assert_eq!(
        h.app.panes[0].tab().preview.frames(),
        Some((3, 3)),
        "the compare toggle did not produce the three views"
    );
    let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
    assert!(
        texts.iter().any(|text| text.contains("↔")),
        "the bar does not name both files: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text == "differences"),
        "the third view is not captioned: {texts:?}"
    );
    // 1 pixel of 2,400 differs — the corner each file tinted differently. Reported on a panel
    // given room for it: at the default share the bar's priority rule has already dropped the
    // comment for a name this long, which the next assertion is about.
    h.app.preview.share = 0.66;
    h.frame(Vec::new());
    let wide: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
    assert!(
        wide.iter().any(|text| text.contains("0.04% differs")),
        "the share that differs is not reported: {wide:?}"
    );
    assert!(
        wide.iter().any(|text| text.contains("60 × 40")),
        "the size is not reported either: {wide:?}"
    );

    // **And the rule, end to end.** Swept from a wide bar to a narrow one, the two details go
    // in order and the name is on the bar the whole way. The invariant that says it is
    // *`comment` never survives `size`* — which holds at every width and does not depend on
    // what this machine's font measures. `what_fits` has the unit test for the arithmetic; this
    // is about what is actually drawn.
    let mut seen = Vec::new();
    for share in [0.66, 0.58, 0.50, 0.42, 0.34, 0.26] {
        h.app.preview.share = share;
        h.frame(Vec::new());
        let bar: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
        // **For a comparison the share that differs is the slot that survives**, and the dimensions
        // are the one that gives way — the priorities are inverted against every other view, because
        // "0.04% differs" is the answer somebody opened the comparison for. See the `differing` arm
        // of the header's detail block, which is where that swap is made and why.
        //
        // So the backwards case is the *dimensions* outliving the share, not the other way about. The
        // sweep only reaches the width where either is dropped once the compare button is on the bar,
        // which is why this went unexercised while two pictures blended by default.
        let headline = bar.iter().any(|text| text.contains("differs"));
        let nicety = bar.iter().any(|text| text.contains("60 × 40"));
        assert!(
            !(nicety && !headline),
            "at {share} the dimensions are on the bar and the share that differs is not, which is \
             backwards: {bar:?}"
        );
        assert!(
            bar.iter().any(|text| text.starts_with("a.png")),
            "at {share} the name is gone, and the details were droppable: {bar:?}"
        );
        // Recorded in the order they give way — the nicety first, then the headline — so the
        // monotonicity check below reads the same way for this view as for every other.
        seen.push((nicety, headline));
    }
    assert_eq!(seen.first(), Some(&(true, true)), "the widest bar: {seen:?}");
    assert_eq!(seen.last(), Some(&(false, false)), "the narrowest: {seen:?}");
    // Monotone: nothing comes back as the bar narrows.
    for pair in seen.windows(2) {
        assert!(
            !(pair[1].0 && !pair[0].0) && !(pair[1].1 && !pair[0].1),
            "a detail reappeared on a narrower bar: {seen:?}"
        );
    }
    h.app.preview.share = crate::ui::preview::SHARE;

    // The toggle takes it down to the difference alone, and back.
    h.app.panes[0].tab_mut().preview.toggle_all();
    h.frame(Vec::new());
    assert_eq!(h.app.panes[0].tab().preview.frames(), Some((3, 1)));
    h.app.panes[0].tab_mut().preview.toggle_all();
    h.frame(Vec::new());
    assert_eq!(h.app.panes[0].tab().preview.frames(), Some((3, 3)));

    // A third selected picture is not a comparison of anything, so the panel goes back to
    // having nothing to say rather than picking two of them.
    h.app.panes[0].tab_mut().toggle(0);
    h.app.panes[0].tab_mut().toggle(1);
    h.app.panes[0].tab_mut().select_all();
    settle_preview(&mut h);
    assert!(
        h.app.panes[0]
            .tab()
            .preview
            .frames()
            .is_none_or(|(_, shown)| shown == 1),
        "three selected pictures produced a comparison"
    );
    crate::sandbox::remove(&root);
}

/// Let the focused pane's preview panel notice the selection and finish reading.
fn settle_preview(h: &mut Harness) {
    h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
    for attempt in 0..400 {
        h.frame(Vec::new());
        if !h.app.preview_pending() && attempt > 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!h.app.preview_pending(), "the read never came back");
    // **The read landing and the read being drawn are different frames.** The loop above stops
    // on the frame the worker's result arrived in, and that frame was already painted from what
    // the panel had before it — so a caller reading `h.texts()` straight afterwards sees the
    // previous contents. It showed up as preview tests that passed alone and failed in the
    // suite, where they are slow enough for the result to arrive a frame later.
    for _ in 0..2 {
        h.frame(Vec::new());
    }
}

/// **The zoom field's list opens, and the field lets the keyboard go again.**
///
/// Two failures this catches, and both were real. A combo box whose popup never appears is a
/// combo box with no presets — you can only type at it. And a text field that keeps focus after
/// you have finished with it takes every shortcut in the window with it: `Ctrl+P`, `F5`, the
/// arrow keys, all of them go to the field instead.
#[test]
fn the_zoom_field_opens_its_list_and_gives_the_keyboard_back() {
    let root = crate::sandbox::dir("zoom");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("a directory in the temp folder");
    image::RgbaImage::from_pixel(80, 60, image::Rgba([30, 90, 200, 255]))
        .save(root.join("swatch.png"))
        .expect("a PNG in the temp folder");

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: root.clone(),
        },
    );
    h.settle();
    // A wide panel, so the zoom group is on the bar at all.
    h.app.preview.share = 0.6;
    h.app.panes[0].tab_mut().preview.open = true;
    h.app.panes[0].tab_mut().select_only(0);
    settle_preview(&mut h);
    assert!(
        h.app.panes[0].tab().preview.frames().is_some(),
        "the picture is not on the canvas, so there is no zoom field"
    );

    // Find the field by its value: `100%` is what an 80×60 picture in a panel this size reads.
    let panel = preview_rect(&h);
    let at = h
        .texts()
        .into_iter()
        .find(|(_, text)| text.ends_with('%'))
        .map(|(at, _)| at + vec2(8.0, 6.0))
        .expect("the zoom field is not on the bar");
    assert!(panel.contains(at), "the field is not in the panel: {at:?}");

    // Clicking it opens the list. Every preset, not only the one the text happens to match:
    // a field that filters its own value down to one row is a list with one row in it.
    h.click_at(at);
    let drawn = h.texts();
    let list: Vec<String> = drawn.iter().map(|(_, text)| text.clone()).collect();
    for preset in ["Fit", "25%", "100%", "400%"] {
        assert!(
            list.iter().any(|text| text == preset),
            "`{preset}` is not in the list: {list:?}"
        );
    }
    // Every label **whole**, which is the other half of it: a list sized to the trigger turns
    // `400%` into `40…`, and the assertion above cannot see that — `Galley::text` reports the
    // string it was *asked* to draw, so an elided label still answers `400%`. `Harness::cropped`
    // asks the galley whether it fitted, which is the only question that distinguishes them.
    let cropped = h.cropped();
    for preset in ["Fit", "25%", "100%", "400%"] {
        assert!(
            !cropped.iter().any(|text| text == preset),
            "`{preset}` is cropped in the list: {cropped:?}"
        );
    }
    // **And flush with the field's left edge**, not indented for an icon column that nothing in
    // a list of percentages could ever fill. `azur::MenuItem::gutter` is the rule; without it
    // every entry here sits 24 points in, which reads as an indent with no cause.
    let fit = drawn
        .iter()
        .find(|(_, text)| text == "Fit")
        .map(|(at, _)| *at)
        .expect("the list was drawn a moment ago");
    assert!(
        fit.x < panel_field_left(&h) + 16.0,
        "the list is indented for an icon column: label at {}, field at {}",
        fit.x,
        panel_field_left(&h)
    );

    // And it is *below the field*, stacked, rather than somewhere arbitrary.
    let entries: Vec<Pos2> = drawn
        .iter()
        .filter(|(_, text)| text == "Fit" || text == "400%")
        .map(|(at, _)| *at)
        .collect();
    assert_eq!(entries.len(), 2, "the two ends of the list were not both drawn");
    for entry in &entries {
        assert!(
            entry.y > at.y,
            "the list is not below the field: entry at {entry:?}, field at {at:?}"
        );
    }
    assert!(
        entries[1].y > entries[0].y,
        "the list is not stacked: {entries:?}"
    );

    // And the field lets go: a click on the canvas takes the keyboard back, so the window's
    // own shortcuts work again. Tested through `Ctrl+P`, which is one of the ones it was
    // swallowing.
    h.click_at(panel.center());
    assert!(
        h.ctx.memory(|m| m.focused()).is_none(),
        "the zoom field still has the keyboard"
    );
    h.take_journal();
    h.modifiers = Modifiers::COMMAND;
    h.frame(vec![Event::Key {
        key: egui::Key::P,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }]);
    h.modifiers = Modifiers::NONE;
    h.frame(Vec::new());
    assert_eq!(
        h.take_journal(),
        vec!["TogglePreview"],
        "the shortcut went to the field rather than the window"
    );
    crate::sandbox::remove(&root);
}

/// Where the zoom field starts, which is where its list is anchored.
///
/// Taken from the `%` the field draws rather than from the geometry: the field is laid out
/// right-to-left off the bar's controls, and re-deriving that here would be re-deriving the
/// thing under test.
fn panel_field_left(h: &Harness) -> f32 {
    h.texts()
        .into_iter()
        .filter(|(_, text)| text.ends_with('%'))
        .map(|(at, _)| at.x)
        .fold(f32::INFINITY, f32::min)
        - 8.0
}

/// **The panel follows the keyboard**, once the keyboard stops moving.
///
/// End to end through the real frame loop: the cursor lands on a binary, nothing happens for
/// a quarter of a second, and then a walk of *that* file is what the panel is showing. Which
/// is the whole of the gesture — open it once, then arrow down a folder and look at each file
/// in turn — and four separate things have to be right for it: the cursor being noticed, the
/// kind being worked out from the name, the wait, and the answer finding its way back to the
/// panel that asked.
///
/// The fixture is the test binary and the folder it is in, which is the one place on any
/// machine guaranteed to hold a real PE image.
#[test]
fn the_preview_panel_follows_the_keyboard() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let me = std::env::current_exe().expect("a test process has an executable");
    let folder = me.parent().expect("it is in a folder").to_path_buf();
    h.app
        .perform(&h.ctx.clone(), Action::Navigate { pane, path: folder });
    h.settle();

    let showing = |h: &Harness| {
        h.app.panes[0]
            .tab()
            .preview
            .showing()
            .map(|path| path.to_path_buf())
    };

    // The panel, open and pointed at nothing yet.
    h.app.panes[0].tab_mut().preview.open = true;
    h.app.panes[0].tab_mut().clear_selection();
    h.frame(Vec::new());

    // The keyboard onto the test binary, the way a click leaves it.
    let name = me
        .file_name()
        .expect("it has a name")
        .to_string_lossy()
        .into_owned();
    let at = {
        let tab = h.app.panes[0].tab();
        let dir = tab.dir.as_ref().expect("the listing arrived");
        tab.order
            .iter()
            .position(|&i| dir.name(i as usize) == name)
            .unwrap_or_else(|| panic!("{name} is not in its own folder's listing"))
    };
    h.app.panes[0].tab_mut().select_only(at);

    // Not yet. This is the assertion the wait exists for: holding an arrow key through a
    // folder of images must not decode thirty of them.
    h.frame(Vec::new());
    assert!(
        showing(&h).is_none() && h.app.preview_pending(),
        "the read started on the keystroke rather than waiting for it to stop"
    );

    // And then it does, on that file, once.
    h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
    for attempt in 0..400 {
        h.frame(Vec::new());
        if !h.app.preview_pending() && attempt > 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        showing(&h).as_deref(),
        Some(me.as_path()),
        "the panel is showing something else"
    );
    let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
    assert!(
        texts.iter().any(|text| text.contains("API set")),
        "the walk landed and the panel is not showing it: {texts:?}"
    );

    // **The keyboard moving to another row never leaves the last answer behind.** This panel is
    // *inside* the pane, so what it shows is read as being about the selection beside it — a
    // dependency tree left next to a `.rmeta` would be a lie about the `.rmeta`.
    //
    // The row is picked as one this program has no decoder for, which since
    // [`crate::preview::kind_of`] started sniffing those and then handing them to the shell is no
    // longer the same thing as "no preview": a `.d` is a `Kind::Shell`, so it is asked about and
    // comes back a moment later — as text, since a dependency file is text, and as
    // `Payload::Unsupported` for whatever else the row lands on. Either way the panel has *let go of
    // the binary*, and that — rather than the panel being empty — is what has to hold.
    let plain = {
        let tab = h.app.panes[0].tab();
        let dir = tab.dir.as_ref().expect("the listing");
        (0..tab.order.len()).find(|&row| {
            tab.entry_at(row).is_some_and(|entry| {
                !matches!(
                    crate::preview::kind_of(
                        dir.leaf(entry),
                        dir.ext(entry),
                        dir.entries[entry].is_dir(),
                    ),
                    Some(crate::preview::Kind::Binary)
                )
            })
        })
    }
    .expect("a build folder holds something that is not a PE image");
    h.app.panes[0].tab_mut().select_only(plain);
    h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
    for _ in 0..200 {
        h.frame(Vec::new());
        if !h.app.preview_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_ne!(
        showing(&h).as_deref(),
        Some(me.as_path()),
        "the panel kept a stale answer about the row the keyboard has left"
    );
    assert_eq!(
        h.app.panes[0].tab().preview.dependency_rows(),
        None,
        "the panel is still holding the binary's dependency tree beside a different row"
    );
}

/// **Everything in a dependency row is on one baseline.**
///
/// Three texts at two sizes — a 14-point name, a 12-point location, a 12-point processor
/// tag — and the assertion is *exact* rather than within a tolerance, because the property
/// is exact: the row commits to one `azur::components::ink_baseline` and every galley is
/// placed by subtracting its own ascent from it.
///
/// What it catches is the way this is usually written. Centring each galley in the row —
/// which is what every other listing in this window does — leaves the two 12-point columns
/// **1.5 points above** the name beside them, because the two fonts differ in line height
/// *and* in ascent and centring the boxes cancels neither. That is invisible in the source,
/// visible on screen as a column that steps down as the eye crosses the row, and this is the
/// test that would fail.
#[test]
fn everything_in_a_dependency_row_sits_on_one_line() {
    let mut h = Harness::new();
    open_preview(&mut h);

    // The panel's rows, which is everything drawn below its header. Taken from what was
    // painted rather than from the geometry, so this does not have to re-derive the layout.
    let panel = preview_rect(&h);
    // **Stopping short of the pane's status strip**, which is drawn over the bottom of the panel
    // and is not a dependency row. Two things live in those last points and neither is on a row's
    // baseline: the `1 / 1133 · 23.8 MB` line itself, and the row underneath it that the panel's
    // scroll area paints and then clips away. A band that reached the full height put the two in
    // the same bucket as the last visible row and compared their baselines — which is a failure
    // about a helper's arithmetic, not about a row. The same `bottom() - STATUS_HEIGHT` that
    // `click_tests::sizes` uses to find that strip.
    let rows_area = Rect::from_min_max(
        pos2(panel.left(), panel.top() + crate::ui::preview::HEADER),
        pos2(
            panel.right(),
            panel.bottom() - crate::ui::filelist::STATUS_HEIGHT,
        ),
    );
    let mut rows: std::collections::BTreeMap<i64, Vec<(f32, String)>> = Default::default();
    for (at, text) in h.baselines() {
        // By rect and not by a y threshold: the panel is *inside* a pane now, so the sidebar
        // and the listing have text at the same heights and only the x tells them apart.
        if !rows_area.contains(at) || text.is_empty() {
            continue;
        }
        // Which row it is in, from the baseline itself: rows are `ROW` apart, so anything
        // within one of them belongs to the same one.
        rows.entry(((at.y - rows_area.top()) / crate::ui::deps::ROW) as i64)
            .or_default()
            .push((at.y, text));
    }
    assert!(
        rows.len() >= 4,
        "the panel drew {} rows of text; there is nothing to compare",
        rows.len()
    );

    let mut widest = 0;
    for (which, texts) in &rows {
        let first = texts[0].0;
        widest = widest.max(texts.len());
        for (baseline, text) in texts {
            assert_eq!(
                *baseline, first,
                "row {which}: `{text}` sits on {baseline} and `{}` on {first}",
                texts[0].1
            );
        }
    }
    // And a row really did have all three columns in it, or the fonts never differed and
    // the assertion above proves nothing.
    assert!(
        widest >= 3,
        "no row had a name, a location and a tag in it: at most {widest} texts"
    );
}

/// **A video the machine cannot play says why, in the panel, and stops being busy.**
///
/// Driven with a file that is a `.mp4` in name and nonsense inside, which is the only video fixture
/// this repository can carry: encoding a real one needs an encoder, and shipping one would be
/// shipping a megabyte of somebody's footage to test a plumbing run.
///
/// Nonsense is enough to test the plumbing, and that is most of what there is to test here. This
/// exercises the whole chain in the running program — `kind_of` answering `Kind::Video`,
/// `collect_previews` opening a player instead of asking the read service, `MFStartup`, the D3D11
/// device, the Media Engine, the path-to-URL conversion, Media Foundation's resolver refusing the
/// file, the `IMFMediaEngineNotify` callback on one of its worker threads, the channel back, and the
/// panel picking the word up on a later frame. Every one of those is a place this can break, and a
/// broken one of them looks the same from here as a missing codec does — which is why the assertion
/// is on the *complaint*: a chain that failed to run at all leaves the panel silent.
///
/// What it deliberately does not test is playback: whether a frame comes out, whether the clock
/// moves, whether the scrubber lands where it was dragged. That needs a real file with a real codec
/// behind it, and it is the gap in this feature's coverage worth knowing about.
#[cfg(windows)]
#[test]
fn a_video_the_machine_cannot_play_says_so_in_the_panel() {
    let dir = crate::sandbox::fresh("preview-video");
    let broken = dir.join("broken.mp4");
    // Long enough that the resolver reads it rather than refusing an empty file, and nothing a
    // container parser will recognise.
    std::fs::write(&broken, vec![0x5Au8; 64 * 1024]).expect("a file in the sandbox");

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: dir.clone(),
        },
    );
    h.settle();

    let at = {
        let tab = h.app.panes[0].tab();
        let listing = tab.dir.as_ref().expect("the listing arrived");
        tab.order
            .iter()
            .position(|&i| listing.name(i as usize) == "broken.mp4")
            .expect("the fixture is in its own folder's listing")
    };
    h.app.panes[0].tab_mut().select_only(at);
    h.app.panes[0].tab_mut().preview.open = true;
    h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;

    // The panel takes the player on the frame after the selection settles, and the engine's refusal
    // arrives on one of its own threads some frames later.
    let mut said = None;
    for _ in 0..400 {
        h.frame(Vec::new());
        if let Some((why, _)) = h.app.panes[0].tab().preview.video() {
            if why.is_some() {
                said = why;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    // It is the video view and not the shell's still: a `.mp4` used to go to `visual::load`, and a
    // regression to that would show up here as `None` rather than as a wrong sentence.
    assert!(
        h.app.panes[0].tab().preview.video().is_some(),
        "the panel is not showing the video view for a .mp4"
    );
    let said = said.expect(
        "the player never complained: either Media Foundation accepted 64 KB of `Z` or the chain \
         from the engine's callback back to the panel is broken",
    );
    assert!(
        said.ends_with("video"),
        "the complaint reads as {said:?}, which is not one of the sentences `windows::video` writes"
    );
    // **And it stops holding a capture open.** `--shot --preview` waits on `preview_pending`, so a
    // player that stayed busy after failing would make a screenshot of a video folder wait out its
    // whole patience and then photograph the complaint anyway.
    assert!(
        !h.app.preview_pending(),
        "a player that has already failed is still reported as busy"
    );

    crate::sandbox::remove(&dir);
}

/// **A file with no preview offers the views it could have, and one of them is a real click away.**
///
/// The whole chain, driven the way a person drives it: the classifier answering `Shell` for an
/// extension nobody has heard of, the shell having no visualizer either, the panel drawing
/// `No preview for a .nosuchthing` with a button under it, the button being where the pointer can
/// reach it, and the pick coming back as a read of the same file as something else.
///
/// **The button's rect is asked of the frame that drew it** rather than derived here. Both halves
/// matter: a test that recomputed the layout would agree with a wrong layout, and one that hard-coded
/// a y would fail the next time the sentence changed length. `read_response` is what the harness's own
/// `hovers` uses, so this is the same question — did the pointer land on the widget — asked once.
#[test]
fn a_file_with_no_preview_offers_to_show_it_another_way() {
    let dir = crate::sandbox::fresh("preview-chooser");
    // An extension no machine has a thumbnail provider for, holding something that is plainly text.
    // Both halves are the fixture: the name is what makes the panel say it has nothing, and the
    // contents are what makes `Text` the right answer once it is asked for.
    let odd = dir.join("notes.nosuchthing");
    let body = "the bytes of this file are words after all";
    std::fs::write(&odd, body).expect("a file in the sandbox");

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: dir.clone(),
        },
    );
    h.settle();

    let at = {
        let tab = h.app.panes[0].tab();
        let listing = tab.dir.as_ref().expect("the listing arrived");
        tab.order
            .iter()
            .position(|&i| listing.name(i as usize) == "notes.nosuchthing")
            .expect("the fixture is in its own folder's listing")
    };
    h.app.panes[0].tab_mut().select_only(at);
    h.app.panes[0].tab_mut().preview.open = true;
    h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
    for attempt in 0..400 {
        h.frame(Vec::new());
        if !h.app.preview_pending() && attempt > 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!h.app.preview_pending(), "the read never came back");

    let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
    assert!(
        texts.iter().any(|text| text == "No preview for a .nosuchthing"),
        "the panel is not in the state this test is about: {texts:?}"
    );

    // The button, where the frame put it.
    let button = Id::new(("preview-as", crate::ui::preview::Spot::tile(pane, 0), "Text"));
    let rect = h
        .ctx
        .read_response(button)
        .map(|response| response.rect)
        .expect("no `Text` button was drawn under the sentence");
    assert!(
        crate::ui::preview::split(pane_body(&h), true, h.app.preview)
            .1
            .expect("the panel has room")
            .contains(rect.center()),
        "the button was drawn outside the panel it belongs to"
    );
    h.click_at(rect.center());

    // And the file comes back as text: the read is a fresh one for the same path, so this waits the
    // way the first one did. No `FOLLOW_DELAY` — a click is not a selection and has nothing to settle.
    for _ in 0..400 {
        h.frame(Vec::new());
        if !h.app.preview_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
    assert!(
        texts.iter().any(|text| text.contains(body)),
        "the pick did not become a text view of the file: {texts:?}"
    );
    // The sentence is gone with it, which is the half that says the panel *replaced* what it was
    // showing rather than drawing a view under it.
    assert!(
        !texts
            .iter()
            .any(|text| text.starts_with("No preview for")),
        "the panel is showing the file as text and still saying it cannot: {texts:?}"
    );

    crate::sandbox::remove(&dir);
}

/// **A screen filled by a video that is not there comes back by itself.**
///
/// The way out that nobody presses, and it is not a corner case: closing the panel, switching tabs and
/// a background scan moving the selection all reach it. Without it the window is left filling the
/// monitor with nothing in it and no way out but the keyboard.
///
/// It also pins the settings-file guard, which is the bug this feature is one line away from at all
/// times: a window filling the screen reports itself as neither maximised nor resized, so a frame
/// drawn in that state will happily record the *monitor's* size as the window's — and that reopens
/// the program filling the screen, for ever, out of a file the user edits by hand.
///
/// **The window itself is not asserted on**, and cannot be: `win::fill_screen` moves a real `HWND`
/// with `SetWindowPos`, and this harness runs the application against a context with no window behind
/// it. What is testable from here is the state the drawing reads and the file that outlives the
/// session, which is where the two failures that survive a restart live. The move itself is one
/// `SetWindowPos` per transition and its reasoning is written out where it happens.
#[test]
fn a_screen_filled_by_a_video_that_is_not_there_comes_back_by_itself() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    let size_before = h.app.window_size_for_tests();

    h.app.perform(&h.ctx.clone(), Action::ToggleVideoFullscreen(pane));
    // The flag straight away, because it is what the *next* frame reads to decide what to draw.
    assert!(
        h.app.fullscreen_for_tests(),
        "the action did not put the window into fullscreen"
    );

    // One frame is enough: `App::theatre` finds no video in this pane, draws nothing, and queues the
    // way out, which `apply` performs at the end of the same frame.
    h.frame(Vec::new());
    assert!(
        !h.app.fullscreen_for_tests(),
        "the window is still filling the screen over a pane with no video in it"
    );
    assert_eq!(
        h.app.window_size_for_tests(),
        size_before,
        "the monitor's size was recorded as the window's during the fullscreen frame, which is what \
         would reopen the program filling the screen"
    );
}

/// **Every tile's clip plays, and only the focused one is audible.**
///
/// The rule for four videos at once: silence for three of them would make the panel a still contact
/// sheet, and sound from four would make it unusable. So they all run and the focused tile has the
/// sound — see [`crate::ui::preview::show`], which asserts that every frame rather than only when a
/// player is opened, because the focus moves under a click.
///
/// Driven with two files that are `.mp4` in name and nonsense inside — the same fixture
/// [`a_video_the_machine_cannot_play_says_so_in_the_panel`] uses, and for the same reason. A player
/// that will not decode is still a player, and `muted` is a field on it either way, so this tests the
/// routing of the sound without needing a codec or a real clip.
#[cfg(windows)]
#[test]
fn every_tile_plays_and_only_the_focused_one_has_the_sound() {
    let dir = crate::sandbox::fresh("preview-video-tiles");
    for name in ["one.mp4", "two.mp4"] {
        std::fs::write(dir.join(name), vec![0x5Au8; 64 * 1024]).expect("a file in the sandbox");
    }

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: dir.clone(),
        },
    );
    h.settle();

    // Both clips selected, which is what asks for two tiles.
    let rows = {
        let tab = h.app.panes[0].tab();
        let listing = tab.dir.as_ref().expect("the listing arrived");
        let find = |want: &str| {
            tab.order
                .iter()
                .position(|&i| listing.name(i as usize) == want)
                .expect("the fixture is in its own folder's listing")
        };
        (find("one.mp4"), find("two.mp4"))
    };
    h.app.panes[0].tab_mut().select_only(rows.0);
    h.app.panes[0].tab_mut().toggle(rows.1);
    h.app.panes[0].tab_mut().preview.open = true;

    // The panel takes its players a frame after the selection settles, and the settle needs the clock
    // to have moved past the debounce — so this waits for both rather than assuming a frame count,
    // the same way the test above waits for the engine's refusal.
    let mut muted = Vec::new();
    for _ in 0..200 {
        h.time += crate::ui::preview::FOLLOW_DELAY;
        h.frame(Vec::new());
        muted = h.app.panes[0].tab().preview.videos_muted();
        if muted.len() == 2 {
            break;
        }
    }

    assert_eq!(
        h.app.panes[0].tab().preview.count(),
        2,
        "two selected clips did not become two tiles"
    );
    assert_eq!(
        muted,
        vec![false, true],
        "the focused tile is not the one with the sound"
    );

    // **And it is written once and then left alone**, which is the assertion this test was missing and
    // the bug it now covers: two owners of the mute — a per-tile draw and a loop after the tiles —
    // settled on the right value every frame and rewrote it twice getting there, which is inaudible as
    // a wrong value and very audible as crackling. Ten frames with nothing changing must cost nothing.
    let before = h.app.panes[0].tab().preview.video_mute_writes();
    for _ in 0..10 {
        h.time += 0.05;
        h.frame(Vec::new());
    }
    assert_eq!(
        h.app.panes[0].tab().preview.video_mute_writes(),
        before,
        "the mute is being rewritten on a frame where nothing changed, which is the crackle"
    );

    // The focus moving takes the sound with it — a tile that kept the sound after losing the focus
    // would mean two audible clips the moment a third was opened.
    h.app.panes[0].tab_mut().preview.focus_on(1);
    h.frame(Vec::new());
    assert_eq!(
        h.app.panes[0].tab().preview.videos_muted(),
        vec![true, false],
        "the sound did not follow the focus"
    );
}
