use super::*;

/// A graph of this program's own test binary, walked for real.
fn walked() -> Arc<Graph> {
    let me = std::env::current_exe().expect("a test process has an executable");
    Arc::new(pe::walk(&me, pe::BUDGET, pe::PATIENCE))
}

/// **Every ink this tree uses can be read on every surface it is drawn on**, in both themes.
///
/// Measured rather than counted — `azur::contrast` is the ruler — and it found two real
/// defects on the first run, both of which are the obvious way to write this:
///
/// | ink | on a row | on a hovered row |
/// | --- | --- | --- |
/// | `text-primary` | 16.6 / 18.1 | 7.3 / 9.7 |
/// | `text-secondary` | 6.9 / 7.4 | 3.0 / 4.0 |
/// | `status-danger` | 4.9 / 4.4 | 2.1 / 2.4 |
/// | ~~`text-tertiary`~~ | 3.8 / 5.8 | **1.7** / 3.1 |
/// | ~~`text-disabled`~~ | 2.3 / 2.5 | **1.0** / 1.3 |
/// | ~~`status-warning`~~ | 8.2 / **2.8** | 3.6 / **1.5** |
///
/// `text-disabled` at 1.00:1 *is* the hover fill in the dark theme: an API set's location
/// line was invisible for as long as the pointer was over the row it was on. And
/// `status-warning` is a legitimate role that simply is not ink at 12 points on a light
/// surface — it was the processor tag on a mismatched module, which is the one row in a
/// dependency list you most need to be able to read.
///
/// So the tree has three inks, and this is what keeps it that way. The two floors:
/// **4.0:1 on the surface a row is normally read on**, and **2.0:1 in the transient hovered
/// state** — the same argument, and a rung above the number, that `azur::desktop`'s own
/// `a_row_can_still_be_read_when_it_is_hovered_or_pressed` settled on at 2.5.
///
/// # And the fourth surface, which is why [`inks`] exists
///
/// A picked row is filled with the accent, and on it two of those three inks stop being ink:
///
/// | ink | on a picked row | picked and hovered |
/// | --- | --- | --- |
/// | `text-primary` | 8.0 / 14.3 | 6.1 / 11.0 |
/// | ~~`text-secondary`~~ | **3.3** / 5.8 | 2.5 / 4.5 |
/// | ~~`status-danger`~~ | **2.3** / 3.4 | **1.8** / 2.7 |
///
/// Asserted in both directions — that `text-primary` clears the floor on all four surfaces, and
/// that the other two really do fail on this one — so that the rule they justify cannot quietly
/// become unnecessary without this test saying so.
#[test]
fn every_ink_in_the_tree_can_be_read() {
    use azur_egui_theme::contrast::ratio;

    const RESTING: f32 = 4.0;
    const HOVERED: f32 = 2.0;

    for t in [Theme::dark(), Theme::light()] {
        let name = if t.dark { "dark" } else { "light" };
        let hovered = crate::ui::hover_fill(&t);
        let picked = crate::ui::row_fill(&t, true, false).expect("a picked row has a fill");
        let both = crate::ui::row_fill(&t, true, true).expect("so does a hovered picked row");

        // The ink every surface has to carry, because a picked row is drawn in nothing else.
        for (surface, fill, floor) in [
            ("a row", t.bg.layer, RESTING),
            ("a hovered row", hovered, HOVERED),
            ("a picked row", picked, RESTING),
            ("a hovered picked row", both, HOVERED),
        ] {
            let got = ratio(t.text.primary, fill);
            assert!(
                got >= floor,
                "{name}: text-primary on {surface} is {got:.2}:1, under {floor}"
            );
        }
        // And the two that only ever appear on a row that is *not* picked.
        for (what, ink) in [
            ("text-secondary", t.text.secondary),
            ("status-danger", t.status.danger),
        ] {
            for (surface, fill, floor) in [
                ("a row", t.bg.layer, RESTING),
                ("a hovered row", hovered, HOVERED),
            ] {
                let got = ratio(ink, fill);
                assert!(
                    got >= floor,
                    "{name}: {what} on {surface} is {got:.2}:1, under {floor}"
                );
            }
        }
        // And the glyphs, at the floor a *shape* gets rather than the one ink gets. The module
        // glyph keeps its own colour on a picked row, which is the one ink besides `text-primary`
        // that survives there — 4.9:1, and 3.8:1 hovered.
        for (what, ink, fill, surface) in [
            ("the module glyph", t.executable, t.bg.layer, "a row"),
            ("the module glyph", t.executable, hovered, "a hovered row"),
            ("the module glyph", t.executable, picked, "a picked row"),
            ("the module glyph", t.executable, both, "a hovered picked row"),
            ("the missing mark", t.status.danger, t.bg.layer, "a row"),
        ] {
            let got = ratio(ink, fill);
            assert!(
                got >= azur_egui_theme::contrast::SHAPE,
                "{name}: {what} on {surface} is {got:.2}:1, under the shape floor"
            );
        }

        // **The rule in [`inks`] is necessary**, stated as the measurement that forces it: on a
        // picked row neither of the other two inks reaches the floor its own state is read at.
        assert!(
            ratio(t.text.secondary, picked) < RESTING || ratio(t.status.danger, picked) < RESTING,
            "{name}: both quiet inks now clear the floor on a picked row, so `inks` could stop \
             flattening them"
        );
        // Which is what the two functions in front of it do, and the assertion is on them rather
        // than on the numbers: nothing a picked row draws is either of the flattened inks.
        let (quiet, loud) = inks(&t, true);
        assert_eq!((quiet, loud), (t.text.primary, t.text.primary));
        for state in [
            State::Found,
            State::ApiSet,
            State::Missing,
            State::Unreadable("damaged headers"),
            State::Unvisited,
        ] {
            for delayed in [false, true] {
                assert_eq!(
                    ink_for(quiet, loud, t.text.primary, state, delayed),
                    t.text.primary,
                    "{name}: {state:?} on a picked row is not text-primary"
                );
            }
        }
    }

    // **And the mark on a missing row is flattened for the same reason the texts are**, which needs
    // both themes to see: `status-danger` on the accent is 3.44:1 in the light theme — a legible
    // shape — and **2.33:1** in the dark one, which is not. A rule derived from the light side alone
    // would have left the one row you most need to notice invisible on the side this window opens on.
    {
        let worst = [Theme::dark(), Theme::light()]
            .iter()
            .map(|t| ratio(t.status.danger, crate::ui::row_fill(t, true, false).unwrap()))
            .fold(f32::INFINITY, f32::min);
        assert!(
            worst < azur_egui_theme::contrast::SHAPE,
            "status-danger measures {worst:.2}:1 at worst on a picked row and is a legible shape              after all, so the mark need not be flattened"
        );
    }

    // And the three that were taken out really are the reason: each is under the hovered
    // floor on some surface. Across **both** themes rather than within one, which is the
    // whole point of checking both — `status-warning` is perfectly legible on the dark side
    // at 3.58:1 and 1.49:1 on the light one, and a rule derived from the dark theme alone is
    // how the two light-theme defects this project has already fixed got in.
    type Pick = fn(&Theme) -> Color32;
    for (what, pick) in [
        ("text-tertiary", (|t: &Theme| t.text.tertiary) as Pick),
        ("text-disabled", |t: &Theme| t.text.disabled),
        ("status-warning", |t: &Theme| t.status.warning),
    ] {
        let worst = [Theme::dark(), Theme::light()]
            .iter()
            .flat_map(|t| {
                let ink = pick(t);
                [t.bg.layer, crate::ui::hover_fill(t)].map(|fill| ratio(ink, fill))
            })
            .fold(f32::INFINITY, f32::min);
        assert!(
            worst < HOVERED,
            "{what} measures {worst:.2}:1 at worst and could be used after all"
        );
    }
}

