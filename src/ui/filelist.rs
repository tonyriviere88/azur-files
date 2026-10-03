//! The details view: a sticky header over virtualised rows.
//!
//! This is the hot loop, and it is built so that the cost of a frame depends on
//! the size of the *window*, never on the size of the folder:
//!
//! - **Only visible rows exist.** The scroll area is told the total height and asked
//!   for the visible range; a folder of 300,000 files draws the same forty rows as
//!   a folder of forty.
//! - **One widget for the whole list.** Not one per row: a single `interact` over
//!   the viewport, with the row derived from the pointer's `y`. Forty widget ids
//!   per frame per pane would otherwise be registered, hit-tested and animated for
//!   nothing.
//! - **No allocation per cell.** Sizes, dates and type names are written into one
//!   `String` that is cleared and reused, and egui's galley cache is keyed on the
//!   finished text — so a row that has not changed is a cache lookup, not a
//!   shaping pass.
//! - **Columns measured once.** Modified is a fixed-width format, Size is measured
//!   from the single longest value, and Type from the handful of distinct labels in
//!   the folder. None of it is re-measured while scrolling.
//!
//! The Name column takes what is left, and the other three are sized to their
//! content — and draggable, because a measurement is a starting point and not a
//! decision.

use azur_egui_theme::components::{galley_on_baseline, ink_baseline};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::{radius, space, typography};
use egui::{pos2, vec2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};

use crate::app::Action;
use crate::fs::time::LocalZone;
use crate::fs::{fmt, Column};
use crate::icons;
use crate::pane::{PaneId, Tab, ROW_HEIGHT};
// `crate::icons` is this program's painted glyphs; this is the shell's icon service.
use crate::shell::icons as shell_icons;
use crate::shell::icons::Icons;
use crate::shell::links;
use crate::theme::Theme;
use crate::ui::{
    icon_rect, row_fill, row_fill_quiet, selection_bar, text_center, text_left, text_right,
    truncated,
};

/// The header strip: a body line with `space-2` above and below, which is Azur's
/// table header at this density.
pub const HEADER_HEIGHT: f32 = typography::LINE_BODY + space::S2 * 2.0;
/// The status line at the bottom of a pane.
pub const STATUS_HEIGHT: f32 = 22.0;
/// The glyph in a row.
///
/// 16, which is the size the shell small image list is drawn at — so a row does not
/// reflow when a painted fallback glyph is replaced by the real icon a frame later.
const GLYPH: f32 = 16.0;
/// Space either side of a cell's text.
const CELL_PAD: f32 = space::S3;
/// Two points off the top of every cell's text in a row.
///
/// Text centred in a box is centred on its *line box*, which reserves room under the
/// baseline for descenders — so a line of mostly-x-height text put beside a 16px icon
/// centred on its own ink reads two points low. Which it did: a guide drawn through the
/// icon's centre passed above the x-height of every name in the listing.
///
/// Applied to all four cells rather than to the name alone, or the columns of one row would
/// no longer sit on the same line as each other. The icon stays where it is — it is the
/// thing the text is being brought level with.
pub const CELL_LIFT: f32 = 2.0;
/// How close to a column edge counts as grabbing it.
const GRIP: f32 = 4.0;
/// The twisty's box: the chevron that opens and shuts a folder in a
/// [`crate::pane::FlatMode::Tree`] listing.
///
/// Fourteen — [`crate::ui::TOOL_ICON`]'s size, which is what every other painted glyph in this
/// window's chrome is drawn at — in a box the height of the row. What it is *hit* on is that whole
/// box and not the chevron's ink: a 6-point arrowhead is not a click target.
const TWISTY: f32 = 14.0;
/// Where a row's own column starts: indented by how far down the tree it is.
///
/// `row.left() + CELL_PAD + 2.0` for every row of every other listing, where `depth` is 0 — the
/// two modes share one expression rather than branching, so an ordinary listing cannot drift from
/// the top level of a tree.
fn stem_x(row: Rect, depth: usize) -> f32 {
    row.left() + CELL_PAD + 2.0 + depth as f32 * INDENT
}

/// The box a tree row's twisty is drawn and clicked in: the row's own indent, one level wide.
///
/// One statement of it, because three things have to agree about where the chevron is — the
/// painting, the click, and the test that checks the click lands on it rather than beside it. A
/// test that restated the arithmetic would happily pass a layout that had drifted.
pub(crate) fn twisty_rect(row: Rect, depth: usize) -> Rect {
    Rect::from_min_size(pos2(stem_x(row, depth), row.top()), vec2(TWISTY, ROW_HEIGHT))
}

/// How far one level of a tree steps in: **exactly the twisty's width.**
///
/// So a child's chevron sits under its parent's icon, and the chevrons down one branch make a
/// single straight column rather than a stagger. Any other number and the ladder wanders.
///
/// It also keeps the step cheap, which matters because the indent comes out of the Name column and
/// nothing else: a tree eight folders deep at Explorer's own 19-point step would have spent 152
/// points before the first letter of a name.
const INDENT: f32 = TWISTY;
/// How big a git badge is, and where on the row's icon it sits.
///
/// Eleven points of a sixteen-point icon, in the bottom-left corner: the shell's own proportion and
/// the shell's own corner — Windows puts its sync overlays there, TortoiseSVN puts its ticks there,
/// so a badge here lands where an eye trained on Explorer already looks. It is also the emptiest
/// corner of every glyph in [`crate::icons`], the page's fold being at the top right.
///
/// **Inside the icon, not beside it.** Hanging it off the left edge is what the first version did, and
/// at the left edge of a row is where the selection bar lives and where the pane's own border is a few
/// points further on — so the badge read as clipped even when it was not.
const BADGE: f32 = 10.0;
fn badge_rect(icon: Rect) -> Rect {
    Rect::from_min_size(
        pos2(icon.left() - 1.0, icon.bottom() - BADGE + 3.0),
        egui::Vec2::splat(BADGE),
    )
}
/// The console's switch at the left end of the status line.
///
/// Smaller than a toolbar button, because it has to sit inside a 22-point bar with air above and
/// below it — [`crate::ui::TOOL_SIZE`] is 24 and would touch both edges. It is the same 18 the
/// preview's in-field toggles take, for the same reason.
const SWITCH: f32 = 18.0;
/// A glyph on the status line: the branch, an arrow, the tick, the limit's warning.
///
/// Eleven rather than the 14 a toolbar uses. These sit *in* a line of 12-point text rather than in a
/// button of their own, and a glyph taller than the capitals beside it reads as an icon that has
/// wandered in.
const MARK: f32 = 11.0;
/// What separates two of the greys on the right of the status line.
///
/// Air alone is not enough between `0.3 ms` and `1016 KB`: two figures in the same ink with a gap
/// between them read as one phrase. The counts need no separator in front of them — they are a
/// different colour, which is a stronger boundary than any character.
const SEPARATOR: &str = "   ·   ";
/// How far above the middle of the bar everything on it sits.
///
/// **Asked for by eye, and it is the last point of an argument arithmetic cannot finish.** The line is
/// centred by its ink rather than by its line box — see [`status_geometry`], which is what stopped it
/// reading a point and a half low at 1.5× — and one point above *that* is where it was wanted. The
/// bar's bottom edge is not its bottom edge: the window's own 1px border is drawn over it, and
/// `background-layer-alt` against `stroke-default` is a boundary the eye reads as the end of the band
/// while the arithmetic does not.
///
/// It moves the whole line and not only the words: the switch, the glyphs and the text all go up
/// together, or the level they were brought to would be given away a point at a time.
const NUDGE: f32 = 1.0;
/// The least air between the status line's two groups.
///
/// **Wider than a separator, because it is a bigger boundary than one.** Without it a narrow pane
/// brought `13 changed` and `0.3 ms` to within eight points of each other and the line read as one
/// long phrase in three colours. The right-hand group gives way instead — that is what having
/// priority means — so what is on the bar is always either separated or absent.
const GROUP_GAP: f32 = space::S6;
/// Nothing under the last file — a row and a half of it.
///
/// So that **the folder is always somewhere to right-click.** The menu for the folder itself —
/// where `New folder`, `Paste` and `Refresh` live — is the one you get by right-clicking a part
/// of the listing that is not a file, and in a folder taller than the pane there was no such
/// part: every pixel from the header to the status line was a row. The only way to reach it was
/// to scroll to the end and find the gap, if the last row happened to leave one.
///
/// Enough to be unmissable rather than merely present, which is also what makes a rubber band easy
/// to start from below the files. It was **three rows**, and three is more than that argument buys:
/// the end of a long folder read as though the listing had stopped short of the pane, and the slack
/// is charged twice over because it is content — a folder that nearly fills its pane grows a
/// scrollbar for it. Half of it is still one and a half times a click target.
///
/// Content and not a margin: at the top of a long folder every pixel of the pane is still rows, and
/// the space appears as you reach the end. In points rather than in rows because half a row is not a
/// row — see the extent in [`rows`], which is `ScrollArea::show_rows`' arithmetic with this in place
/// of a whole number of them.
pub const TAIL: f32 = ROW_HEIGHT * 1.5;
/// Room for the sort triangle beside a header label.
const SORT_ARROW: f32 = 10.0 + space::S2;

/// What the listing wants the application to do that it cannot do itself.
pub struct Outcome {
    /// Prefetch this folder — the cursor has landed on it.
    pub prefetch: Option<std::path::PathBuf>,
    /// Where each visible folder row was drawn, so files can be dropped *into* it.
    ///
    /// Reported rather than resolved here: a drop target has to be answered synchronously from
    /// an OLE callback on another stack, so the application publishes rects rather than the
    /// listing being asked at the moment of the drop. Read one frame later than it is written,
    /// which is a frame the rows have not moved in.
    pub drop_rows: Vec<(Rect, std::path::PathBuf)>,
    /// The rows' own rectangle — where a drop into *this* folder lands, and what lights up
    /// for one.
    ///
    /// Not the pane: the column header sorts and the status line counts, and neither takes a
    /// drop, so promising them as the destination was promising the wrong thing. `NOTHING` for
    /// a pane too short to list a single row, which cannot show a highlight either.
    pub drop_area: Rect,
}

/// Draw the header and the rows inside `rect`, and the status line in `floor`.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    t: &Theme,
    zone: &LocalZone,
    rect: Rect,
    // The status line's own band, across the whole width of the pane and under everything in it —
    // including the preview panel, whichever side that is docked to.
    //
    // Handed in rather than taken off the bottom of `rect`, which is what this used to do. `rect` is
    // the listing's region, and when the preview panel is docked to the *right* that region is a
    // column: a status line inside it stopped where the panel began and the pane had two different
    // things along its bottom edge. It is the pane's floor rather than the listing's, so the pane is
    // what decides where it goes — see `App::pane`.
    floor: Rect,
    pane: PaneId,
    tab: &mut Tab,
    focused: bool,
    icons_cache: &mut crate::shell::icons::Icons,
    links_cache: &mut crate::shell::links::Links,
    cut: &[std::path::PathBuf],
    status: Option<&str>,
    // Whether this pane's console is open, for the switch at the left of the status line.
    //
    // Told rather than inferred from `reserved`: a pane too short to give the console a band leaves
    // that at zero, and a switch that unlatched itself when the pane was squeezed would be reporting
    // the pane's height as the console's state.
    console_open: bool,
    // The band at the bottom of `rect` that belongs to somebody else — the console panel.
    //
    // The rows stop short of it. Without this they are laid out across the console and the console
    // merely paints over them — which reads as a floating overlay rather than as a panel in the stack,
    // and hit-tests as one too.
    reserved: f32,
    scratch: &mut String,
    out: &mut Vec<Action>,
) -> Outcome {
    // The frame's clock, for the one thing here that depends on how long something has taken:
    // whether a scan has been slow enough to admit to. See [`crate::pane::SLOW_SCAN`].
    let now = ui.input(|i| i.time);
    let mut outcome = Outcome {
        prefetch: None,
        drop_rows: Vec::new(),
        drop_area: Rect::NOTHING,
    };

    let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER_HEIGHT));
    // A pane squeezed shorter than its own furniture would give an inverted body
    // rect, which turns into an empty visible range and an underflow downstream. The
    // header and the status line are worth more than a row nobody could read.
    if rect.bottom() - reserved <= header.bottom() + ROW_HEIGHT {
        header_strip(ui, t, header, pane, tab, &resolved_widths(tab, rect.width()), out);
        status_line(
            ui, t, floor, pane, tab, console_open, status, now, scratch, out,
        );
        return outcome;
    }
    let body = Rect::from_min_max(
        header.left_bottom(),
        pos2(rect.right(), rect.bottom() - reserved),
    );
    outcome.drop_area = body;

    // Columns have to be resolved before the header can be drawn, and measuring
    // needs a painter — so this happens first, once per listing.
    if !tab.widths_measured {
        measure_columns(ui, t, tab, scratch);
    }
    let widths = resolved_widths(tab, body.width());

    header_strip(ui, t, header, pane, tab, &widths, out);

    // Whether any rows were drawn, and so whether anything is listening for a click over
    // the body. See [`bare_body`].
    let mut listed = false;

    if tab.dir.as_ref().is_some_and(|d| d.error.is_some()) {
        let message = tab
            .dir
            .as_ref()
            .and_then(|d| d.error.clone())
            .unwrap_or_default();
        text_center(
            ui.painter(),
            body,
            t.fonts.body.clone(),
            t.status.danger,
            &message,
        );
    } else if tab.dir.is_none() {
        // A scan that has not landed yet, and **only once it has been long enough to notice**.
        // A local folder comes back in single-digit milliseconds, so this used to be a word that
        // flashed up and vanished on every navigation, in the place the listing was about to be.
        // See [`crate::pane::SLOW_SCAN`]; the frame that gets here at the right moment is booked
        // by `App::start_scans`.
        if tab.waiting_visibly(now) {
            text_center(
                ui.painter(),
                body,
                t.fonts.body.clone(),
                t.text.tertiary,
                "Reading…",
            );
        }
    } else if tab.order.is_empty() {
        // "Empty" and "everything is filtered out" are different facts, and telling
        // them apart is the difference between a dead end and a hint.
        let message = if tab.dir.as_ref().is_some_and(|d| d.is_empty()) {
            "This folder is empty"
        } else if tab.filter.is_empty() {
            "Everything here is hidden — Ctrl+H shows it"
        } else {
            "Nothing matches the filter"
        };
        text_center(
            ui.painter(),
            body,
            t.fonts.body.clone(),
            t.text.tertiary,
            message,
        );
    } else {
        rows(
            ui,
            t,
            zone,
            body,
            pane,
            tab,
            focused,
            &widths,
            icons_cache,
            links_cache,
            cut,
            scratch,
            out,
            &mut outcome,
        );
        listed = true;
    }
    if !listed {
        bare_body(ui, body, pane, tab, out);
    }

    status_line(
        ui, t, floor, pane, tab, console_open, status, now, scratch, out,
    );
    outcome
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

