//! The window: chrome, sidebar, breadcrumb, listing.
//!
//! Everything here paints at explicit rects rather than laying widgets out. That
//! is a deliberate choice and the reason a listing of a hundred thousand files
//! scrolls at the refresh rate: a row is six painter calls against a rect this
//! module computed, not a `horizontal()` layout with a `Label` per cell that egui
//! has to measure, allocate, hit-test and register a widget id for. The design
//! system's own collections do the same thing for the same reason.
//!
//! The rule that keeps it honest: **no colour is chosen here.** Every fill, line
//! and text colour comes from a role on [`crate::theme::Theme`], so the whole
//! window still re-themes from one place.

pub mod breadcrumb;
pub mod chrome;
pub mod console;
pub mod deps;
pub mod filelist;
pub mod menu;
pub mod preview;
pub mod sidebar;

use std::sync::Arc;

use azur_egui_theme::icons::Icon;
use azur_egui_theme::tokens::{radius, space, typography};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Painter, Rect, Response, Sense, Stroke,
    Ui,
};

use crate::theme::Theme;

/// The air inside a tab strip: `tokens::space::S2`.
///
/// It used to be the canvas showing *around* the panels as well, and the panels stopped short
/// of the window's edges by it. They no longer do — nothing separates a panel from the window
/// but the window's own border — so what is left of it is the inset a strip gives its tabs, and
/// `chrome::RESIZE_BAND` is the four points the resize edges take back out of the panels.
pub const GUTTER: f32 = space::S2;

/// The line *between* two panels: one point wide, showing [`seam`] through it.
///
/// Panels used to be separated by [`GUTTER`] and each ringed in `stroke-subtle`, which
/// made every boundary three lines wide — two borders and a channel of canvas between
/// them — and read as a row of loose cards rather than as one window divided up.
pub const SEAM: f32 = azur_egui_theme::desktop::SEAM;

/// The colour a panel boundary shows, and the fill of the surfaces welded to it.
///
/// One colour doing three jobs on purpose: the selected tab, the path bar directly under it,
/// and the seams between panels are the frame the panels sit in, so they are the same surface
/// seen in three places rather than three decisions that happen to agree.
///
/// `stroke-subtle`, and the design system's decision now — see `azur::desktop::seam`, which
/// carries the measurements and the floor the column header puts under it. It was
/// `background-control-active` here, and came down a step because as a filled band across the top
/// of every pane that was too loud in the dark theme; the dark frame is deliberately the quieter
/// of the two sides, at ΔL\* 6.1 against the light theme's unchanged `GRAY_14`.
pub fn seam(t: &Theme) -> Color32 {
    azur_egui_theme::desktop::seam(t)
}

/// Draw the widgets `add` puts up with square corners.
///
/// **The three text inputs in this program are square**: the rename field over a name, the path
/// field the breadcrumb turns into, and the filter box. All three appear in place of something
/// square and take its shape while they are up — a row, the breadcrumb, a bar of flush panels —
/// and a 4px radius on a 20px-tall box in the middle of that reads as a bubble.
///
/// `azur::desktop::squared` is the rule and the mechanism; this is the name the call sites here
/// already use.
pub fn squared<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    azur_egui_theme::desktop::squared(ui, add)
}

/// The hover and pressed fills for a control, whatever surface it is painted on.
///
/// [`hover_fill`] and then one rung further in the same direction. `surface` is not read, and
/// that is the point: Azur's `control` / `control-hover` / `control-active` ladder is written for
/// a control on `background-layer`, so a control on a surface that is *itself* one of those steps
/// had nowhere to go and painted the surface's own colour onto the surface — the fill vanished at
/// the moment of the press instead of deepening. That was the path bar in the **light** theme,
/// where [`seam`] and `control-active` are the same `GRAY_14`.
///
/// Both rungs are `azur::desktop`'s now, and so is the check that they step away from every
/// surface in both themes. `control_active` is left to the two places here that use it as an
/// inert fill rather than as a press — a sidebar row and a tab go quiet while they are dragged.
pub fn control_fills(t: &Theme, _surface: Color32) -> (Color32, Color32) {
    azur_egui_theme::desktop::control_fills(t)
}

/// A toolbar button's side, and the icon box inside it. `tokens::control::SMALL`
/// with `ICON_SMALL`, which is the design system's dense-toolbar pairing.
pub const TOOL_SIZE: f32 = 24.0;
pub const TOOL_ICON: f32 = 14.0;