/// The tree is a walk of a graph, and the two things that make that finite: a module
/// already on the branch is not descended into, and expansion is what decides the rest.
#[test]
fn the_tree_folds_and_never_goes_round_a_cycle() {
    let graph = walked();
    let mut view = View::new(graph.clone());

    assert_eq!(view.rows[0].module, 0, "the root is the first row");
    assert_eq!(view.rows[0].depth, 0);
    assert!(view.rows[0].open, "the root opens expanded");
    // The root's own imports are the rows at the top, in order, whatever the auto-expand has opened
    // further down — which is the half of the opening state that is about what you actually see. See
    // the module header on what opening the way down to a missing module costs.
    let mine: HashSet<usize> = graph.modules[0].imports.iter().map(|edge| edge.to).collect();
    assert!(view.rows.len() > mine.len());
    assert_eq!(
        view.rows
            .iter()
            .filter(|row| row.from == Some(0))
            .map(|row| row.module)
            .collect::<HashSet<_>>(),
        mine,
        "the rows under the root are not the root's own imports"
    );
    assert!(
        mine.contains(&view.rows[1].module),
        "the row under the root is not one of its imports"
    );

    // Folding the root leaves one row; unfolding puts them back.
    view.toggle(0);
    view.rebuild();
    assert_eq!(view.rows.len(), 1);
    assert!(!view.rows[0].open);
    view.toggle(0);
    view.rebuild();
    assert!(view.rows.len() > 1);

    // Expand everything there is, which is what would run away if either rule were
    // missing: this graph has cycles in it — `kernel32` and `kernelbase` refer to each
    // other — so a walk with no branch rule would not come back.
    for i in 0..graph.modules.len() {
        view.expanded.insert(i);
    }
    view.rebuild();
    assert!(view.rows.len() > graph.modules[0].imports.len());
    assert!(view.rows.len() <= ROW_CAP);
    for row in &view.rows {
        assert!((row.depth as usize) <= DEPTH_CAP);
        // Nothing is both a repeat and expanded: that is the rule, stated as an assertion.
        assert!(!(row.cyclic && row.open));
    }
    // And something really was a repeat, or the rule was never exercised.
    assert!(
        view.rows.iter().any(|row| row.cyclic),
        "no cycle was reached, so the branch rule proves nothing here"
    );
    // Every row but the root knows what imported it, which is what the `used` panel is about.
    assert_eq!(view.rows[0].from, None, "the root is imported by nothing");
    for row in view.rows.iter().skip(1) {
        let from = row.from.expect("a row under the root has an importer");
        assert!(
            graph.modules[from]
                .imports
                .iter()
                .any(|edge| edge.to == row.module),
            "{} says it was imported by {}, which does not import it",
            graph.modules[row.module].name,
            graph.modules[from].name
        );
    }
}

