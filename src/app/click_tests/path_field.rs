//! The path field, its completion dropdown, and the other text fields in the window.

use super::*;

/// Opening a rename leaves the name exactly where it was.
///
/// It used to jump two points right and one point down — a field brings its own padding and
/// its own idea of where a line sits — and on the one word you are looking at that reads as a
/// flinch. Measured here the same way it was found: the position of the *text* in the frame
/// before and the frame after, which is the thing the eye is complaining about. Asserting on
/// the field's rect instead would only be checking this test's own arithmetic.
#[test]
fn opening_a_rename_does_not_move_the_name() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;

    let name = {
        let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
        tab.select_only(0);
        let dir = tab.dir.clone().expect("a listing");
        let entry = tab.entry_at(0).expect("a first row");
        dir.name(entry).to_owned()
    };
    h.frame(Vec::new());

    let find = |h: &Harness, what: &str| {
        h.texts()
            .into_iter()
            .find(|(_, text)| text == &name)
            .map(|(at, _)| at)
            .unwrap_or_else(|| panic!("`{name}` was not drawn {what}"))
    };
    let before = find(&h, "in the listing");

    h.app.perform(&h.ctx.clone(), Action::BeginRename(pane));
    h.frame(Vec::new());
    assert!(
        h.tab(0).renaming.is_some(),
        "the rename did not open, so this test proves nothing"
    );
    let after = find(&h, "in the rename field");

    assert_eq!(
        after, before,
        "the name moved by {:?} when the rename opened",
        after - before
    );
}

/// Arriving in a text field selects what is in it, so the next keystroke replaces it.
///
/// Asserted by *typing* rather than by reading egui's cursor state, for two reasons. The
/// selection lives in `TextEdit`'s state under an id the design system generates internally,
/// so there is nothing to read from out here without the library handing it over. And "the
/// text is selected" is not the point — "one keystroke replaces the path" is, and that is a
/// claim about what happens when you type, which is a thing a test can do.
#[test]
fn arriving_in_a_field_selects_what_is_there() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;

    // ---- The path field ------------------------------------------------
    {
        let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
        tab.editing_path = true;
        tab.edit_text = r"C:\Windows\System32".to_owned();
    }
    // It asks for focus itself on the frame it opens; two frames for egui to grant it and
    // for the field to see it arrive.
    h.frame(Vec::new());
    h.frame(Vec::new());
    h.frame(vec![Event::Text("D".to_owned())]);
    assert_eq!(
        h.tab(0).edit_text,
        "D",
        "typing into a freshly opened path field appended instead of replacing"
    );
    h.app.pane_mut(pane).expect("the pane").tab_mut().editing_path = false;
    h.frame(Vec::new());

    // ---- The filter, reached by clicking it ----------------------------
    {
        let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
        tab.filter = "old".to_owned();
    }
    h.settle();
    // Found by sweeping the bar rather than by deriving the box's rect: a text field is what
    // asks for `CursorIcon::Text`, and coming in from the right edge the filter is the first
    // thing that does. (The empty part of the breadcrumb asks for it too, and is further
    // left.)
    //
    // The whole run of it, and then the middle — not the first point that answered. The
    // clearable ✕ sits at the right-hand end of the box and is registered *after* the text
    // area, so it wins the pointer there; clicking the edge of the run emptied the filter
    // instead of typing into it, which is a real click target doing its real job.
    let y = h.path_bar_y(0);
    let right = h.pane_rect(0).right();
    let mut run: Vec<f32> = Vec::new();
    for dx in (8..400).step_by(2) {
        let at = pos2(right - dx as f32, y);
        h.frame(vec![Event::PointerMoved(at)]);
        if h.cursor == egui::CursorIcon::Text {
            run.push(at.x);
        } else if !run.is_empty() {
            break;
        }
    }
    assert!(!run.is_empty(), "the filter box is not reachable by the pointer");
    let at = pos2((run[0] + run[run.len() - 1]) * 0.5, y);
    h.click_at(at);
    h.frame(vec![Event::Text("n".to_owned())]);
    assert_eq!(
        h.tab(0).filter,
        "n",
        "clicking into the filter and typing appended instead of replacing"
    );
}

