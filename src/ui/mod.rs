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
use azur_egui_theme::tokens::{radius, space};
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
