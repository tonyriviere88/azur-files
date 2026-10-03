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
pub mod grid;
pub mod menu;
pub mod preview;
pub mod sidebar;
pub mod transfers;

use std::path::{Path, PathBuf};
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

/// The colour a panel boundary shows: the line between two panes, behind the sidebar's edge,
/// and along the top of a preview or console panel.
///
/// **The line only.** This and [`bar`] were one function until a palette wanted a pale path bar
/// and a seam you can see — see [`crate::theme::Surfaces::separator`], which is where that split
/// is argued. In the dark palette the two still return the same colour, so nothing there moved;
/// what changed is that a palette can now answer them differently.
///
/// `stroke-subtle` there, and the design system's decision — see `azur::desktop::seam`, which
/// carries the measurements and the floor the column header puts under it. It was
/// `background-control-active` here, and came down a step because as a filled band across the top
/// of every pane that was too loud in the dark theme; the dark frame is deliberately the quieter
/// of the two sides, at ΔL\* 6.1 against `paper::SLATE_7` — the rung Azur's cool light ramp put
/// where `GRAY_14` used to be, half a ΔL\* away, so this figure is the one it always was.
pub fn seam(t: &Theme) -> Color32 {
    t.surfaces.separator
}

/// The fill of the bar across the top of a pane, and of the tab welded to it.
///
/// One colour doing three jobs on purpose: the focused pane's active tab, the path bar directly
/// under it, and the filter field at that bar's right-hand end are one surface seen in three
/// places rather than three decisions that happen to agree. That is why no hairline is drawn
/// between the tab and the bar — a `stroke-subtle` line through a weld cuts it in half — and why
/// the filter field takes this rather than `background-control`: a field that is a different
/// colour from the two points of bar around it reads as a hole in it.
///
/// The same value as [`seam`] in the dark palette, and a pale surface of its own in the light
/// one.
pub fn bar(t: &Theme) -> Color32 {
    t.surfaces.bar
}