/// Open the path field on `pane` with `text` in it, and run frames until the completion has
/// worked out what to offer.
///
/// The waiting is the point: the folder being completed in comes from [`crate::loader`], on a
/// worker, so the offers are not there on the frame the text lands. A fixed frame count is how
/// this suite used to be flaky about scans — see `Harness::settle`.
fn open_path_field(h: &mut Harness, pane: PaneId, text: &str) {
    {
        let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
        tab.editing_path = true;
        tab.edit_text = text.to_owned();
    }
    for attempt in 0..200 {
        h.frame(Vec::new());
        if attempt >= 2 && !h.app.complete.offering().0.is_empty() {
            return;
        }
        if attempt >= 2 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    panic!("the path field never offered anything for `{text}`");
}

/// **The completion gesture, driven by the keys that make it.**
///
/// Every claim in [`crate::ui::breadcrumb::PathComplete`]'s documentation that a test can
/// reach: the offers are matched without regard to case, nothing is highlighted and no
/// dropdown is up until an arrow is pressed, `Down` does both at once, `Right` appends the
/// name *and the separator*, and what is offered after that is what is inside the folder just
/// named — which is the whole `Down Right Down Right` walk.
///
/// Driven through `Harness::frame` rather than by calling the methods, because the interesting
/// half is not the state machine: it is whether the keys ever reach it. `Down`, `Up`, `Right`
/// and `Tab` all mean something to a focused `TextEdit` and to egui's focus machinery, and a
/// test that skipped the field would pass against a bar where the arrows only moved the caret.
#[test]
fn the_path_field_completes_what_is_typed_into_it() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    // This pane's own folder, which is therefore already in the loader's cache — the case a
    // real `Ctrl+L` starts from.
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // A capital `S` against a folder called `src`, which is the case-insensitivity claim.
    open_path_field(&mut h, pane, &format!("{}\\S", here.display()));
    let (offers, hot) = h.app.complete.offering();
    assert_eq!(
        offers, ["src"],
        "the completion offered the wrong folders for `S`"
    );
    assert_eq!(hot, None, "something was highlighted before an arrow was pressed");
    assert!(
        !h.app.complete.showing(),
        "the dropdown came up on its own -- `Ctrl+L` fills the field with where you already \
         are, and a list of that is a list in the way"
    );

    // Down: the dropdown, and an offer under the keyboard, in one press.
    h.frame(tap(egui::Key::ArrowDown));
    assert!(h.app.complete.showing(), "Down did not open the dropdown");
    assert_eq!(
        h.app.complete.offering().1,
        Some(0),
        "Down opened the dropdown without landing on anything, so every offer costs two presses"
    );

    // Right: the name, and the separator that starts the next one.
    h.frame(tap(egui::Key::ArrowRight));
    let walked = format!("{}\\src\\", here.display());
    assert_eq!(
        h.tab(0).edit_text, walked,
        "Right did not put the highlighted name in the field"
    );

    // And the caret went with it. Typing is the only honest way to ask: the caret lives in
    // `TextEdit`'s own memory under an id the design system generates internally, and "the
    // next keystroke lands at the end" is the claim that matters.
    h.frame(vec![Event::Text("u".to_owned())]);
    assert_eq!(
        h.tab(0).edit_text,
        format!("{walked}u"),
        "the caret stayed where the typing left it, so the next keystroke landed inside the \
         name that had just been completed"
    );

    // What is offered now is what is inside the folder that was just named -- the second half
    // of the walk. Backspace first, so the `u` is not narrowing it.
    h.frame(tap(egui::Key::Backspace));
    for attempt in 0..200 {
        h.frame(Vec::new());
        if attempt >= 2 && h.app.complete.offering().0.len() > 1 {
            break;
        }
        if attempt >= 2 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    let (offers, _) = h.app.complete.offering();
    assert!(
        offers.contains(&"ui") && offers.contains(&"shell"),
        "after Right the offers are still the old folder's: {offers:?}"
    );
}

/// **`Tab` completes and stays in the field**, which is the one key here that consuming is not
/// enough for.
///
/// egui decides whether a `Tab` moves the focus at the top of the frame, from the raw events,
/// before any widget has run — so the bar cannot take the key back, it has to have said in
/// advance that it wants it. That is `breadcrumb::keep_tab`, and this is the test of it: a
/// green assertion on the *text* alone would pass against a bar that completed the name and
/// then threw the field away in the same keystroke, which is not a completion anybody can use.
///
/// From a field that has only just opened, with the dropdown still down and nothing
/// highlighted, because that is where the reflex puts it: `Ctrl+L`, three letters, `Tab`.
#[test]
fn tab_completes_without_leaving_the_field() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    open_path_field(&mut h, pane, &format!("{}\\s", here.display()));
    assert!(
        !h.app.complete.showing(),
        "the dropdown is already up, so this proves nothing about Tab reaching past it"
    );

    h.frame(tap(egui::Key::Tab));
    assert_eq!(
        h.tab(0).edit_text,
        format!("{}\\src\\", here.display()),
        "Tab did not complete the one offer there was"
    );
    h.frame(Vec::new());
    assert!(
        h.tab(0).editing_path,
        "Tab completed and then handed the keyboard to the next widget, which closed the field"
    );
    // And it is still the field that has the keyboard, so the next keystroke is still a path.
    h.frame(vec![Event::Text("u".to_owned())]);
    assert_eq!(
        h.tab(0).edit_text,
        format!("{}\\src\\u", here.display()),
        "the field kept the keyboard but not the caret"
    );
}