/// Lay one line out, cut to `width` with an ellipsis.
///
/// The galley is cached by egui on the finished job, so re-laying an unchanged row
/// every frame costs a hash rather than a shaping pass.
pub fn truncated(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<egui::Galley> {
    let mut job =
        egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    painter.layout_job(job)
}

/// Snap a text origin to whole *device* pixels.
///
/// A glyph atlas is a texture, so a quad landing between texels is sampled between them and
/// the text comes out soft with a grey fringe down each stem. Both coordinates matter, and
/// the x as much as the y: panes are split by fractions of a window, so a column's left edge
/// is fractional about half the time.
///
/// `round_to_pixels` rather than `f32::round`, because a logical point is not a pixel. At
/// 125% scaling, rounding to a whole point leaves the glyph on a quarter-pixel — which is
/// most of the blur it was meant to remove, on the displays most likely to have it.
///
/// It does a second job wherever text is laid out in a run of rows, and that one is not about
/// sharpness: epaint rounds a galley's rows to whole pixels *relative to the galley's own origin*,
/// so a column of rows placed at fractional and differing y values has each baseline rounded a
/// different way. The text still looks sharp and no longer sits on one line — see
/// [`console`], where a fractional row pitch is exactly what that looked like.
pub fn snap(painter: &Painter, at: egui::Pos2) -> egui::Pos2 {
    use egui::emath::GuiRounding as _;
    at.round_to_pixels(painter.pixels_per_point())
}

/// What every inline text field in this window takes off its caret, and how far it drops it.
///
/// Three points off the top and three points down, applied by [`nudge_caret`]. One pair rather than
/// a figure per field: the correction is not about any one box, it is about egui measuring the caret
/// against the galley's *line box* — see [`nudge_caret`] — so a field of mostly x-height text gets a
/// caret that stands above the words and below them. That is the same in the breadcrumb's filter and
/// in the preview's find bar, and it would be the same in the next one.
///
/// Two figures because they are two decisions. Reducing the height alone leaves the caret high;
/// dropping it alone leaves it long. It is the caret's half of the correction
/// `breadcrumb::FILTER_TEXT_LIFT` makes for the words — the lift deliberately does not move the
/// caret, because the two do not want the same number.
pub const CARET_SHORTER: f32 = 3.0;
pub const CARET_LOWER: f32 = 3.0;

/// Where the next shape a widget paints will land, so [`nudge_caret`] can find it again.
pub fn shape_mark(ui: &Ui) -> egui::layers::ShapeIdx {
    let layer = ui.layer_id();
    ui.ctx().graphics_mut(|g| g.entry(layer).next_idx())
}

/// How wide a value may get before it wraps.
///
/// The design system's tooltip measure is 280 — a *paragraph's*, and the reason this tooltip is not
/// one: `9.38 KB (9,605 bytes)` in a column beside its key does not fit in it, so every value long
/// enough to be worth reading wrapped under its own key. Twice that clears every value the four
/// columns can hold and most names besides, and it is still a bound rather than none: a two-hundred
/// character name wraps instead of making the tooltip as wide as the window.
pub const TIP_VALUE: f32 = 560.0;

/// Draw a tooltip's key/value pairs as two columns.
///
/// **Two columns and not two labels a line**, which is the whole reason this is drawn rather than
/// written into a string: a value has to start at the same `x` on every line, and a proportional font
/// cannot be padded into a column with spaces. Both halves are laid out here, the keys' column is as
/// wide as the widest key, and the values start after it.
///
/// The keys are `text-secondary` against the values' `text-primary` — the same two colours the dimmed
/// half of a Name cell uses, saying the same thing about which half of the line is the answer.
///
/// A pair is **one line unless the value wraps**, and then the line is as tall as the value and the key
/// sits on the first of its rows. Both are the same font, so a shared top is a shared baseline — which
/// is what the two halves of a line have to agree on, and the one thing that would go wrong if a key
/// and a value were ever set differently. `space-1` between the lines: this is a table of five or six
/// things read at a glance, not a paragraph, so it is set tighter than the caption's own leading.
pub fn tooltip_table<S: AsRef<str>>(ui: &mut Ui, t: &Theme, about: &[(&str, S)]) {
    let font = t.fonts.caption.clone();
    let painter = ui.painter().clone();

    let keys: Vec<_> = about
        .iter()
        .map(|(key, _)| painter.layout_no_wrap(key.to_string(), font.clone(), t.text.secondary))
        .collect();
    let values: Vec<_> = about
        .iter()
        .map(|(_, value)| {
            painter.layout(value.as_ref().to_owned(), font.clone(), t.text.primary, TIP_VALUE)
        })
        .collect();

    let column = keys.iter().map(|g| g.size().x).fold(0.0, f32::max) + space::S4;
    let width = column + values.iter().map(|g| g.size().x).fold(0.0, f32::max);
    let height: f32 = values
        .iter()
        .map(|g| g.size().y.max(typography::LINE_CAPTION) + space::S1)
        .sum();
    // Allocated, so the tooltip's frame is the size of its table: the popup has no measure of its own
    // — see `tooltip_at_pointer_ui` — and this is what it hugs.
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());

    let mut y = rect.top();
    for (key, value) in keys.into_iter().zip(values) {
        let step = value.size().y.max(typography::LINE_CAPTION) + space::S1;
        painter.galley(
            crate::ui::snap(&painter, pos2(rect.left(), y)),
            key,
            Color32::PLACEHOLDER,
        );
        painter.galley(
            crate::ui::snap(&painter, pos2(rect.left() + column, y)),
            value,
            Color32::PLACEHOLDER,
        );
        y += step;
    }
}

