use super::*;
use crate::shell::menu::Own;

fn entry(label: &str) -> Entry {
    Entry {
        label: label.to_owned(),
        shortcut: String::new(),
        kind: Kind::Command(Command::Own(Own::CopyHere)),
        enabled: true,
        checked: false,
        icon: None,
    }
}

fn submenu(label: &str, children: Vec<Entry>) -> Entry {
    Entry {
        kind: Kind::complete(children),
        ..entry(label)
    }
}

/// A shell entry under a canonical verb, which is how Properties is picked out.
fn verb(label: &str, verb: &str) -> Entry {
    Entry {
        kind: Kind::Command(Command::Shell {
            verb: Some(verb.to_owned()),
            id: 0,
            path: Vec::new(),
            label: label.to_owned(),
        }),
        ..entry(label)
    }
}

fn divider() -> Entry {
    Entry {
        kind: Kind::Separator,
        ..entry("")
    }
}

/// A submenu row as the shell hands it over: known to be one, not yet asked about.
fn unfilled(label: &str, source: u32) -> Entry {
    Entry {
        kind: Kind::unfilled(source),
        ..entry(label)
    }
}

fn menu(entries: Vec<Entry>) -> Open {
    Open::new(
        1,
        pos2(100.0, 100.0),
        Vec::new(),
        std::path::PathBuf::from(r"C:\x"),
        entries,
        crate::shell::menu::Depth::Full,
        1,
    )
}

/// A pass over a menu, for the tests that need one drawn.
fn pass(open: &mut Open, screen: Rect, times: usize) -> egui::Context {
    let ctx = egui::Context::default();
    let theme = Theme::dark();
    let mut input = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    input.viewports.entry(egui::ViewportId::ROOT).or_default().inner_rect = Some(screen);
    for _ in 0..times {
        let _ = ctx.run_ui(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show(ui, &theme, open);
            });
        });
    }
    ctx
}

/// Opening an unfilled submenu asks for it once, by the id the shell handed over, and
/// the level appears when the answer does -- not before, and not by asking again every
/// frame.
///
/// The id is the part worth holding down. It was a path of entry indices, and that broke
/// the moment this program's own entries went above the shell's: the menu asked about
/// index 8 for a submenu the shell had filed under index 1, every lookup missed, and
/// every submenu in the program came back empty. Hence `unfilled("Send to", 41)` --
/// deliberately not 1, so a version that went back to computing the index from the tree
/// cannot pass.
#[test]
fn an_unfilled_submenu_is_asked_for_once_by_id_and_drawn_when_it_arrives() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
    let mut open = menu(vec![entry("Open"), unfilled("Send to", 41)]);

    // Nothing is asked for until it is opened.
    pass(&mut open, screen, 2);
    assert!(open.fills.is_empty(), "an unopened submenu was asked about");

    open.open = vec![1];
    pass(&mut open, screen, 3);
    assert_eq!(
        open.fills,
        vec![41],
        "opening it should have asked once, by the shell's id, across three frames"
    );
    // The caller sends it on; nothing more should accumulate.
    open.fills.clear();
    pass(&mut open, screen, 3);
    assert!(open.fills.is_empty(), "it was asked for twice");

    // The answer arrives and the level becomes drawable.
    assert!(open.level(&[1]).is_some_and(Vec::is_empty));
    open.filled(41, vec![entry("Desktop"), entry("Mail recipient")]);
    assert_eq!(open.level(&[1]).map(Vec::len), Some(2));
    assert!(open.entries[1].enabled);
}

/// An answer for a submenu nested inside another one finds its way in, and an id nobody
/// is holding changes nothing.
#[test]
fn a_fill_finds_its_submenu_at_any_depth() {
    let mut open = menu(vec![
        entry("Open"),
        submenu("More", vec![entry("Here"), unfilled("Deeper", 9)]),
    ]);
    open.filled(9, vec![entry("Bottom")]);
    assert_eq!(open.level(&[1, 1]).map(Vec::len), Some(1));

    // An id from a menu that has already gone is not going to match anything, and must
    // not overwrite whatever is holding a different one.
    open.filled(1234, vec![entry("Wrong")]);
    assert_eq!(open.level(&[1, 1]).map(Vec::len), Some(1));
    assert_eq!(open.entries.len(), 2);
}

