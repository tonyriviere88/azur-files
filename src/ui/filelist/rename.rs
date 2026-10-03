//! Renaming in place: a field over the row, at the width the name needs.

use super::*;

/// How wide the rename field should be, given how wide its text has turned out.
///
/// The Name column's width, unless the text needs more — and never past the right edge of the
/// row, because a field whose text is outside the window is no better than a cropped one.
///
/// `ink` is the laid-out width of the text, `left..right` the name cell, `row_right` the row's
/// own right edge.
pub(crate) fn rename_width(ink: f32, left: f32, right: f32, row_right: f32) -> f32 {
    /// Room for the caret past the last character. Without it, typing at the end pushes the
    /// text the field was just widened for straight back out of view.
    const CARET: f32 = space::S2;
    let wanted = ink + CARET;
    let limit = (row_right - CELL_PAD - left).max(MIN_FIELD);
    wanted.max(right - left).max(MIN_FIELD).min(limit)
}

/// Narrow enough to type in, on a pane too narrow for anything.
pub(crate) const MIN_FIELD: f32 = 60.0;

/// The field's height. The line plus room for a caret above and below it, and enough of a target
/// to click into. Nothing to do with where the *text* goes — that is `text_row`'s centre.
pub(crate) const FIELD_HEIGHT: f32 = 20.0;

/// The same field, over a rect a caller already knows — which is what [`crate::ui::grid`] has: the
/// label under a tile, or the name cell of a folder's row in a tree.
///
/// A thin wrapper rather than a second implementation, because everything awkward about an in-place
/// rename is the same wherever it opens: the caret, the stem being what starts selected, the two
/// points of vertical alignment, the focus ring being taken off, and Enter / Escape / clicking away.
/// `at` is where the *text* goes — its left and right edges, and its vertical middle. `limit` is how
/// far right the field may grow when the name needs more than `at` gives it, which over a tile is the
/// pane's own edge: a name too long for two lines of a label is precisely the one somebody is
/// renaming, and a field the width of the tile would be that name seen through a letterbox.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rename_over(
    ui: &mut Ui,
    t: &Theme,
    pane: PaneId,
    tab: &mut Tab,
    entry: usize,
    at: Rect,
    limit: f32,
    out: &mut Vec<Action>,
) {
    let room = Rect::from_min_max(at.min, pos2(limit.max(at.right()), at.max.y));
    rename_field(ui, t, pane, tab, entry, at.left(), at.right(), room, out);
}

