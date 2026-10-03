//! The column strip: how wide each column ends up, and where its header sits.

use super::*;
use azur_egui_theme::tokens::typography;

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
/// - **Keywords** is only there where rows can have any — see [`crate::fs::Dir::keyed`] — and is
///   measured from the longest keywords a row on show has, between [`KEYWORDS_MIN`] and
///   [`KEYWORDS_MAX`]. One hash lookup per row, once per listing.
///
/// - **Modified** is a fixed-width format, so one measurement of the template does.
/// - **Size** is measured from the one value whose formatted text is longest, found
///   by comparing lengths in bytes rather than by laying anything out.
/// - **Type** has as many distinct labels as the folder has kinds of file, which is
///   a handful — collected with a small linear scan of already-interned strings.
pub(crate) fn measure_columns(ui: &Ui, t: &Theme, tab: &mut Tab, scratch: &mut String) {
    let font = t.fonts.body.clone();
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
    let keyed = tab.dir.as_ref().is_some_and(|dir| dir.keyed());

    if let Some(dir) = tab.dir.clone() {
        // Size: the longest formatted string, found without formatting them all.
        //
        // **Except while the folders are being measured**, where it is the template instead. The
        // totals arrive one folder at a time over seconds — see [`crate::sizes`] — and a column
        // measured from its content would then be re-measured every time one landed, so a listing
        // would spend the whole measurement widening by a pixel or two under the reader's hands.
        // The template is what [`fmt::size`] can produce at its widest, so the column is decided
        // once and no answer can ever need more room than it has.
        if tab.sizes.on {
            size_width = measure(fmt::SIZE_TEMPLATE);
        } else {
            let mut widest_size = 0u64;
            let mut longest = 0usize;
            for &i in &tab.order {
                let entry = &dir.entries[i as usize];
                if entry.is_dir() {
                    continue;
                }
                scratch.clear();
                fmt::size(entry.size, scratch);
                if scratch.len() > longest {
                    longest = scratch.len();
                    widest_size = entry.size;
                }
            }
            scratch.clear();
            fmt::size(widest_size, scratch);
            size_width = measure(scratch);
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

        // Keywords: the longest set on show, by bytes, and then measured once — the same shortcut
        // Size takes.
        if keyed {
            let store = crate::fs::keywords::read();
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
        (header_of(Column::Status).max(GLYPH) + CELL_PAD * 2.0).ceil()
    } else {
        0.0
    };
    tab.widths[Column::Size.index()] =
        (size_width.max(header_of(Column::Size)) + CELL_PAD * 2.0).ceil();
    tab.widths[Column::Type.index()] =
        (type_width.max(header_of(Column::Type)) + CELL_PAD * 2.0).ceil();
    tab.widths[Column::Modified.index()] =
        (date_width.max(header_of(Column::Modified)) + CELL_PAD * 2.0).ceil();
    tab.widths_measured = true;
}

/// The least the Keywords column is fitted to: room to aim at, in a folder where nothing has any yet
/// — an empty cell is the one place a first keyword can be typed into.
pub(crate) const KEYWORDS_MIN: f32 = 120.0;
/// And the most. Keywords are a list, and a list that took half the pane would be taking it from
/// the names; the rest is elided, and the tooltip has it whole.
pub(crate) const KEYWORDS_MAX: f32 = 260.0;

/// Widths for this frame: the fitted columns as stored, and whatever is left
/// for Name.
///
/// When the pane is too narrow for all of them, the fitted columns give way —
/// Type first, then Modified, then Keywords, then Status — because a name you cannot read is worse
/// than a date you cannot see.
pub(crate) fn resolved_widths(tab: &Tab, total: f32) -> [f32; Column::COUNT] {
    let mut widths = tab.widths;
    const NAME_MIN: f32 = 120.0;
    let fitted = |widths: &[f32; Column::COUNT]| widths[1..].iter().sum::<f32>();

    let mut spare = total - fitted(&widths);
    if spare < NAME_MIN {
        for column in [
            Column::Type,
            Column::Modified,
            Column::Keywords,
            Column::Status,
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

/// The left edge of each column, given the resolved widths, and the right edge of the last.
pub(crate) fn column_x(rect: Rect, widths: &[f32; Column::COUNT]) -> [f32; Column::COUNT + 1] {
    let mut edges = [rect.left(); Column::COUNT + 1];
    for i in 0..Column::COUNT {
        edges[i + 1] = edges[i] + widths[i];
    }
    edges
}

/// The header: labels, the sort indicator, and the drag grips between columns.
pub(crate) fn header_strip(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &mut Tab,
    widths: &[f32; Column::COUNT],
    out: &mut Vec<Action>,
) {
    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.surfaces.header);
    let edges = column_x(rect, widths);

    for (index, column) in Column::ALL.into_iter().enumerate() {
        if widths[index] <= 0.0 {
            continue;
        }
        let cell = Rect::from_min_max(
            pos2(edges[index], rect.top()),
            pos2(edges[index + 1], rect.bottom()),
        );
        let response = ui.interact(cell, Id::new(("th", pane, index)), Sense::click());
        if response.hovered() {
            ui.painter()
                .rect_filled(cell, CornerRadius::ZERO, t.bg.control_hover);
        }
        if response.clicked() {
            out.push(Action::Sort { pane, column });
        }

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

        // The grip on this column's right edge. Not on Name, whose width is whatever
        // the others leave behind.
        if index > 0 {
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
                tab.widths[index] = (tab.widths[index] + delta).clamp(48.0, 480.0);
            }
            // Double-clicking an edge re-fits the column, which is the gesture every
            // table in every operating system has.
            if drag.double_clicked() {
                tab.widths_measured = false;
            }
        }
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

pub(crate) fn sort_glyph(ui: &Ui, _t: &Theme, rect: Rect, ascending: bool, color: Color32) {
    if ascending {
        azur_icons::sort_asc(ui.painter(), rect, color);
    } else {
        azur_icons::sort_desc(ui.painter(), rect, color);
    }
}