/// Measure the three fitted columns against the listing's own content.
///
/// Cheap because none of the three needs every entry looked at:
///
/// - **Modified** is a fixed-width format, so one measurement of the template does.
/// - **Size** is measured from the one value whose formatted text is longest, found
///   by comparing lengths in bytes rather than by laying anything out.
/// - **Type** has as many distinct labels as the folder has kinds of file, which is
///   a handful — collected with a small linear scan of already-interned strings.
fn measure_columns(ui: &Ui, t: &Theme, tab: &mut Tab, scratch: &mut String) {
    let font = t.fonts.body.clone();
    let measure = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };
    // A header can be wider than everything under it.
    let header_of = |column: Column| {
        ui.painter()
            .layout_no_wrap(
                column.header().to_owned(),
                t.fonts.body_strong.clone(),
                Color32::PLACEHOLDER,
            )
            .size()
            .x
            + SORT_ARROW
    };

    let mut size_width: f32 = 0.0;
    let mut type_width: f32 = 0.0;

    if let Some(dir) = tab.dir.clone() {
        // Size: the longest formatted string, found without formatting them all.
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
    }

    let date_width = measure(fmt::DATE_TEMPLATE);

    tab.widths[Column::Size.index()] =
        (size_width.max(header_of(Column::Size)) + CELL_PAD * 2.0).ceil();
    tab.widths[Column::Type.index()] =
        (type_width.max(header_of(Column::Type)) + CELL_PAD * 2.0).ceil();
    tab.widths[Column::Modified.index()] =
        (date_width.max(header_of(Column::Modified)) + CELL_PAD * 2.0).ceil();
    tab.widths_measured = true;
}

/// Widths for this frame: the three fitted columns as stored, and whatever is left
/// for Name.
///
/// When the pane is too narrow for all four, the fitted columns give way from the
/// right — Type first, then Modified — because a name you cannot read is worse
/// than a date you cannot see.
fn resolved_widths(tab: &Tab, total: f32) -> [f32; 4] {
    let mut widths = tab.widths;
    const NAME_MIN: f32 = 120.0;

    let fitted = widths[1] + widths[2] + widths[3];
    let mut spare = total - fitted;
    if spare < NAME_MIN {
        for column in [Column::Type, Column::Modified, Column::Size] {
            if spare >= NAME_MIN {
                break;
            }
            let index = column.index();
            spare += widths[index];
            widths[index] = 0.0;
        }
    }
    widths[0] = (total - widths[1] - widths[2] - widths[3]).max(NAME_MIN);
    widths
}

/// The left edge of each column, given the resolved widths.
fn column_x(rect: Rect, widths: &[f32; 4]) -> [f32; 5] {
    let mut edges = [rect.left(); 5];
    for i in 0..4 {
        edges[i + 1] = edges[i] + widths[i];
    }
    edges
}

/// The header: labels, the sort indicator, and the drag grips between columns.
fn header_strip(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &mut Tab,
    widths: &[f32; 4],
    out: &mut Vec<Action>,
) {
    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg.layer_alt);
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
            column.header(),
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
        // the other three leave behind.
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
    ui.painter().rect_filled(
        Rect::from_min_size(rect.left_bottom() - vec2(0.0, 1.0), vec2(rect.width(), 1.0)),
        CornerRadius::ZERO,
        Color32::from_rgb(0x20, 0x23, 0x29),
    );
}

/// How wide the rename field should be, given how wide its text has turned out.
///
/// The Name column's width, unless the text needs more — and never past the right edge of the
/// row, because a field whose text is outside the window is no better than a cropped one.
///
/// `ink` is the laid-out width of the text, `left..right` the name cell, `row_right` the row's
/// own right edge.
fn rename_width(ink: f32, left: f32, right: f32, row_right: f32) -> f32 {
    /// Room for the caret past the last character. Without it, typing at the end pushes the
    /// text the field was just widened for straight back out of view.
    const CARET: f32 = space::S2;
    let wanted = ink + CARET;
    let limit = (row_right - CELL_PAD - left).max(MIN_FIELD);
    wanted.max(right - left).max(MIN_FIELD).min(limit)
}

/// Narrow enough to type in, on a pane too narrow for anything.
const MIN_FIELD: f32 = 60.0;
/// The field's height. The line plus room for a caret above and below it, and enough of a target
/// to click into. Nothing to do with where the *text* goes — that is `text_row`'s centre.
const FIELD_HEIGHT: f32 = 20.0;

/// The dashed rectangle that marks the row the keyboard is on but has not selected.
///
/// Square corners, because it is drawn one pixel inside a row whose fill has none — a rounded
/// ring inside a square edge reads as a mistake at this size.
///
/// One closed polyline rather than four dashed edges: `dashed_line` walks the points it is
/// given, so a rectangle handed over as five points comes back with its dashes in step all the
/// way round instead of restarting at every corner.
fn cursor_ring(painter: &egui::Painter, rect: Rect, color: Color32) {
    const DASH: f32 = 2.0;
    const GAP: f32 = 2.0;
    // Half-pixel centres, so a one-pixel stroke lands on one row of pixels rather than
    // straddling two and coming out grey and two wide.
    let r = Rect::from_min_max(
        pos2(rect.left() + 0.5, rect.top() + 0.5),
        pos2(rect.right() - 0.5, rect.bottom() - 0.5),
    );
    painter.extend(egui::Shape::dashed_line(
        &[
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ],
        Stroke::new(1.0, color),
        DASH,
        GAP,
    ));
}