/// **`Use / in path`, ticked in the path field's own menu.**
///
/// Three claims, and only the first is about the setting. The menu is *reachable* — a menu
/// nothing can open is one of the two failures only a driven frame can see, which is why this
/// gesture is real all the way through: right-click the field, find the entry by reading it,
/// click it. The path in the field turns over on the tick, which is the whole of what the
/// setting does and what somebody ticking it is looking at. And **the field survives it**: a
/// press in a popup is a press outside the field, so egui takes the keyboard off it, and the
/// frame that ticked would otherwise be the frame that closed the field and put the breadcrumb
/// back — with nothing on screen to show what the tick did. See `breadcrumb::edit_field`.
///
/// Then off again, because a setting that cannot be untied is not a setting, and because the
/// swap has to be exact in the direction nobody thinks about.
#[test]
fn the_path_fields_menu_switches_which_slash_it_writes() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();

    assert!(!h.app.forward_slashes, "`\\` is what a fresh profile shows");
    // `Ctrl+L`'s own path into the field, so what it is prefilled with is under test too.
    h.app.perform(&ctx, Action::EditPath(pane));
    h.frame(Vec::new());
    h.frame(Vec::new());
    let filled = h.tab(0).edit_text.clone();
    assert!(
        filled.contains('\\') && !filled.contains('/'),
        "the field opened with the wrong separator: {filled}"
    );

    // The field's rect, taken from the hover-only anchor that shares it — the `TextEdit` inside
    // a `TextField` has an id the design system generates, and there is nothing to read it by
    // from out here.
    let field = h
        .ctx
        .read_response(Id::new(("crumb-complete", pane)))
        .map(|r| r.rect)
        .expect("the path field was not laid out");
    h.click_with(field.center(), PointerButton::Secondary, Modifiers::NONE);

    let entry = |h: &Harness, label: &str| {
        h.texts()
            .into_iter()
            .find(|(_, text)| text == label)
            .map(|(at, _)| at)
    };
    let tick = entry(&h, "Use / in path").unwrap_or_else(|| {
        panic!(
            "the field's menu did not open, or has no slash entry in it: {:?}",
            h.texts().into_iter().map(|(_, t)| t).collect::<Vec<_>>()
        )
    });
    // A couple of points into the label, which is inside the entry whatever its padding is.
    let done = h.click_at(pos2(tick.x + 2.0, tick.y + 6.0));
    assert!(
        done.contains(&"SetForwardSlashes"),
        "clicking `Use / in path` did nothing, got {done:?}"
    );
    assert!(h.app.forward_slashes, "the entry did not turn it on");
    // A setting, so it is part of what the window writes down. Asked of `settings()` rather
    // than of `config_dirty`, which the frame after the one that sets it has already cleared.
    assert!(h.app.settings().forward_slashes, "and it was not written down");

    assert!(
        h.tab(0).editing_path,
        "the tick closed the field, so there is nothing left on screen to show what it did"
    );
    let turned = h.tab(0).edit_text.clone();
    assert_eq!(
        turned,
        filled.replace('\\', "/"),
        "the path in the field did not turn over on the tick"
    );
    // The dropdown is *not* what a tick brings up: the text changed without anybody typing, and
    // a list of the folder you are standing in is the list `Ctrl+L` is careful not to show.
    assert!(
        !h.app.complete.showing(),
        "the completion came up on its own after the tick"
    );
    // And the keyboard came back, so the field is still a field. Whether what is in it is
    // selected is the design system's business — an `x` landing in it at all is the claim.
    h.frame(vec![Event::Text("x".to_owned())]);
    assert!(
        h.tab(0).edit_text.contains('x'),
        "the field kept its text but not the keyboard: {:?}",
        h.tab(0).edit_text
    );

    // Off again, from a field reopened on the same folder — which is now filled with `/`,
    // the other half of what the setting is for.
    h.app.perform(&ctx, Action::EditPath(pane));
    h.frame(Vec::new());
    h.frame(Vec::new());
    assert_eq!(
        h.tab(0).edit_text, turned,
        "the setting did not survive the field being reopened"
    );
    h.click_with(field.center(), PointerButton::Secondary, Modifiers::NONE);
    let tick = entry(&h, "Use / in path").expect("the menu did not open a second time");
    h.click_at(pos2(tick.x + 2.0, tick.y + 6.0));
    assert!(!h.app.forward_slashes, "the entry does not untick");
    assert_eq!(
        h.tab(0).edit_text, filled,
        "turning it off did not put the path back the way Windows writes it"
    );
}