/// An extension that really has nothing leaves the row there and inert, rather than
/// deleting it from under the pointer that is on it.
#[test]
fn a_submenu_that_fills_to_nothing_stops_being_usable() {
    let mut open = menu(vec![entry("Open"), unfilled("Nothing here", 3)]);
    open.filled(3, Vec::new());
    assert_eq!(open.entries.len(), 2, "the row stayed");
    assert!(!open.entries[1].enabled, "and stopped being usable");
    // Asked and answered: it must not be asked again.
    open.open = vec![1];
    pass(&mut open, Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), 2);
    assert!(open.fills.is_empty());
}

#[test]
fn the_measured_row_heights_are_the_ones_the_components_allocate() {
    // `measure` adds these up to place the menu before a single row is drawn, so a
    // number that drifts from what the design system allocates puts the menu in the
    // wrong place — and the taller the menu, the further out. It drifted once already,
    // when the design system dropped its menu density from 36 points to 28, so this
    // asks the components rather than trusting either arithmetic.
    let ctx = egui::Context::default();
    let mut taken = 0.0;
    let _ = ctx.run_ui(Default::default(), |ctx| {
        egui::Area::new(Id::new("probe")).show(ctx, |ui| {
            ui.set_width(240.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let top = ui.cursor().top();
            ui.add(MenuItem::new("one"));
            ui.add(MenuItem::new("two"));
            menu_divider(ui);
            taken = ui.cursor().top() - top;
        });
    });
    assert_eq!(taken, row_height() * 2.0 + separator_height());
}

#[test]
fn a_menu_longer_than_the_screen_is_capped_and_drawn_the_size_it_measured() {
    // What this holds down: a menu with more entries than the window has room for is
    // capped to the screen — without which the entries past the edge are simply
    // unreachable — and what gets drawn is the height that was measured, since the
    // position was computed from it.
    //
    // It is not proof against the whole class of failure. The real one — an `Area`
    // whose `Ui` reports an available height derived from the area's own size last
    // frame, so that a scroll area sizing itself from it settled at 400 points against
    // a measured 584 — reproduces in a real window and not in a pass driven from here;
    // it was found by capturing the window and comparing. The fix is at the call site,
    // where the rows are given an explicit rect.
    let ctx = egui::Context::default();
    let theme = Theme::dark();
    let mut open = menu((0..12).map(|i| entry(&format!("entry {i}"))).collect());

    let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0));
    let mut input = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    input.viewports.entry(egui::ViewportId::ROOT).or_default().inner_rect = Some(screen);

    for _ in 0..4 {
        let _ = ctx.run_ui(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show(ui, &theme, &mut open);
            });
        });
    }

    let expected = measure(&ctx, &theme, &open.entries, screen);
    assert!(
        expected.y < row_height() * 12.0,
        "twelve rows should not fit in a 200-point window, or this proves nothing"
    );
    // Within the frame's own stroke.
    assert!(
        (open.drawn.y - expected.y).abs() <= 2.0,
        "measured {} and drew {}",
        expected.y,
        open.drawn.y
    );
    assert!(open.drawn.y <= screen.height(), "and it stays on screen");
}

