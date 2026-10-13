//! The column strip: how wide each column ends up, and where its header sits.

use super::*;
use azur_egui_theme::components::{ContextMenu, MenuItem};
use azur_egui_theme::tokens::typography;
use egui::Rangef;

/// The header strip: a body line with `space-2` above and below, which is Azur's
/// table header at this density.
pub const HEADER_HEIGHT: f32 = typography::LINE_BODY + space::S2 * 2.0;

/// Room for the sort triangle beside a header label.
pub(crate) const SORT_ARROW: f32 = 10.0 + space::S2;

/// Measure the fitted columns against the listing's own content.
///
/// Cheap because none of them needs every entry looked at:
///
/// - **Status** is a glyph under its header, and only there at all in a synced folder — see
///   [`crate::fs::Dir::synced`], decided when the listing was read.
/// - **Hard links** is a glyph header over a count, and only there once some file in the folder has
///   been counted at two names or more — see [`crate::hardlinks`]. Fitted to the glyph, or to the
///   widest count if that is wider.
/// - **Keywords** is only there where rows can have any — see [`crate::fs::Dir::keyed`] — and is
///   measured from the longest keywords a row on show has, between [`KEYWORDS_MIN`] and
///   [`KEYWORDS_MAX`]. One hash lookup per row, once per listing.
///
/// - **Modified** is a fixed-width format, so one measurement of the template does.
/// - **Size** is measured from the one value whose formatted text is longest, found
///   by comparing lengths in bytes rather than by laying anything out.
/// - **Type** has as many distinct labels as the folder has kinds of file, which is
///   a handful — collected with a small linear scan of already-interned strings.
///
/// A column the user has hidden is not measured where measuring costs anything — Keywords is a
/// hash lookup a row — and comes out of [`resolved_widths`] at zero whatever is stored for it.
pub(crate) fn measure_columns(
    ui: &Ui,
    t: &Theme,
    tab: &mut Tab,
    columns: &Columns,
    scratch: &mut String,
) {
    // The font the cells are drawn in — `meta_font` in `rows` — or every fitted column comes out
    // wider than what is in it, by the difference between two sizes of the same text.
    let font = t.fonts.caption.clone();
    let measure = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };
    // A header can be wider than everything under it.
    let over = tab.dir.clone();
    let header_of = |column: Column| {
        ui.painter()
            .layout_no_wrap(
                column.header_over(over.as_deref()).to_owned(),
                t.fonts.body_strong.clone(),
                Color32::PLACEHOLDER,
            )
            .size()
            .x
            + SORT_ARROW
    };

    let mut size_width: f32 = 0.0;
    let mut type_width: f32 = 0.0;
    let mut keywords_width: f32 = 0.0;
    let synced = tab.dir.as_ref().is_some_and(|dir| dir.synced);
    let keyed = columns.shown(Column::Keywords) && tab.dir.as_ref().is_some_and(|dir| dir.keyed());

    if let Some(dir) = tab.dir.clone() {
        // Size: the widest formatted string, found without formatting them all.
        //
        // **Except while the folders are being measured**, where it is the template instead. The
        // totals arrive one folder at a time over seconds — see [`crate::sizes`] — and a column
        // measured from its content would then be re-measured every time one landed, so a listing
        // would spend the whole measurement widening by a pixel or two under the reader's hands.
        // The template is what [`fmt::size`] can produce at its widest, so the column is decided
        // once and no answer can ever need more room than it has.
        if tab.sizes.on {
            // And the same shape in every other unit, because the template's length is the widest
            // there is and its `KB` is not: `1023 MB` is the same four digits and wider.
            size_width = measure(fmt::SIZE_TEMPLATE);
            for unit in 2..=5 {
                scratch.clear();
                fmt::size(1023 * 1024u64.pow(unit), scratch);
                size_width = size_width.max(measure(scratch));
            }
        } else {
            // Not the longest string by bytes: `4.56 MB` and `1.23 KB` are the same length and
            // not the same width, and a column measured off the narrower one crops the other. The
            // largest value in each of `fmt::size`'s layouts instead, which is that layout's widest
            // text — found with integer arithmetic, so the scan formats nothing, and then at most
            // `SIZE_SHAPES` strings measured. See [`fmt::size_shape`].
            let mut largest = [None::<u64>; fmt::SIZE_SHAPES];
            for &i in &tab.order {
                let entry = &dir.entries[i as usize];
                if !entry.is_dir() {
                    let slot = &mut largest[fmt::size_shape(entry.size)];
                    *slot = Some(slot.map_or(entry.size, |seen| seen.max(entry.size)));
                }
            }
            for bytes in largest.into_iter().flatten() {
                scratch.clear();
                fmt::size(bytes, scratch);
                size_width = size_width.max(measure(scratch));
            }
        }

        // Type: the distinct *extensions*, which is a much smaller set than the
        // entries and maps one-to-one onto the labels. Comparing extensions rather
        // than rendered labels means the inner loop touches no allocated string at
        // all, and the cap stops a folder of ten thousand unique extensions from
        // turning this into a quadratic scan.
        let mut seen: Vec<&str> = Vec::new();
        for &i in &tab.order {
            let ext = dir.ext(i as usize);
            let is_dir = dir.entries[i as usize].is_dir();
            let key = if is_dir { "\0dir" } else { ext };
            if seen.contains(&key) {
                continue;
            }
            scratch.clear();
            fmt::type_label(ext, is_dir, scratch);
            type_width = type_width.max(measure(scratch));
            if seen.len() >= 96 {
                break;
            }
            seen.push(key);
        }

        // Keywords: the longest set on show, by bytes, and then measured once. One hash lookup a
        // row, so none at all while no file anywhere has keywords — which is a flattened drive's
        // million lookups for nothing.
        let store = keyed.then(crate::fs::keywords::read);
        if let Some(store) = store.filter(|store| !store.is_empty()) {
            let longest = tab
                .order
                .iter()
                .filter_map(|&i| store.get(dir.key(i as usize)?))
                .max_by_key(|text| text.len());
            if let Some(text) = longest {
                keywords_width = measure(text);
            }
        }
    }

    let date_width = measure(fmt::DATE_TEMPLATE);

    tab.widths[Column::Keywords.index()] = if keyed {
        (keywords_width.max(header_of(Column::Keywords)) + CELL_PAD * 2.0)
            .clamp(KEYWORDS_MIN, KEYWORDS_MAX)
            .ceil()
    } else {
        0.0
    };
    tab.widths[Column::Status.index()] = if synced {
        GLYPH_COLUMN
    } else {
        0.0
    };
    // The largest count is the widest one: every digit is the same width in the caption font.
    tab.widths[Column::HardLinks.index()] = match tab.hard_links.iter().copied().max() {
        Some(most) if most > 1 => {
            (measure(&most.to_string()).max(GLYPH) + CELL_PAD * 2.0).ceil()
        }
        _ => 0.0,
    };
    tab.widths[Column::Size.index()] =
        (size_width.max(header_of(Column::Size)) + CELL_PAD * 2.0).ceil();
    tab.widths[Column::Type.index()] =
        (type_width.max(header_of(Column::Type)) + CELL_PAD * 2.0).ceil();
    tab.widths[Column::Modified.index()] =
        (date_width.max(header_of(Column::Modified)) + CELL_PAD * 2.0).ceil();
    tab.widths_measured = true;
}