/// What outlines a field that sits *in* the path bar, at rest: the filter box.
///
/// **[`seam`] when it reads against [`bar`], and `stroke-control` when it does not.**
///
/// The seam's colour is what this wants to be. With the field's fill matching the bar — see
/// [`bar`] — the outline is the whole of what makes the box a box, and Azur's `stroke-control` is
/// sized to separate a field from `background-layer`: against a pale bar it lands as a hard dark
/// rectangle where the window's other boundaries are all one quiet line.
///
/// The condition is not defensive coding, it is the dark palette. There the bar *is* the seam —
/// one value, `stroke-subtle`, because the tab, the path bar and the panel boundaries are one
/// surface — so an outline in the seam's colour would be an outline the colour of its own fill,
/// and the filter box would have no edge at all. Measured in ΔL\* rather than assumed, because
/// that is the question being asked: do these two read as different surfaces. A palette that
/// gives the bar a colour of its own gets the seam; one that does not keeps what it always had,
/// to the byte.
pub fn field_outline(t: &Theme) -> Color32 {
    use azur_egui_theme::contrast::{apart, SAME};
    if apart(seam(t), bar(t)) >= SAME {
        seam(t)
    } else {
        t.stroke.control
    }
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
/// where [`seam`] and `control-active` are the same `paper::SLATE_7`.
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

/// Whether the entry called `name`, in `folder`, is waiting on a paste — and so is drawn faded.
///
/// By name and parent rather than by joining the two, because the listing has the name as a
/// borrowed `&str` out of its arena and the folder once for the whole listing — so this asks the
/// question without building a `PathBuf` per row per frame.
///
/// **Both halves have to hold of the same path.** Both views had a copy of this, and both asked
/// `any(name matches)` and `any(parent matches)` as two separate questions — so a cut file called
/// `a.txt` in one folder plus any cut file in *this* one dimmed a local `a.txt` that was never cut.
/// One copy here, asked the once, is the fix.
///
/// **The rows a drag picked up are not marked at all**, which is a decision and not an omission: a
/// row's style does not change for being in the air. The gesture is drawn under the pointer — see
/// [`drag_ghost`] and [`drag_saying`] — where the pointer is looking, and the listing is left as it
/// was. A mark on the row could not have been right anyway: two panes showing the same folder are
/// two listings of the same names with *separate* selections, and matching by name marked the rows
/// in both.
pub fn is_cut(cut: &[PathBuf], folder: &Path, name: &str) -> bool {
    cut.iter().any(|path| {
        path.parent() == Some(folder) && path.file_name().is_some_and(|leaf| leaf == name)
    })
}

/// How many icons the ghost draws, however many files the drag is carrying.
///
/// Three leans enough to read as a pile; a fourth is only more paper. The rest is the number in
/// the corner — see [`drag_ghost`].
pub const GHOST_STACK: usize = 3;

/// The files a drag is carrying, drawn under the pointer: their icons, stacked.
///
/// **This is the drag image, and this program draws it rather than the shell.** Windows will build
/// one — `IDragSourceHelper` off the data object's shell items, which is where Explorer's ghost
/// comes from — and that route was built here and taken back out: see the module header of
/// [`crate::shell::dnd`]. Drawing it costs one textured quad per icon out of the atlas the listing
/// already fills.
///
/// **Above the pointer and centred on it**, which is the one position that does not fight the
/// gesture: the row being aimed at is *under* the pointer, and a pile sitting on top of it hides
/// the thing you are trying to hit. The sentence goes below instead — carrying above, consequence
/// below, and the cursor between them. It is pushed back inside `bounds` at the top of the
/// window, where there is nothing above the pointer to sit in.
///
/// `icons` are back-to-front, so the last one drawn is the one on top and the stack leans down and
/// to the right the way a pile of paper does. The caller caps them at [`GHOST_STACK`] and never
/// hands over none — a drag with no icon to draw draws no ghost at all. `count` is how many files
/// there really are, which is the one thing a stack of three cannot say for itself.
///
/// Returns the rectangle the stack filled, so a test can find it.
pub fn drag_ghost(
    painter: &Painter,
    t: &Theme,
    at: egui::Pos2,
    bounds: Rect,
    icons: &[(egui::TextureId, Rect)],
    count: usize,
) -> Rect {
    /// How far each icon behind the top one peeks out, down and to the right.
    const LEAN: f32 = 6.0;
    /// The ghost's icons, a size up from a row's: it is the one thing the pointer is carrying, and
    /// at row size it reads as a row that came loose.
    const SIZE: f32 = 24.0;
    /// The air between the bottom of the pile and the pointer's hotspot. Small: the pile is what
    /// the pointer is *carrying*, and any further makes it a separate thing floating above.
    const GAP: f32 = 4.0;

    let size = vec2(SIZE, SIZE) + vec2(LEAN, LEAN) * icons.len().saturating_sub(1) as f32;
    let wanted = pos2(at.x - size.x * 0.5, at.y - GAP - size.y);
    let stack = Rect::from_min_size(
        pos2(
            wanted
                .x
                .min(bounds.right() - size.x - GUTTER)
                .max(bounds.left() + GUTTER),
            wanted.y.max(bounds.top() + GUTTER),
        )
        .round(),
        size,
    );
    for (index, (texture, uv)) in icons.iter().enumerate() {
        let corner = stack.min + vec2(LEAN, LEAN) * index as f32;
        painter.image(
            *texture,
            Rect::from_min_size(corner, vec2(SIZE, SIZE)),
            *uv,
            // Translucent, because it is a thing in the air over the window rather than in it —
            // and enough of it left to recognise the icon, which is the whole point of drawing
            // the files' own.
            Color32::from_white_alpha(210),
        );
    }
    if count > 1 {
        // How many, in a pill in the stack's far corner — the one furthest from the pointer, since
        // the stack sits above it. Wholly *inside* the stack rather than hung off the corner: the
        // corner nearest the cursor is where it used to be, and with the stack above the pointer
        // that put a badge on the hotspot. It is the one fact the icons cannot carry between them,
        // three of them standing for three files and for thirty.
        let font = FontId::new(typography::SIZE_CAPTION, egui::FontFamily::Proportional);
        let galley = painter.layout_no_wrap(count.to_string(), font, t.text.on_accent);
        let padding = vec2(space::S2, 1.0);
        let size = galley.size() + padding * 2.0;
        let pill = Rect::from_min_size(pos2(stack.right() - size.x, stack.top()), size);
        painter.rect_filled(pill, CornerRadius::same(radius::CIRCULAR), t.accent.default);
        text_left(painter, pill.shrink2(padding), galley);
    }
    stack
}

/// What a drop would do, in words, beside the pointer: *Copy one.txt into docs*.
///
/// The sentence is decided in [`crate::shell::dnd`] — see [`crate::shell::dnd::Told`] — and arrives
/// here in pieces, because **the two ends of it are drawn in the accent**: what is being carried
/// and where it would land. `accent-mark` and not `accent-default`, which is the rung meant to be
/// read *as ink on a surface* rather than to be a surface; the default one is a fill dark enough
/// to carry white text and reads as a smudge at caption size.
///
/// One galley and not three, laid out from a job with a section each. Two reasons and both matter:
/// the pieces are kerned and spaced as one line, and the frame can be measured before anything is
/// drawn, which is what lets it be pushed back inside the window below.
///
/// **Drawn against a position rather than through egui's tooltip**, and that is forced: while an
/// OLE drag is running the pointer belongs to the drag, so egui has no pointer to hang a tooltip
/// off — the position comes from the drop target's callbacks, which is the same place the highlight
/// comes from. The frame is `components::tooltip`'s: `background-layer-alt` inside `stroke-subtle`
/// at `radius-small`, `space-2` by `space-3` of padding, caption type. It is laid out here because
/// there is no `Response` to give the component.
///
/// **Every sentence wears a mark at its head** saying what the drop would do — see [`drag_sign`]
/// for which mark and why the sentence is where they live. A refusal's is the one in a colour of
/// its own, and the two names stay in the accent either way: the sentence is about the same pair of
/// things whether or not it can happen.
///
/// `at` is the top left it wants, which the caller has already put clear of the pointer and of the
/// ghost. It is then pushed back inside `bounds`, which against the window's bottom or right edge
/// means level with the pointer rather than clear of it: a sentence under the cursor is still
/// readable, and one drawn off the window is not there at all.
///
/// Returns the rectangle it used, so a test can find it.
pub fn drag_saying(
    painter: &Painter,
    t: &Theme,
    at: egui::Pos2,
    bounds: Rect,
    told: &crate::shell::dnd::Told,
) -> Rect {
    use azur_egui_theme::tokens::shadow;

    let font = FontId::new(typography::SIZE_CAPTION, egui::FontFamily::Proportional);
    let mut job = egui::text::LayoutJob::default();
    for (text, blue) in told.runs() {
        job.append(
            text,
            0.0,
            egui::TextFormat::simple(
                font.clone(),
                if blue { t.accent.mark } else { t.text.primary },
            ),
        );
    }
    let galley = painter.layout_job(job);
    let padding = vec2(space::S3, space::S2);
    // Room at the head of it for the mark, and the air between the mark and the first word.
    let sign = SIGN + space::S2;
    let size = galley.size() + vec2(sign, 0.0) + padding * 2.0;

    // `min` before `max`, so a window narrower than the sentence still shows its start rather than
    // its end.
    let at = pos2(
        at.x.min(bounds.right() - size.x - GUTTER)
            .max(bounds.left() + GUTTER),
        at.y.min(bounds.bottom() - size.y - GUTTER)
            .max(bounds.top() + GUTTER),
    );
    let rect = Rect::from_min_size(at.round(), size);

    let radius = CornerRadius::same(radius::SMALL);
    painter.add(shadow::S16.as_shape(rect, radius));
    painter.rect_filled(rect, radius, t.bg.layer_alt);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, t.stroke.subtle),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(padding);
    let (glyph, ink) = drag_sign(told, t);
    glyph(painter, icon_rect(inner, inner.left(), SIGN), ink);
    text_left(
        painter,
        Rect::from_min_max(pos2(inner.left() + sign, inner.top()), inner.max),
        galley,
    );
    rect
}