/// **A walk opens the way down to every module that is not there** — every one, and not only the ones
/// that would stop the program from starting, which is the defect this asserts against.
///
/// The panel's title says "5 missing"; on an ordinary machine all five are delay-loaded from
/// somewhere deep inside `shell32`, so a tree that only opened the branches to a *start-up* failure
/// opened none of them. The count was reporting something the display would not then show.
#[test]
fn a_walk_opens_the_way_down_to_everything_that_is_missing() {
    let graph = walked();
    let view = View::new(graph.clone());

    // Every module opened is either the root or on a chain to something missing.
    let chains = graph.missing();
    for &module in &view.expanded {
        let on_a_chain = chains
            .iter()
            .any(|chain| chain[..chain.len() - 1].contains(&module));
        assert!(
            module == 0 || on_a_chain,
            "{} was opened for no reason",
            graph.modules[module].name
        );
    }
    // And every chain really is open, all the way down.
    for chain in &chains {
        for &module in &chain[..chain.len() - 1] {
            assert!(
                view.expanded.contains(&module),
                "the way down to a missing module is not open at {}",
                graph.modules[module].name
            );
        }
    }

    // Which is the point: as many missing modules on show as the walk counted, each reachable by
    // folding nothing. `tally` is what the title counts with, so it is what this compares against.
    let (_, _, missing) = graph.tally();
    let shown: HashSet<usize> = view
        .rows
        .iter()
        .filter(|row| {
            matches!(
                graph.modules[row.module].state,
                State::Missing | State::Unreadable(_)
            )
        })
        .map(|row| row.module)
        .collect();
    assert_eq!(
        shown.len(),
        missing,
        "the title counts {missing} missing and the tree opens with {} of them on show",
        shown.len()
    );
}