/// A [glyph-only](Column::glyph_only) column: one glyph and its padding, and no room for a sort
/// arrow — the header turning to the sorted ink is what says the listing is in its order.
pub(crate) const GLYPH_COLUMN: f32 = GLYPH + CELL_PAD * 2.0;

/// The least the Keywords column is fitted to: room to aim at, in a folder where nothing has any yet
/// — an empty cell is the one place a first keyword can be typed into.
pub(crate) const KEYWORDS_MIN: f32 = 120.0;
/// And the most. Keywords are a list, and a list that took half the pane would be taking it from
/// the names; the rest is elided, and the tooltip has it whole.
pub(crate) const KEYWORDS_MAX: f32 = 260.0;

/// Widths for this frame: the fitted columns as stored, less any the user has hidden, and whatever
/// is left for Name. Indexed by [`Column::index`] whatever order they are drawn in.
///
/// When the pane is too narrow for all of them, the fitted columns give way —
/// Type first, then Modified, then Keywords, then Status, then Hard links — because a name you cannot
/// read is worse than a date you cannot see.
pub(crate) fn resolved_widths(tab: &Tab, columns: &Columns, total: f32) -> [f32; Column::COUNT] {
    let mut widths = tab.widths;
    for column in columns.hidden() {
        widths[column.index()] = 0.0;
    }
    const NAME_MIN: f32 = 120.0;
    let fitted = |widths: &[f32; Column::COUNT]| widths[1..].iter().sum::<f32>();

    let mut spare = total - fitted(&widths);
    if spare < NAME_MIN {
        for column in [
            Column::Type,
            Column::Modified,
            Column::Keywords,
            Column::Status,
            Column::HardLinks,
            Column::Size,
        ] {
            if spare >= NAME_MIN {
                break;
            }
            let index = column.index();
            spare += widths[index];
            widths[index] = 0.0;
        }
    }
    widths[0] = (total - fitted(&widths)).max(NAME_MIN);
    widths
}