/// The mark's box in [`drag_saying`]: a caption line's own height, so the sign sits level with the
/// words rather than standing over them.
const SIGN: f32 = typography::LINE_CAPTION;

/// The mark at the head of [`drag_saying`]'s sentence, and the ink to draw it in.
///
/// **What the drop would do, as a sign as well as in words**: a `+` for a copy, a forward arrow for
/// a move, the bookmark star for a pin, an opening folder for a tab, and `icons::error` — an ✕ in a
/// circle — for a drop that will not happen. The same pairing the platform puts on the cursor, moved
/// to the head of the sentence, and that is the point of it: OLE's badge rides *under* the pointer
/// while the words sit beside it, so the two are never read together. Here the sign and the sentence
/// it belongs to are one thing.
///
/// **The refusal is the one with a colour of its own.** `status-danger` against the accent the
/// others wear, because it is the one that has to be seen before it is read; the rest are
/// saying the same thing as the words next to them, in the same ink as the two names those words
/// pick out.
///
/// The star and not a link arrow for a pin, though the effect reported to the pointer is
/// `DROPEFFECT_LINK`: a star is what the path bar's bookmark button and every pinned row in the
/// sidebar already are, and the sentence beside it says *Pin*. A shortcut arrow would be naming the
/// mechanism instead of the gesture.
///
/// And an opening folder for a tab, for the same reason and not the `+` the strip's own new-tab
/// button wears: a plus at the head of this sentence has already been spent on a copy, and the drop
/// that opens a tab is the one gesture here that changes nothing on disk. The effect it reports is
/// `DROPEFFECT_LINK` as well — see `crate::shell::dnd::Onto::Tabs`.
fn drag_sign(
    told: &crate::shell::dnd::Told,
    t: &Theme,
) -> (fn(&Painter, Rect, Color32), Color32) {
    use crate::shell::dnd::Doing;

    if told.refused.is_some() {
        return (azur_egui_theme::icons::error, t.status.danger);
    }
    let glyph: fn(&Painter, Rect, Color32) = match told.doing {
        Doing::Copy => crate::icons::plus,
        Doing::Move => crate::icons::arrow_right,
        // The badge Windows puts on every shortcut, which is what the drop is about to make.
        Doing::Link => crate::icons::link,
        Doing::Pin => crate::icons::star,
        Doing::Open => crate::icons::folder_open,
    };
    (glyph, t.accent.mark)
}