/// The modules of one level come out **same folder first, then in name order**.
///
/// Checked against the rule rather than against a list of names: which DLLs this test binary
/// imports is the linker's business and changes with the crate, but the *order* they are shown in
/// is this module's and can be asserted for any graph at all.
#[test]
fn a_level_is_ordered_by_folder_and_then_by_name() {
    let graph = walked();
    let mut view = View::new(graph.clone());
    for i in 0..graph.modules.len() {
        view.expanded.insert(i);
    }
    view.rebuild();

    // Two rows are siblings when they are adjacent at the same depth under the same importer.
    let mut checked = 0;
    for pair in view.rows.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.depth != b.depth || a.from != b.from {
            continue;
        }
        let beside = a.from.and_then(|from| folder_of(&graph.modules[from]));
        assert_ne!(
            order(&graph, beside, b.module, a.module),
            Ordering::Less,
            "{} comes before {} under {}",
            graph.modules[a.module].name,
            graph.modules[b.module].name,
            a.from
                .map(|from| graph.modules[from].name.as_str())
                .unwrap_or("the root"),
        );
        checked += 1;
    }
    assert!(checked > 20, "only {checked} sibling pairs were compared");

    // And the rule itself, on the one folder every walk has: the root's own. A module found beside
    // the binary sorts before one out of `System32` whatever the two are called — which is the
    // point, `System32` being full of short names that are alphabetically early.
    let beside = folder_of(&graph.modules[0]);
    let local = (0..graph.modules.len()).find(|&at| folder_of(&graph.modules[at]) == beside);
    let far = (0..graph.modules.len())
        .find(|&at| graph.modules[at].path.is_some() && folder_of(&graph.modules[at]) != beside);
    let (Some(local), Some(far)) = (local, far) else {
        panic!("this walk found no two folders, so the grouping was never exercised");
    };
    assert_eq!(order(&graph, beside, local, far), Ordering::Less);
    assert_eq!(order(&graph, beside, far, local), Ordering::Greater);

    // Something that is not a file at all is in no folder, so it is never in the first group.
    let api = (0..graph.modules.len()).find(|&at| graph.modules[at].state == State::ApiSet);
    if let Some(api) = api {
        assert_eq!(order(&graph, beside, api, local), Ordering::Greater);
    }
}

/// The flat list is **every module once**, and it is the other way of finding a missing one.
#[test]
fn the_flat_list_holds_every_module_once() {
    let graph = walked();
    let mut view = View::new(graph.clone());
    view.sync(true);

    assert_eq!(
        view.rows.len(),
        graph.modules.len(),
        "the flat list is not one row per module"
    );
    let seen: HashSet<usize> = view.rows.iter().map(|row| row.module).collect();
    assert_eq!(seen.len(), view.rows.len(), "a module is listed twice");
    for row in &view.rows {
        assert_eq!(row.depth, 0, "a flat row is at no depth");
        assert!(!row.expandable, "a flat row has nothing to unfold");
        assert_eq!(row.from, None, "a flat row is about no one importer");
    }

    // In the same order the tree uses, against the root's folder.
    let beside = folder_of(&graph.modules[0]);
    for pair in view.rows.windows(2) {
        assert_ne!(
            order(&graph, beside, pair[1].module, pair[0].module),
            Ordering::Less,
            "the flat list is out of order at {}",
            graph.modules[pair[0].module].name
        );
    }

    // **A row is drawn as delay-loaded here when nothing opens it before `main`**, which is the flat
    // list's version of the tree's per-edge answer — and what keeps the five harmless missing ones in
    // this program's own graph from being drawn as a fault.
    //
    // Against [`pe::Graph::upfront`], which is the *one* rule. There were two: this list took "anything
    // imports it without delay-loading it" and the title took "reachable from the root along undelayed
    // imports", and they disagree about a DLL that only a delay-loaded module imports normally. The
    // test asserted one against the other and passed because no module on this machine sits in the gap.
    let upfront = graph.upfront();
    for row in &view.rows {
        assert_eq!(
            row.delayed,
            row.module != 0 && !upfront[row.module],
            "{} is drawn the wrong way round",
            graph.modules[row.module].name
        );
    }
}