/// Shorten the text caret a field has just painted, and move it down.
///
/// # Why it is done to the shape and not to the field
///
/// egui owns the caret. `TextEdit` paints it in `paint_cursor_end` as a one-point line segment from
/// the centre-top of the galley row's box to its centre-bottom — a *line box*, so it spans the
/// ascender space above the capitals and the descender space below the baseline, and in a field of
/// mostly x-height text it reads tall and high. There is no knob for it: `Visuals::text_cursor`
/// carries the colour and the blink and no geometry, and the rect comes from the galley.
///
/// `TextField::text_lift` cannot do this either — it moves the whole inner rect, so the words go
/// with the caret. That is the correction the *text* needs and it is already applied; this is the
/// separate one the caret needs on top, and the two have to be able to differ.
///
/// So the segment is edited after the fact. Painting is retained-mode here: the shape sits in the
/// layer's list until the frame is tessellated, and [`shape_mark`] is where the field's shapes
/// start. The caret is picked out of that range by being a vertical segment in the caret's own
/// colour ([`azur_egui_theme`] sets it to `text.primary`), and only the first match is touched.
///
/// `shorter_by` comes off the **top**, which is the end that overshoots; `lower_by` moves both ends.
/// A blink that is currently off has painted nothing, and then there is nothing to find and nothing
/// to do — which is why this is a no-op rather than an assertion.
pub fn nudge_caret(ui: &Ui, from: egui::layers::ShapeIdx, shorter_by: f32, lower_by: f32) {
    use egui::epaint::Shape;
    use egui::layers::ShapeIdx;

    let want = ui.visuals().text_cursor.stroke.color;
    let layer = ui.layer_id();
    ui.ctx().graphics_mut(|g| {
        let list = g.entry(layer);
        let end = list.next_idx().0;
        let mut found = false;
        for i in from.0..end {
            if found {
                break;
            }
            list.mutate_shape(ShapeIdx(i), |clipped| {
                let Shape::LineSegment { points, stroke } = &mut clipped.shape else {
                    return;
                };
                let vertical = (points[0].x - points[1].x).abs() < 0.5;
                if !vertical || stroke.color != want {
                    return;
                }
                let (top, bottom) = if points[0].y <= points[1].y { (0, 1) } else { (1, 0) };
                points[top].y += shorter_by + lower_by;
                points[bottom].y += lower_by;
                found = true;
            });
        }
    });
}

/// Paint a galley vertically centred in `rect`, starting at its left edge.
///
/// This is the helper every file name in the listing goes through.
pub fn text_left(painter: &Painter, rect: Rect, galley: Arc<egui::Galley>) {
    let at = pos2(rect.left(), rect.center().y - galley.size().y * 0.5);
    painter.galley(snap(painter, at), galley, Color32::PLACEHOLDER);
}