/// The grey a drop mark is filled with, over the opaque base it is laid on.
///
/// `text-secondary`, translucent: a mid grey in *both* themes — light over the dark theme's
/// surfaces, dark over the light theme's — so a mark reads the same way round either way, exactly
/// as the accent wash does.
///
/// **On a surface rather than on the listing**, which is the correction. A translucent tint alone
/// was faint enough that the rows underneath read straight through the mark and crossed the dashed
/// diagram inside it with `File folder`, twice. A mark with a drawing in it has to be a surface, so
/// [`drop_hint`] lays `background-layer-alt` — what a menu or a tooltip is made of, this window's
/// answer to "a small thing floating over the content" — and puts this on top of it to lift it off
/// the pane it is over.
///
/// Public so a test can name the colour it is looking for without knowing the figure.
pub fn hint_wash(t: &Theme) -> Color32 {
    let grey = t.text.secondary;
    Color32::from_rgba_unmultiplied(grey.r(), grey.g(), grey.b(), 56)
}

/// The accent wash inside a drop mark: the part of it a drop would take, filled.
///
/// `accent-default` — the accent as a *surface*, which is what a filled region is — translucent
/// over the mark's grey. Stronger than [`drop_preview`]'s wash, because it is laid on that grey
/// rather than on a listing: the same figure that tints a whole pane went to almost nothing on a
/// mid grey plate. Still a wash and not a solid, so the dashed edge over it stays an edge in the
/// light theme, where the accent's ink and surface rungs are one value.
///
/// Public so a test can name the colour without knowing the figure.
pub fn hint_fill(t: &Theme) -> Color32 {
    let accent = t.accent.default;
    Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 110)
}

/// The part of a drop mark that dropping there would take: **the whole square, or exactly half of
/// it.**
///
/// The mark is the pane in miniature, so the diagram inside it has to be the miniature of the
/// result — and a half is a half. Anything short of the mark's own edges reads as some *other*
/// fraction and stops being a diagram of "the right-hand side of this pane".
///
/// Which is why this is not [`crate::dock::preview_rect`] of the mark, tempting as the reuse was:
/// that one takes the seam between two panes out of the middle, which is right at full size and a
/// point of missing edge at this one.
pub fn hint_part(mark: Rect, zone: crate::dock::Zone) -> Rect {
    use crate::pane::Side;
    let middle = mark.center();
    match zone {
        crate::dock::Zone::Into => mark,
        crate::dock::Zone::Split(Side::Left) => {
            Rect::from_min_max(mark.min, pos2(middle.x, mark.max.y))
        }
        crate::dock::Zone::Split(Side::Right) => {
            Rect::from_min_max(pos2(middle.x, mark.min.y), mark.max)
        }
        crate::dock::Zone::Split(Side::Top) => {
            Rect::from_min_max(mark.min, pos2(mark.max.x, middle.y))
        }
        crate::dock::Zone::Split(Side::Bottom) => {
            Rect::from_min_max(pos2(mark.min.x, middle.y), mark.max)
        }
    }
}