/// What comes out of the scroll area and what stays under it.
#[test]
fn properties_and_the_divider_above_it_are_pinned_out_of_the_scrolling_part() {
    // A menu of this program's own entries has no Properties in it and nothing to pin,
    // and neither does any submenu.
    assert_eq!(
        pinned_from(&[entry("Copy here"), entry("Move here")]),
        2,
        "nothing was pinned, so the whole level scrolls"
    );

    // The shell's own menu. The divider comes with it: it belongs to the row below rather
    // than to whatever is above, and left behind it would scroll away from what it separates.
    let shell = vec![
        entry("Ouvrir"),
        entry("Renommer"),
        divider(),
        verb("Propriétés", "properties"),
    ];
    assert_eq!(pinned_from(&shell), 2);

    // Recognised by **verb** and not by label, which is why neither label here is the
    // English one: `properties` is the shell's own name for it on every Windows.
    assert_eq!(pinned_from(&[entry("Öffnen"), verb("Eigenschaften", "Properties")]), 1);

    // Without a divider it is the row on its own.
    assert_eq!(pinned_from(&[entry("Open"), verb("Properties", "properties")]), 1);

    // And anything an extension has put *below* it is pinned too, rather than Properties
    // being lifted out of the middle and re-hung at the bottom — the menu keeps the order
    // the shell gave it.
    let after = vec![
        entry("Open"),
        divider(),
        verb("Properties", "properties"),
        entry("Scan with something"),
    ];
    assert_eq!(pinned_from(&after), 1);
}

#[test]
fn a_capped_menu_keeps_its_pinned_tail_and_still_comes_out_the_size_it_measured() {
    // The split has to add up. The scroll area is handed the level's height *less* the
    // pinned tail's, so the two together are what `measure` said and the position computed
    // from it is still right. A tail left undrawn, or a height charged twice, is what this
    // catches — `drawn` is the rect the frame actually wrapped around the rows.
    let mut entries: Vec<Entry> = (0..14).map(|i| entry(&format!("entry {i}"))).collect();
    entries.push(divider());
    entries.push(verb("Propriétés", "properties"));
    let mut open = menu(entries);

    let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0));
    let ctx = pass(&mut open, screen, 4);

    let expected = measure(&ctx, &Theme::dark(), &open.entries, screen);
    assert!(
        expected.y < stack_height(&open.entries),
        "sixteen entries should not fit a 200-point window, or this proves nothing"
    );
    assert!(
        (open.drawn.y - expected.y).abs() <= 2.0,
        "measured {} and drew {}",
        expected.y,
        open.drawn.y
    );

    // And the scrolling part really is short of its own content, which is the situation
    // the pinned rows exist to escape: they are reachable while it is not.
    let pin = pinned_from(&open.entries);
    let scrollable = expected.y - space::S2 * 2.0 - stack_height(&open.entries[pin..]);
    assert!(
        scrollable < stack_height(&open.entries[..pin]),
        "the entries above Properties fit after all"
    );
}