fn sort_glyph(ui: &Ui, _t: &Theme, rect: Rect, ascending: bool, color: Color32) {
    if ascending {
        azur_icons::sort_asc(ui.painter(), rect, color);
    } else {
        azur_icons::sort_desc(ui.painter(), rect, color);
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn rows(
    ui: &mut Ui,
    t: &Theme,
    zone: &LocalZone,
    body: Rect,
    pane: PaneId,
    tab: &mut Tab,
    focused: bool,
    widths: &[f32; 4],
    icons_cache: &mut crate::shell::icons::Icons,
    links_cache: &mut crate::shell::links::Links,
    cut: &[std::path::PathBuf],
    scratch: &mut String,
    out: &mut Vec<Action>,
    outcome: &mut Outcome,
) {
    let Some(dir) = tab.dir.clone() else { return };
    let count = tab.order.len();

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(body.intersect(ui.clip_rect()));
    // The extent and the band below are counted in whole [`ROW_HEIGHT`]s, and the installed style's
    // item spacing is `space-3`. Anything egui adds per item would put the two out of step the moment
    // something scrolled — rows landing in the wrong place and the pointer hit-testing a different one
    // than it looks like, which is what this cost when `ScrollArea::show_rows` was reserving
    // `row_height + item_spacing.y` a row. Rows are painted at exact rects here, so it is nothing.
    child.spacing_mut().item_spacing = egui::Vec2::ZERO;

    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("rows", pane))
        .auto_shrink([false, false]);

    // Keyboard movement has to bring the cursor with it.
    //
    // Nudged into view rather than centred: a listing that recentres on every arrow
    // press is unreadable. `ScrollArea` takes an absolute offset, so the nudge is
    // computed against last frame's, which the pane records below.
    if let Some(offset) = tab.scroll_to.take() {
        // The rubber-band's auto-scroll: an absolute offset, which wins over the
        // cursor nudge because the pointer is the thing being followed.
        scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        tab.scroll_to_cursor = false;
    } else if tab.scroll_to_cursor {
        if let Some(at) = tab.cursor {
            let top = at as f32 * ROW_HEIGHT;
            let bottom = top + ROW_HEIGHT;
            let view = body.height();
            let mut offset = tab.scroll_y;
            if top < offset {
                offset = top;
            } else if bottom > offset + view {
                offset = bottom - view;
            }
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        tab.scroll_to_cursor = false;
    }

    // **`ScrollArea::show_rows`, with a fractional row at the end of it.** That is the whole reason
    // this is `show_viewport` and not the one-liner: `show_rows` counts the extent in whole rows, and
    // [`TAIL`] is a row and a half. Everything below is its own arithmetic — the extent, the band of
    // rows the viewport crosses, and one row past the end of it so a row half off the bottom edge is
    // still drawn — with the tail in points instead of rows.
    //
    // The band stops at the rows that exist, and the slack under them is deliberately not in it: the
    // extra is scrollable space and not a row, and anything drawn or hit-tested there would be a
    // file, where what is wanted is the folder. `first.min(last)` is that same rule for the
    // degenerate case — a viewport shorter than the tail, scrolled past every row — which asks for an
    // empty band rather than for a negative one.
    let output = scroll.show_viewport(&mut child, |ui, viewport| {
        ui.set_height(count as f32 * ROW_HEIGHT + TAIL);
        // `saturating_add`, because the offset is not always a sane one: the rubber band's
        // auto-scroll and `Tab::scroll_to` hand over whatever they were asked for, and egui does not
        // clamp it until after the viewport has been worked out. `show_rows`' own `+ 1` panics there.
        let last = ((viewport.max.y / ROW_HEIGHT).ceil() as usize)
            .saturating_add(1)
            .min(count);
        let first = ((viewport.min.y / ROW_HEIGHT).floor().max(0.0) as usize).min(last);
        let band = Rect::from_x_y_ranges(
            ui.max_rect().x_range(),
            (ui.max_rect().top() + first as f32 * ROW_HEIGHT)
                ..=(ui.max_rect().top() + last as f32 * ROW_HEIGHT),
        );
        // A child over the band, rather than `scope_builder`'s closure: the rows are painted at
        // explicit rects and the extent is the `set_height` above, so there is nothing for a scope to
        // measure and hand back — and this keeps `ui` meaning the band all the way down.
        let mut band_ui = ui.new_child(egui::UiBuilder::new().max_rect(band));
        let ui = &mut band_ui;
        // The widget ids in here are per row, so they have to come out the same at every scroll
        // position — which is what `show_rows` uses this for at exactly this point.
        ui.skip_ahead_auto_ids(first);
        let range = first..last;
        let visible = Rect::from_min_max(
            pos2(body.left(), ui.min_rect().top()),
            pos2(body.right(), ui.min_rect().top() + range.len() as f32 * ROW_HEIGHT),
        );
        // One interaction for the whole visible block, and the row worked out from
        // the pointer. Forty per-row widgets would cost forty ids, forty hit-tests
        // and forty animation slots for a hover highlight that arithmetic gives for
        // nothing.
        let response = ui.interact(
            visible,
            Id::new(("rows-hit", pane)),
            Sense::click_and_drag(),
        );
        let last_visible = range.len().saturating_sub(1);
        let row_at = |at: egui::Pos2| {
            (visible.contains(at) && !range.is_empty())
                .then(|| first + (((at.y - visible.top()) / ROW_HEIGHT) as usize).min(last_visible))
        };
        let hovered_row = response.hover_pos().and_then(row_at);

        // Where the button went down, while it is down.
        //
        // Not the same row as the one under the pointer, and that difference is the whole
        // point: egui only calls a press a drag once it has *travelled*, by which time the
        // pointer is a row or two along. Deciding from the pointer would pick up the file
        // the drag arrived at rather than the one it started on.
        let press = ui.input(|i| {
            i.pointer
                .any_down()
                .then(|| i.pointer.press_origin())
                .flatten()
        });
        let pressed_row = press.and_then(row_at);

        let edges = column_x(visible, widths);
        let name_font = t.fonts.body.clone();
        let meta_font = t.fonts.caption.clone();

        // Every row's shell icon, drawn in one batch once the rows are done.
        //
        // **Why this exists.** egui starts a new draw call whenever the texture changes, and a
        // shell icon is the only thing in a row that is not the font atlas — the fills, the
        // rules, the painted glyphs and all four columns of text come out of that one texture.
        // Drawing an icon inside the row loop therefore cuts the frame's primitive stream in
        // two at every row: forty rows become eighty-odd draw calls instead of a handful.
        //
        // That is not merely slow, it *leaks*, and not in this program: a plain eframe window
        // drawing forty textured quads holds steady, and the same forty interleaved with text
        // grows by 190 MB in thirty seconds. `examples/spin.rs` is that measurement. Batching
        // the icons is the workaround available from here, and it is what a hand-painted
        // listing should have been doing anyway.
        //
        // Drawing them last is invisible: an icon sits in its own column, over the row fill
        // and clear of the text.
        let mut deferred: Vec<(egui::TextureId, Rect, Rect, Color32)> = Vec::new();
        // The row being renamed, if one of the visible ones is, and where its name cell was.
        //
        // Drawn after the loop for the same reason the icons are, but to a different end: the
        // field is allowed to be wider than the Name column, and a widget drawn in the middle
        // of the loop would have the next three columns of its own row painted on top of it.
        let mut renaming: Option<(usize, f32, f32, Rect)> = None;
        // A git badge per row that has one, drawn after the icons for the plain reason that it goes
        // *on top* of one — a shell icon is a textured quad and would cover a badge painted first.
        //
        // The row's own fill travels with it: the badge is punched out of whatever it is sitting on,
        // and on a selected row that is the accent rather than the surface.
        let mut badges: Vec<(Rect, crate::git::State, Color32)> = Vec::new();
        // What git said about this folder, taken once. An `Arc` clone rather than a borrow, because
        // the loop below needs `&mut Tab` for the icon and shortcut columns it fills in as it goes.
        let git = tab.git.clone();
        // What a row actually has *ink* on, cell by cell, so a click can tell "on the file" from
        // "on the row it happens to be in". Two gestures ask:
        //
        // - A **drag**, which picks the file up from its ink and draws a rubber band from the
        //   space around it. That asks about the row the button went *down* on.
        // - A **right click**, which is the file's menu on its ink and the folder's menu off it.
        //   That asks about the row under the pointer, because a click arrives on the release,
        //   by which time no button is down and there is no press to ask about.
        //
        // So both rows are collected — at most two, and their rects cannot be confused for each
        // other's, since a row is a horizontal band and the two are at different heights.
        let mut ink: Vec<Rect> = Vec::with_capacity(10);
        // Whether this listing is a flattened tree rather than a flat list of the same rows, taken
        // once: it decides the indent, the twisty and what the Name column says after the name.
        // See [`crate::pane::FlatMode`].
        let tree = tab.is_tree();
        // Every visible twisty's box and the row it opens, for the click below. A click on one is
        // *not* a click on the row — it opens a folder rather than selecting it — so it has to be
        // tested before the selection is touched, and only the boxes actually drawn are here: a
        // folder with nothing in it has no twisty and nothing to hit.
        let mut twisties: Vec<(Rect, usize)> = Vec::new();
        // The joined folders in front of a merged chain's own name — `src > main > java > `. One
        // buffer down the whole loop, so a tree of chains allocates once for the frame rather than
        // once a row. Left holding whatever the last row put in it, like `scratch`.
        let mut chain_text = String::new();

        for position in range.clone() {
            let entry_index = tab.order[position] as usize;
            let entry = &dir.entries[entry_index];
            let row = Rect::from_min_size(
                pos2(
                    visible.left(),
                    visible.top() + (position - first) as f32 * ROW_HEIGHT,
                ),
                vec2(visible.width(), ROW_HEIGHT),
            );
            // Every cell's text is centred in this instead of in the row — see [`CELL_LIFT`].
            // Fills, the selection bar, the cursor ring and the drag ink all stay on `row`.
            let text_row = row.translate(vec2(0.0, -CELL_LIFT));

            let selected = tab.selected.get(entry_index).copied().unwrap_or(false);
            let is_hovered = hovered_row == Some(position);
            // The two rows whose ink is worth collecting: see where `ink` is declared.
            let inked = pressed_row == Some(position) || is_hovered;
            let renaming_here = matches!(tab.renaming, Some((at, _)) if at == entry_index);
            // The row being renamed wears none of it. The field has a border and a focus ring of
            // its own, and a selected row's fill and accent bar sit right behind them competing
            // for the same edge -- so the one row you are actually looking at is the one that
            // reads worst. Explorer drops the highlight while renaming too.
            //
            // What the row ends up filled with is kept: a git badge is punched out of it.
            let mut under = t.bg.layer;
            if !renaming_here {
                // A selection in a listing that does not have the keyboard goes quiet. `focused` is
                // false both when another pane has it and when *this* pane's console does, which is
                // the same statement either way: these rows are still selected, and the arrow keys
                // are not about them. See [`crate::ui::row_fill_quiet`].
                let fill = match (selected, focused) {
                    (true, false) => Some(row_fill_quiet(t)),
                    _ => row_fill(t, selected, is_hovered),
                };
                if let Some(fill) = fill {
                    ui.painter().rect_filled(row, CornerRadius::ZERO, fill);
                    under = fill;
                }
                if selected {
                    selection_bar(ui.painter(), row, t);
                }
            }
            // The keyboard cursor, when it is not simply the selection.
            //
            // Dashed and grey rather than a solid accent outline. The accent is what this
            // window says "selected" with — the fill and the bar down the left edge of a
            // selected row — and spending it on a row that is *not* selected said the opposite
            // of what it meant. A dashed grey rectangle is what every list on the platform has
            // marked the focused-not-selected row with since long before any of them had a
            // theme, and it cannot be confused with a selection at a glance.
            if focused && tab.cursor == Some(position) && !selected {
                cursor_ring(ui.painter(), row.shrink(1.0), t.stroke.strong);
            }

            // A hidden or system entry is dimmed rather than hidden-when-shown:
            // seeing that it *is* hidden is the point of showing it. A row waiting on a
            // paste is dimmed for a different reason — it is going somewhere — and
            // Explorer marks it the same way, so the two share the treatment.
            let pending_cut = !cut.is_empty()
                && cut
                    .iter()
                    .any(|p| p.file_name().is_some_and(|n| n == dir.name(entry_index)))
                && cut.iter().any(|p| p.parent() == Some(dir.path.as_path()));
            let dim = entry.is_hidden() || pending_cut;
            let name_color = if dim { t.text.tertiary } else { t.text.primary };
            let meta_color = if dim { t.text.disabled } else { t.text.secondary };

            // ---- Name ----
            let kind = fmt::kind_of(dir.ext(entry_index), entry.is_dir());
            // How far down the tree this row is drawn, and so how far in its column starts. Zero for
            // every row of every other listing. `Tab::row_depth` and not `Dir::depth`, because a
            // merged chain of folders stands where the first of them stood — see `sort::TreeRow`.
            //
            // The twisty's width is reserved whether or not one is drawn, so that a folder's
            // children line up with each other whether they are folders or files.
            let depth = tab.row_depth(position);
            // How many folders are merged into this row's name, which is what turns it from `com`
            // into `src > main > java > com`. Never anything but zero outside a tree.
            let merged = tab.row_merged(position);
            let stem = stem_x(row, depth);
            let glyph_x = if tree { stem + TWISTY } else { stem };
            let box_rect = icon_rect(row, glyph_x, GLYPH);

            // The twisty, on a folder that has something under it.
            //
            // **Which is asked of the display order, not of the file system** — see
            // `Tab::has_children_below`, which is where that reasoning and its edge cases live. A
            // folder the user has shut has no children in the order at all, which is why that is the
            // other half of the test rather than a special case: it is shut, so it opens.
            //
            // The name it is shut *under* is the row's own — the innermost folder of a merged chain,
            // which is the one whose children the twisty is about.
            if tree && entry.is_dir() {
                let shut = tab.collapsed.contains(dir.name(entry_index));
                let has_kids = shut || tab.has_children_below(position);
                if has_kids {
                    let hit = twisty_rect(row, depth);
                    let chevron: azur_icons::Icon<'_> = if shut {
                        &azur_icons::chevron_right
                    } else {
                        &azur_icons::chevron_down
                    };
                    // `text-secondary`, so the ladder of chevrons down a branch does not compete
                    // with the names beside it — and `text-primary` under the pointer, which is
                    // the only affordance a painted glyph with no button around it has.
                    chevron(
                        ui.painter(),
                        icon_rect(row, hit.left(), TWISTY),
                        if is_hovered {
                            t.text.primary
                        } else {
                            t.text.secondary
                        },
                    );
                    twisties.push((hit, position));
                }
            }

            // The dimmed half of the Name cell: **what this row points at, or where it is.**
            //
            // A shortcut's target wins when it is known, because that is what the row *is* — a
            // name standing for somewhere else — and in an ordinary listing every row shares
            // the same parent anyway, so the target is the only context there is to give. The
            // parent is what a flattened listing shows, and it is the answer for every row in
            // one that is not a shortcut.
            //
            // Asked once per row per view and answered on a worker thread: reading a `.lnk`
            // means COM, and one pointing at a share that is not currently reachable is the
            // classic Explorer hang. Until the answer lands the row draws its name alone, which
            // is what it did before this existed. See `crate::shell::links`.
            let context: Option<std::borrow::Cow<'_, str>> = {
                let row_index = entry_index as u32;
                match links::kind_of(dir.ext(entry_index), entry.is_link()) {
                    Some(kind) => {
                        if !tab.links.contains_key(&row_index)
                            && links_cache.request(
                                tab.view,
                                row_index,
                                dir.target(entry_index),
                                kind,
                            )
                        {
                            // Present means asked, so the row does not ask again next frame.
                            tab.links.insert(row_index, None);
                        }
                        // Cloned rather than borrowed: the map is behind the same `&mut Tab`
                        // that the next row's request writes to, and one short path per
                        // shortcut row on screen is not a cost worth threading a lifetime for.
                        tab.links
                            .get(&row_index)
                            .and_then(|target| target.clone())
                            .map(std::borrow::Cow::Owned)
                    }
                    None => None,
                }
                // Where the row is, for a flattened listing. `""` in every other one — and
                // deliberately nothing in a **tree**, where the row is already sitting under the
                // folder it is in: the indent says it, and saying it twice is a column of dimmed
                // paths repeating what the shape of the listing already shows.
                .or_else(|| match dir.within(entry_index) {
                    _ if tree => None,
                    "" => None,
                    parent => Some(std::borrow::Cow::Borrowed(parent)),
                })
            };

            // The shell icon if one is known, and the painted glyph until it is. Nothing
            // here blocks or allocates: the per-file answer is a lookup in the tab's own
            // four-bytes-a-row column, and the per-type one is a small map.
            let shell_icon = match dir.explicit_target(entry_index) {
                // A row that stands for somewhere else — a volume under This PC. Its icon
                // is its own, not its type's, and there are at most a couple of dozen of
                // them, so this is the one listing that can afford to ask per path.
                Some(target) => icons_cache.place(target),
                None => {
                    let ext = dir.ext(entry_index);
                    // A file whose icon lives inside it: the answer belongs to this view of
                    // this folder, and the path to ask with is built once per file rather
                    // than once per file per frame.
                    let own = if !entry.is_dir() && Icons::is_per_file(ext) {
                        match tab.file_icons.get(entry_index).copied() {
                            Some(shell_icons::UNASKED) => {
                                let path = dir.path.join(dir.name(entry_index));
                                if icons_cache.request_file(
                                    tab.view,
                                    entry_index as u32,
                                    path,
                                ) {
                                    tab.file_icons[entry_index] = shell_icons::ASKED;
                                }
                                None
                            }
                            Some(index) if index >= 0 => Some(shell_icons::Icon { index }),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    // Falling through to the type's icon means an `.exe` shows the generic
                    // application glyph rather than nothing while its own is fetched.
                    own.or_else(|| icons_cache.kind(ext, entry.is_dir()))
                }
            }
            .and_then(|icon| icons_cache.uv(ui.ctx(), icon));

            match shell_icon {
                Some((texture, uv)) => {
                    // Held back and drawn after the loop — see `deferred`. A shell icon is a
                    // textured quad and everything else on a row comes out of the font atlas,
                    // so drawing it here would split the frame's primitive stream in two at
                    // every row.
                    deferred.push((
                        texture,
                        uv,
                        box_rect,
                        // A hidden entry is faded rather than recoloured: a shell icon
                        // carries its own colours, and tinting them would misreport
                        // what kind of file it is.
                        if dim {
                            Color32::from_white_alpha(110)
                        } else {
                            Color32::WHITE
                        },
                    ));
                }
                None => {
                    let glyph: azur_icons::Icon<'_> = if entry.is_dir() && entry.is_link() {
                        &icons::folder_link
                    } else {
                        icons::for_kind(kind)
                    };
                    let glyph_color = if dim { t.text.disabled } else { t.kind(kind) };
                    glyph(ui.painter(), box_rect, glyph_color);
                }
            }

            // **What git says about this row**, by the name the row is showing — which in a
            // flattened listing is a path, and git speaks in paths, so both work out of the same
            // lookup. One hash lookup per visible row per frame, against a map built once when the
            // answer landed; nothing here walks the repository's changes.
            if let Some(state) = git.as_ref().and_then(|repo| repo.state(dir.name(entry_index))) {
                badges.push((badge_rect(box_rect), state, under));
            }

            let name_left = glyph_x + GLYPH + space::S3;
            let name_right = edges[1] - CELL_PAD;
            // A folder row is somewhere files can be dropped. Recorded for every visible one,
            // because that is what makes dragging onto a folder mean "into that folder" rather
            // than "into the folder I am looking at".
            if entry.is_dir() {
                outcome.drop_rows.push((row, dir.target(entry_index)));
            }
            if inked {
                // The icon counts as the file too: it is the most obvious thing to take
                // hold of, and it is what Explorer's own drag handle is.
                ink.push(box_rect);
            }
            if renaming_here {
                // Held back until every column of every row has been drawn, so that a field
                // wider than the Name column covers Size, Type and Modified instead of being
                // painted under them. See where `renaming` is declared.
                // `text_row`, not `row`: the field has to put its text exactly where the label
                // would have gone, and the label is centred in `text_row`. See `rename_field`.
                //
                // **A merged chain renames its last folder**, because that folder is what the row is
                // — its columns are its, opening it goes there — so the field opens over that
                // segment rather than over the whole chain. The folders in front stay on screen
                // beside it, which is also what says which of them is being renamed.
                let ahead = if merged > 0 {
                    let (_, chain) = chain_split(dir.name(entry_index), merged);
                    chain_ahead(chain_folders(chain), false, &mut chain_text);
                    ui.painter()
                        .layout_no_wrap(chain_text.clone(), name_font.clone(), meta_color)
                        .size()
                        .x
                } else {
                    0.0
                };
                renaming = Some((
                    entry_index,
                    (name_left + ahead).min(name_right),
                    name_right,
                    text_row,
                ));
            } else if name_right > name_left {
                let galley = if merged > 0 {
                    let (_, chain) = chain_split(dir.name(entry_index), merged);
                    chain_galley(
                        ui.painter(),
                        chain,
                        name_font.clone(),
                        name_color,
                        meta_color,
                        name_right - name_left,
                        &mut chain_text,
                    )
                } else {
                    name_galley(
                        ui.painter(),
                        dir.leaf(entry_index),
                        context.as_deref(),
                        name_font.clone(),
                        name_color,
                        meta_color,
                        name_right - name_left,
                    )
                };
                if inked {
                    ink.push(Rect::from_min_max(
                        pos2(name_left, row.top()),
                        pos2((name_left + galley.size().x).min(name_right), row.bottom()),
                    ));
                }
                text_left(
                    ui.painter(),
                    Rect::from_min_max(
                        pos2(name_left, text_row.top()),
                        pos2(name_right, text_row.bottom()),
                    ),
                    galley,
                );
            }

            // ---- Size ----
            if widths[1] > 0.0 && !entry.is_dir() {
                scratch.clear();
                fmt::size(entry.size, scratch);
                let cell = Rect::from_min_max(
                    pos2(edges[1] + CELL_PAD, text_row.top()),
                    pos2(edges[2] - CELL_PAD, text_row.bottom()),
                );
                let galley = truncated(
                    ui.painter(),
                    scratch,
                    meta_font.clone(),
                    meta_color,
                    cell.width(),
                );
                if inked {
                    // Right-aligned, so its ink is against the right edge of the cell.
                    ink.push(Rect::from_min_max(
                        pos2((cell.right() - galley.size().x).max(cell.left()), row.top()),
                        pos2(cell.right(), row.bottom()),
                    ));
                }
                text_right(ui.painter(), cell, galley);
            }

            // ---- Type ----
            if widths[2] > 0.0 {
                scratch.clear();
                fmt::type_label(dir.ext(entry_index), entry.is_dir(), scratch);
                let cell = Rect::from_min_max(
                    pos2(edges[2] + CELL_PAD, text_row.top()),
                    pos2(edges[3] - CELL_PAD, text_row.bottom()),
                );
                let galley = truncated(
                    ui.painter(),
                    scratch,
                    meta_font.clone(),
                    meta_color,
                    cell.width(),
                );
                if inked {
                    // The ink is what a drag is measured against, so it stays on the row
                    // rather than following the text's two-point lift.
                    ink.push(Rect::from_min_max(
                        pos2(cell.left(), row.top()),
                        pos2((cell.left() + galley.size().x).min(cell.right()), row.bottom()),
                    ));
                }
                text_left(ui.painter(), cell, galley);
            }

            // ---- Modified ----
            if widths[3] > 0.0 {
                scratch.clear();
                fmt::modified(entry.modified, zone, scratch);
                let cell = Rect::from_min_max(
                    pos2(edges[3] + CELL_PAD, text_row.top()),
                    pos2(edges[4] - CELL_PAD, text_row.bottom()),
                );
                let galley = truncated(
                    ui.painter(),
                    scratch,
                    meta_font.clone(),
                    meta_color,
                    cell.width(),
                );
                if inked {
                    ink.push(Rect::from_min_max(
                        pos2(cell.left(), row.top()),
                        pos2((cell.left() + galley.size().x).min(cell.right()), row.bottom()),
                    ));
                }
                text_left(ui.painter(), cell, galley);
            }
        }

        // Every visible row's icon, in one run — see where `deferred` is declared.
        if !deferred.is_empty() {
            let painter = ui.painter();
            for (texture, uv, rect, tint) in &deferred {
                painter.image(*texture, *rect, *uv, *tint);
            }
        }

        // And the git badges over them. Painted glyphs, so they cost the font atlas's own draw call
        // rather than one each, and they go last because a badge belongs on top of the icon it is
        // about — see [`badge_rect`].
        if !badges.is_empty() {
            let painter = ui.painter();
            for (rect, state, under) in &badges {
                icons::git_badge(painter, *rect, *state, t.git(*state), *under);
            }
        }

        // And last of all, over everything including its own row's other columns.
        if let Some((entry_index, left, right, text_row)) = renaming {
            rename_field(ui, t, pane, tab, entry_index, left, right, text_row, out);
        }

        // ---- What the row is, in words -------------------------------------
        //
        // **Nothing here is only in the tooltip**, and that is deliberate: it is the four columns of
        // the row it is over, plus the two things the columns cannot say — a name too long for the
        // Name column, and the exact byte count — plus what git says, which has only a badge on the
        // row. So it is worth reading and never worth waiting for.
        //
        // Not while the pointer is *doing* something. A tooltip over a rubber band is a tooltip in the
        // way of the gesture, and one over a row being renamed covers the field.
        //
        // **At the pointer**, because the response it hangs off is the whole visible block and not the
        // row — see the `ui.interact` above. Anchored to that response the way a button's tooltip is,
        // it would come up below the last row on screen, forty rows from the one it is about.
        let busy = tab.renaming.is_some()
            || tab.band.is_some()
            || ui.input(|i| i.pointer.any_down() || i.pointer.any_released());
        if let Some(position) = hovered_row.filter(|_| !busy) {
            if let Some(about) = row_tooltip(tab, zone, position, scratch) {
                azur_egui_theme::components::tooltip_at_pointer_ui(response.clone(), |ui| {
                    crate::ui::tooltip_table(ui, t, &about);
                });
            }
        }

        // ---- Clicks --------------------------------------------------------
        let modifiers = ui.input(|i| i.modifiers);
        if tab.renaming.is_some() {
            // The field has the keyboard and the pointer; a click that lands outside it
            // is handled by the field losing focus, not by moving the selection.
            return;
        }
        if response.clicked() || response.secondary_clicked() {
            out.push(Action::Focus(pane));
        }
        // ---- A twisty, before anything else a click could mean ---------------
        //
        // Opening or shutting a folder in a tree is not selecting it, so this is tested first and
        // returns: a click that both opened a folder and moved the selection onto it would make
        // the twisty unusable as a way of *looking* at a branch without disturbing what is picked
        // out. Explorer's tree draws the same distinction, and so does every other one.
        //
        // The whole box is the target rather than the chevron's ink — a 6-point arrowhead is not
        // something to ask anybody to hit. Nothing here needs a hovered row: a box was only pushed
        // for a row that drew one.
        if response.clicked() {
            let at = response
                .interact_pointer_pos()
                .or_else(|| ui.ctx().pointer_interact_pos());
            if let Some(&(_, position)) = at
                .and_then(|at| twisties.iter().find(|(hit, _)| hit.contains(at)))
            {
                // The keyboard has already been claimed by the `Focus` above, which every click in
                // the listing pushes — a twisty is still a click in this pane.
                out.push(Action::ToggleCollapsed { pane, position });
                return;
            }
        }

        // Which row the context menu is *for*, decided below. `None` means the folder's own menu
        // — the one with `New folder` and `Paste` in it.
        let mut menu_row = hovered_row;
        if let Some(position) = hovered_row {
            if response.clicked() {
                if modifiers.command {
                    tab.toggle(position);
                } else if modifiers.shift {
                    tab.select_range_to(position);
                } else {
                    tab.select_only(position);
                }
                if tab.is_dir_at(position) {
                    outcome.prefetch = tab.target_at(position);
                }
            }
            if response.double_clicked() {
                if let Some(path) = tab.target_at(position) {
                    if tab.is_dir_at(position) {
                        out.push(Action::Navigate { pane, path });
                    } else {
                        out.push(Action::Open(path));
                    }
                }
            }
            // Middle click opens a folder in its own tab — including a folder *shortcut*, which
            // is a row this cannot tell apart from a file without reading it. So the reading is
            // left to the action, which resolves it the same way opening one does; a shortcut to
            // a file lands there and does nothing, which is what a middle click on any other
            // file does.
            if response.middle_clicked() {
                if let Some(path) = tab.target_at(position) {
                    if tab.is_dir_at(position) {
                        out.push(Action::NavigateNewTab { pane, path });
                    } else if tab.is_shortcut_at(position) {
                        out.push(Action::OpenNewTab(path));
                    }
                }
            }
            // ---- Right click, on the file or merely in its row -------------
            //
            // A row is mostly space: a 24-point row across a wide pane has ink on perhaps a
            // third of it, and the rest is the listing's background as much as the gap below the
            // last file is. So a right click that lands on the name, the icon or one of the
            // three values is about *that file*, and one that lands in the space around them is
            // about the folder — which is where `New folder` and `Paste` are, and which used to
            // need finding a gap under the last row to reach.
            //
            // Only for a row that is not already in the selection. Right-clicking one that is
            // means the selection, wherever in the row it lands: the files are picked out
            // already, and taking that away because the pointer was between two columns would
            // undo work rather than ask a question. That is also the rule the drag follows.
            if response.secondary_clicked() && !tab.is_selected(position) {
                let at = response
                    .interact_pointer_pos()
                    .or_else(|| ui.ctx().pointer_interact_pos());
                let on_file =
                    at.is_some_and(|at| ink.iter().any(|rect| rect.expand(1.0).contains(at)));
                if on_file {
                    tab.select_only(position);
                } else {
                    tab.clear_selection();
                    menu_row = None;
                }
            }
        } else if response.clicked() {
            // A click on the empty space below the rows clears the selection, which
            // is how every file manager cancels one.
            tab.clear_selection();
        }

        // ---- Dragging: the file, or a band ---------------------------------
        //
        // Which one comes from where the button went *down*: on the icon, the name or one
        // of the three values — a drag of the file — or in the space around them, which
        // bands exactly as it does below the last row. A row is mostly space (a 24-point
        // row across a wide pane has ink on maybe a third of it), and treating all of it
        // as a drag handle is what makes a band a gesture you can only start by finding
        // the bottom of the listing first.
        //
        // Keyed on the *pressed* row and not the hovered one. egui only calls a press a
        // drag once it has travelled, and by then the pointer is a row or two along — so
        // the hovered row is the row the drag arrived at, and using it would both test the
        // wrong ink and pick up the wrong file.
        // Only the two buttons that mean anything here. A middle-button drag scrolls in some
        // applications and does nothing in this one, and the thumb buttons navigate — none of
        // them should pick a file up or draw a selection box, which is what `drag_started()`
        // without a button lets them all do.
        let dragging_files = response.drag_started_by(egui::PointerButton::Primary);
        let dragging_to_ask = response.drag_started_by(egui::PointerButton::Secondary);
        if dragging_files || dragging_to_ask {
            if let Some(grabbed) = pressed_row {
                let on_file = press
                    .is_some_and(|at| ink.iter().any(|rect| rect.expand(1.0).contains(at)));
                if on_file {
                    // A drag that starts on something already selected takes the whole
                    // selection; one that starts anywhere else makes that row the selection
                    // first, which is what makes dragging a single file work without clicking
                    // it beforehand.
                    if !tab.is_selected(grabbed) {
                        tab.select_only(grabbed);
                    }
                    out.push(Action::DragOut {
                        pane,
                        items: tab.selection_paths(),
                    });
                } else {
                    start_band(ui, body, tab, press);
                    out.push(Action::Focus(pane));
                }
            }
        }

        context_menu(ui, &response, pane, tab, menu_row, out);
    });

    // Remembered so the next scroll-into-view can nudge rather than jump, and so a
    // tab keeps its place when the pane it lives in is redrawn elsewhere.
    tab.scroll_y = output.state.offset.y;

    // Everything below the last row: the canvas a short listing leaves above the status line, and
    // the [`TAIL`] a long one leaves once it is scrolled to the end. That space is part of
    // the list — clicking it cancels the selection, as it does in every file manager, and
    // right-clicking it is how the folder's own menu is reached — but the band above covers the rows
    // and nothing else, deliberately, so nothing up there is listening for it.
    //
    // Measured from the rows rather than from `content_size`, which now includes the tail.
    let rows_bottom = body.top() + count as f32 * ROW_HEIGHT - output.state.offset.y;
    let empty = Rect::from_min_max(
        pos2(body.left(), rows_bottom.clamp(body.top(), body.bottom())),
        pos2(output.inner_rect.right(), body.bottom()),
    );
    if empty.height() > 1.0 && empty.width() > 1.0 {
        let response = child.interact(
            empty,
            Id::new(("rows-empty", pane)),
            Sense::click_and_drag(),
        );
        if response.clicked() {
            tab.clear_selection();
            out.push(Action::Focus(pane));
        }
        if response.secondary_clicked() {
            out.push(Action::Focus(pane));
        }
        // The folder's own menu, so right-clicking the space below the files offers New and
        // the rest rather than nothing.
        context_menu(&child, &response, pane, tab, None, out);

        // ---- The rubber band ------------------------------------------------
        //
        // It starts here rather than on a row because a drag *from* a row is how you
        // pick files up and move them — which is the other half of this gesture and
        // the reason the two have to start in different places, exactly as they do in
        // Explorer.
        if response.drag_started_by(egui::PointerButton::Primary)
            || response.drag_started_by(egui::PointerButton::Secondary)
        {
            let origin = child.input(|i| i.pointer.press_origin());
            start_band(&child, body, tab, origin);
            out.push(Action::Focus(pane));
        }
    }

    // **There is more above, and there is more below.** The design system's rule for the edge of a
    // scrolling collection — see `azur_egui_theme::components::scroll_fades` — and a file listing is
    // the collection it was written for.
    //
    // Its own two figures rather than the `ScrollArea`'s, because [`TAIL`] of the extent is
    // deliberately empty: measured off the content, a listing scrolled to its last file would fade at
    // the bottom for the slack under it and say there were more files. Measured off the rows, the
    // fade goes out exactly as the last one arrives.
    let above = output.state.offset.y;
    let below = (count as f32 * ROW_HEIGHT - output.inner_rect.height() - above).max(0.0);
    azur_egui_theme::components::scroll_fades(
        &child.painter_at(output.inner_rect),
        output.inner_rect,
        t.bg.layer,
        above,
        below,
    );

    if tab.band.is_some() {
        band(&mut child, t, body, pane, tab);
    }
}

/// Begin a rubber band at where the button went down.
///
/// `press_origin` rather than the current position: those are different pixels — a drag has
/// to travel before egui calls it one — and anchoring at the later of the two loses whatever
/// the pointer crossed on the way, so a quick flick would select nothing.
fn start_band(ui: &Ui, body: Rect, tab: &mut Tab, origin: Option<egui::Pos2>) {
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
fn content_pos(ui: &Ui, body: Rect, tab: &Tab, at: Option<egui::Pos2>) -> egui::Pos2 {
    let at = at.or_else(|| ui.ctx().pointer_interact_pos()).unwrap_or(body.min);
    pos2(at.x, at.y - body.top() + tab.scroll_y)
}

/// Track, apply and paint a rubber-band selection.
fn band(ui: &mut Ui, t: &Theme, body: Rect, pane: PaneId, tab: &mut Tab) {
    let held = ui.input(|i| i.pointer.any_down());
    let pointer = ui.ctx().pointer_interact_pos();

    if !held || pointer.is_none() {
        tab.band = None;
        return;
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
        let rows = tab.order.len() as f32 * ROW_HEIGHT;
        let limit = (rows - body.height()).max(0.0);
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
    tab.apply_band();

    // Painted last, so it lies over the rows it is selecting. Clipped to the body, or
    // a band dragged past the edge would spill onto the status line.
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
    let _ = pane;
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
fn stem_chars(name: &str, is_dir: bool) -> usize {
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
fn select_stem(ctx: &egui::Context, id: Id, name: &str, is_dir: bool) {
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
#[allow(clippy::too_many_arguments)]
fn rename_field(
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

/// Ask for the shell context menu on a right click.
///
/// The menu itself is Explorer's — see [`crate::shell::menu`] — so this only has to
/// A body with no rows in it still answers a right click.
///
/// [`rows`] is what wires the listing's clicks up, and a listing with nothing in it never gets
/// there: an empty folder, a folder filtered down to nothing, one still being read, and one
/// that refused to be read all draw a line of text over a body that nothing was listening to.
/// So right-clicking an empty folder did nothing at all — and an empty folder is precisely
/// where `New folder` and `Paste` are most wanted.
///
/// The folder's own menu, the same one the space below a short listing gives.
fn bare_body(ui: &Ui, body: Rect, pane: PaneId, tab: &Tab, out: &mut Vec<Action>) {
    if body.height() <= 1.0 || body.width() <= 1.0 {
        return;
    }
    // Clicks only. A drag over an empty body is a rubber band around nothing, and leaving
    // dragging alone keeps the drop zone underneath able to take files dropped in.
    let response = ui.interact(body, Id::new(("bare-body", pane)), Sense::click());
    if response.clicked() || response.secondary_clicked() {
        out.push(Action::Focus(pane));
    }
    context_menu(ui, &response, pane, tab, None, out);
}

/// decide *what* it is for and *where* it goes. Showing it is deferred to the
/// application through an action, because `TrackPopupMenuEx` is modal: it must not run
/// with the listing borrowed and half-drawn.
fn context_menu(
    ui: &Ui,
    response: &egui::Response,
    pane: PaneId,
    tab: &Tab,
    row: Option<usize>,
    out: &mut Vec<Action>,
) {
    if !response.secondary_clicked() {
        return;
    }
    // Where the pointer was when the button went down, in screen pixels — a Win32 menu
    // is positioned in physical coordinates, and egui works in points.
    let at = response
        .interact_pointer_pos()
        .or_else(|| ui.ctx().pointer_interact_pos())
        .unwrap_or(response.rect.center());
    let scale = ui.ctx().pixels_per_point();

    // `row` is what the menu is for, and by the time this is called the selection already agrees
    // with it — the caller selected the row the click landed on, or cleared the selection and
    // passed `None`. So the selection is the answer, with the row itself as the fallback for the
    // case that cannot happen: a row that is not selected and not selectable, because the listing
    // went away between the click and here.
    let items = match row {
        Some(_) if tab.selected_count > 0 => tab.selection_paths(),
        Some(row) => tab.target_at(row).into_iter().collect(),
        // The background: the folder's own menu, which is the only one New is on.
        None => Vec::new(),
    };

    out.push(Action::ShellMenu {
        pane,
        items,
        at: ((at.x * scale) as i32, (at.y * scale) as i32),
    });
}

// ---------------------------------------------------------------------------
// Status line
// ---------------------------------------------------------------------------

/// `"s"` unless there is exactly one.
fn plural(count: u32) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// The separator between a row's name and the context after it, and between the folders of a
/// merged chain — which is the same mark meaning the same thing, one path step.
///
/// Spaces either side, and both of them belong to the *dimmed* half: what the eye should find
/// first is where the name ends, and a gap in body-coloured text before a gap in secondary
/// makes that edge fuzzy. In a chain the last separator is the one before the row's own folder,
/// and it is dim for exactly that reason.
const CONTEXT: &str = " > ";

/// Split a flattened row's relative path into **where the row is** and **what it shows**.
///
/// `merged` is [`crate::pane::Tab::row_merged`]: how many folders above this one are drawn as part
/// of it. So `x\a\b\c` with two merged is `("x", "a\b\c")` — the row is in `x`, and it shows the
/// chain `a > b > c`. With none merged it is the split [`Dir::within`] and [`Dir::leaf`] already
/// make, and this answers exactly the same pair.
fn chain_split(name: &str, merged: usize) -> (&str, &str) {
    let mut start = name.len();
    for _ in 0..=merged {
        match name[..start].rfind(['\\', '/']) {
            Some(at) => start = at,
            // The chain reaches the folder being listed: all of the path is the row's own.
            None => return ("", name),
        }
    }
    (&name[..start], name[start..].trim_start_matches(['\\', '/']))
}

/// The folders of a chain without the row's own: `a\b\c` is `a\b`, and `c` alone is nothing.
fn chain_folders(chain: &str) -> &str {
    chain
        .rsplit_once(['\\', '/'])
        .map_or("", |(folders, _)| folders)
}

/// Those folders joined for display, with a trailing separator: `a\b` is `a > b > `. `elided` puts a
/// `…` in place of any that were dropped.
///
/// Written into a buffer the caller owns, because this is per row per frame and the answer is a
/// handful of bytes: a `String` returned here would be an allocation a listing does not need.
fn chain_ahead(folders: &str, elided: bool, into: &mut String) {
    into.clear();
    if elided {
        into.push('…');
        into.push_str(CONTEXT);
    }
    for folder in folders.split(['\\', '/']).filter(|part| !part.is_empty()) {
        into.push_str(folder);
        into.push_str(CONTEXT);
    }
}

/// The Name cell of a merged chain: `src > main > java > com`, cut from the **front** when it will
/// not fit.
///
/// Which is [`name_galley`]'s problem the other way round and takes the same answer. There, the row's
/// own name comes first and the context after it, so truncating at the end eats the context — the
/// right thing to lose. Here the part that must survive is at the *end*: the row is the folder the
/// chain arrives at, and `src > main > java > …` would have elided the only word that says what the
/// row is. So a folder is dropped off the front, an `…` says so, and it tries again.
///
/// The folders in front are dimmed and the last one is not, for the reason the dimmed half of an
/// ordinary flattened row is dimmed: they are where the row *is* rather than what it is.
fn chain_galley(
    painter: &egui::Painter,
    chain: &str,
    font: egui::FontId,
    color: egui::Color32,
    dim: egui::Color32,
    width: f32,
    ahead: &mut String,
) -> std::sync::Arc<egui::Galley> {
    // No separator in it is no chain: one folder, drawn the way any other row's name is.
    let Some((folders, leaf)) = chain.rsplit_once(['\\', '/']) else {
        return truncated(painter, chain, font, color, width);
    };

    let lay = |ahead: &str| {
        let mut job = egui::text::LayoutJob::default();
        job.append(ahead, 0.0, egui::TextFormat::simple(font.clone(), dim));
        job.append(leaf, 0.0, egui::TextFormat::simple(font.clone(), color));
        job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
        painter.layout_job(job)
    };

    let mut from = folders;
    loop {
        chain_ahead(from, from.len() < folders.len(), ahead);
        let galley = lay(ahead);
        if !galley.elided {
            return galley;
        }
        match from.split_once(['\\', '/']) {
            // One fewer folder in front, and an ellipsis where it was.
            Some((_, rest)) => from = rest,
            // Down to the last one: try the row's own folder with nothing but the ellipsis before it.
            None if !from.is_empty() => from = "",
            // And not even that fits. The leaf is still first in the *elision*, so what is on screen
            // is as much of the row's own name as there was room for — which is the right way round.
            None => return galley,
        }
    }
}


/// A row's name, and — dimmed, after a `>` — where it is or what it points at.
///
/// Two sections of one galley rather than two galleys, so the pair share a baseline, a
/// truncation and a single draw. The name comes first and is what survives: everything after
/// the separator is context, and context is the thing to give up when the column is narrow.
///
/// `context` is `None` for the ordinary case — a file in the folder you are looking at, which
/// is not a shortcut — and then this is [`crate::ui::truncated`] and nothing else.
///
/// **The context is elided from its front, not its back.** For a flattened row that means
/// `translations_fr.json > …\Resources\Lang` rather than `> PluginGeosystem\Resour…`: the
/// folder immediately holding the file is what identifies it, and the same is true of a
/// shortcut's target, where the last component is the program it runs. A component at a time,
/// never mid-name, because a path cut mid-component reads as a different path.
///
/// At most one extra layout per level, only for the rows that do not fit, and only for the ~40
/// on screen; egui caches finished galleys, so a row that has not changed costs a hash lookup
/// on every frame after the first.
fn name_galley(
    painter: &egui::Painter,
    name: &str,
    context: Option<&str>,
    font: egui::FontId,
    color: egui::Color32,
    dim: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let Some(context) = context.filter(|text| !text.is_empty()) else {
        return truncated(painter, name, font, color, width);
    };

    let lay = |context: &str| {
        let mut job = egui::text::LayoutJob::default();
        job.append(name, 0.0, egui::TextFormat::simple(font.clone(), color));
        job.append(
            &format!("{CONTEXT}{context}"),
            0.0,
            egui::TextFormat::simple(font.clone(), dim),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
        painter.layout_job(job)
    };

    let whole = lay(context);
    if !whole.elided {
        return whole;
    }
    let mut shortest = None;
    let mut cut = 0;
    while let Some(at) = context[cut..].find(['\\', '/']) {
        cut += at + 1;
        let galley = lay(&format!("…\\{}", &context[cut..]));
        let fits = !galley.elided;
        shortest = Some(galley);
        if fits {
            break;
        }
    }
    // Not even the last component fits. The name is still first, so what is on screen is the
    // name and as much of the context as there was room for, which is the right way round.
    shortest.unwrap_or(whole)
}

/// What a row says about itself when the pointer rests on it, as **key and value**.
///
/// ```text
/// Name      LgsxItemBuilders.cpp
/// In        Inspect\Model              ← only in a flattened listing
/// Type      C++ source
/// Size      9.38 KB (9,605 bytes)      ← only on a file
/// Modified  07/08/2026 18:24
/// Git       Changed on disk            ← only where git has something to say
/// ```
///
/// **The name is the reason it exists.** The Name column is whatever the other three leave, and on a
/// narrow pane that is narrower than plenty of names — so the first line is the one thing the row
/// might not have been able to show. Everything after it is either the row's own columns (worth
/// repeating, because those columns can be dragged to nothing) or something the row can only draw as a
/// mark: the exact size, and the badge's meaning in words.
///
/// It reads down the keys rather than across a paragraph, and **the keys are the column headers** —
/// `Name`, `Size`, `Type`, `Modified` — because that is what the four of them already are on screen
/// two inches above. It used to be four unlabelled lines with the type and the size run together by an
/// interpunct, which needed reading rather than glancing at: `C++ source · 9.38 KB (9,605 bytes)` is
/// two facts and a piece of punctuation doing a column's job.
///
/// `None` for a row that is not there. `scratch` is borrowed for the formatters and left holding
/// rubbish, which is what it is for.
fn row_tooltip(
    tab: &Tab,
    zone: &LocalZone,
    position: usize,
    scratch: &mut String,
) -> Option<Vec<(&'static str, String)>> {
    let dir = tab.dir.as_ref()?;
    let entry_index = tab.entry_at(position)?;
    let entry = dir.entries.get(entry_index)?;
    let mut about: Vec<(&'static str, String)> = Vec::with_capacity(6);

    // The row's own name and where it is — which for a merged chain of folders is the whole chain and
    // the folder the chain starts in. `chain_split` is the one place that division is made, so the
    // tooltip cannot disagree with the Name column about which is which.
    let (within, chain) = chain_split(dir.name(entry_index), tab.row_merged(position));
    let mut name = String::new();
    for (i, folder) in chain.split(['\\', '/']).enumerate() {
        if i > 0 {
            name.push_str(CONTEXT);
        }
        name.push_str(folder);
    }
    about.push(("Name", name));

    // Where it is, for a flattened listing — the same fact the Name column shows dimmed after the
    // name, and the first thing that column gives up when it runs out of room.
    if !within.is_empty() {
        about.push(("In", within.to_owned()));
    }

    scratch.clear();
    fmt::type_label(dir.ext(entry_index), entry.is_dir(), scratch);
    about.push(("Type", scratch.clone()));
    if !entry.is_dir() {
        scratch.clear();
        fmt::size(entry.size, scratch);
        // The rounded figure the column shows *and* the exact one, because they answer different
        // questions: `9.38 KB` is for comparing two files at a glance and `9,605 bytes` is for the
        // times only the number will do. Grouped in threes by hand — `fmt` has no separator for it,
        // and one call site does not make a formatter.
        about.push((
            "Size",
            format!("{scratch} ({} bytes)", grouped(entry.size)),
        ));
    }
    scratch.clear();
    fmt::modified(entry.modified, zone, scratch);
    about.push(("Modified", scratch.clone()));

    // And what git says, which the row itself can only say with a badge.
    if let Some(state) = tab
        .git
        .as_ref()
        .and_then(|repo| repo.state(dir.name(entry_index)))
    {
        about.push(("Git", state.describe(entry.is_dir()).to_owned()));
    }
    Some(about)
}

/// A byte count with its thousands grouped: `9605` as `9,605`.
///
/// A comma, and not the locale's separator: reading the user's number format means a call into the
/// platform per row, and this is one figure in a tooltip rather than a column of them. The same trade
/// [`fmt::date`] makes, and for the same reason.
fn grouped(bytes: u64) -> String {
    let digits = bytes.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The band the status line actually paints, and the one baseline everything on it sits on.
///
/// Two steps that have to happen in this order, which is why they are one function with a test of
/// their own rather than two lines at the top of [`status_line`].
///
/// **The band goes onto whole device pixels first.** A pane's bottom edge is a fraction of a window
/// divided by splits, so the rect arrives at a fractional `y` about half the time — and then the fill
/// is feathered across two rows at each edge while the text inside it is snapped to the grid by
/// epaint. The band's *visible* middle and the middle everything was centred on are then up to a
/// pixel apart, which is how a status line comes to look a pixel low on one window height and right
/// on the next. Rounded rather than floored, so the band stays [`STATUS_HEIGHT`] tall.
///
/// **Then the baseline comes off the snapped band**, and it is the *ink* baseline: this line holds
/// glyphs as well as words, and a glyph is centred on its own ink while a line of text centred in a
/// box is not — a line box reserves room under the baseline for descenders and above the capitals for
/// accents, and a file name uses neither. `azur::components::ink_baseline` carries the measurements;
/// [`CELL_LIFT`] is the same rule applied to a row of the listing.
///
/// Two rects come back, and the difference between them is [`NUDGE`]: the **band** is what gets
/// painted, and the **line** is what everything on it is placed against.
fn status_geometry(painter: &egui::Painter, t: &Theme, rect: Rect) -> (Rect, Rect, f32) {
    use egui::emath::GuiRounding as _;

    let band = rect.round_to_pixels(painter.pixels_per_point());
    let line = band.translate(vec2(0.0, -NUDGE));
    let baseline = ink_baseline(painter, &t.fonts.caption, line.top(), line.height());
    (band, line, baseline)
}

/// The console's switch and what git says, then — from the other end — how long the folder took,
/// how much is in it, and how much of that is selected.
///
/// # Two groups, and the left one wins
///
/// The left is a *control* and a fact about the repository; the right is arithmetic about the
/// folder. When the bar is too narrow for both, the right gives way — the scan's figure first, then
/// the size, and the counts last, because the counts are the part of this line a listing cannot be
/// read without. The left is never dropped: a switch nobody can see is a switch nobody can find,
/// and the branch you are on is the one thing here that is said nowhere else in the window.
///
/// # One baseline
///
/// Everything on the line sits on a single [`ink_baseline`] — the branch name, three greys in two
/// alignments, a coloured count, and four glyphs. Not because the fonts differ (they are all
/// `caption`) but because the *glyphs* do not care about line boxes: a line box reserves room under
/// the baseline for descenders and above the capitals for accents, and text centred in it therefore
/// reads off-centre beside a glyph centred on its own ink. It is the same rule [`CELL_LIFT`] applies
/// to a row of the listing, taken from the design system rather than measured again here.
///
/// The timing is not decoration: a file manager that claims to be fast should be willing to be
/// checked, and a folder that suddenly takes 200ms is how you find out something is wrong.
#[allow(clippy::too_many_arguments)]
fn status_line(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &Tab,
    console_open: bool,
    override_text: Option<&str>,
    now: f64,
    scratch: &mut String,
    out: &mut Vec<Action>,
) {
    use std::fmt::Write as _;

    let (band, line, baseline) = status_geometry(ui.painter(), t, rect);
    ui.painter()
        .rect_filled(band, CornerRadius::ZERO, t.bg.layer_alt);
    ui.painter().line_segment(
        [band.left_top(), band.right_top()],
        Stroke::new(1.0, t.stroke.subtle),
    );

    // ---- The console's switch, at the left edge --------------------------
    //
    // A control rather than a status, and the only one on this line. It is here because this is
    // where the console *is* — the band it opens is directly above this bar — and because a panel
    // whose only door is a keystroke is a panel most people never find.
    let switch = Rect::from_min_size(
        pos2(
            line.left() + space::S2,
            (line.center().y - SWITCH * 0.5).round(),
        ),
        vec2(SWITCH, SWITCH),
    );
    if crate::ui::tool_button(
        ui,
        t,
        switch,
        Id::new(("console-switch", pane)),
        &icons::terminal,
        if console_open {
            "Hide the console (Ctrl+²)"
        } else {
            "Show the console (Ctrl+²)"
        },
        true,
        console_open,
        t.bg.layer_alt,
    )
    .clicked()
    {
        out.push(Action::ToggleConsole(pane));
    }
    let mut left = switch.right() + space::S3;
    // Cloned, so the rest of this can paint while `ui` is still available for the hit rects the
    // tooltips need.
    let painter = ui.painter().clone();
    let ink = |text: &str, color: Color32| {
        painter.layout_no_wrap(text.to_owned(), t.fonts.caption.clone(), color)
    };

    // A flatten that stopped at its limit has to say so, and it says so here — inside the group
    // that never gives way. **A listing quietly missing rows is the one wrong answer a file manager
    // must not give**: everything else on this line can be checked against the folder, and this
    // cannot. A glyph and a tooltip rather than a sentence, because the sentence was the first
    // thing a narrow pane dropped.
    if tab.dir.as_ref().is_some_and(|dir| dir.truncated) {
        let at = icon_rect(line, left, MARK);
        azur_icons::warning(&painter, at, t.bar.warning);
        let hit = ui.interact(at, Id::new(("status-limit", pane)), Sense::hover());
        azur_egui_theme::components::tooltip(
            hit,
            "This is as much of the tree as was read — not all of it is here",
        );
        left = at.right() + space::S3;
    }

    // A copy in progress or something that went wrong displaces everything but the switch: it is
    // the more urgent fact, and the counts have not changed anyway.
    if let Some(text) = override_text {
        let galley = truncated(
            &painter,
            text,
            t.fonts.caption.clone(),
            t.text.primary,
            (line.right() - space::S3 - left).max(0.0),
        );
        galley_on_baseline(&painter, left, baseline, galley);
        return;
    }

    // What the listing is, while it is not a listing yet. It takes the git summary's place rather
    // than sitting beside it: a folder that has not been read has no answer from git either, so the
    // two are never both there.
    let word = match &tab.dir {
        // Nothing until the wait is worth mentioning, and then the same word the body uses.
        None if tab.waiting_visibly(now) => Some("Reading…"),
        Some(dir) if dir.error.is_some() => Some("Could not be read"),
        _ => None,
    };
    match word {
        Some(word) => {
            let galley = truncated(
                &painter,
                word,
                t.fonts.caption.clone(),
                t.text.tertiary,
                (line.right() - space::S3 - left).max(0.0),
            );
            galley_on_baseline(&painter, left, baseline, galley);
        }
        None => left = git_summary(ui, &painter, t, line, baseline, pane, tab, left, out),
    }

    // ---- The right, from the edge inwards, in the order they give way ----
    //
    // Each run is laid out left to right and placed as a block, and every one but the first carries
    // its own separator on its right — so a run that will not fit takes its separator with it and
    // the line never ends in a dangling interpunct.
    let Some(dir) = &tab.dir else { return };
    if dir.error.is_some() {
        return;
    }
    let shown = tab.order.len();
    let hidden = dir.len().saturating_sub(shown);
    let mut runs: Vec<Vec<std::sync::Arc<egui::Galley>>> = Vec::new();

    // **`selected / on show (not shown)`, in three colours rather than three words.** It is the
    // shortest thing that says all of it, and the colour is what keeps it from reading as one
    // number: the selection is the accent's, the total is `text-secondary` because it is the fact
    // the other two are measured against, and what is being held back is quieter still. The
    // tooltip spells it out, and carries the folders-and-files breakdown this line used to show.
    let mut counts = vec![
        ink(&tab.selected_count.to_string(), t.bar.counted),
        ink(&format!(" / {shown}"), t.text.secondary),
    ];
    if hidden > 0 {
        counts.push(ink(&format!(" ({hidden})"), t.text.tertiary));
    }
    runs.push(counts);

    // The folder's size, or the selection's the moment there is one — which is the question
    // somebody selecting files is usually asking. Left out when it is zero rather than shown as
    // `0 B`: a selection of nothing but folders has no size this program knows, since a directory's
    // own byte count is noise, and `0 B` would be an answer rather than a silence.
    let bytes = if tab.selected_count > 0 {
        tab.selected_size
    } else {
        dir.total_size
    };
    if bytes > 0 {
        scratch.clear();
        fmt::size(bytes, scratch);
        runs.push(vec![
            ink(scratch, t.text.secondary),
            ink(SEPARATOR, t.text.disabled),
        ]);
    }

    // How long the folder took. In `text-tertiary` and not `text-disabled`, which is what it wore
    // and which measures **2.20:1** on this surface in the dark theme — under any floor there is. The
    // whole argument for putting a timing on the bar is that a claim about speed nobody can check is
    // not a claim, and a figure nobody can read is not checkable. `text-tertiary` is 3.62:1: still the
    // quietest thing on the line, and legible.
    let millis = dir.scan_micros as f64 / 1000.0;
    let mut figure = String::new();
    let _ = if millis < 10.0 {
        write!(figure, "{millis:.1} ms")
    } else {
        write!(figure, "{millis:.0} ms")
    };
    runs.push(vec![
        ink(&figure, t.text.tertiary),
        ink(SEPARATOR, t.text.tertiary),
    ]);

    let mut right = line.right() - space::S3;
    let mut counted: Option<Rect> = None;
    for (which, run) in runs.iter().enumerate() {
        let width: f32 = run.iter().map(|galley| galley.size().x).sum();
        // Whatever is further left than the first thing that will not fit goes too: it is further
        // from the edge, so drawing it would leave a hole where this one would have been.
        if right - width < left + GROUP_GAP {
            break;
        }
        let mut x = right - width;
        if which == 0 {
            counted = Some(Rect::from_min_max(
                pos2(x, band.top()),
                pos2(right, band.bottom()),
            ));
        }
        for galley in run {
            let step = galley.size().x;
            galley_on_baseline(&painter, x, baseline, galley.clone());
            x += step;
        }
        right -= width;
    }

    if let Some(at) = counted {
        let mut tip = String::new();
        let _ = write!(
            tip,
            "{} selected of {shown} on show\n{} folder{} and {} file{} in this folder",
            tab.selected_count,
            dir.dir_count,
            plural(dir.dir_count),
            dir.file_count,
            plural(dir.file_count),
        );
        if hidden > 0 {
            // **A shut folder is the third way a row can be out**, and only in a tree — where it
            // is also much the most likely of the three, since shutting a branch of a large tree
            // takes thousands of rows out of the order at once. Naming only the other two would
            // leave the biggest number on the line explained by neither.
            let why = if tab.is_tree() {
                "inside a folder that is shut, hidden, or filtered out"
            } else {
                "hidden, or filtered out"
            };
            let _ = write!(tip, "\n{hidden} not shown: {why}");
        }
        let hit = ui.interact(at, Id::new(("status-counts", pane)), Sense::hover());
        azur_egui_theme::components::tooltip(hit, &tip);
    }
}

/// The branch, how far it is from its remote, and how much is changed — laid out to the right from
/// `left`, and returning where it ended.
///
/// **Nothing at all when there is no repository**, which is most folders: the line is about the
/// folder, and inventing a "not a repository" state to display would be furniture that is wrong more
/// often than it is right. Nothing while the answer is still coming either — it arrives within a
/// frame or two of the listing, and a placeholder that flickers past is worse than one row of
/// stillness.
///
/// # `N changed` is a button
///
/// It is the one thing on this line that is also a *question*, and the answer was three gestures
/// away: flatten the folder, find the filter box, know the word to type in it. Pressed, it does both
/// halves at once — see [`Action::ShowChanges`]. Everything else here stays a fact.
///
/// **Subtle, in the sense the rest of this window's chrome uses the word**: no border and no fill at
/// rest, so the line still reads as a line of figures rather than growing a control in the middle of
/// it, and the same quiet [`crate::ui::control_fills`] step under the pointer that every toolbar
/// button on every other surface wears. Nothing about the text changes — a count that restyled
/// itself for being pressable would be a count you read twice.
#[allow(clippy::too_many_arguments)]
fn git_summary(
    ui: &mut Ui,
    painter: &egui::Painter,
    t: &Theme,
    // The status line's *nudged* rect, not the band it paints — see `status_geometry`.
    line: Rect,
    baseline: f32,
    pane: PaneId,
    tab: &Tab,
    left: f32,
    out: &mut Vec<Action>,
) -> f32 {
    use std::fmt::Write as _;

    let Some(repo) = &tab.git else { return left };
    let caption = t.fonts.caption.clone();

    // Left to right, in the order they matter: the branch — the one thing that says the rest of this
    // is git at all — and then what is between it and its remote, and then the working tree. The
    // flag is whether the segment is the button; only one of them ever is.
    let mut segments: Vec<(
        Option<azur_egui_theme::icons::Icon<'static>>,
        String,
        Color32,
        bool,
    )> = Vec::new();
    let mut text = String::new();

    if !repo.head.is_empty() {
        segments.push((
            Some(&icons::branch),
            repo.head.clone(),
            if repo.detached {
                // A detached head is a state to notice: a commit made here belongs to no branch.
                t.bar.warning
            } else {
                t.bar.info
            },
            false,
        ));
    }
    // Ahead and behind, which only mean anything against an upstream — a branch that has never been
    // pushed is not "0 ahead", it is a branch with nowhere to be ahead of.
    if repo.upstream.is_some() {
        if repo.ahead > 0 {
            text.clear();
            let _ = write!(text, "{}", repo.ahead);
            segments.push((Some(&icons::arrow_up), text.clone(), t.bar.success, false));
        }
        if repo.behind > 0 {
            text.clear();
            let _ = write!(text, "{}", repo.behind);
            segments.push((Some(&icons::arrow_down), text.clone(), t.bar.danger, false));
        }
    }
    if repo.dirty() {
        text.clear();
        let _ = write!(text, "{} changed", repo.changed);
        segments.push((None, text.clone(), t.bar.warning, true));
    } else {
        // A clean tree says so with the same tick a clean file wears, and no number: "0 changed" is
        // a sentence about nothing. Nothing to press either — the listing it would ask for is empty.
        segments.push((
            Some(&azur_egui_theme::icons::check),
            String::new(),
            t.bar.success,
            false,
        ));
    }

    let mut x = left;
    /// The air either side of the button's text, so the fill under the pointer is a shape around
    /// the words rather than a box wrapped tight on them.
    const PRESS_PAD: f32 = space::S1;
    // Where the button ended up, and the shape slot its fill goes in. Reserved *before* the text is
    // painted and filled in long after, because whether there is a fill at all depends on an
    // interaction that has to be registered after the group's own hit rect below — see there.
    let mut button: Option<(Rect, egui::layers::ShapeIdx)> = None;
    for (icon, label, color, pressable) in &segments {
        // The branch name is capped rather than given the line: a repository whose branch names are
        // paragraphs must not push the folder's own figures off the bar.
        let galley = truncated(painter, label, caption.clone(), *color, 160.0);
        let width = galley.size().x
            + if icon.is_some() {
                MARK + if label.is_empty() { 0.0 } else { space::S1 }
            } else {
                0.0
            };
        if x + width > line.right() - space::S3 {
            break;
        }
        let slot = pressable.then(|| painter.add(egui::Shape::Noop));
        let from = x;
        if let Some(icon) = icon {
            icon(painter, icon_rect(line, x, MARK), *color);
            x += MARK;
            if !label.is_empty() {
                x += space::S1;
            }
        }
        if !label.is_empty() {
            let step = galley.size().x;
            galley_on_baseline(painter, x, baseline, galley);
            x += step;
        }
        if let Some(slot) = slot {
            // The console switch's height, centred on the line: the two are the only controls on
            // this bar, and a target that agrees with the one at the other end of it is one
            // decision instead of two.
            let hit = Rect::from_min_max(
                pos2(from - PRESS_PAD, (line.center().y - SWITCH * 0.5).round()),
                pos2(x + PRESS_PAD, (line.center().y + SWITCH * 0.5).round()),
            );
            button = Some((hit, slot));
        }
        x += space::S3;
    }

    // What `↑2 ↓1` is counted against, which is the one thing the summary cannot show and the first
    // thing anybody asks of it. Over the whole group, since every part of it is about this
    // repository.
    if x > left {
        let mut tip = String::new();
        if repo.detached {
            let _ = write!(tip, "Detached at {}", repo.head);
        } else {
            let _ = write!(tip, "On {}", repo.head);
        }
        match &repo.upstream {
            Some(upstream) => {
                let _ = write!(tip, ", tracking {upstream}");
                if repo.ahead > 0 || repo.behind > 0 {
                    let _ = write!(tip, "\n{} ahead, {} behind", repo.ahead, repo.behind);
                }
            }
            None => tip.push_str(", not tracking a remote"),
        }
        if repo.dirty() {
            let _ = write!(
                tip,
                "\n{} staged, {} changed, {} untracked",
                repo.staged, repo.unstaged, repo.untracked
            );
            if repo.conflicted > 0 {
                let _ = write!(tip, ", {} conflicted", repo.conflicted);
            }
        } else {
            tip.push_str("\nNothing to commit");
        }
        let at = Rect::from_min_max(pos2(left, line.top()), pos2(x, line.bottom()));
        let hit = ui.interact(at, Id::new(("status-git", pane)), Sense::hover());
        azur_egui_theme::components::tooltip(hit, &tip);
    }

    // **The button is registered last, so it wins the pointer inside the group.** The group's hit
    // rect above covers the whole summary including this, and the one registered later is the one on
    // top — the other way round, the count could not be clicked and the tooltip explaining what the
    // click does would never appear. It is the same ordering the filter box's ✕ relies on.
    if let Some((hit, slot)) = button {
        let response = ui.interact(hit, Id::new(("status-changed", pane)), Sense::click());
        let (hover, pressed) = crate::ui::control_fills(t, t.bg.layer_alt);
        let fill = if response.is_pointer_button_down_on() {
            Some(pressed)
        } else if response.hovered() {
            Some(hover)
        } else {
            None
        };
        if let Some(fill) = fill {
            painter.set(
                slot,
                egui::Shape::rect_filled(hit, CornerRadius::same(radius::SMALL), fill),
            );
        }
        if response.has_focus() {
            azur_egui_theme::icons::focus_ring_inset(
                painter,
                hit,
                CornerRadius::same(radius::SMALL),
                t.stroke.focus,
            );
        }
        // The word is the constant's, not a copy of it: a tooltip that says to type something the
        // box no longer understands is worse than no tooltip.
        azur_egui_theme::components::tooltip(
            response.clone(),
            &format!(
                "Show what has changed: this folder's whole tree, filtered to {}",
                crate::fs::sort::CHANGED
            ),
        );
        if response.clicked() {
            out.push(Action::ShowChanges(pane));
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A merged chain's path split into where the row is and what it shows, and joined back for the
    /// column.
    ///
    /// [`chain_split`] is asked by the Name column and by the tooltip, and the two must not disagree
    /// about which folders belong to the row — so it is one function with one test rather than the
    /// same arithmetic written twice. The cases that matter are the ends: no merging at all, where it
    /// has to be the split `Dir::within` and `Dir::leaf` already make; a chain that reaches the folder
    /// being listed, which has nothing in front of it; and a level count larger than the path has,
    /// which cannot be produced by the builder but must not be a panic if it ever is.
    #[test]
    fn a_chain_splits_into_where_it_is_and_what_it_shows() {
        assert_eq!(chain_split(r"x\a\b\c", 0), (r"x\a\b", "c"));
        assert_eq!(chain_split("c", 0), ("", "c"));
        assert_eq!(chain_split(r"x\a\b\c", 2), ("x", r"a\b\c"));
        assert_eq!(chain_split(r"a\b\c", 2), ("", r"a\b\c"));
        assert_eq!(chain_split(r"a\b", 9), ("", r"a\b"));

        // And the dimmed half of the cell, which is those folders and a trailing separator.
        let mut into = String::new();
        chain_ahead(chain_folders(r"a\b\c"), false, &mut into);
        assert_eq!(into, "a > b > ");
        chain_ahead(chain_folders(r"a\b\c"), true, &mut into);
        assert_eq!(into, "… > a > b > ", "and an ellipsis for what was dropped");
        chain_ahead(chain_folders("c"), false, &mut into);
        assert_eq!(into, "", "one folder has nothing in front of it");
    }

    /// A rename field is as wide as its text, not as wide as the column.
    ///
    /// The Name column is whatever the other three leave, which on a narrow pane is narrower
    /// than plenty of names — and a field pinned to it scrolls the name sideways under the
    /// caret, so renaming a long one meant editing it through a letterbox. It grows instead,
    /// over Size, Type and Modified, and stops at the row's right edge.
    #[test]
    fn a_rename_field_grows_past_its_column_but_not_past_the_row() {
        // A name cell from 40 to 160 in a row 600 wide.
        let (left, right, row_right) = (40.0, 160.0, 600.0);
        let column = right - left;

        // Short text: the column's width, unchanged. Nothing overlaps that does not need to.
        assert_eq!(rename_width(30.0, left, right, row_right), column);
        // Text that just fits stays put too.
        assert_eq!(rename_width(column - 12.0, left, right, row_right), column);

        // Longer than the column: wide enough for the text, and wider than the column.
        let long = rename_width(300.0, left, right, row_right);
        assert!(
            long > column,
            "a name wider than its column got a field the width of the column ({long})"
        );
        assert!(
            long >= 300.0,
            "the field is narrower than the text it has to show ({long})"
        );

        // Never past the row. A 5000px name gets the rest of the row and no more.
        let huge = rename_width(5_000.0, left, right, row_right);
        assert!(
            left + huge <= row_right,
            "the field runs {} past the right edge of the row",
            left + huge - row_right
        );
        // And on a pane too narrow for any of this, still something to type in.
        assert!(rename_width(400.0, 40.0, 44.0, 60.0) >= MIN_FIELD);
    }

    /// The Name cell: the name, then its context in secondary ink, and the context is what
    /// gives way when the column is narrow.
    ///
    /// Measured against the real font, because every claim here is about what fits: the widths
    /// below are found by laying the text out, not assumed.
    #[test]
    fn a_row_says_where_it_is_after_its_name_and_gives_that_up_first() {
        let ctx = egui::Context::default();
        azur_egui_theme::fonts::install(&ctx);
        let font = egui::FontId::proportional(14.0);
        let (ink, dim) = (egui::Color32::WHITE, egui::Color32::GRAY);
        let mut checked = false;
        let _ = ctx.run_ui(Default::default(), |ui| {
            let painter = ui.painter();
            let width_of = |text: &str| {
                crate::ui::truncated(painter, text, font.clone(), ink, f32::INFINITY)
                    .size()
                    .x
            };
            let cell = |context: Option<&str>, width: f32| {
                name_galley(
                    painter,
                    "translations_fr.json",
                    context,
                    font.clone(),
                    ink,
                    dim,
                    width,
                )
            };

            // No context — a file in the folder you are looking at, which is not a shortcut.
            // One section, in body ink, and nothing else drawn.
            let plain = cell(None, 400.0);
            assert_eq!(plain.text(), "translations_fr.json");
            assert_eq!(plain.job.sections.len(), 1);
            assert_eq!(plain.job.sections[0].format.color, ink);

            // With context: `name > where`, and **everything from the separator on is
            // secondary**, which is the whole of what makes the name the thing you read.
            const WHERE: &str = "PluginGeosystem\\Resources\\Lang";
            let whole = cell(Some(WHERE), 900.0);
            assert_eq!(whole.text(), format!("translations_fr.json > {WHERE}"));
            assert!(!whole.elided);
            assert_eq!(whole.job.sections.len(), 2);
            assert_eq!(whole.job.sections[0].format.color, ink);
            assert_eq!(
                whole.job.sections[1].format.color, dim,
                "the context is drawn in the same ink as the name"
            );
            let dimmed = whole.job.sections[1].byte_range.clone();
            let separator = &whole.text()[dimmed.start.0..dimmed.end.0];
            assert!(
                separator.starts_with(" > "),
                "the separator belongs to the dimmed half, and got {separator:?}"
            );

            // Narrower: the context loses its *leading* folders, not its last one. The folder
            // immediately holding the file is what identifies it.
            let tail = "translations_fr.json > …\\Lang";
            let cut = cell(Some(WHERE), width_of(tail) + 4.0);
            assert_eq!(
                cut.text(),
                tail,
                "the folders furthest from the file are what should have gone"
            );

            // Narrower still: the name survives and the context is whatever fits. The name is
            // first in the job, so it is the last thing egui cuts.
            let squeezed = cell(Some(WHERE), width_of("translations_fr.json") + 6.0);
            assert!(
                squeezed.text().starts_with("translations_fr.json"),
                "what survived was {:?}, which is not the name",
                squeezed.text()
            );
            checked = true;
        });
        assert!(checked, "the pass never ran, so nothing was checked");
    }

    /// What a rename starts with selected.
    ///
    /// Explorer selects the stem, so that typing replaces the name and leaves the extension
    /// alone. The awkward cases are the point: a dotfile has no extension to speak of, a
    /// double extension only counts the last one, and a folder is a folder whatever is in its
    /// name.
    #[test]
    fn a_rename_selects_the_name_and_not_the_extension() {
        assert_eq!(stem_chars("report.docx", false), "report".len());
        assert_eq!(stem_chars("archive.tar.gz", false), "archive.tar".len());
        // A dotfile is all name.
        assert_eq!(stem_chars(".gitignore", false), ".gitignore".chars().count());
        // No extension at all.
        assert_eq!(stem_chars("Makefile", false), "Makefile".len());
        // A folder called `my.folder` is not a `folder` file.
        assert_eq!(stem_chars("my.folder", true), "my.folder".chars().count());
        // Counted in characters, not bytes, or the caret lands mid-glyph: `réunion` is seven
        // characters and eight bytes, so this number is the whole point of the assertion.
        assert_eq!(stem_chars("réunion.txt", false), 7);
        assert_eq!("réunion".len(), 8, "and it would be 8 if this counted bytes");
        assert_eq!(stem_chars("", false), 0);
    }

    /// Every ink on the status line can be read on the status line.
    ///
    /// The bar is `background-layer-alt`, one rung off the surface every other status mark in this
    /// window sits on — so the one thing this catches is a colour chosen for its meaning and never
    /// checked where it landed. It has caught three, and none of them was subtle: Azur's
    /// `status.success` measures **2.37:1** on the light bar and its `status.warning` **2.23:1**, both
    /// well under the floor for a shape, let alone for `13 changed`; and the scan's own figure was in
    /// `text-disabled` at **2.20:1** in the dark theme, which is no way to show a number somebody is
    /// invited to check. [`crate::theme::Bar`] is the answer to the first two and carries the
    /// measurements.
    ///
    /// **Nearly everything here is held to [`TEXT`]**, because nearly everything here is read rather
    /// than glanced at: a count, a branch name, `13 changed`. The exception is `text-tertiary` at
    /// **3.62:1** on the dark bar, which is the deliberately quietest thing on the line — the scan's
    /// figure and the parenthesis saying how much is not on show — and is held to [`SHAPE`].
    /// `crate::theme::Syntax` draws the same line the other way, where a screenful of diff *is* text
    /// and gets two rungs of its own for it.
    #[test]
    fn every_ink_on_the_status_line_can_be_read() {
        use azur_egui_theme::contrast::{ratio, SHAPE, TEXT};

        for t in [Theme::dark(), Theme::light()] {
            let name = if t.dark { "dark" } else { "light" };
            let bar = t.bg.layer_alt;
            for (what, ink) in [
                ("the selected count", t.bar.counted),
                ("the total", t.text.secondary),
                ("an operation in progress", t.text.primary),
                ("the branch", t.bar.info),
                ("commits ahead", t.bar.success),
                ("commits behind", t.bar.danger),
                ("what has changed", t.bar.warning),
            ] {
                let got = ratio(ink, bar);
                assert!(
                    got >= TEXT,
                    "{name}: {what} is {got:.2}:1 on the status bar, under the {TEXT}:1 floor for text"
                );
            }
            for (what, ink) in [
                ("what is not shown", t.text.tertiary),
                ("the scan's own figure", t.text.tertiary),
            ] {
                let got = ratio(ink, bar);
                assert!(
                    got >= SHAPE,
                    "{name}: {what} is {got:.2}:1 on the status bar, under the {SHAPE}:1 floor"
                );
            }
        }
    }

    /// A word on the status line and a glyph on it come out on the same middle, at any scale and
    /// wherever the pane's bottom edge happens to land.
    ///
    /// The two failures this catches are the two halves of [`status_geometry`]. **Pairing the wrong
    /// two helpers**: the line used to centre each galley's *box* while centring each glyph on its
    /// ink, which is the mistake [`CELL_LIFT`] exists to correct one row up. And **a band off the
    /// pixel grid**: a fractional rect is feathered at both edges, so its visible middle is not the
    /// one anything was centred on — which is why the rect is rounded before the baseline is taken.
    ///
    /// The ink's own middle is derived from `galley_baseline` and `ink_lift` rather than measured off
    /// the glyphs: `azur::components` owns the measurement, and what is being checked here is that
    /// this line asks it the right question.
    #[test]
    fn the_status_line_puts_its_words_and_its_glyphs_on_one_middle() {
        use azur_egui_theme::components::{galley_baseline, ink_lift};
        use egui::emath::GuiRounding as _;

        let t = Theme::dark();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let ctx = egui::Context::default();
            azur_egui_theme::fonts::install(&ctx);
            ctx.set_pixels_per_point(scale);
            // Twice, because the first pass has no fonts laid out yet and `set_pixels_per_point`
            // only reaches the painter on the pass after it.
            for _ in 0..2 {
                let _ = ctx.run_ui(Default::default(), |ui| {
                    let painter = ui.painter();
                    // Every awkward fraction of a point a split can leave a pane's bottom edge on.
                    for offset in [0.0, 0.1, 0.3, 0.5, 0.72, 0.9] {
                        let asked = Rect::from_min_size(
                            pos2(0.0, 400.0 + offset),
                            vec2(600.0, STATUS_HEIGHT),
                        );
                        let (band, line, baseline) = status_geometry(painter, &t, asked);
                        let device = 1.0 / scale;

                        // The band is on whole device pixels, and it is still the height it asked to
                        // be — rounded, not floored.
                        assert_eq!(
                            band.top(),
                            band.top().round_to_pixels(scale),
                            "the band's top is off the grid at {scale}× / {offset}"
                        );
                        assert_eq!(band.bottom(), band.bottom().round_to_pixels(scale));
                        assert!(
                            (band.height() - STATUS_HEIGHT).abs() <= device,
                            "the band came out {} tall rather than {STATUS_HEIGHT}",
                            band.height()
                        );

                        // The line the content is placed against is the band nudged up, and it is the
                        // *whole* line that moves: a glyph centred on `line` and a word on `line`'s
                        // own baseline still have to land on one middle, or [`NUDGE`] would be
                        // giving away the level the rest of this bought.
                        assert_eq!(
                            band.top() - line.top(),
                            NUDGE,
                            "the line is not {NUDGE} above the band it is painted in"
                        );
                        let glyph = icon_rect(line, 8.0, MARK).center().y;
                        let probe = painter.layout_no_wrap(
                            "Ay".to_owned(),
                            t.fonts.caption.clone(),
                            t.text.primary,
                        );
                        // `ink_lift` is the ink's middle measured from a *box's* middle, so half the
                        // ink's height above the baseline is what it and the box baseline give back.
                        let half = galley_baseline(&probe)
                            - probe.size().y * 0.5
                            - ink_lift(painter, &t.fonts.caption);
                        let middle = baseline - half;
                        assert!(
                            (middle - glyph).abs() <= 1.0_f32.max(device),
                            "at {scale}× / {offset}: the words sit {:+.2} off the glyphs",
                            middle - glyph
                        );
                    }
                });
            }
        }
    }
}
