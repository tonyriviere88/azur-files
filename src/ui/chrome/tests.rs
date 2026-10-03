use super::*;

/// A window 1000 wide, and the panes area a sidebar leaves of it.
fn window() -> (Rect, f32, f32) {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
    let bar = bar_rect(screen);
    (bar, 250.0, 996.0)
}

/// **No pane is ever given two tab strips**, however the rows line up.
///
/// The case that broke it: two banded rows exactly [`STRIP_ROW`] apart. Planning the upper row
/// takes the band's height off its panes, which moves their tops onto the lower row's top — and
/// a membership test made against `panes` after that puts them in the lower row as well. Both
/// strips then lay tabs out under the same `Id::new(("tab", pane, index))`, which egui answers
/// with "First/Second use of widget ID" painted in red over two stacked tab bars.
///
/// Reached in the window by dragging the divider between a split column's two panes: the moving
/// top sweeps through every pixel, including that one. It lasted a frame, so the fix is checked
/// here rather than by eye.
#[test]
fn two_rows_a_strip_apart_do_not_give_one_pane_two_strips() {
    // A bar with no room for tabs, so every row is banded and both go through the loop.
    let bar = Rect::from_min_size(Pos2::ZERO, vec2(40.0, 32.0));
    let top = 100.0;
    let mut panes = vec![
        (1u32, Rect::from_min_max(pos2(0.0, top), pos2(500.0, 400.0))),
        (
            2u32,
            Rect::from_min_max(pos2(500.0, top + STRIP_ROW), pos2(1000.0, 400.0)),
        ),
    ];
    let plan = plan_strips(&mut panes, bar);
    assert!(
        plan.in_bar.is_empty(),
        "this bar is too narrow to hold a strip, so nothing should be in it"
    );

    let mut seen: Vec<PaneId> = plan
        .in_bar
        .iter()
        .chain(plan.rows.iter().flat_map(|row| row.strips.iter()))
        .map(|(id, _)| *id)
        .collect();
    let planned = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        planned,
        seen.len(),
        "a pane was given more than one tab strip: {:?}",
        plan.rows
            .iter()
            .map(|r| r.strips.iter().map(|(id, _)| *id).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    assert_eq!(seen, vec![1, 2], "both panes should still get a strip each");
}

#[test]
fn one_pane_puts_its_tabs_in_the_title_bar_above_itself() {
    let (bar, left, right) = window();
    let mut panes = vec![(1u32, Rect::from_min_max(pos2(left, 40.0), pos2(right, 690.0)))];
    let before = panes[0].1;

    let plan = plan_strips(&mut panes, bar);
    assert!(plan.rows.is_empty(), "one row needs no band");
    assert_eq!(plan.in_bar.len(), 1);
    let (id, strip) = plan.in_bar[0];
    assert_eq!(id, 1);
    assert_eq!(strip.left(), left, "the strip starts where the pane does");
    assert!(strip.right() <= controls_left(bar), "and clears the buttons");
    assert_eq!(panes[0].1, before, "nothing was taken off the pane");
}

#[test]
fn panes_side_by_side_each_get_a_strip_over_themselves() {
    let (bar, left, right) = window();
    let middle = 620.0;
    let mut panes = vec![
        (1u32, Rect::from_min_max(pos2(left, 40.0), pos2(middle, 690.0))),
        (2u32, Rect::from_min_max(pos2(middle + 4.0, 40.0), pos2(right, 690.0))),
    ];

    let plan = plan_strips(&mut panes, bar);
    assert!(plan.rows.is_empty());
    assert_eq!(plan.in_bar.len(), 2);
    assert_eq!(plan.in_bar[0].1.left(), left);
    assert_eq!(plan.in_bar[1].1.left(), middle + 4.0);
    assert!(
        plan.in_bar[0].1.right() <= plan.in_bar[1].1.left(),
        "a strip never reaches over its neighbour's pane"
    );
}

#[test]
fn a_pane_in_a_row_below_gets_a_band_of_its_own() {
    let (bar, left, right) = window();
    let split = 370.0;
    let mut panes = vec![
        (1u32, Rect::from_min_max(pos2(left, 40.0), pos2(right, split))),
        (2u32, Rect::from_min_max(pos2(left, split + 4.0), pos2(right, 690.0))),
    ];

    let plan = plan_strips(&mut panes, bar);
    assert_eq!(plan.in_bar.len(), 1, "the top row still uses the title bar");
    assert_eq!(plan.in_bar[0].0, 1);
    assert_eq!(plan.rows.len(), 1, "the row below gets a band");

    let row = &plan.rows[0];
    assert_eq!(row.band.top(), split + 4.0, "the band is where the pane was");
    assert_eq!(row.band.height(), STRIP_ROW);
    assert_eq!(row.strips.len(), 1);
    assert_eq!(row.strips[0].0, 2);
    assert_eq!(
        panes[1].1.top(),
        split + 4.0 + STRIP_ROW,
        "and the room came out of the pane under it"
    );
    assert_eq!(panes[0].1.top(), 40.0, "the top pane is untouched");
}

#[test]
fn a_band_covers_only_the_panes_in_its_own_row() {
    // A left pane the full height, and the right half split in two: the lower right
    // pane's band must not reach across the pane beside it.
    let (bar, left, right) = window();
    let middle = 620.0;
    let split = 370.0;
    let mut panes = vec![
        (1u32, Rect::from_min_max(pos2(left, 40.0), pos2(middle, 690.0))),
        (2u32, Rect::from_min_max(pos2(middle + 4.0, 40.0), pos2(right, split))),
        (
            3u32,
            Rect::from_min_max(pos2(middle + 4.0, split + 4.0), pos2(right, 690.0)),
        ),
    ];

    let plan = plan_strips(&mut panes, bar);
    assert_eq!(plan.in_bar.len(), 2, "both top-row panes are in the bar");
    assert_eq!(plan.rows.len(), 1);
    let row = &plan.rows[0];
    assert_eq!(row.band.left(), middle + 4.0, "the band starts at its pane");
    assert_eq!(row.band.right(), right);
    assert_eq!(panes[0].1.top(), 40.0, "the full-height pane keeps its top");
    assert_eq!(panes[1].1.top(), 40.0);
    assert_eq!(panes[2].1.top(), split + 4.0 + STRIP_ROW);
}

#[test]
fn a_row_the_caption_buttons_would_squeeze_moves_to_a_band() {
    // Four panes across a narrow window: the rightmost has almost no title bar to
    // itself, so the whole row moves down rather than one pane being treated
    // differently from the rest.
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(720.0, 500.0));
    let bar = bar_rect(screen);
    let mut panes: Vec<(PaneId, Rect)> = (0..4)
        .map(|i| {
            let x = 150.0 + i as f32 * 140.0;
            (i as PaneId + 1, Rect::from_min_max(pos2(x, 40.0), pos2(x + 136.0, 490.0)))
        })
        .collect();

    let plan = plan_strips(&mut panes, bar);
    assert!(
        plan.in_bar.is_empty(),
        "no strip goes in the bar when one of them would not fit"
    );
    assert_eq!(plan.rows.len(), 1, "the row gets a band instead");
    assert_eq!(plan.rows[0].strips.len(), 4, "and every pane is on it");
    for (_, rect) in &panes {
        assert_eq!(rect.top(), 40.0 + STRIP_ROW);
    }
}