/// The same, pushed against the right edge — for the numeric column.
pub fn text_right(painter: &Painter, rect: Rect, galley: Arc<egui::Galley>) {
    let at = pos2(
        rect.right() - galley.size().x,
        rect.center().y - galley.size().y * 0.5,
    );
    painter.galley(snap(painter, at), galley, Color32::PLACEHOLDER);
}

/// The square an icon of `size` occupies, vertically centred with its left edge at
/// `x`.
pub fn icon_rect(row: Rect, x: f32, size: f32) -> Rect {
    Rect::from_min_size(
        pos2(x.round(), (row.center().y - size * 0.5).round()),
        vec2(size, size),
    )
}

/// A hand-painted icon button at a rect this caller already knows.
///
/// Azur's `IconButton` would do the same job through the layout system, which is
/// the wrong shape for chrome laid out by arithmetic — a breadcrumb has to know
/// where its buttons end before it can measure the segments that follow. The states are the
/// design system's: `desktop::latched` when it is on — the same surface a selected row wears —
/// and otherwise the two [`control_fills`] for the surface behind it.
///
/// `surface` is that surface. It has to be told, because a painter cannot read back what
/// is already under the rect, and the two toolbars this button appears on are different
/// colours — the tab strip is `background-layer`, the path bar is [`seam`].
#[allow(clippy::too_many_arguments)]
pub fn tool_button(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    id: Id,
    glyph: Icon<'_>,
    tooltip: &str,
    enabled: bool,
    active: bool,
    surface: Color32,
) -> Response {
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );

    let corner = CornerRadius::same(radius::SMALL);
    let (hover, pressed) = control_fills(t, surface);
    // A latched button wears the surface a selected row wears — `azur::desktop::latched`, which
    // is where that rule and its numbers live. It used to be `accent-subtle`, Azur's own latch
    // tint, which in the dark theme is a navy barely off the bar it sits on: the one toggle on
    // this bar read as *nearly* on.
    let latched = azur_egui_theme::desktop::latched(t, response.hovered());
    let fill = if !enabled {
        None
    } else if active {
        Some(latched.0)
    } else if response.is_pointer_button_down_on() {
        Some(pressed)
    } else if response.hovered() {
        Some(hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, corner, fill);
    }

    let color = if !enabled {
        t.text.disabled
    } else if active {
        latched.1
    } else if response.hovered() {
        t.text.primary
    } else {
        t.text.secondary
    };
    glyph(
        ui.painter(),
        Rect::from_center_size(rect.center(), vec2(TOOL_ICON, TOOL_ICON)),
        color,
    );

    if response.has_focus() {
        azur_egui_theme::icons::focus_ring_inset(ui.painter(), rect, corner, t.stroke.focus);
    }
    if !tooltip.is_empty() && enabled {
        azur_egui_theme::components::tooltip(response.clone(), tooltip);
    }
    response
}

/// A horizontal `stroke-subtle` rule across `rect`'s bottom edge.
pub fn rule_below(painter: &Painter, rect: Rect, t: &Theme) {
    let y = rect.bottom().round() - 0.5;
    painter.line_segment(
        [pos2(rect.left(), y), pos2(rect.right(), y)],
        Stroke::new(1.0, t.stroke.subtle),
    );
}

/// A section label in the sidebar: `body-strong` in `text-secondary`, which is what
/// `collection_label` gives a group of rows.
pub fn section_label(painter: &Painter, rect: Rect, t: &Theme, text: &str) {
    let galley = truncated(
        painter,
        text,
        t.fonts.caption.clone(),
        t.text.tertiary,
        rect.width(),
    );
    text_left(painter, rect, galley);
}

/// Paint the accent bar Azur puts down the left edge of a selected row.
///
/// `accent.mark` — "the accent as ink rather than as a surface: … a 2px selection bar", which is
/// the role's own job description. It was `accent.default`, and became indistinguishable from the
/// row the moment the selected *fill* became `accent.default` too.
pub fn selection_bar(painter: &Painter, row: Rect, t: &Theme) {
    azur_egui_theme::desktop::selection_bar(painter, row, t)
}

