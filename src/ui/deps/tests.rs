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
#[test]
fn every_ink_in_the_tree_can_be_read() {
    use azur_egui_theme::contrast::ratio;

    const RESTING: f32 = 4.0;
    const HOVERED: f32 = 2.0;

    for t in [Theme::dark(), Theme::light()] {
        let name = if t.dark { "dark" } else { "light" };
        let hovered = crate::ui::hover_fill(&t);
        for (what, ink) in [
            ("text-primary", t.text.primary),
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
        // And the glyphs, at the floor a *shape* gets rather than the one ink gets.
        for (what, ink, fill, surface) in [
            ("the module glyph", t.executable, t.bg.layer, "a row"),
            ("the module glyph", t.executable, hovered, "a hovered row"),
            ("the missing mark", t.status.danger, t.bg.layer, "a row"),
        ] {
            let got = ratio(ink, fill);
            assert!(
                got >= azur_egui_theme::contrast::SHAPE,
                "{name}: {what} on {surface} is {got:.2}:1, under the shape floor"
            );
        }
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
    assert_eq!(
        view.rows.len(),
        1 + graph.modules[0].imports.len(),
        "a freshly walked binary shows its own imports and no more"
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
}

/// A walk opens the branches that lead to something that would stop the program starting,
/// and nothing else.
#[test]
fn a_walk_opens_only_what_is_worth_looking_at() {
    let graph = walked();
    let view = View::new(graph.clone());

    // Every module opened is either the root or on a chain to something missing.
    let chains = graph.breaks_loading();
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
}

/// The search order is reported with what it does not model, because a location means
/// nothing without the list of places that were tried.
#[test]
fn the_whole_answer_is_on_the_title() {
    let graph = walked();
    let text = about(&graph);
    let (files, _, _) = graph.tally();
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
}