/// Picking a row reads its symbols, and the read is about the **edge** and not only the module.
#[test]
fn picking_a_row_reads_what_it_offers_and_what_is_taken_from_it() {
    let graph = walked();
    let mut view = View::new(graph.clone());

    assert_eq!(view.picked(), None, "nothing is picked to begin with");
    assert_eq!(view.symbol_counts(), None, "and nothing has been read");

    // A row under the root: a real DLL, imported by the binary the walk started from — the only
    // case where both panels have something to say.
    let (at, module) = view
        .rows
        .iter()
        .enumerate()
        .find(|(_, row)| row.from == Some(0) && graph.modules[row.module].state == State::Found)
        .map(|(at, row)| (at, row.module))
        .expect("a real binary imports at least one real DLL");

    let pick = Pick {
        module,
        from: Some(0),
    };
    view.picked = Some(pick);
    view.ensure_symbols();
    let (exports, used) = view.symbol_counts().expect("the symbols were read");
    let name = &graph.modules[module].name;
    assert!(
        exports.is_some_and(|n| n > 0),
        "{name} is a real DLL and exports nothing"
    );
    assert!(
        used.is_some_and(|n| n > 0),
        "this binary imports {name} and uses nothing out of it"
    );
    assert!(
        used <= exports,
        "{name}: {used:?} symbols are used out of the {exports:?} it exports"
    );
    assert!(view.is_picked(&view.rows[at]));

    // The read is kept rather than repeated: it is the click that pays for it.
    view.ensure_symbols();
    assert_eq!(view.symbols.as_ref().map(|both| both.of), Some(pick));

    // **The same module under a different importer is a different pick**, because one of the two
    // panels is about the edge — so it is read again, and only one of the two answers changes.
    view.picked = Some(Pick { module, from: None });
    view.ensure_symbols();
    let (exports_again, used_again) = view.symbol_counts().expect("read for the new pick");
    assert_eq!(exports_again, exports, "the export table is the module's own");
    assert_eq!(
        used_again, None,
        "with no importer there is nothing it could be used by"
    );

    // Nothing picked, nothing read.
    view.picked = None;
    view.ensure_symbols();
    assert_eq!(view.symbol_counts(), None);
}

/// **The panels are a column down the right unless the canvas is under [`BESIDE_AT`] wide**, and
/// where there is room for neither they are not drawn at all.
///
/// Both cases are real: this panel is docked either along the bottom of a pane, where it is the
/// width of the pane, or down the side, where it is 300 points wide and twenty-five rows tall.
#[test]
fn the_symbol_panels_take_the_room_they_have_and_no_more() {
    let at = |w: f32, h: f32| Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));

    // Nothing picked: the tree has the canvas, whatever shape it is. The one answer that is not
    // about how much room there is.
    for (w, h) in [(1200.0, 200.0), (320.0, 800.0), (60.0, 60.0)] {
        let where_ = places(at(w, h), false, false, Sizes::default());
        assert_eq!(where_.tree, at(w, h));
        assert!(where_.exports.is_none() && where_.used.is_none());
    }

    // ---- Wide: beside the tree, stacked ----
    let wide = at(1200.0, 400.0);
    let where_ = places(wide, true, true, Sizes::default());
    let (used, exports) = (
        where_.used.expect("a wide canvas has room for both"),
        where_.exports.expect("and for the exports"),
    );
    assert!(where_.tree.width() >= TREE_MIN_W);
    assert_eq!(where_.tree.left(), wide.left());
    assert_eq!(exports.right(), wide.right());
    assert_eq!(used.top(), wide.top(), "what is used is the upper panel");
    assert!(used.bottom() <= exports.top(), "the two panels overlap");
    assert!(used.height() >= symbols::MIN_H && exports.height() >= symbols::MIN_H);
    assert!(
        where_.tree.right() <= used.left(),
        "the tree runs under the panels"
    );

    // Too short to stack: the exports panel takes the column on its own.
    let squat = at(1200.0, symbols::MIN_H + 8.0);
    let where_ = places(squat, true, true, Sizes::default());
    assert!(where_.used.is_none(), "there is no room for two panels here");
    assert_eq!(
        where_.exports.expect("but there is for one").height(),
        squat.height()
    );

    // ---- Narrow: under the tree instead, side by side ----
    let tall = at(500.0, 900.0);
    let where_ = places(tall, true, true, Sizes::default());
    let (used, exports) = (
        where_.used.expect("a tall canvas puts them underneath"),
        where_.exports.expect("both of them"),
    );
    assert_eq!(where_.tree.top(), tall.top());
    assert!(where_.tree.height() >= TREE_MIN_H);
    assert!(
        where_.tree.bottom() <= used.top(),
        "the band overlaps the tree"
    );
    assert_eq!(used.left(), tall.left(), "what is used is the left panel");
    assert!(used.right() <= exports.left());
    assert_eq!(exports.right(), tall.right());
    assert_eq!(used.bottom(), tall.bottom());

    // Narrow enough that only one fits in the band.
    let narrow = at(symbols::MIN_W + 20.0, 900.0);
    let where_ = places(narrow, true, true, Sizes::default());
    assert!(where_.used.is_none());
    assert_eq!(
        where_.exports.expect("one panel still fits").width(),
        narrow.width()
    );

    // ---- And a canvas too small for any of it keeps the tree whole ----
    for (w, h) in [
        (TREE_MIN_W + symbols::MIN_W - 10.0, TREE_MIN_H + 10.0),
        (symbols::MIN_W - 10.0, 900.0),
        (1200.0, symbols::MIN_H - 4.0),
    ] {
        let where_ = places(at(w, h), true, true, Sizes::default());
        assert_eq!(
            where_.tree,
            at(w, h),
            "the tree gave up room it could not spare at {w}x{h}"
        );
        assert!(where_.exports.is_none() && where_.used.is_none());
    }
}

