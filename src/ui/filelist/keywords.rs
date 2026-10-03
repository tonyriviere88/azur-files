//! The Keywords cell: quiet text, a field after the pointer has rested on it, and the field itself.
//!
//! # Why the cell has to be *rested on*
//!
//! A row is clicked to select it, anywhere along it, and the Keywords column is a quarter of that
//! width. A cell that opened a text field under every click would take a quarter of every row away
//! from selecting — and it would do it silently, because the click that was meant to select a file
//! would *also* look like it had worked. So a click on the cell is a click on the row, as it is on
//! every other cell, until the pointer has stayed there for [`KEYWORDS_ARM`]. Then the cell draws
//! itself as a field — the box the editor will be — and the pointer turns into a caret, and a click
//! from then on is a click into the field.
//!
//! The row's other furniture is left alone throughout: the fill, the cursor ring and the selection
//! all mean what they meant, because resting on a cell is not doing anything to the row yet.
//!
//! # What is kept, and where
//!
//! See [`crate::fs::keywords`]. The text is committed to the *file's key*, not to the row: a row's
//! index is only good until the folder is read again, and the watcher reads it again whenever
//! anything is written there.

use super::*;

/// How long the pointer has to rest on a Keywords cell before a click there edits it. Half a second,
/// which is past a click on the way somewhere else and short of a wait.
pub const KEYWORDS_ARM: f64 = 0.5;

/// The box a Keywords cell is drawn as once it is armed, and the editor is drawn in.
///
/// **The text does not move** between the three states: its left edge is the cell's padding in all
/// of them, so the box is drawn half a padding out from the text and the field is given no margin.
/// [`FIELD_HEIGHT`] tall, as the rename field is, centred on the row.
pub(crate) fn keywords_box(cell: Rect, row: Rect) -> Rect {
    let top = (row.center().y - FIELD_HEIGHT * 0.5).round();
    Rect::from_min_max(
        pos2((cell.left() - CELL_PAD * 0.5).round(), top),
        pos2((cell.right() + CELL_PAD * 0.5).round(), top + FIELD_HEIGHT),
    )
}

/// The armed cell: the field's outline, before there is a field.
///
/// `background-layer` inside, which is the rename field's fill and what the editor will be drawn on,
/// and `stroke-control` round it — the outline every text box in the design system has at rest. The
/// focus stroke is kept for when there really is a caret in it.
pub(crate) fn armed_box(painter: &egui::Painter, t: &Theme, rect: Rect) {
    painter.rect_filled(rect, CornerRadius::ZERO, t.bg.layer);
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, t.stroke.control),
        StrokeKind::Inside,
    );
}

/// What an armed cell with nothing in it says, so there is something to see become a field.
pub(crate) const KEYWORDS_HINT: &str = "Add keywords";

/// The keywords editor, drawn over the cell.
///
/// `Enter` commits, `Escape` abandons, and clicking away commits — the rename field's rules, and every
/// other in-place editor's on the platform. Keywords are separated by `;` or `,`, and tidied into
/// `a; b` when they are kept: see [`crate::fs::keywords::normalize`].
///
/// The caret goes at the end rather than the text being selected. A keyword is usually being
/// *added*, and a field that opened with everything selected would throw the others away at the
/// first key.
#[allow(clippy::too_many_arguments)]
pub(crate) fn keywords_field(
    ui: &mut Ui,
    t: &Theme,
    pane: PaneId,
    tab: &mut Tab,
    entry: usize,
    cell: Rect,
    row: Rect,
    out: &mut Vec<Action>,
) {
    let fresh = std::mem::take(&mut tab.keywords_fresh);
    let Some(text) = tab.keywords_text(entry) else {
        return;
    };

    let font = t.fonts.caption.clone();
    let line = ui
        .painter()
        .layout_no_wrap(text.clone(), font.clone(), Color32::PLACEHOLDER)
        .size();

    // Where `text_left` puts the label's first pixel, snapped to device pixels the same way — see
    // `rename_field`, where the half a point this is about was measured.
    use egui::emath::GuiRounding as _;
    let baseline = (cell.center().y - line.y * 0.5).round_to_pixels(ui.painter().pixels_per_point());
    let field = Rect::from_min_size(
        pos2(cell.left(), baseline - (FIELD_HEIGHT - line.y) * 0.5),
        vec2(cell.width().max(MIN_FIELD), FIELD_HEIGHT),
    );

    let outline = keywords_box(cell, row);
    ui.painter().rect_filled(outline, CornerRadius::ZERO, t.bg.layer);
    ui.painter().rect_stroke(
        outline,
        CornerRadius::ZERO,
        Stroke::new(1.0, t.stroke.focus),
        StrokeKind::Inside,
    );

    let id = Id::new(("keywords", pane, entry));
    // The same swap the rename field makes, for the same reason: the outline above is the field's
    // frame, and egui's own ring on top of it would be a second one.
    let ring = ui.visuals().selection.stroke;
    ui.visuals_mut().selection.stroke = Stroke::new(0.0, ring.color);
    let response = crate::ui::squared(ui, |ui| {
        ui.put(
            field,
            egui::TextEdit::singleline(text)
                .id(id)
                .font(font)
                .text_color(t.text.primary)
                .margin(egui::Margin::ZERO)
                .vertical_align(egui::Align::Center)
                .background_color(t.bg.layer)
                .frame(egui::Frame::NONE)
                .desired_width(field.width()),
        )
    });
    ui.visuals_mut().selection.stroke = ring;
    if fresh {
        response.request_focus();
        caret_at_end(ui.ctx(), id, text);
    }

    let (enter, escape) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if escape {
        out.push(Action::CancelKeywords(pane));
    } else if enter || response.lost_focus() {
        out.push(Action::CommitKeywords {
            pane,
            text: text.clone(),
        });
    }
}

fn caret_at_end(ctx: &egui::Context, id: Id, text: &str) {
    use egui::text::{CCursor, CCursorRange};
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(text.chars().count()))));
    state.store(ctx, id);
}