/// Where each column runs across `rect`: laid out left to right in `order`, and indexed by
/// [`Column::index`] so a cell is found by what it is rather than by where it has been put. A column
/// with no width is an empty span at the place it would have been.
pub(crate) fn column_x(
    rect: Rect,
    widths: &[f32; Column::COUNT],
    order: &[Column; Column::COUNT],
) -> [Rangef; Column::COUNT] {
    let mut spans = [Rangef::point(rect.left()); Column::COUNT];
    let mut x = rect.left();
    for column in order {
        let width = widths[column.index()];
        spans[column.index()] = Rangef::new(x, x + width);
        x += width;
    }
    spans
}

/// The header: labels, the sort indicator, and the drag grips between columns.
///
/// **A click sorts, a drag moves the column, and a right click is the list of columns to tick** —
/// the same three gestures the sidebar's group headings have, and egui tells the first two apart by
/// distance. A move and a tick are pushed as actions, because the arrangement is the window's and
/// every pane draws it: see [`crate::app::Action::MoveColumn`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn header_strip(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &mut Tab,
    columns: &Columns,
    widths: &[f32; Column::COUNT],
    out: &mut Vec<Action>,
) {
    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.surfaces.header);
    let order = columns.order();
    let spans = column_x(rect, widths, &order);
    // The columns on show, left to right, for a header being dragged to land between.
    let mut shown: Vec<(Column, Rangef)> = Vec::with_capacity(Column::COUNT);
    // The header under the pointer's drag, and whether this is the frame it was let go.
    let mut held: Option<(Column, bool)> = None;

    for column in order {
        let index = column.index();
        if widths[index] <= 0.0 {
            continue;
        }
        shown.push((column, spans[index]));
        let cell = Rect::from_x_y_ranges(spans[index], rect.y_range());
        let response = ui.interact(cell, Id::new(("th", pane, index)), Sense::click_and_drag());
        if response.dragged() {
            ui.painter()
                .rect_filled(cell, CornerRadius::ZERO, t.bg.control_active);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(cell, CornerRadius::ZERO, t.bg.control_hover);
        }
        if response.clicked() {
            out.push(Action::Sort { pane, column });
        }
        if response.dragged() || response.drag_stopped() {
            held = Some((column, response.drag_stopped()));
        }
        columns_menu(&response, columns, out);

        let sorted = tab.sort_by == column;
        let color = if response.hovered() || sorted {
            t.text.primary
        } else {
            t.text.secondary
        };
        let arrow = if sorted { SORT_ARROW } else { 0.0 };
        let inner = Rect::from_min_max(
            pos2(cell.left() + CELL_PAD, cell.top()),
            pos2(cell.right() - CELL_PAD, cell.bottom()),
        );

        // A column of glyphs has a glyph for a header, in the header's ink rather than the cells'
        // colours, with the word in the tooltip — and no sort arrow. Drawn where the cells draw
        // theirs, so the two line up.
        if column.glyph_only() {
            let under = if response.hovered() { t.bg.control_hover } else { t.surfaces.header };
            header_glyph(ui.painter(), column, icon_rect(cell, inner.left(), GLYPH), color, under);
            azur_egui_theme::components::tooltip(response, column.header());
        } else {
            let galley = truncated(
                ui.painter(),
                column.header_over(tab.dir.as_deref()),
                t.fonts.body_strong.clone(),
                color,
                (inner.width() - arrow).max(0.0),
            );
            let label_width = galley.size().x;
            if column.numeric() {
                // Right-aligned, with the arrow tucked inside the padding so the label
                // stays flush with the numbers below it.
                let shifted = Rect::from_min_max(
                    inner.min,
                    pos2(inner.right() - arrow, inner.bottom()),
                );
                text_right(ui.painter(), shifted, galley);
                if sorted {
                    let x = shifted.right() + space::S2 * 0.5;
                    sort_glyph(ui, t, icon_rect(cell, x, 10.0), tab.ascending, color);
                }
            } else {
                text_left(ui.painter(), inner, galley);
                if sorted {
                    let x = inner.left() + label_width + space::S2;
                    sort_glyph(ui, t, icon_rect(cell, x, 10.0), tab.ascending, color);
                }
            }
        }
    }

    // The grips, **in a second pass, after every cell.** A grip straddles an edge, so half of it is
    // over the next column's header — and a header can be dragged now, which it could not when it
    // only took clicks. Interacted after all of them, every grip is on top of both its neighbours,
    // and a drag that starts on an edge resizes rather than picking the next column up.
    for &(column, span) in &shown {
        let index = column.index();
        let cell = Rect::from_x_y_ranges(span, rect.y_range());
        // The grip on this column's right edge. Not on Name, whose width is whatever
        // the others leave behind.
        if column != Column::Name {
            let grip = Rect::from_min_max(
                pos2(cell.right() - GRIP, rect.top()),
                pos2(cell.right() + GRIP, rect.bottom()),
            );
            let drag = ui.interact(
                grip,
                Id::new(("th-grip", pane, index)),
                Sense::click_and_drag(),
            );
            if drag.hovered() || drag.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                ui.painter().rect_filled(
                    Rect::from_min_size(pos2(cell.right() - 1.0, rect.top()), vec2(1.0, rect.height())),
                    CornerRadius::ZERO,
                    t.accent.default,
                );
            }
            if drag.dragged() {
                let delta = drag.drag_delta().x;
                // A glyph column fits in less than a drag otherwise stops at, and would jump wider
                // on the first pixel of one.
                let least = if column.glyph_only() { GLYPH_COLUMN } else { 48.0 };
                tab.widths[index] = (tab.widths[index] + delta).clamp(least, 480.0);
            }
            // Double-clicking an edge re-fits the column, which is the gesture every
            // table in every operating system has.
            if drag.double_clicked() {
                tab.widths_measured = false;
            }
        }
    }

    if let Some((from, released)) = held {
        moving_column(ui, t, rect, &shown, from, released, out);
    }

    // A rule under the header, which is what separates a header strip from the
    // rows in Azur's table.
    //
    // **`surfaces.separator`, and it was a hard-coded `#202329` — a bug in both light palettes.**
    // That hex is `tokens::palette::GRAY_4`, which is the *dark* theme's `stroke-subtle`, so the
    // one line in the window that is supposed to divide the header from the listing was drawing a
    // near-black hairline across every pane on a near-white panel. It looked right in the dark
    // theme by coincidence, which is exactly why nothing caught it: the value was correct for the
    // palette it was written in and had no way of following the palette anywhere else.
    //
    // It is the same colour and the same decision as the seam between two panes — a boundary
    // between two surfaces — so it reads the region rather than a literal. See
    // [`crate::theme::Surfaces::separator`].
    ui.painter().rect_filled(
        Rect::from_min_size(rect.left_bottom() - vec2(0.0, 1.0), vec2(rect.width(), 1.0)),
        CornerRadius::ZERO,
        t.surfaces.separator,
    );
}