/// **A click away from the bar closes the field even with the field's menu open**, which is the
/// other half of the rule the tick above rests on.
///
/// The two pull in opposite directions from one frame. `egui::Popup` decides to close *after* its
/// body has run, so the frame a click outside the popup lands on is a frame the popup is still
/// open for — the same state a tick leaves behind, and the field is forgiving its focus loss in
/// exactly one of the two cases. Getting that wrong does not show up as a menu that misbehaves:
/// it shows up here, as a path field welded over the breadcrumb with the keyboard, over a
/// listing that has just been clicked in. Which is why this sits beside the test above rather
/// than inside it.
#[test]
fn a_click_past_the_path_fields_menu_still_closes_the_field() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();

    h.app.perform(&ctx, Action::EditPath(pane));
    h.frame(Vec::new());
    h.frame(Vec::new());
    let field = h
        .ctx
        .read_response(Id::new(("crumb-complete", pane)))
        .map(|r| r.rect)
        .expect("the path field was not laid out");
    h.click_with(field.center(), PointerButton::Secondary, Modifiers::NONE);
    assert!(
        h.texts().iter().any(|(_, text)| text == "Use / in path"),
        "the menu is not open, so this proves nothing"
    );

    // The listing, well clear of both the field and the menu the right click put up.
    h.click_at(h.row_center(0, 4));
    assert!(
        !h.tab(0).editing_path,
        "the field stayed open with the keyboard, over a listing that had just been clicked in"
    );
}