/// The in-place rename field, drawn over the name cell.
///
/// Explorer renames where the name sits rather than in a dialog, which keeps the
/// neighbouring names on screen — usually the reason you are renaming in the first
/// place. `Enter` commits, `Escape` abandons, and clicking away commits, which is
/// what every other in-place editor on the platform does.
///
/// # It is allowed to be wider than the column
///
/// The Name column is as wide as the other three leave it, which on a narrow pane is narrower
/// than plenty of names. A field pinned to that width scrolls a long name sideways under the
/// caret, so renaming `2026-04-report-final-v3.xlsx` meant editing eleven characters of it
/// through a letterbox and guessing at the rest. So it grows to fit its text and runs on over
/// Size, Type and Modified — which are nothing anybody needs to read while typing a name, and
/// come back the moment the rename ends. It stops at the right edge of the row, because past
/// that it would be a field with its text outside the window.
///
/// # The name does not move when you start typing
///
/// Which sounds like nothing and was the most obvious thing wrong with it. A field is a box with
/// its own padding and its own idea of where a line sits inside it, so the name jumped **two
/// points right and one point down** the instant a rename opened — measured, off a screenshot,
/// against the same row drawn as a label. It reads as a flinch on the one thing you are looking
/// at.
///
/// So the box is given no padding at all, and both sides now centre the line by the same rule:
/// `text_left` centres a label's galley in `text_row`, and the field is centred on that same
/// `text_row` with `Align::Center`. The name stays exactly where it was, which is what makes it
/// look like the row itself became editable rather than like a widget appeared over it.
///
/// `background-layer`, the panel's own fill, for the same reason: not `background-control`, which
/// is a *control's* colour and drew a grey slab over the row. Opaque rather than transparent
/// though, because covering the Size, Type and Modified cells it runs over is the whole point of
/// being wider than the column.
///
/// [`rename_over`] is the same field over a rect a caller already knows, which is how
/// [`crate::ui::grid`] opens one under a tile.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rename_field(
    ui: &mut Ui,
    t: &Theme,
    pane: PaneId,
    tab: &mut Tab,
    entry: usize,
    left: f32,
    right: f32,
    text_row: Rect,
    out: &mut Vec<Action>,
) {
    let fresh = tab.rename_fresh;
    // A folder keeps its whole name selected even when it has a dot in it, which is what
    // Explorer does with `my.folder`.
    let is_dir = tab
        .dir
        .as_ref()
        .and_then(|dir| dir.entries.get(entry))
        .is_some_and(|entry| entry.is_dir());
    let Some(text) = tab.rename_text(entry) else {
        return;
    };
    let whole = text.clone();

    // What the text will take, measured with the font the field will lay it out in.
    let font = egui::TextStyle::Body.resolve(ui.style());
    let line = ui
        .painter()
        .layout_no_wrap(whole.clone(), font, Color32::PLACEHOLDER)
        .size();
    let width = rename_width(line.x, left, right, text_row.right());

    // Where `text_left` would have put the label's first pixel — the same arithmetic, including
    // the snap to *device* pixels, which is the part that matters and the part that is easy to
    // miss. Centring the field on `text_row` and leaving egui to divide by two got within half a
    // point, and half a point is a blurred word rather than a sharp one.
    //
    // Then the field is placed so that `Align::Center` inside it lands the line exactly there:
    // egui centres the galley in the field's inner rect, and the margin is nothing, so the line
    // sits `(FIELD_HEIGHT - line) / 2` below the top.
    use egui::emath::GuiRounding as _;
    let baseline = (text_row.center().y - line.y * 0.5).round_to_pixels(ui.painter().pixels_per_point());
    let field = Rect::from_min_size(
        pos2(left, baseline - (FIELD_HEIGHT - line.y) * 0.5),
        vec2(width, FIELD_HEIGHT),
    );

    let id = Id::new(("rename", pane, entry));
    // Not the accent ring egui puts round a focused field. The field is already an obviously
    // editable box — a filled rectangle with a caret in it, sitting on a row that has had its
    // own highlight taken away for exactly this reason — and the ring on top of that was the
    // loudest thing in the window while renaming. Swapped rather than scoped, so nothing about
    // the layout of this row changes with it.
    //
    // The *width* and not the whole stroke, and the difference is the whole name being
    // unreadable. `Visuals::selection::stroke` is two unrelated things in one field: the frame
    // round a focused `TextEdit` — `builder.rs` reads all of it — and, in
    // `text_selection::visuals`, `stroke.color` alone, which is **the colour selected text is
    // drawn in**. `Stroke::NONE` is transparent, so zeroing the field took the frame off and
    // painted the selected stem in nothing: a solid accent block with the name invisible inside
    // it, which is what a rename opens with every time. A zero-width stroke draws no frame and
    // leaves the colour where it was.
    //
    // The colour it leaves is right because the design system was wrong too, and got fixed: it
    // pointed that field at `stroke-focus`, for the border, which put selected text at 2.3:1 on
    // the selection behind it. It is `text-on_accent` now — see
    // `azur_egui_theme::style::tests::selected_text_reads_on_the_selection_behind_it`.
    let ring = ui.visuals().selection.stroke;
    ui.visuals_mut().selection.stroke = Stroke::new(0.0, ring.color);
    // Square, like the row it is standing in for. See [`crate::ui::squared`].
    let response = crate::ui::squared(ui, |ui| {
        ui.put(
            field,
            egui::TextEdit::singleline(text)
                .id(id)
                .margin(egui::Margin::ZERO)
                .vertical_align(egui::Align::Center)
                .background_color(t.bg.layer)
                .desired_width(width),
        )
    });
    ui.visuals_mut().selection.stroke = ring;
    if fresh {
        response.request_focus();
        select_stem(ui.ctx(), id, &whole, is_dir);
        tab.rename_fresh = false;
    }

    // **`Ctrl+G` types a fresh GUID**, at the caret and over whatever is selected — so pressed on the
    // frame a rename opens, where what is selected is the stem, `report.pdf` becomes
    // `f81d4fae-…-….pdf` and the extension is left alone. Which is the gesture: a name that has to be
    // unique and say nothing.
    //
    // Read *after* the field has been drawn, like Enter and Escape below, and consumed so that a
    // future `Ctrl+G` elsewhere in the window cannot also fire off the same keystroke. Only while the
    // field has the keyboard: this function is drawn for one row at a time, but the guard is what
    // makes that a property of the code rather than of the caller.
    if response.has_focus()
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::G))
    {
        if let Some(guid) = crate::shell::new_guid() {
            if let Some(text) = tab.rename_text(entry) {
                type_into(ui.ctx(), id, text, &guid);
            }
        }
    }

    let (enter, escape) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    let name = tab
        .rename_text(entry)
        .map(|text| text.clone())
        .unwrap_or_default();

    if escape {
        out.push(Action::CancelRename(pane));
    } else if enter || response.lost_focus() {
        out.push(Action::CommitRename { pane, name });
    }
}