/// Every string a frame painted, dug out of that frame's own shapes.
///
/// The frame's rather than the context's, because what is being asked is whether *this* frame
/// drew anything: a menu that was skipped leaves last frame's `drawn` and last frame's areas
/// behind it, and both of those answer yes.
fn painted(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, into);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// The arrow keys move the cursor *and* leave the menu on screen.
///
/// The reported bug: navigating with the arrows made the menu disappear for a frame. `keyboard`
/// returned `Outcome::Open` before a single level was drawn, so the frame that took the key put
/// nothing on screen — one press blinked the whole menu out and back, and an arrow held down
/// strobed it.
///
/// The second half is the trap that comes with the fix. Now that a key falls through to the
/// drawing, the *pointer* gets a say on the same frame — and `hovered()` is true for a pointer
/// that is merely resting there, on a menu that opened underneath it. Without the gate in
/// [`show`], the row the menu happened to open on top of takes the open chain straight back off
/// the keyboard and Right never gets a submenu open.
#[test]
fn the_arrow_keys_draw_the_menu_on_the_frame_they_move_it() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0));
    let ctx = egui::Context::default();
    let theme = Theme::dark();
    let mut base = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    base.viewports.entry(egui::ViewportId::ROOT).or_default().inner_rect = Some(screen);

    // One frame, and what it painted.
    let frame = |open: &mut Open, events: Vec<egui::Event>| -> Vec<String> {
        let mut input = base.clone();
        input.events = events;
        let out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = show(ui, &theme, open);
            });
        });
        painted(&out.shapes)
    };
    let press = |key: egui::Key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let entries = || {
        vec![
            entry("Open"),
            entry("Rename"),
            submenu("Send to", vec![entry("Desktop")]),
        ]
    };
    // The middle of the first row: the menu is at (100, 100) with `space-2` of padding and
    // `row::COMPACT` rows, so the first one is 104..132.
    let on_the_first_row = pos2(190.0, 118.0);

    // ---- The pointer really is on that row -------------------------------
    //
    // Which the rest of this depends on twice over, and neither an `Open` nor a shape says so
    // directly. What does is the hover doing its job: a submenu open beside a pointer that is
    // over a different row closes.
    let mut hovered = menu(entries());
    hovered.open = vec![2];
    for _ in 0..3 {
        frame(&mut hovered, vec![egui::Event::PointerMoved(on_the_first_row)]);
    }
    assert!(
        hovered.open.is_empty(),
        "the point the rest of this test uses is not on the menu's first row, so nothing below \
         proves anything"
    );

    // ---- An arrow key draws, rather than blanking the frame ---------------
    let mut open = menu(entries());
    let first = frame(&mut open, vec![egui::Event::PointerMoved(on_the_first_row)]);
    assert!(
        first.iter().any(|text| text == "Rename"),
        "the menu is not on screen even without a key: {first:?}"
    );

    let moved = frame(&mut open, vec![press(egui::Key::ArrowDown)]);
    assert_eq!(open.cursor.as_deref(), Some([0].as_slice()), "the key did nothing");
    assert!(
        moved.iter().any(|text| text == "Rename"),
        "the frame that took the arrow key drew no menu at all: {moved:?}"
    );

    // ---- And Right opens the submenu, on the frame it is pressed ----------
    frame(&mut open, vec![press(egui::Key::ArrowDown)]);
    let at_the_submenu = frame(&mut open, vec![press(egui::Key::ArrowDown)]);
    assert_eq!(open.cursor.as_deref(), Some([2].as_slice()), "three rows down");
    assert!(
        at_the_submenu.iter().any(|text| text == "Send to"),
        "the menu went away on the way down it: {at_the_submenu:?}"
    );

    let opened = frame(&mut open, vec![press(egui::Key::ArrowRight)]);
    assert_eq!(
        open.open,
        vec![2],
        "the pointer resting on the first row took the submenu straight back off the keyboard"
    );
    assert!(
        opened.iter().any(|text| text == "Desktop"),
        "the submenu was opened but the frame that opened it drew nothing of it: {opened:?}"
    );

    // ---- The pointer takes over again the moment it moves -----------------
    //
    // The gate is for a pointer that is sitting still, not one being used: a move onto the second
    // row closes the submenu that the keyboard opened, which is what a pointer is asking for.
    let second_row = pos2(on_the_first_row.x, on_the_first_row.y + 28.0);
    frame(&mut open, vec![egui::Event::PointerMoved(second_row)]);
    assert!(
        open.open.is_empty(),
        "the keyboard kept the submenu open against a pointer that had moved off it"
    );
}