/// The keyboard's highlight is a band across the highlighted offer, in the fill a row under
/// the pointer wears — and it moves with the highlight.
///
/// `MenuItem` has no keyboard state of its own: it lights up hovered or focused, and focusing a
/// row here would take the keyboard off the field being typed into. So the fill is painted
/// *behind* the row, into a slot reserved before the row is added, and this is the test that it
/// lands on the row rather than under it. The colour is asserted too, because two different
/// greys for one meaning would read as two different things.
///
/// The band is found by shape and the *move* is measured between two presses, rather than
/// either being checked against a computed row position — a test that re-derives where the
/// layout put something is a test of its own arithmetic, and the offers' own names are no help
/// here: the listing behind the dropdown is showing most of the same folders.
///
/// Frames are run out before the shapes are read, because a popup fades in. Sampled three
/// frames after it opens, every colour in it comes back part transparent.
#[test]
fn the_highlight_is_a_band_that_follows_the_keyboard() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // A folder with several to choose from, so a band across the whole dropdown rather than
    // across one row of it comes out as the wrong height.
    open_path_field(&mut h, pane, &format!(r"{}\", here.display()));

    let hover = h.app.theme.bg.control_hover;
    let row = azur::components::menu_item_height();
    let band = |h: &Harness| -> Rect {
        let found: Vec<Rect> = h
            .rects()
            .into_iter()
            .filter(|(rect, _, fill)| {
                *fill == hover && (rect.height() - row).abs() < 0.5 && rect.width() > 200.0
            })
            .map(|(rect, _, _)| rect)
            .collect();
        assert_eq!(
            found.len(),
            1,
            "expected one highlight band the height of a menu row, found {}: {found:?}",
            found.len()
        );
        found[0]
    };

    h.frame(tap(egui::Key::ArrowDown));
    for _ in 0..12 {
        h.frame(Vec::new());
    }
    assert!(
        h.app.complete.offering().0.len() >= 3,
        "not enough offers to tell a row from a list: {:?}",
        h.app.complete.offering().0
    );
    assert_eq!(h.app.complete.offering().1, Some(0));
    let first = band(&h);

    h.frame(tap(egui::Key::ArrowDown));
    for _ in 0..12 {
        h.frame(Vec::new());
    }
    assert_eq!(h.app.complete.offering().1, Some(1));
    let second = band(&h);

    assert_eq!(
        second.left(),
        first.left(),
        "the band moved sideways between two offers"
    );
    assert!(
        (second.top() - first.top() - row).abs() < 0.5,
        "the band moved by {} between the first offer and the second, and a row is {row}",
        second.top() - first.top()
    );
}

/// **The dropdown holds ten offers and not eleven**, which is `breadcrumb::OFFERS_SHOWN` and
/// the reason it is a whole number of rows.
///
/// Asserted on the dropdown's *height* rather than by counting the names in the frame, and the
/// difference is the whole point of the constant: a scroll area lays out the row past its
/// bottom edge and clips it, so eleven names are in the shape list either way and only ten of
/// them have any pixels. Height is what a reader sees, and bracketing it — ten whole rows fit,
/// eleven do not — says the ceiling lands between two rows without this test having to know
/// what the frame adds around them.
///
/// Fourteen folders in a sandbox rather than a real one deep enough to overflow, so the count
/// does not depend on what is installed on the machine running the suite.
#[test]
fn the_dropdown_holds_ten_offers_and_scrolls_the_rest() {
    let root = crate::sandbox::dir("offers");
    crate::sandbox::remove(&root);
    let names: Vec<String> = (1..=14).map(|i| format!("folder-{i:02}")).collect();
    for name in &names {
        std::fs::create_dir_all(root.join(name)).expect("a directory in the temp folder");
    }

    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;

    // The sandbox is not the folder either pane is showing, so this is also the case where the
    // completion has to wait on the loader for a folder nothing has read yet.
    open_path_field(&mut h, pane, &format!("{}\\", root.display()));
    // A field filled from outside keeps its dropdown down until a key asks for it, exactly as a
    // `Ctrl+L` does. `Down` is that key, and it leaves the list unscrolled at its first row.
    h.frame(tap(egui::Key::ArrowDown));
    for _ in 0..8 {
        h.frame(Vec::new());
    }
    let (offers, hot) = h.app.complete.offering();
    assert_eq!(
        offers.len(),
        names.len(),
        "every folder in the sandbox should be on offer: {offers:?}"
    );
    assert_eq!(hot, Some(0), "the list should be sitting at its first row");

    let height = dropdown_rect(&h, pane).height();
    let row = azur::components::menu_item_height();
    let shown = crate::ui::breadcrumb::OFFERS_SHOWN as f32;
    assert!(
        height >= shown * row,
        "the dropdown is {height} tall and {shown} rows of {row} do not fit in it"
    );
    assert!(
        height < (shown + 1.0) * row,
        "the dropdown is {height} tall, which is room for more than {shown} rows -- the              ceiling has to land between two of them or the last one comes out half drawn"
    );
}

/// The completion dropdown's own frame, found by the surface `menu_frame` paints it in.
///
/// By its three colour channels only: a popup fades in, and the last step of that fade lands on
/// 254 of 255 rather than on opaque, so an equality that counted the alpha would find nothing.
/// Anchored on the widget that the dropdown hangs off rather than on a computed rect -- a test
/// that re-derives where the layout put something is a test of its own arithmetic.
fn dropdown_rect(h: &Harness, pane: PaneId) -> Rect {
    let field = h
        .ctx
        .read_response(Id::new(("crumb-complete", pane)))
        .expect("the path field's dropdown anchor")
        .rect;
    let surface = h.app.theme.bg.layer_alt.to_array();
    let found: Vec<Rect> = h
        .rects()
        .into_iter()
        .filter(|(rect, _, fill)| {
            fill.to_array()[..3] == surface[..3]
                && fill.a() > 200
                && rect.top() >= field.bottom()
                && (rect.width() - field.width()).abs() < 8.0
        })
        .map(|(rect, _, _)| rect)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected one dropdown under the field, found {}: {found:?}",
        found.len()
    );
    found[0]
}