/// **800 points is the threshold**, and it is about width alone: a canvas wider than that gets the
/// column, and one under it gets the band along the bottom even when it is wider than it is tall.
#[test]
fn eight_hundred_points_is_where_the_panels_move_to_the_right() {
    let at = |w: f32, h: f32| Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));

    // Wider than it is tall, and roomy enough for both by every floor there is — but under the
    // threshold, so the panels go underneath. Which is the case a floor-based rule got wrong: it put
    // them beside a 500-point dock, where the 300 points left could not hold the Location column.
    let narrow = at(BESIDE_AT - 20.0, 300.0);
    let under = places(narrow, true, true, Sizes::default());
    let (used, exports) = (
        under.used.expect("there is room for both"),
        under.exports.expect("there is"),
    );
    // The band spans the canvas and the two panels divide it — which is what "underneath" means, and
    // is the arrangement a column would not have.
    assert_eq!(used.left(), narrow.left(), "the band does not start at the edge");
    assert_eq!(exports.right(), narrow.right(), "nor reach the other one");
    assert!(
        under.tree.bottom() <= used.top() && under.tree.bottom() <= exports.top(),
        "the panels are beside the tree under the threshold"
    );
    assert_eq!(under.tree.width(), narrow.width(), "the tree gave up width");

    // And at the threshold exactly, the column.
    let beside = places(at(BESIDE_AT, 300.0), true, true, Sizes::default());
    let column = beside.exports.expect("there is room for a panel");
    assert!(
        beside.tree.right() <= column.left(),
        "the panels are still underneath at {BESIDE_AT} points"
    );
    assert_eq!(column.bottom(), 300.0, "the column is not full height");
}