/// The fill under the pointer on anything you hover in order to *go* somewhere: a row in the
/// listing, a row in the sidebar, a segment or a chevron on the path bar.
///
/// `background-control-hover`, three rungs along Azur's neutral ramp from the `GRAY_5` it
/// specifies, which `crate::theme` puts there through `azur::desktop::apply`. Read back from the
/// theme rather than named here so that there is one value and not two: the same token is what the
/// context menu, the column headers, the caption buttons, the tab strip, the application mark and
/// the design system's own `MenuItem` read, and a second constant living here is how Back,
/// Forward, Up and Refresh came to be the only four hovers in the window still wearing the old
/// grey.
///
/// `GRAY_8` in the dark theme, not the `GRAY_9` first asked for, because on a listing that is
/// nearly black the band read a shade hot — and darker is *more* legible, not less: a row's name
/// on it goes from 5.7:1 to 7.2:1 and its metadata from 2.4:1 to 3.0:1.
pub fn hover_fill(t: &Theme) -> Color32 {
    azur_egui_theme::desktop::hover_fill(t)
}

/// The row fill for a state, or `None` to leave the surface showing.
///
/// Azur's list vocabulary is `background-card-hover` hovered and `accent-subtle` selected. The
/// hover is [`hover_fill`], and in the dark theme the selection is the accent's *surface* ramp
/// instead — `accent.active`, with `accent.default` under the pointer as well.
///
/// It got there by walking back down, every figure sampled off the framebuffer. `AZURE_ACCENT`
/// (`#3aa0ff`) was asked for first and was far too bright: a selected row's name measured 2.5:1
/// and its Size, Type and Modified columns 1.06:1, which is to say they were gone.
/// `accent.default` was halfway back at 6.0:1 and 2.5:1; one rung further again is **8.0:1 and
/// 3.3:1**. Each step down bought legibility rather than cost it, which is the tell that the first
/// one was three rungs too far.
///
/// `azur::desktop::row_fill` is where that lives now, along with what the light theme does
/// instead — it keeps `accent-subtle`, because the ink on a row does not change when the row is
/// selected, and a dark fill under near-black metadata is the same mistake pointing the other way.
pub fn row_fill(t: &Theme, selected: bool, hovered: bool) -> Option<Color32> {
    azur_egui_theme::desktop::row_fill(t, selected, hovered)
}

/// The fill under a selected row in a listing that does **not** have the keyboard.
///
/// A window with two panes and a console in one of them can show three selections at once, only one
/// of which the arrow keys are about. Without a second fill there is nothing on screen that says
/// which — so `Shift+Down` appeared to do nothing, in a pane full of highlighted rows.
///
/// **Quieter, not greyer.** The platform's own answer is to drain the colour out of an unfocused
/// selection, and that is the wrong end of it here: the rows are still selected, and the next paste or
/// delete is still about them. So it stays blue and loses its emphasis instead — `accent-subtle`,
/// which in the dark theme is the darkest rung of the accent's surface ramp.
///
/// The light theme cannot go the same way, because there `accent-subtle` *is* the selected fill and
/// the ramp has nowhere lighter to go. It gets `background-control-active` instead: the neutral a
/// pressed control wears, which is the one surface in the palette that reads as "set, but not the
/// thing you are working in".
///
/// A candidate to move into `azur::desktop` beside [`row_fill`] — an inactive selection is a desktop
/// pattern rather than a file manager's idea. It is here until something other than this window wants
/// it, so that the design system is not given an API with one caller and no second opinion.
pub fn row_fill_quiet(t: &Theme) -> Color32 {
    if t.dark {
        t.accent.subtle
    } else {
        t.bg.control_active
    }
}

/// The rectangle a file drop would land in: a folder row, or the listing of the folder on show.
///
/// Square, and painted *over* the listing rather than under it. Both of those are corrections,
/// and both are `azur::desktop::drop_target`'s now: a selected row's fill is an opaque surface of
/// its own drawn after this, so a wash underneath it vanished completely — dropping onto a folder
/// you had selected showed nothing at all.
pub fn drop_target(painter: &Painter, rect: Rect, t: &Theme) {
    azur_egui_theme::desktop::drop_target(painter, rect, t)
}

/// Draw a translucent accent wash and outline — the drop preview for a docking
/// gesture, and the "this pane has focus" hint while dragging.
///
/// The same mark as [`drop_target`], for the same reason it is square: it is showing where a
/// rectangle will go. It carried `radius-medium` for as long as the panes did.
pub fn drop_preview(painter: &Painter, rect: Rect, t: &Theme) {
    azur_egui_theme::desktop::drop_target(painter, rect, t)
}