/// **A dropdown that was short stays short — the bug this is really about.**
///
/// Typing `d:/` showed two rows where there were fifteen folders to show. An `Area` hands its
/// content last frame's size as this frame's room, and a `ScrollArea` fits itself into whatever
/// room it is given without asking for more, so the two lock each other: `d` offers one drive,
/// the popup becomes one row tall, and the fifteen folders that `d:/` turns up then shrink to
/// fit the one row of room that is left. It never recovers, because nothing ever asks it to.
///
/// So this drives the list *through* a small one, with real keystrokes, and then asks how tall
/// it is. Setting the text wholesale would not reproduce it — the first list would already be
/// the long one, which is exactly why every capture of this looked right.
#[test]
fn a_dropdown_that_was_short_grows_back() {
    let root = crate::sandbox::dir("grow");
    crate::sandbox::remove(&root);
    let names: Vec<String> = (1..=14).map(|i| format!("folder-{i:02}")).collect();
    for name in &names {
        std::fs::create_dir_all(root.join(name)).expect("a directory in the temp folder");
    }

    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;

    // One offer: `folder-01` matches only itself.
    open_path_field(&mut h, pane, &format!(r"{}\folder-01", root.display()));
    h.frame(tap(egui::Key::ArrowDown));
    for _ in 0..8 {
        h.frame(Vec::new());
    }
    assert_eq!(
        h.app.complete.offering().0.len(),
        1,
        "this has to start from a one-row dropdown or it proves nothing"
    );
    let short = dropdown_rect(&h, pane).height();

    // `End` first: a field selects what it holds when focus arrives, so a `Backspace` here
    // would take the whole path out rather than one character of the name.
    h.frame(tap(egui::Key::End));
    // Two backspaces leave `folder-`, which every one of the fourteen matches.
    h.frame(tap(egui::Key::Backspace));
    h.frame(tap(egui::Key::Backspace));
    for _ in 0..8 {
        h.frame(Vec::new());
    }
    assert_eq!(
        h.app.complete.offering().0.len(),
        names.len(),
        "the offers did not grow, so the height cannot be what this is measuring"
    );

    let row = azur::components::menu_item_height();
    let shown = crate::ui::breadcrumb::OFFERS_SHOWN as f32;
    let grown = dropdown_rect(&h, pane).height();
    assert!(
        grown > short,
        "the dropdown stayed {short} tall after its list grew from 1 offer to {}",
        names.len()
    );
    assert!(
        grown >= shown * row,
        "the dropdown grew to {grown}, which is not room for {shown} rows of {row} -- it is              still wearing the size it had when the list was short"
    );
    assert!(
        grown < (shown + 1.0) * row,
        "the dropdown is {grown} tall, which is room for more than {shown} rows"
    );

    crate::sandbox::remove(&root);
}