/// **The two shares are what the grips move**, and neither can be dragged past the floor of the area
/// it would take the room from.
#[test]
fn the_panels_are_the_size_they_were_dragged_to() {
    let canvas = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 600.0));
    let room = canvas.width() - SEAM;

    // The share is the column's share of the canvas, so a quarter is a quarter.
    let quarter = places(canvas, true, true, Sizes { share: 0.25, ..Sizes::default() });
    let column = quarter.exports.expect("a panel");
    assert!(
        ((canvas.right() - column.left()) - room * 0.25).abs() <= 1.0,
        "a quarter share is {} points of {room}",
        canvas.right() - column.left()
    );
    // And the split is the upper panel's share of the column.
    let used = quarter.used.expect("both panels");
    let inner = used.height() + SEAM + column.height();
    assert!(
        (used.height() - (inner - SEAM) * SPLIT).abs() <= 1.0,
        "the split put {} points of {inner} in the upper panel",
        used.height()
    );

    // Dragged to either end, both areas keep their floors — which is what the clamps are for, and
    // what stops a drag from making the tree unreadable or a panel useless.
    for share in [0.0, 0.02, 0.98, 1.0, f32::MAX] {
        let where_ = places(canvas, true, true, Sizes { share, ..Sizes::default() });
        let column = where_.exports.expect("a panel survives any share");
        assert!(
            where_.tree.width() >= TREE_MIN_W,
            "share {share} left the tree {} points",
            where_.tree.width()
        );
        assert!(
            column.width() >= symbols::MIN_W,
            "share {share} left the panel {} points",
            column.width()
        );
    }
    for split in [0.0, 0.02, 0.98, 1.0] {
        let where_ = places(canvas, true, true, Sizes { split, ..Sizes::default() });
        let (used, exports) = (
            where_.used.expect("both panels survive any split"),
            where_.exports.expect("both"),
        );
        assert!(
            used.height() >= symbols::MIN_H && exports.height() >= symbols::MIN_H,
            "split {split} left {} and {} points",
            used.height(),
            exports.height()
        );
    }
}

/// **A search on the tree asks the graph, not the rows on show.**
///
/// Which is the whole of that decision: what comes back is the chain from the root down to every
/// module whose name matches — through modules that do not match themselves, drawn quieter because
/// the route is not the answer. A filter over the rows on show would have searched the thirty that
/// happen to be unfolded.
#[test]
fn a_search_on_the_tree_finds_where_a_module_comes_in() {
    let graph = walked();
    let mut view = View::new(graph.clone());

    // A name that is in the graph but *not* among the root's own imports, which is the case a filter
    // over the visible rows could not answer at all.
    let deep = graph
        .modules
        .iter()
        .enumerate()
        .skip(1)
        .find(|(at, module)| {
            module.state == State::Found
                && !graph.modules[0].imports.iter().any(|edge| edge.to == *at)
        })
        .map(|(_, module)| module.name.clone())
        .expect("a real graph reaches something the root does not import directly");

    view.search_modules(&deep);
    let rows = view.shown_rows();
    assert!(!rows.is_empty(), "searching for {deep} found nothing");
    assert_eq!(rows[0].0, graph.modules[0].name, "the root leads every chain");
    assert!(
        rows.iter().any(|(name, faint)| *name == deep && !faint),
        "{deep} is not among the answers: {rows:?}"
    );
    // Every row is either an answer or on the way to one — and the ones on the way are the faint ones.
    for (name, faint) in &rows {
        let matches = name.to_lowercase().contains(&deep.to_lowercase());
        assert_eq!(
            *faint, !matches,
            "{name} is drawn {} and {} the query",
            if *faint { "faint" } else { "as an answer" },
            if matches { "matches" } else { "does not match" }
        );
    }
    // Which is a small tree and not the whole graph — the point of chains rather than a full
    // expansion.
    assert!(
        rows.len() < graph.modules.len() / 2,
        "the search opened {} rows of a {}-module graph",
        rows.len(),
        graph.modules.len()
    );

    // A query nothing matches leaves nothing: the root on its own is not an answer.
    view.search_modules("no-such-module-anywhere");
    assert!(view.shown_rows().is_empty());

    // Cleared, and the tree is what it was.
    view.search_modules("");
    assert_eq!(view.shown_rows().len(), View::new(graph).shown_rows().len());
}

/// And a search on the **flat list** is the plain filter it can afford to be, every module being a
/// row there already.
#[test]
fn a_search_on_the_flat_list_keeps_the_modules_that_match() {
    let graph = walked();
    let mut view = View::new(graph.clone());
    view.sync(true);
    view.search_modules("crypt");
    let rows = view.shown_rows();
    assert!(!rows.is_empty(), "no module in this graph is called *crypt*");
    for (name, faint) in &rows {
        assert!(
            name.to_lowercase().contains("crypt"),
            "{name} does not match the query"
        );
        assert!(!faint, "a flat row is never a stepping stone");
    }
    assert_eq!(
        rows.len(),
        graph
            .modules
            .iter()
            .filter(|m| m.name.to_lowercase().contains("crypt"))
            .count()
    );
}