/// Centre a single line in a rect — for the empty and error states.
pub fn text_center(painter: &Painter, rect: Rect, font: FontId, color: Color32, text: &str) {
    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use azur_egui_theme::desktop;

    /// **The two columns line up, and the keys are the quiet half.**
    ///
    /// The two things [`tooltip_table`] exists to do, and neither of them survives being written into
    /// a string: a meaning has to start at the same `x` whatever the marker beside it is as wide as,
    /// and the marker is `text-secondary` against the meaning's `text-primary`. Both were untested
    /// while this was private to the listing — it drew the row tooltip, where a wrong column reads as
    /// untidy. The filter box's syntax is the case where a ragged column is the whole problem.
    ///
    /// The keys here are deliberately of **different widths**, because equal ones would pass with the
    /// column arithmetic removed altogether.
    #[test]
    fn a_tooltip_table_aligns_its_values_and_dims_its_keys() {
        let ctx = egui::Context::default();
        azur_egui_theme::fonts::install(&ctx);
        let t = Theme::dark();
        let pairs: &[(&str, &str)] = &[
            ("!word", "leave it out"),
            ("@git", "only what git says changed"),
            ("word$", "match at the end"),
        ];

        let mut drawn: Vec<(egui::Pos2, Color32, String)> = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let from = shape_mark(ui);
            tooltip_table(ui, &t, pairs);
            let layer = ui.layer_id();
            ui.ctx().graphics_mut(|g| {
                for clipped in g.entry(layer).all_entries().skip(from.0) {
                    if let egui::epaint::Shape::Text(text) = &clipped.shape {
                        let colour = text
                            .galley
                            .job
                            .sections
                            .first()
                            .map(|s| s.format.color)
                            .unwrap_or(Color32::PLACEHOLDER);
                        drawn.push((text.pos, colour, text.galley.text().to_owned()));
                    }
                }
            });
        });

        assert_eq!(drawn.len(), pairs.len() * 2, "expected a key and a value each: {drawn:?}");
        let find = |want: &str| {
            drawn
                .iter()
                .find(|(_, _, text)| text == want)
                .unwrap_or_else(|| panic!("`{want}` was not painted: {drawn:?}"))
        };

        // Every meaning starts at the same x, however wide its marker is.
        let xs: Vec<f32> = pairs.iter().map(|(_, meaning)| find(meaning).0.x).collect();
        for x in &xs {
            assert_eq!(
                *x, xs[0],
                "the meanings start at {xs:?}, so they are not a column"
            );
        }
        // And that column is past the widest key, not merely a shared arbitrary x.
        let widest = pairs
            .iter()
            .map(|(key, _)| find(key).0.x + 1.0)
            .fold(0.0_f32, f32::max);
        assert!(xs[0] > widest, "the values start at {} , inside the keys", xs[0]);

        // The markers are the quiet half; the meanings are the answer.
        for (key, meaning) in pairs {
            assert_eq!(find(key).1, t.text.secondary, "`{key}` is not `text-secondary`");
            assert_eq!(find(meaning).1, t.text.primary, "`{meaning}` is not `text-primary`");
        }
    }

    /// The caret really is the shape [`nudge_caret`] goes looking for.
    ///
    /// This is a guard against **egui**, not against this crate. The helper finds the caret by what
    /// it looks like — a vertical one-point segment in `Visuals::text_cursor`'s colour — so an egui
    /// that painted it as a rect, or in a colour of its own, would turn `nudge_caret` into a silent
    /// no-op and the filter's caret would quietly go back to standing tall. Nothing else would fail.
    ///
    /// So the caret here is painted by a real `TextEdit` and then measured: found at all, moved by
    /// the figures asked for, and shorter by the difference between them.
    #[test]
    fn the_caret_is_found_shortened_from_the_top_and_dropped() {
        /// The first vertical segment in the caret's colour, at or after `from`.
        fn caret_of(ui: &Ui, from: egui::layers::ShapeIdx) -> Option<[egui::Pos2; 2]> {
            let want = ui.visuals().text_cursor.stroke.color;
            let layer = ui.layer_id();
            ui.ctx().graphics_mut(|g| {
                g.entry(layer)
                    .all_entries()
                    .skip(from.0)
                    .find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::LineSegment { points, stroke }
                            if stroke.color == want
                                && (points[0].x - points[1].x).abs() < 0.5 =>
                        {
                            Some(*points)
                        }
                        _ => None,
                    })
            })
        }

        // Run one frame with a focused field and return the caret, nudged by `by` or left alone.
        let run = |by: Option<(f32, f32)>| -> [egui::Pos2; 2] {
            let ctx = egui::Context::default();
            azur_egui_theme::fonts::install(&ctx);
            let id = Id::new("azur-caret-probe");
            let mut text = String::from("abc");
            let mut found = None;
            // Twice: focus asked for in one frame is focus the field has in the next, and the
            // caret is only painted for a field that has it.
            for _ in 0..2 {
                let mut seen = None;
                let _ = ctx.run_ui(Default::default(), |ui| {
                    // A blink that is off has painted nothing, and this test is not about timing.
                    ui.visuals_mut().text_cursor.blink = false;
                    ui.memory_mut(|m| m.request_focus(id));
                    let from = shape_mark(ui);
                    ui.add(egui::TextEdit::singleline(&mut text).id(id));
                    if let Some((shorter, lower)) = by {
                        nudge_caret(ui, from, shorter, lower);
                    }
                    seen = caret_of(ui, from);
                });
                found = seen;
            }
            found.expect(
                "no caret was painted for a focused field -- either egui no longer draws one as a \
                 vertical segment in `text_cursor`'s colour, in which case `nudge_caret` is now a \
                 no-op, or this test failed to give the field focus",
            )
        };

        let plain = run(None);
        let moved = run(Some((3.0, 3.0)));

        let ends = |seg: [egui::Pos2; 2]| {
            let (a, b) = (seg[0].y, seg[1].y);
            if a <= b { (a, b) } else { (b, a) }
        };
        let (plain_top, plain_bottom) = ends(plain);
        let (moved_top, moved_bottom) = ends(moved);

        assert!(
            plain_bottom - plain_top > 3.0,
            "the caret is only {} points tall, so taking 3 off it is not a correction",
            plain_bottom - plain_top
        );
        assert_eq!(
            moved_bottom, plain_bottom + 3.0,
            "the caret's bottom did not drop by 3"
        );
        assert_eq!(
            moved_top,
            plain_top + 6.0,
            "the caret's top should drop by the 3 it loses in height plus the 3 it moves"
        );
        assert_eq!(
            (moved_bottom - moved_top),
            (plain_bottom - plain_top) - 3.0,
            "the caret is not 3 points shorter"
        );
    }

    /// This window is wearing the design system's application-window preset, and every
    /// colour helper here is the same value the preset decided.
    ///
    /// The three things it can catch, none of which the design system's own tests can:
    /// [`crate::theme`] forgetting to call `desktop::apply`, so the hover token never
    /// moves and every widget in the window quietly agrees on Azur's `GRAY_5`; a helper
    /// here drifting back to a value of its own, which is how four toolbar buttons ended
    /// up a rung and a half off everything around them; and the two hover tokens parting
    /// company, so a hovered row and a hovered button are different greys.
    ///
    /// What the values *are*, and that they step away from every surface in both themes,
    /// is `azur_egui_theme::desktop`'s to hold — it is the one that knows why.
    #[test]
    fn the_window_wears_the_desktop_preset() {
        for t in [Theme::dark(), Theme::light()] {
            let name = if t.dark { "dark" } else { "light" };
            let (hover, pressed) = control_fills(&t, seam(&t));
            assert_eq!(
                hover,
                desktop::hover_fill(t.azur()),
                "{name}: this window's hover is not the preset's"
            );
            assert_eq!(
                t.bg.card_hover, t.bg.control_hover,
                "{name}: a hovered row and a hovered button are different greys"
            );
            assert_ne!(
                hover,
                azur_egui_theme::Theme::of(t.azur().kind()).bg.control_hover,
                "{name}: `desktop::apply` was never called — the hover is still Azur's own rung"
            );
            assert_eq!(pressed, desktop::press_fill(t.azur()));
            for (selected, hovered) in [(true, true), (true, false), (false, true), (false, false)]
            {
                assert_eq!(
                    row_fill(&t, selected, hovered),
                    desktop::row_fill(t.azur(), selected, hovered),
                    "{name}: a row painted {selected}/{hovered} is not the preset's"
                );
            }
        }
    }
}
