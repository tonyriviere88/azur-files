//! The rubber band, and selecting part of a name.

use super::*;

/// Begin a rubber band at where the button went down.
///
/// `press_origin` rather than the current position: those are different pixels — a drag has
/// to travel before egui calls it one — and anchoring at the later of the two loses whatever
/// the pointer crossed on the way, so a quick flick would select nothing.
pub(crate) fn start_band(ui: &Ui, body: Rect, tab: &mut Tab, origin: Option<egui::Pos2>) {
    let anchor = content_pos(ui, body, tab, origin);
    let modifiers = ui.input(|i| i.modifiers);
    tab.band = Some(crate::pane::Band {
        anchor,
        current: anchor,
        // A plain drag replaces the selection; Ctrl or Shift builds on it.
        base: if modifiers.command || modifiers.shift {
            tab.selected.clone()
        } else {
            Vec::new()
        },
    });
}

/// A pointer position as a distance from the top of the whole listing.
pub(crate) fn content_pos(ui: &Ui, body: Rect, tab: &Tab, at: Option<egui::Pos2>) -> egui::Pos2 {
    let at = at.or_else(|| ui.ctx().pointer_interact_pos()).unwrap_or(body.min);
    pos2(at.x, at.y - body.top() + tab.scroll_y)
}

/// Track a rubber-band selection: follow the pointer, auto-scroll past the edges, and let go when
/// the button does. Answers whether there is still a band.
///
/// **Split from applying it and from painting it**, because the middle step is the one thing the two
/// views cannot share: a row is covered when the band crosses its `y`, and a tile when the band
/// overlaps its box. So the caller does this, then works out its own coverage, then
/// [`band_paint`]s — and neither view has its own copy of the auto-scroll.
///
/// `content` is how tall the whole listing is, which is what bounds the scroll. In rows that is a
/// multiplication; in the grid it is [`crate::ui::grid::Layout::height`].
pub(crate) fn band_move(ui: &mut Ui, body: Rect, tab: &mut Tab, content: f32) -> bool {
    let held = ui.input(|i| i.pointer.any_down());
    let pointer = ui.ctx().pointer_interact_pos();

    if !held || pointer.is_none() {
        tab.band = None;
        return false;
    }

    // Auto-scroll when the band is dragged past an edge, at a rate that grows with
    // how far past it the pointer is — so a small overshoot creeps and a big one
    // moves, without a separate "fast" mode to discover.
    let at = pointer.unwrap_or(body.min);
    let overshoot = if at.y < body.top() {
        at.y - body.top()
    } else if at.y > body.bottom() {
        at.y - body.bottom()
    } else {
        0.0
    };
    if overshoot != 0.0 {
        let limit = (content - body.height()).max(0.0);
        let step = (overshoot * 0.35).clamp(-ROW_HEIGHT * 3.0, ROW_HEIGHT * 3.0);
        let next = (tab.scroll_y + step).clamp(0.0, limit);
        if next != tab.scroll_y {
            tab.scroll_to = Some(next);
            tab.scroll_y = next;
        }
        // A drag held still outside the view has to keep scrolling, and egui only
        // redraws on demand.
        ui.ctx().request_repaint();
    }

    let current = content_pos(ui, body, tab, pointer);
    if let Some(band) = &mut tab.band {
        band.current = current;
    }
    true
}

/// Paint the band.
///
/// Last of everything, so it lies over what it is selecting. Clipped to the body, or a band dragged
/// past the edge would spill onto the status line.
pub(crate) fn band_paint(ui: &Ui, t: &Theme, body: Rect, tab: &Tab) {
    let Some(band) = &tab.band else { return };
    let to_screen = |p: egui::Pos2| pos2(p.x, p.y + body.top() - tab.scroll_y);
    let rect = Rect::from_two_pos(to_screen(band.anchor), to_screen(band.current))
        .intersect(body);
    if rect.width() < 1.0 && rect.height() < 1.0 {
        return;
    }
    let painter = ui.painter().with_clip_rect(body);
    let accent = t.accent.default;
    painter.rect_filled(
        rect,
        CornerRadius::ZERO,
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 36),
    );
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, accent),
        StrokeKind::Inside,
    );
}

/// How much of a name a rename should start with selected: the stem, not the extension.
///
/// Typing over `report.docx` means replacing `report`, and an editor that hands you the
/// extension as well is an editor that turns every rename into a file with no type. Explorer
/// selects the stem; so does this.
///
/// `file_stem` gets the awkward cases right without help. `.gitignore` has no stem to speak of
/// and comes back whole, which is what Explorer selects too; `archive.tar.gz` comes back as
/// `archive.tar`, because only the last extension is one.
pub(crate) fn stem_chars(name: &str, is_dir: bool) -> usize {
    let whole = name.chars().count();
    if is_dir {
        return whole;
    }
    match std::path::Path::new(name).file_stem().and_then(|s| s.to_str()) {
        Some(stem) if !stem.is_empty() => stem.chars().count(),
        // No stem, or a name the platform will not split: select all of it.
        _ => whole,
    }
}

/// Put the caret over the stem of the name in a freshly opened rename field.
pub(crate) fn select_stem(ctx: &egui::Context, id: Id, name: &str, is_dir: bool) {
    use egui::text::{CCursor, CCursorRange};

    let Some(mut state) = egui::TextEdit::load_state(ctx, id) else {
        return;
    };
    let end = stem_chars(name, is_dir);
    state
        .cursor
        .set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(end))));
    state.store(ctx, id);
}
