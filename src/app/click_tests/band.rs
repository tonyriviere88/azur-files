//! The rubber band: dragging a selection out of empty space.

use super::*;

#[test]
fn dragging_from_empty_space_bands_over_rows() {
    let mut h = Harness::new();
    let pane = h.pane_rect(0);
    let rows = h.tab(0).order.len();
    assert!(rows >= 4, "need a few rows to band over");

    // Start below the last row and drag up across the first three.
    let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
    let up_to = h.row_center(0, 2);
    h.drag(below, up_to);

    assert_eq!(
        h.tab(0).selected_count,
        rows - 2,
        "the band has to select every row it crossed"
    );
    assert!(
        h.tab(0).band.is_none(),
        "and let go of the band when the button comes up"
    );
}

#[test]
fn a_band_shrinking_back_deselects() {
    let mut h = Harness::new();
    let pane = h.pane_rect(0);
    let rows = h.tab(0).order.len();
    let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);

    h.wait();
    h.frame(vec![Event::PointerMoved(below)]);
    h.frame(vec![Event::PointerButton {
        pos: below,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    // Out to the top of the list...
    h.frame(vec![Event::PointerMoved(h.row_center(0, 0))]);
    let wide = h.tab(0).selected_count;
    assert!(wide > 1, "the band should have caught several rows");
    // ...and back down to just below the last row.
    h.frame(vec![Event::PointerMoved(below)]);
    assert!(
        h.tab(0).selected_count < wide,
        "pulling the band back has to let rows go again, not keep them"
    );
    h.frame(vec![Event::PointerButton {
        pos: below,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
}

#[test]
fn ctrl_dragging_a_band_keeps_what_was_selected() {
    let mut h = Harness::new();
    let pane = h.pane_rect(0);
    let rows = h.tab(0).order.len();

    // Select the last row on its own first.
    h.click_at(h.row_center(0, rows - 1));
    assert_eq!(h.tab(0).selected_count, 1);

    // Then Ctrl-band across the first two, which must not lose it.
    let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
    h.wait();
    h.modifiers = Modifiers::COMMAND;
    h.frame(vec![Event::PointerMoved(below)]);
    h.frame(vec![Event::PointerButton {
        pos: below,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::COMMAND,
    }]);
    h.frame(vec![Event::PointerMoved(h.row_center(0, rows - 2))]);
    h.frame(vec![Event::PointerButton {
        pos: below,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::COMMAND,
    }]);
    h.modifiers = Modifiers::NONE;
    h.frame(Vec::new());

    assert!(
        h.tab(0).selected_count >= 2,
        "a Ctrl band adds to the selection instead of replacing it"
    );
}

#[test]
fn a_plain_band_replaces_the_selection() {
    let mut h = Harness::new();
    let pane = h.pane_rect(0);
    let rows = h.tab(0).order.len();

    h.click_at(h.row_center(0, 0));
    let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
    h.drag(below, h.row_center(0, rows - 1));

    // Only the last row was crossed, so the first one is no longer selected.
    assert!(!h.tab(0).is_selected(0), "a plain band starts from nothing");
    assert!(h.tab(0).is_selected(rows - 1));
}