/// **One place the dragged tab *could* go**: the quiet twin of [`drop_preview`].
///
/// Drawn once per zone of every pane that would take the tab — five equal squares in a plus, see
/// `dock::hint_rect` — so the arrangement on offer is on screen instead of being something to find
/// by sweeping the pointer around the window and watching for the blue. The docking gesture was
/// the one thing in this program you had to be told about.
///
/// **Grey, because the blue already means something else.** [`drop_preview`]'s accent says
/// "release now and it lands *here*" — one rect at a time, and the only one the pointer is in.
/// If possibility wore the same colour there would be five of them and no way to tell which was
/// the answer. Two colours, two facts.
///
/// The wash is [`hint_wash`]. One point of outline against the preview's two, because five of these
/// are on screen per pane and this is the quieter mark of the pair — but `stroke-strong` rather than
/// `stroke-control`, which is a rung that disappeared over a listing: the outline is what gives each
/// square its edge.
///
/// Square, per `azur::desktop::structural_radius`, and for the same reason the preview is: it
/// shows where a rectangle would go, and a rounded corner only blurs where the edge is.
///
/// **Inside it, what dropping there does**: a dashed rectangle over [`hint_part`] — the whole
/// square for the middle mark, exactly half of it for each of the four around it. Five identical
/// squares are five identical squares, and their arrangement is the only thing saying which is
/// which; the diagram is what makes each one legible on its own, before the pointer has been
/// anywhere near it. The mark is the pane, and the dashes are the half of it this tab would take.
///
/// **Filled and outlined, which makes each mark a miniature of [`drop_preview`]** — the same accent
/// wash inside the same accent edge, so what a mark promises and what the pane it is in shows a
/// moment later are visibly the same mark at two sizes. The dashes are the whole of the difference
/// between them: dashed is *would*, solid is *will*.
///
/// The fill is translucent rather than solid, and that is what keeps the light theme working: there
/// `accent-mark` and `accent-default` are one value — see `azur::theme::Accents::mark` — so a
/// dashed edge over a solid fill of the accent would be a dashed line drawn in the colour it is
/// drawn on. Over a wash it still reads as an edge in both themes.
///
/// The edge is `accent-mark` and not `accent-default`, because it is a line on a surface rather
/// than a surface: the default rung is a fill dark enough to carry white text, which as a hairline
/// on a dark listing is barely there. Two points of it, which is [`drop_preview`]'s width — the
/// accent saying "this part of this pane" is one statement whether it is drawn across a whole pane
/// or inside an eighty-point mark, so it is drawn at one weight. At a single point the dashes read
/// as a dotted guide laid on the mark rather than as the thing the mark is about.
pub fn drop_hint(painter: &Painter, mark: Rect, zone: crate::dock::Zone, t: &Theme) {
    let radius = azur_egui_theme::desktop::structural_radius();
    // Opaque first, then the grey on top of it: see [`hint_wash`] for why a mark with a drawing
    // in it cannot be a tint on the listing.
    painter.rect_filled(mark, radius, t.bg.layer_alt);
    painter.rect_filled(mark, radius, hint_wash(t));
    painter.rect_stroke(
        mark,
        radius,
        Stroke::new(1.0, t.stroke.strong),
        egui::StrokeKind::Inside,
    );

    let part = hint_part(mark, zone);
    painter.rect_filled(part, radius, hint_fill(t));
    // Long enough to read as a dash rather than as a dotted line, off the mark so it does not
    // become a solid line on a small pane's smaller marks.
    let dash = (mark.width().min(mark.height()) * 0.11).clamp(2.0, 7.0);
    // Closed, and starting at each corner: `dashed_line` lays its first dash from the start of
    // every run, so going round the four sides in turn puts ink in all four corners.
    let outline = [
        part.left_top(),
        part.right_top(),
        part.right_bottom(),
        part.left_bottom(),
        part.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(
        &outline,
        Stroke::new(2.0, t.accent.mark),
        dash,
        dash * 0.7,
    ));
}