/// `Enter` on a highlighted offer goes there, without asking the disk whether the text names
/// anything: the offer came out of a listing and carries its own path.
#[test]
fn enter_on_a_highlighted_completion_goes_there() {
    let mut h = Harness::new();
    h.settle();
    let pane = h.app.panes[0].id;
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    open_path_field(&mut h, pane, &format!("{}\\s", here.display()));
    h.frame(tap(egui::Key::ArrowDown));
    assert_eq!(h.app.complete.offering().1, Some(0), "nothing to press Enter on");

    h.take_journal();
    h.frame(tap(egui::Key::Enter));
    h.settle();
    assert_eq!(
        h.tab(0).path,
        here.join("src"),
        "Enter on the highlighted offer did not navigate to it"
    );
    assert!(
        !h.tab(0).editing_path,
        "the field is still open after going somewhere"
    );
}

/// Every text field in the window is square, and wears the fill it is supposed to.
///
/// All three at once, in one frame, because they do not come from one place: the rename field
/// is egui's `TextEdit` and takes its frame from `Style::interact`, while the path field and
/// the filter are Azur's `TextField` and took theirs from a hard-coded `control_radius()`
/// until the design system was changed to read the installed style too. One mechanism now —
/// `ui::squared` — but a test that checked only one of them would pass while the other stayed
/// a bubble.
///
/// Found by what a field *is* rather than by re-deriving where the layout put it — a test that
/// computes a field's rect is a test of its own arithmetic. `background-control` is the fill,
/// and that alone is not enough: in the dark theme it is the same `GRAY_4` as `stroke-subtle`,
/// so the block behind the panels, the path bar, the selected tab, the divider on the bar and
/// the drive gauges all match it too. Adding "no taller than a control, and wider than a line"
/// leaves exactly the three, and the count is asserted so that stops being true loudly.
#[test]
fn the_text_fields_are_square_and_wear_the_right_fill() {
    let mut h = Harness::new();
    h.settle();

    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();
    // The filter is up whenever the pane is wide enough, which it is. The other two have to
    // be opened.
    {
        let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
        tab.select_only(0);
        tab.filter = "r".to_owned();
        tab.editing_path = true;
    }
    h.app.perform(&ctx, Action::BeginRename(pane));
    h.frame(Vec::new());

    // Two fills, not one: the path field and the filter are `background-control`, and the
    // rename field is `background-layer` on purpose — it stands in for a row and has to look
    // like the panel rather than like a control dropped on top of one.
    let fills = [h.app.theme.bg.control, h.app.theme.bg.layer];
    let field_shaped = |rect: &Rect| {
        rect.height() >= 16.0
            && rect.height() <= azur::tokens::control::SMALL + 0.5
            && rect.width() >= 40.0
    };
    let fields: Vec<(Rect, egui::CornerRadius, egui::Color32)> = h
        .rects()
        .into_iter()
        .filter(|(rect, _, fill)| fills.contains(fill) && field_shaped(rect))
        .collect();
    assert_eq!(
        fields.len(),
        3,
        "expected the rename field, the path field and the filter, found {}: {:?}",
        fields.len(),
        fields.iter().map(|(r, _, _)| *r).collect::<Vec<_>>()
    );
    // All of them, not the first: the three come from two different widgets, and a failure
    // that names only one leaves you guessing whether the other is square or merely earlier
    // in the paint order.
    let rounded: Vec<Rect> = fields
        .iter()
        .filter(|(_, corner, _)| *corner != egui::CornerRadius::ZERO)
        .map(|(rect, _, _)| *rect)
        .collect();
    assert!(
        rounded.is_empty(),
        "{} of 3 text fields have rounded corners: {rounded:?}",
        rounded.len()
    );

    // And exactly one of them is the panel's own colour: the rename field, which stands in for
    // a row. `background-control` on that one drew a grey slab over the name.
    let on_panel = fields
        .iter()
        .filter(|(_, _, fill)| *fill == h.app.theme.bg.layer)
        .count();
    assert_eq!(
        on_panel, 1,
        "the rename field should be the only `background-layer` one of the three"
    );
}