/// Which columns the details view shows, as a list of ticks, on every header.
///
/// **Sticky**, like the sidebar's list of groups and for the same reason: these are settings rather
/// than commands, and taking two columns off should not mean opening the menu twice. Listed in the
/// order they are drawn in, hidden ones where they would be. **Name is ticked and greyed** — it is
/// the one column that cannot go, and leaving it out of the list would read as a column this
/// program does not have.
fn columns_menu(target: &egui::Response, columns: &Columns, out: &mut Vec<Action>) {
    ContextMenu::new(target).sticky(true).show(&target.ctx, |ui| {
        for column in columns.order() {
            let shown = columns.shown(column);
            let item = MenuItem::new(column.header()).selected(shown);
            if column == Column::Name {
                ui.add_enabled(false, item);
            } else if ui.add(item).clicked() {
                out.push(Action::ShowColumn {
                    column,
                    shown: !shown,
                });
            }
        }
        // Only once there is something to undo.
        if !columns.is_default() {
            azur_egui_theme::components::menu_divider(ui);
            if ui.add(MenuItem::new("Reset columns")).clicked() {
                out.push(Action::ResetColumns);
                ui.close();
            }
        }
    });
}

/// A header being dragged: where it would go while it is held, and the move when it is let go.
///
/// The accent caret the sidebar draws between groups, stood upright on the edge of a column.
fn moving_column(
    ui: &Ui,
    t: &Theme,
    strip: Rect,
    shown: &[(Column, Rangef)],
    from: Column,
    released: bool,
    out: &mut Vec<Action>,
) {
    let Some(pointer) = ui.ctx().pointer_interact_pos() else {
        return;
    };
    let Some((to, span, after)) = column_landing(shown, pointer.x) else {
        return;
    };
    if released {
        if to != from {
            out.push(Action::MoveColumn { from, to, after });
        }
        return;
    }
    let x = if after { span.max } else { span.min };
    // Kept inside the strip, so the caret on its outer edges is not half clipped away.
    let x = x.clamp(strip.left() + 1.0, strip.right() - 1.0);
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(x - 1.0, strip.top()), pos2(x + 1.0, strip.bottom())),
        CornerRadius::same(radius::CIRCULAR),
        t.accent.default,
    );
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
}