/// Put `typed` into the field as though it had been typed: over the selection, and the caret after it.
///
/// **Both halves, and the second is the one that is easy to leave out.** Changing the `String` behind
/// a `TextEdit` is not editing it — the caret and the selection live in egui's memory, keyed by the
/// field's id, and a text that has grown by 36 characters under a selection that still names the old
/// ones leaves the next keystroke deleting a stretch of the GUID that was just inserted. So the range
/// is read from that state, the text is spliced, and a bare caret past the insertion is written back.
///
/// A cursor with nothing stored is the end of the text, which is where a field the keyboard has only
/// just reached has its caret.
///
/// # Characters and bytes
///
/// egui counts the caret in **characters** and `String` is indexed in **bytes**, and a name on a
/// Windows disk is frequently not ASCII — `Résumé.pdf` is nine characters and eleven bytes. Getting
/// that wrong does not merely misplace the insertion: `replace_range` on a boundary inside a character
/// panics. Hence the conversion, and hence its falling back to the end of the string rather than
/// indexing past it — the stored range describes the text as it was last laid out, which is one frame
/// old.
fn type_into(ctx: &egui::Context, id: Id, text: &mut String, typed: &str) {
    use egui::text::{CCursor, CCursorRange};

    let Some(mut state) = egui::TextEdit::load_state(ctx, id) else {
        return;
    };
    let counted = text.chars().count();
    let range = state
        .cursor
        .char_range()
        .unwrap_or_else(|| CCursorRange::one(CCursor::new(counted)));
    // `CharIndex` is a newtype over the count, unwrapped here so the rest of this is arithmetic.
    let (one, two) = (range.primary.index.0, range.secondary.index.0);
    let (from, to) = (one.min(two).min(counted), one.max(two).min(counted));
    // Characters to bytes. `char_indices().nth` rather than arithmetic, because that is the only
    // thing that is right for every string.
    let byte = |chars: usize| {
        text.char_indices()
            .nth(chars)
            .map_or(text.len(), |(at, _)| at)
    };
    text.replace_range(byte(from)..byte(to), typed);
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(
            from + typed.chars().count(),
        ))));
    state.store(ctx, id);
}