/// A menu opens at its first entry, however the last one was left.
///
/// The reported bug: a `ScrollArea` keeps its offset in egui's memory under an id that
/// outlives the menu, so right-clicking a file, scrolling to the bottom of a long shell
/// menu and dismissing it left the *next* menu opening halfway down itself. Two `Open`s on
/// one `Context`, which is what a second right click is.
#[test]
fn a_reopened_menu_starts_at_the_top() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0));
    let ctx = egui::Context::default();
    let theme = Theme::dark();
    let mut base = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    base.viewports.entry(egui::ViewportId::ROOT).or_default().inner_rect = Some(screen);

    let run = |open: &mut Open, events: Vec<egui::Event>| {
        let mut input = base.clone();
        input.events = events;
        let _ = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show(ui, &theme, open);
            });
        });
    };
    // A scroll, with the pointer over the middle of where the menu lands, and then the
    // pointer parked off it. Both halves matter: egui hands a wheel event to a scroll area
    // over several frames rather than one, so the frames after it are what finish the
    // scroll — and they have to happen while the *first* menu is the one under the pointer,
    // or the tail of the gesture lands on the second one and this tests nothing.
    let scroll = |open: &mut Open| {
        run(
            open,
            vec![
                egui::Event::PointerMoved(pos2(150.0, 120.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, -200.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        run(open, vec![egui::Event::PointerMoved(pos2(390.0, 195.0))]);
        for _ in 0..30 {
            run(open, Vec::new());
        }
    };
    let long = || (0..16).map(|i| entry(&format!("entry {i}"))).collect::<Vec<_>>();

    let mut first = menu(long());
    for _ in 0..3 {
        run(&mut first, Vec::new());
    }
    scroll(&mut first);
    assert!(
        first.scrolled > 0.0,
        "the wheel did not scroll the menu, so nothing below this proves anything"
    );

    // The same window, a new menu: at the top, not where the last one was left.
    let mut again = menu(long());
    run(&mut again, Vec::new());
    assert_eq!(again.scrolled, 0.0, "it reopened part way down itself");

    // And the reset is once, on the frame it appears — a version that put the offset back
    // every frame would be a menu that cannot be scrolled at all.
    scroll(&mut again);
    assert!(again.scrolled > 0.0, "the reset kept firing and pinned it to the top");
}

#[test]
fn levels_and_entries_resolve_by_path() {
    let m = menu(vec![
        entry("one"),
        submenu("more", vec![entry("deep"), submenu("deeper", vec![entry("bottom")])]),
    ]);
    assert_eq!(m.entry(&[0]).unwrap().label, "one");
    assert_eq!(m.entry(&[1]).unwrap().label, "more");
    assert_eq!(m.entry(&[1, 0]).unwrap().label, "deep");
    assert_eq!(m.entry(&[1, 1, 0]).unwrap().label, "bottom");
    assert!(m.entry(&[9]).is_none());
    // A path through a leaf is not a path.
    assert!(m.entry(&[0, 0]).is_none());

    assert_eq!(m.level(&[]).unwrap().len(), 2);
    assert_eq!(m.level(&[1]).unwrap().len(), 2);
    assert!(m.level(&[0]).is_none());
}

#[test]
fn a_menu_that_would_run_off_the_right_flips() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
    let size = vec2(200.0, 300.0);

    // Room to the right: it hangs from the pointer.
    let anchor = Rect::from_min_size(pos2(100.0, 100.0), Vec2::ZERO);
    assert_eq!(place(anchor, size, screen, true), pos2(100.0, 100.0));

    // No room: it flips to the other side rather than sliding, so the pointer is not
    // left inside the menu it just opened.
    let anchor = Rect::from_min_size(pos2(950.0, 100.0), Vec2::ZERO);
    assert_eq!(place(anchor, size, screen, true), pos2(750.0, 100.0));
}

#[test]
fn a_menu_that_would_run_off_the_bottom_flips_up() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
    let size = vec2(200.0, 300.0);
    let anchor = Rect::from_min_size(pos2(100.0, 700.0), Vec2::ZERO);
    assert_eq!(place(anchor, size, screen, true), pos2(100.0, 400.0));
}

#[test]
fn a_menu_taller_than_the_screen_still_starts_on_it() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 400.0));
    let size = vec2(200.0, 900.0);
    let anchor = Rect::from_min_size(pos2(100.0, 300.0), Vec2::ZERO);
    let at = place(anchor, size, screen, true);
    assert_eq!(at, pos2(100.0, 0.0), "clamped to the top rather than off it");
}

#[test]
fn a_submenu_hangs_off_the_right_of_its_row() {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
    let row = Rect::from_min_size(pos2(100.0, 200.0), vec2(180.0, 36.0));
    let at = place(row, vec2(200.0, 100.0), screen, false);
    assert!(at.x > row.left(), "to the right of the row it came from");
    assert!(at.x <= row.right(), "with a small overlap so the pointer can cross");
}