/// Centre a single line in a rect — for the empty and error states.
pub fn text_center(painter: &Painter, rect: Rect, font: FontId, color: Color32, text: &str) {
    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use azur_egui_theme::desktop;

    /// **The ghost under the pointer draws one icon per file it is carrying, and says how many.**
    ///
    /// The drag image, which this program draws rather than the shell — see [`drag_ghost`], and
    /// [`crate::shell::dnd`]'s module header for why the shell's own route was taken back out.
    ///
    /// Driven with stand-in textures rather than through the application, deliberately: the icons
    /// come out of a shell atlas filled by a worker thread, so an end-to-end test would be
    /// asserting on when a bitmap happened to arrive. What is worth pinning is the arithmetic —
    /// one quad per icon, the stack leaning down and to the right, and the count drawn only when
    /// there is more than one file.
    #[test]
    fn the_drag_ghost_draws_one_icon_each_and_counts_the_rest() {
        let ctx = egui::Context::default();
        azur_egui_theme::fonts::install(&ctx);
        let t = Theme::dark();
        // Not `TextureId::default()`, which is the font atlas: these have to look like image quads.
        let icons: Vec<(egui::TextureId, Rect)> = (0..GHOST_STACK)
            .map(|n| {
                (
                    egui::TextureId::Managed(1 + n as u64),
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                )
            })
            .collect();

        type Drawn = (usize, Vec<String>, Rect);
        let at = pos2(100.0, 100.0);
        let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let drawn = |count: usize, icons: &[(egui::TextureId, Rect)]| -> Drawn {
            let mut quads = 0;
            let mut texts = Vec::new();
            let mut rect = Rect::NOTHING;
            let _ = ctx.run_ui(Default::default(), |ui| {
                let from = ui.painter().add(egui::Shape::Noop);
                rect = drag_ghost(ui.painter(), &t, at, bounds, icons, count);
                let layer = ui.layer_id();
                ui.ctx().graphics_mut(|g| {
                    for clipped in g.entry(layer).all_entries().skip(from.0) {
                        match &clipped.shape {
                            egui::Shape::Mesh(mesh)
                                if mesh.texture_id != egui::TextureId::default() =>
                            {
                                quads += 1;
                            }
                            egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
                            _ => {}
                        }
                    }
                });
            });
            (quads, texts, rect)
        };

        // Four files, three icons, and the number in the corner saying so.
        let (quads, texts, ghost) = drawn(4, &icons);
        assert_eq!(quads, GHOST_STACK, "one quad per icon in the stack");
        // Above the pointer and over it, so the row being aimed at stays visible.
        assert!(
            ghost.bottom() < at.y,
            "the pile is under the pointer at {at:?} rather than above it: {ghost:?}"
        );
        assert!(
            ghost.left() < at.x && ghost.right() > at.x,
            "the pile is not over the pointer: {ghost:?}"
        );
        assert!(
            texts.contains(&"4".to_owned()),
            "the ghost has to say how many files it is carrying, drew {texts:?}"
        );

        // One file: one icon, and no count — `1` beside a single icon says nothing.
        let (quads, texts, one) = drawn(1, &icons[..1]);
        assert_eq!(quads, 1);
        assert!(texts.is_empty(), "a single file was counted: {texts:?}");
        assert!(
            one.width() < ghost.width(),
            "one icon is not smaller than three: {one:?} against {ghost:?}"
        );
    }

    /// **The two names in the drag's sentence are laid out in the accent, and the words are not.**
    ///
    /// [`crate::shell::dnd::Told::runs`] says which pieces are the blue ones; this is the other
    /// half of that claim — that the colour reaches the galley. Asserted on the *sections of one
    /// galley* rather than on three separate texts, because being one galley is itself the point:
    /// the pieces have to be kerned and spaced as a single line and measured as one box.
    #[test]
    fn the_drag_sentence_puts_its_two_names_in_the_accent() {
        use crate::shell::dnd::{Doing, Told};

        let ctx = egui::Context::default();
        azur_egui_theme::fonts::install(&ctx);
        let t = Theme::dark();
        let told = Told {
            doing: Doing::Copy,
            refused: None,
            source: Some("one.txt".to_owned()),
            target: "docs".to_owned(),
        };

        let mut sections: Vec<(String, Color32)> = Vec::new();
        let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let _ = ctx.run_ui(Default::default(), |ui| {
            let from = ui.painter().add(egui::Shape::Noop);
            drag_saying(ui.painter(), &t, pos2(100.0, 100.0), bounds, &told);
            let layer = ui.layer_id();
            ui.ctx().graphics_mut(|g| {
                for clipped in g.entry(layer).all_entries().skip(from.0) {
                    if let egui::Shape::Text(text) = &clipped.shape {
                        let whole = text.galley.text();
                        for section in &text.galley.job.sections {
                            let range = section.byte_range.start.0..section.byte_range.end.0;
                            sections.push((whole[range].to_owned(), section.format.color));
                        }
                    }
                }
            });
        });

        assert_eq!(
            sections
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<Vec<_>>(),
            ["Copy ", "one.txt", " into ", "docs"],
            "the sentence was not laid out in its pieces: {sections:?}"
        );
        for (text, colour) in &sections {
            let wanted = if text == "one.txt" || text == "docs" {
                t.accent.mark
            } else {
                t.text.primary
            };
            assert_eq!(
                *colour, wanted,
                "`{text}` is the wrong colour for its part of the sentence"
            );
        }
    }

    /// **Every sentence wears a mark, and a refused one wears the refusal's.**
    ///
    /// The mark is what makes the drop something you *see* rather than something you read — see
    /// [`drag_sign`]. Both halves are asserted because both break quietly: a refusal drawn in the
    /// accent reads as an ordinary drop, and a copy drawn in `status-danger` reads as a refused one.
    ///
    /// The ink is the claim rather than the shape. What each glyph looks like is `icons`' business and
    /// a test that counted its segments would be pinning down a drawing; what matters here is that
    /// the frame makes room for one, that it is drawn inside that room, and that the refusal is the
    /// only one wearing the warning colour.
    #[test]
    fn every_sentence_wears_a_mark_and_a_refused_one_wears_the_refusal_s() {
        use crate::shell::dnd::{Doing, Refused, Told};

        let ctx = egui::Context::default();
        azur_egui_theme::fonts::install(&ctx);
        let t = Theme::dark();
        let told = |doing: Doing, refused: Option<Refused>| Told {
            doing,
            refused,
            source: Some("src".to_owned()),
            target: "main".to_owned(),
        };

        /// Where the ink of one colour ended up, and the words beside it.
        struct Drawn {
            marks: Vec<Color32>,
            ink: Rect,
            words: String,
            /// How far into the frame the first word starts, which is the room the mark was given.
            indent: f32,
            rect: Rect,
        }
        let drawn = |told: &Told| -> Drawn {
            let mut marks = Vec::new();
            let mut ink = Rect::NOTHING;
            let mut words = String::new();
            let mut text_x = 0.0;
            let mut rect = Rect::NOTHING;
            let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
            let _ = ctx.run_ui(Default::default(), |ui| {
                let from = ui.painter().add(egui::Shape::Noop);
                rect = drag_saying(ui.painter(), &t, pos2(100.0, 100.0), bounds, told);
                let layer = ui.layer_id();
                ui.ctx().graphics_mut(|g| {
                    // Every glyph in `icons` is strokes, circles and filled polygons in the one
                    // colour it was handed — so the mark is whatever is drawn in something other
                    // than the frame's own greys, wherever the glyph puts it.
                    for clipped in g.entry(layer).all_entries().skip(from.0) {
                        let (colour, box_) = match &clipped.shape {
                            egui::Shape::LineSegment { stroke, points } => (
                                stroke.color,
                                Rect::from_two_pos(points[0], points[1]),
                            ),
                            egui::Shape::Circle(circle) => {
                                (circle.stroke.color, clipped.shape.visual_bounding_rect())
                            }
                            // A filled glyph carries its colour in the fill and an outlined one in
                            // the stroke — the star is drawn both ways, so both are read.
                            egui::Shape::Path(path) => {
                                let ink = match (path.fill, &path.stroke.color) {
                                    (fill, _) if fill.a() > 0 => fill,
                                    (_, egui::epaint::ColorMode::Solid(colour)) => *colour,
                                    _ => Color32::TRANSPARENT,
                                };
                                (ink, clipped.shape.visual_bounding_rect())
                            }
                            egui::Shape::Text(text) => {
                                words = text.galley.text().to_owned();
                                text_x = text.pos.x;
                                continue;
                            }
                            _ => continue,
                        };
                        // A glyph drawn as a stroked outline leaves a `Path` with no fill; its ink
                        // is in the segments beside it, so a transparent colour is not a mark.
                        if colour.a() > 0 && colour != t.stroke.subtle && colour != t.bg.layer_alt {
                            marks.push(colour);
                            ink = ink.union(box_);
                        }
                    }
                });
            });
            Drawn {
                marks,
                ink,
                words,
                indent: text_x - rect.left(),
                rect,
            }
        };

        // ---- A move: the arrow, in the accent, inside the room made for it ----
        let moving = drawn(&told(Doing::Move, None));
        assert_eq!(moving.words, "Move src into main");
        assert!(
            !moving.marks.is_empty(),
            "an allowed drop is not marked at all"
        );
        for mark in &moving.marks {
            assert_eq!(*mark, t.accent.mark, "the mark is not the accent's");
        }
        // At the head of the sentence: inside the frame, and to the left of the words.
        assert!(
            moving.rect.contains_rect(moving.ink),
            "the mark is outside the frame: {:?} in {:?}",
            moving.ink,
            moving.rect
        );
        assert!(
            moving.ink.right() <= moving.rect.left() + SIGN + space::S3 + space::S2,
            "the mark is not at the head of the sentence: {:?} in {:?}",
            moving.ink,
            moving.rect
        );

        // ---- Copy and pin: their own signs, the same colour and the same box ----
        for doing in [Doing::Copy, Doing::Pin] {
            let other = drawn(&told(doing, None));
            assert!(!other.marks.is_empty(), "{doing:?} is not marked");
            for mark in &other.marks {
                assert_eq!(*mark, t.accent.mark, "{doing:?}'s mark is not the accent's");
            }
            // The words start at the same place whichever sign is in front of them: the room is the
            // mark's box and not the glyph's own ink, so three different shapes are one column.
            assert_eq!(
                other.indent, moving.indent,
                "{doing:?} laid its sentence out around a different sized mark"
            );
        }

        // ---- And a refusal: the warning colour, and the words that say so ----
        let refused = drawn(&told(Doing::Move, Some(Refused::Inside)));
        assert_eq!(refused.words, "Cannot move src into main, which is inside it");
        assert!(
            refused.marks.iter().all(|mark| *mark == t.status.danger),
            "a refused drop is marked in {:?} rather than in the warning colour",
            refused.marks
        );
        assert!(
            !refused.marks.is_empty(),
            "a refused drop is not marked at all"
        );
        // Room for the mark on the line rather than above it, whichever mark it is.
        assert_eq!(refused.rect.height(), moving.rect.height());
    }

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
            ("^src", "match at the start"),
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
    /// **The filter box has an edge, in every palette.**
    ///
    /// Its fill is the bar it sits in — see [`bar`] — so the outline is the whole of what makes it
    /// a box, and an outline that has drifted onto its own fill is a field you cannot see. That is
    /// not hypothetical: pointing the border straight at [`seam`] does it wherever the bar and
    /// the seam are one value, which is the dark palette — and the box did disappear there before
    /// this measured it. [`field_outline`] is the rule; this is the floor under it.
    ///
    /// Held to `SAME` rather than to a WCAG figure, because the question is whether two surfaces
    /// read as two — the case `azur::contrast`'s own header is about.
    #[test]
    fn the_filter_box_has_an_edge_in_every_palette() {
        use azur_egui_theme::contrast::{apart, SAME};

        for t in crate::theme::Theme::all() {
            let name = t.palette.key();
            let got = apart(field_outline(&t), bar(&t));
            assert!(
                got >= SAME,
                "{name}: the filter box's outline is {got:.1} ΔL* from the bar it is drawn on, so \
                 the box has no visible edge"
            );
            // And where the palette gives the bar a colour of its own, the outline really is the
            // seam — the whole point of the rule, and the half a `>=` cannot say.
            if apart(seam(&t), bar(&t)) >= SAME {
                assert_eq!(
                    field_outline(&t),
                    seam(&t),
                    "{name}: the bar reads off the seam, so the outline should be the seam"
                );
            } else {
                assert_eq!(
                    field_outline(&t),
                    t.stroke.control,
                    "{name}: the bar *is* the seam here, so the outline has to stay Azur's"
                );
            }
        }
    }

    /// What the values *are*, and that they step away from every surface in both themes,
    /// is `azur_egui_theme::desktop`'s to hold — it is the one that knows why.
    #[test]
    fn the_window_wears_the_desktop_preset() {
        for t in Theme::all() {
            let name = t.palette.key();
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