/// **The symbols are demangled, and the search looks at what is on screen.**
///
/// Both halves matter together: a row says `rsh::App3D::Exec(...)` rather than `?Exec@App3D@rsh@@…`,
/// so a search that compared against the decorated name would not find what the reader can see.
#[test]
fn the_symbol_panels_search_the_names_they_show() {
    let graph = walked();
    let mut view = View::new(graph.clone());
    let module = view
        .rows
        .iter()
        .find(|row| row.from == Some(0) && graph.modules[row.module].state == State::Found)
        .map(|row| row.module)
        .expect("a real binary imports at least one real DLL");
    view.picked = Some(Pick {
        module,
        from: Some(0),
    });
    view.ensure_symbols();
    let both = view.symbols_mut().expect("the symbols were read");

    // Nothing is decorated, whatever the file held: a name either demangled or was never a C++ one.
    let all = both.exports_shown("").len();
    assert!(all > 0, "{} exports nothing", graph.modules[module].name);
    for name in both.exports_shown("") {
        assert!(
            !name.starts_with('?'),
            "{name} reached the panel still decorated"
        );
    }

    // A query narrows it, keeps only what matches, and does not care about case.
    let needle: String = both
        .exports_shown("")
        .first()
        .map(|name| name.chars().take(4).collect())
        .expect("there is a first export");
    let kept = both.exports_shown(&needle).len();
    assert!(kept > 0 && kept <= all, "{needle:?} kept {kept} of {all}");
    for name in both.exports_shown(&needle) {
        assert!(
            name.to_lowercase().contains(&needle.to_lowercase()),
            "{name} does not hold {needle:?}"
        );
    }
    assert_eq!(
        both.exports_shown(&needle.to_uppercase()).len(),
        kept,
        "the search is case-sensitive"
    );
    // And cleared, everything is back.
    assert_eq!(both.exports_shown("").len(), all);

    // The used panel gets the same treatment, and its rows carry the tag that says which of them are
    // delay-loaded — per symbol, which is the thing that panel is for.
    let used = both.used_shown("");
    assert!(!used.is_empty(), "nothing is used out of this module");
    for (name, _) in &used {
        assert!(!name.starts_with('?'), "{name} is still decorated");
    }
}

/// The search order is reported with what it does not model, because a location means
/// nothing without the list of places that were tried — **and the missing modules are named**,
/// because a count of something the reader cannot then find is the one thing this must not do.
#[test]
fn the_whole_answer_is_on_the_title() {
    let graph = walked();
    let text = about(&graph);
    let (files, _, missing) = graph.tally();
    assert!(
        text.starts_with(&format!("{files} file")),
        "the counts are not the first thing said: {text}"
    );
    assert!(text.contains(" ms"), "the timing is gone");
    assert!(text.contains("Looked for in order:"));
    assert!(
        text.contains(&graph.search[0].display().to_string()),
        "the first place looked is not in the list"
    );
    assert!(
        text.contains("KnownDLLs"),
        "the caveats are gone, and the list now reads as the whole rule"
    );

    if missing == 0 {
        println!("nothing is missing on this machine, so there was nothing to name");
        return;
    }
    assert!(
        text.contains("Not found:"),
        "{missing} modules are missing and none of them is named: {text}"
    );
    for chain in graph.missing().iter().take(4) {
        let &at = chain.last().expect("a chain ends somewhere");
        assert!(
            text.contains(&graph.modules[at].name),
            "{} is missing and unnamed on the title",
            graph.modules[at].name
        );
    }
    // Each with the module that wants it, and whether anything wants it up front — which is the
    // difference between "this will not start" and "one feature will not work, later".
    assert!(text.contains("wanted by"));
    assert!(
        text.contains("delay-loaded") || text.contains("at start-up"),
        "the missing modules are named without saying which kind they are"
    );
}