/// Which column a dragged header is going beside, where that column is, and whether it goes after
/// it: the first column the pointer is left of the right edge of, and which half of it. Right of the
/// last column is after the last column; left of the first is before the first.
fn column_landing(shown: &[(Column, Rangef)], x: f32) -> Option<(Column, Rangef, bool)> {
    let &(last, span) = shown.last()?;
    match shown.iter().find(|(_, span)| x < span.max) {
        Some(&(column, span)) => Some((column, span, x >= span.center())),
        None => Some((last, span, true)),
    }
}

/// The glyph a [glyph-only](Column::glyph_only) column's header shows: the one its cells show most.
fn header_glyph(p: &egui::Painter, column: Column, rect: Rect, ink: Color32, under: Color32) {
    match column {
        Column::Status => crate::icons::sync_status(p, rect, crate::fs::dir::Sync::Online, ink, under),
        Column::HardLinks => crate::icons::chain(p, rect, ink),
        // Named rather than `_`, so a new column has to say whether it is one.
        Column::Name | Column::Keywords | Column::Size | Column::Type | Column::Modified => {}
    }
}

pub(crate) fn sort_glyph(ui: &Ui, _t: &Theme, rect: Rect, ascending: bool, color: Color32) {
    if ascending {
        azur_icons::sort_asc(ui.painter(), rect, color);
    } else {
        azur_icons::sort_desc(ui.painter(), rect, color);
    }
}
