//! The title bar: the tab strips, the window buttons, and the drag that docks a
//! tab into a split.
//!
//! The window asks the platform not to draw its own caption
//! (`with_decorations(false)`), so everything a title bar does is done here —
//! including the resize borders, which come free with a decorated window and have
//! to be built by hand without one.
//!
//! # Where the tabs are
//!
//! **Above the pane they belong to.** A strip is laid out at its pane's own x-range rather
//! than packed along the bar, so a tab is directly over the listing it governs and there is
//! nothing to work out about which is which.
//!
//! For the top row of panes that place is the title bar, which is the strip of window
//! immediately above them. A pane in any row below has nothing above it but another pane,
//! so its row gets a band of its own — painted like the title bar, because for that row
//! that is exactly what it is — and the band's height comes off the top of the panes under
//! it. [`plan_strips`] decides all of this before anything is drawn, which is why the panes
//! are laid out before the bar rather than after.
//!
//! [`tab_strip`] draws a strip at any rect and every strip goes through it, so a tab
//! behaves identically wherever it ends up.
//!
//! # Why the whole drag gesture lives in this file
//!
//! [`resolve_drag`] runs after the panes have been drawn, so the rects it tests the
//! pointer against are this frame's. It paints its preview into a foreground layer,
//! which is ordered above the panes regardless of when it was added to. One place owns
//! the gesture, and nothing has to be threaded through the rest of the frame.

use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::{radius, row, space};
use egui::{
    pos2, vec2, Color32, CornerRadius, Id, LayerId, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
};

use crate::app::{Action, WindowAction};
use crate::dock::{self, Zone};
use crate::icons;
use crate::pane::{Pane, PaneId};
use crate::theme::Theme;
use crate::ui::{icon_rect, truncated, TOOL_ICON, TOOL_SIZE};

/// `tokens::row::TITLE_BAR`.
pub const HEIGHT: f32 = row::TITLE_BAR;

/// How far down the top edge belongs to the window-resize band rather than to the
/// title bar.
///
/// Everything in the title bar takes its input from below this line, so the two
/// never compete: Windows' own top edge is four pixels, and it is reliably hittable.
pub const TOP_BAND: f32 = 4.0;

/// How far along each edge a corner claims.
const CORNER: f32 = 16.0;

/// How far in from each edge the window-resize bands reach.
///
/// This used to be [`crate::ui::GUTTER`], and the bands' whole claim to never stealing a click
/// was that the panels stopped short of it. They no longer do — the panels reach the window's
/// edges — so these four points are taken *out of* the panels, which is what every window with
/// chrome of its own does on this platform: the frame is inside the client area or there is no
/// frame to grab.
///
/// What that costs, measured rather than assumed: the listing's scrollbar is `10` points wide at
/// the right edge, so `6` of it stays grabbable, which
/// `the_scrollbar_survives_the_window_resize_band` holds. The other three edges give up a row's
/// left padding, the status line's bottom, and nothing respectively. A maximised window skips
/// the bands entirely, because there is nothing to resize.
const RESIZE_BAND: f32 = space::S2;

/// How much of a strip a tab leaves above itself, and the only air around one.
///
/// The same four points the window's top resize edge claims, so a tab in the title bar starts
/// below that edge rather than racing it for the pointer — and a tab in a band of its own keeps
/// the same shoulder, so it looks the same wherever it is.
const TAB_TOP: f32 = TOP_BAND;
/// A tab's height in a band of its own. In the title bar it takes the bar's height instead;
/// either way it reaches the bottom edge, which is what makes it read as a browser tab rather
/// than as a chip floating in a strip.
const TAB_HEIGHT: f32 = TOOL_SIZE;
/// Only the top corners, and only just.
///
/// A browser tab is a shape with a bottom edge welded to the content below it, so rounding that
/// edge would be rounding a join that is not there.
const TAB_CORNER: CornerRadius = CornerRadius {
    nw: radius::SMALL,
    ne: radius::SMALL,
    sw: 0,
    se: 0,
};
const TAB_MIN: f32 = 52.0;
const TAB_MAX: f32 = 220.0;
/// Two points off the top of a tab's name, and of every other name on this bar.
///
/// The third time this program has needed the same two points — see
/// [`crate::ui::filelist::CELL_LIFT`] and the breadcrumb's `TEXT_LIFT`, which say it at length.
/// A line of text centred in a box is centred on its *line box*, which reserves room under the
/// baseline for descenders, so a name beside a 14px glyph centred on its own ink sits low. Every
/// name in a tab is x-height and ascenders; the room being reserved is for descenders most of
/// them do not have.
const TEXT_LIFT: f32 = 2.0;
/// Everything in a tab that is not its label: two 14px glyphs, the gaps around
/// them, and the padding at each end.
const TAB_FURNITURE: f32 = 8.0 + TOOL_ICON + space::S2 + space::S2 + TOOL_ICON + space::S2;

/// A tab being dragged.
pub struct TabDrag {
    pub pane: PaneId,
    pub tab: usize,
    /// Where in the tab the pointer took hold, so the ghost does not jump.
    pub grab_dx: f32,
    pub title: String,
    /// Whether the pointer has moved far enough for this to be a drag rather than
    /// a click that has not finished yet.
    pub live: bool,
}

/// Where a dragged tab would land.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Drop {
    /// Into a pane's own strip, at this position.
    Strip { pane: PaneId, index: usize },
    /// Onto a pane, splitting it.
    Split { pane: PaneId, side: crate::pane::Side },
    /// Onto a pane's centre: move the tab there.
    Into { pane: PaneId },
    Nowhere,
}

/// One tab's place on screen, resolved for this frame.
pub struct Slot {
    pub pane: PaneId,
    pub tab: usize,
    pub rect: Rect,
}

/// A caption button's width. The platform's are 46×32, and matching that is what makes
/// this window read as a window.
const CAPTION_WIDTH: f32 = 46.0;

/// The title bar's own rect, given the window.
pub fn bar_rect(screen: Rect) -> Rect {
    Rect::from_min_size(screen.min, vec2(screen.width(), HEIGHT))
}

/// Where the title bar's content begins: past the application mark.
pub fn content_left(bar: Rect) -> f32 {
    bar.left() + space::S2 + TOOL_SIZE + space::S2
}

/// Where it has to stop: the left edge of the caption buttons.
pub fn controls_left(bar: Rect) -> f32 {
    bar.right() - CAPTION_WIDTH * 3.0
}

/// The room a row of tab strips needs when it is not in the title bar.
pub const STRIP_ROW: f32 = TAB_HEIGHT + space::S2 * 2.0;

/// Below this a strip is not worth putting in the title bar: one tab shrunk to its
/// glyphs, plus the `+`.
const MIN_STRIP: f32 = TAB_MIN + TOOL_SIZE + space::S2 * 2.0;

/// One row of panes' tab strips, drawn on a band of its own.
pub struct StripRow {
    /// The band to fill. Painted like the title bar, because that is what it is.
    pub band: Rect,
    /// Where each pane's strip goes inside it.
    pub strips: Vec<(PaneId, Rect)>,
}

/// Where every pane's tab strip goes.
pub struct StripPlan {
    /// Strips that fit in the title bar — the top row of panes.
    pub in_bar: Vec<(PaneId, Rect)>,
    /// A band per row of panes that could not use the title bar: every row below the
    /// first, and the first as well when the caption buttons leave it too little room.
    pub rows: Vec<StripRow>,
}

/// Decide where each pane's tabs go, and take the room they need out of the panes.
///
/// A tab belongs directly above the pane it governs, so a strip is laid out at that pane's
/// own x-range rather than packed from the left. The top row of panes can use the title bar
/// for that — it is the strip of window directly above them. A pane in any row below has
/// nothing above it but another pane, so its row gets a band of its own, painted like the
/// title bar and taking [`STRIP_ROW`] off the top of the panes in that row.
///
/// `panes` is adjusted in place, which is why this runs before anything is drawn.
///
/// The first row falls back to a band too when the caption buttons would leave one of its
/// strips under [`MIN_STRIP`] — all of it or none of it, because a window with one pane's
/// tabs in the bar and another's in a band below reads as a bug rather than as a rule.
pub fn plan_strips(panes: &mut [(PaneId, Rect)], bar: Rect) -> StripPlan {
    let mut plan = StripPlan {
        in_bar: Vec::new(),
        rows: Vec::new(),
    };
    if panes.is_empty() {
        return plan;
    }

    // Rows, by top edge. Panes in one row share a top because that is what a row *is*
    // in a tree of splits; rounding absorbs the half-point a ratio can land on.
    let mut tops: Vec<i32> = panes.iter().map(|(_, r)| r.top().round() as i32).collect();
    tops.sort_unstable();
    tops.dedup();

    let row_of = |top: i32, panes: &[(PaneId, Rect)]| -> Vec<(PaneId, Rect)> {
        let mut row: Vec<(PaneId, Rect)> = panes
            .iter()
            .filter(|(_, r)| r.top().round() as i32 == top)
            .copied()
            .collect();
        row.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
        row
    };

    // ---- The top row, if the title bar can hold it ----------------------
    let (left, right) = (content_left(bar), controls_left(bar) - space::S3);
    let first = row_of(tops[0], panes);
    let candidates: Vec<(PaneId, Rect)> = first
        .iter()
        .map(|(id, rect)| {
            let strip = Rect::from_min_max(
                pos2(rect.left().max(left), bar.top()),
                pos2(rect.right().min(right), bar.bottom()),
            );
            (*id, strip)
        })
        .collect();
    let fits = right > left
        && candidates
            .iter()
            .all(|(_, strip)| strip.width() >= MIN_STRIP.min(right - left));

    let banded: &[i32] = if fits {
        plan.in_bar = candidates;
        &tops[1..]
    } else {
        &tops[..]
    };

    // ---- A band for every other row -------------------------------------
    //
    // **Membership is decided against the tops the panes came in with**, not against `panes`, which
    // this loop is in the middle of moving. Taking a band's height off a pane (below) pushes its top
    // down by `STRIP_ROW`; ask `panes` again afterwards and that pane answers to the *next* row down
    // whenever the two rows are `STRIP_ROW` apart. It then gets a strip in both, so its tabs are laid
    // out twice under one `Id::new(("tab", pane, index))` — egui's duplicate-id warning, in red, over
    // two stacked copies of the same tab bar.
    //
    // Which is reachable by dragging: with the window split in four and the left divider moving, the
    // left column's bottom top sweeps continuously past the right column's, and one of the pixels it
    // crosses is exactly `STRIP_ROW` below the row above. A glitch that appears for one frame of a
    // drag and leaves nothing behind.
    let arrived: Vec<(PaneId, Rect)> = panes.to_vec();
    for &top in banded {
        let row = row_of(top, &arrived);
        let (from, to) = row.iter().fold((f32::MAX, f32::MIN), |(from, to), (_, r)| {
            (from.min(r.left()), to.max(r.right()))
        });
        plan.rows.push(StripRow {
            band: Rect::from_min_max(pos2(from, top as f32), pos2(to, top as f32 + STRIP_ROW)),
            strips: row
                .iter()
                .map(|(id, rect)| {
                    (
                        *id,
                        Rect::from_min_max(
                            pos2(rect.left(), top as f32),
                            pos2(rect.right(), top as f32 + STRIP_ROW),
                        ),
                    )
                })
                .collect(),
        });
        // The band's room comes out of the panes under it. A pane squeezed shorter than
        // its own strip keeps the strip — losing the tabs would lose the way out.
        for (id, rect) in panes.iter_mut() {
            if row.iter().any(|(row_id, _)| row_id == id) {
                rect.min.y = (rect.top() + STRIP_ROW).min(rect.bottom());
            }
        }
    }

    plan
}

/// Draw the title bar. Everything it resolves goes into `out`.
///
/// Returns where every tab in the window ended up, which the drag resolution needs.
#[allow(clippy::too_many_arguments)]
pub fn title_bar(
    ui: &mut Ui,
    t: &Theme,
    panes: &[Pane],
    strips: &[(PaneId, Rect)],
    focused: PaneId,
    maximized: bool,
    // Whether the panel down the left is showing, for the entry in the menu under the mark.
    sidebar: bool,
    // Whether `Win+E` opens this build, for the tick beside it in that same menu. A copy of what
    // the registry said — see `crate::shell::winkey`, which is where the answer lives.
    win_key: bool,
    drag: &Option<TabDrag>,
    icons_cache: &mut crate::shell::icons::Icons,
    out: &mut Vec<Action>,
) -> Vec<Slot> {
    let bar = ui
        .allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover())
        .0;
    // The title bar is a *region*, so it names one — `surfaces.titlebar`, which is
    // `background-layer` in the dark palette (`#14171A`, the design system's
    // `tokens::palette::GRAY_2`) and a blue of its own in the light one.
    // Named by its region rather than by the role so that a palette which wants the strip a step
    // off the panels can say so; see `crate::theme::Surfaces`.
    ui.painter()
        .rect_filled(bar, CornerRadius::ZERO, t.surfaces.titlebar);

    // ---- Dragging and maximising, over the whole bar --------------------
    //
    // Registered *first*, so everything below claims its own pixels back: within a layer
    // egui gives a click to the last widget that asked for it, which means the mark, the
    // tabs, the `+` and the caption buttons all win over this without any of them having
    // to be subtracted from it. What is left over — and it is the whole bar minus those —
    // drags the window and maximises on a double click, which is what a title bar does.
    //
    // Below `TOP_BAND`, so the top resize edge and this are different pixels rather than a
    // race between two gestures.
    let caption = Rect::from_min_max(pos2(bar.left(), bar.top() + TOP_BAND), bar.max);
    let caption = ui.interact(caption, Id::new("caption-drag"), Sense::click_and_drag());
    if caption.double_clicked() {
        out.push(Action::Window(WindowAction::ToggleMaximize));
    } else if caption.drag_started() {
        out.push(Action::Window(WindowAction::Drag));
    }

    // ---- The application mark, which is also the only global menu --------
    let mut x = bar.left() + space::S2;
    let mark = Rect::from_min_size(
        pos2(x, (bar.center().y - TOOL_SIZE * 0.5).round()),
        vec2(TOOL_SIZE, TOOL_SIZE),
    );
    let mark_response = ui.interact(mark, Id::new("app-menu"), Sense::click());
    if mark_response.hovered() {
        ui.painter()
            .rect_filled(mark, CornerRadius::same(radius::SMALL), t.bg.control_hover);
    }
    // The one icon in the window that takes no colour from the theme. Everywhere else an icon
    // is a glyph in the colour of the text beside it; this is the brand, it is a full-colour
    // image, and `brand::mark` picks the rung of its size ladder that matches the monitor.
    crate::brand::mark(
        ui.painter(),
        Rect::from_center_size(mark.center(), vec2(16.0, 16.0)),
    );
    app_menu(ui, &mark_response, t.palette, sidebar, win_key, out);
    x = mark.right() + space::S2;

    // ---- Window buttons, from the right ---------------------------------
    let controls_left = window_buttons(ui, t, bar, maximized, out);

    // ---- The bar's bottom edge, before the tabs -------------------------
    //
    // Before, so a tab with a surface of its own paints over it. That is the whole of what
    // "a tab is welded to the content below it" means now that the panes reach the bar: the
    // active tab and the path bar beneath it are the same colour, and a `stroke-subtle`
    // hairline drawn across them afterwards cut the weld in half — which is precisely what it
    // used to do, invisibly, because there were four points of canvas under it anyway.
    //
    // A *quiet* tab has no fill and still shows the line through it, which is right: a tab
    // that is not the active one is not welded to anything.
    ui.painter().line_segment(
        [bar.left_bottom(), bar.right_bottom()],
        Stroke::new(1.0, t.stroke.subtle),
    );

    // ---- The tabs, where the plan put them ------------------------------
    //
    // Each strip is at the x-range of the pane it belongs to, so a tab sits above the
    // listing it governs. `strips` is empty when the top row of panes has its own band
    // instead, and then the bar names the folder in front — a title bar's usual job.
    let mut slots = Vec::new();
    for (pane, strip) in strips {
        if let Some(pane) = panes.iter().find(|p| p.id == *pane) {
            slots.extend(tab_strip(
                ui,
                t,
                *strip,
                pane,
                pane.id == focused,
                drag,
                icons_cache,
                out,
            ));
        }
    }
    if strips.is_empty() {
        let label = panes
            .iter()
            .find(|p| p.id == focused)
            .map(|p| p.tab().title.clone())
            .unwrap_or_default();
        if !label.is_empty() {
            let galley = truncated(
                ui.painter(),
                &label,
                t.fonts.body.clone(),
                t.text.secondary,
                (controls_left - space::S3 - x).max(0.0),
            );
            crate::ui::text_left(
                ui.painter(),
                Rect::from_min_max(
                    pos2(x, bar.top() - TEXT_LIFT),
                    pos2(controls_left, bar.bottom() - TEXT_LIFT),
                ),
                galley,
            );
        }
    }

    slots
}

/// Lay out and paint one pane's tabs at `strip`, with its `+` button at the end.
///
/// Used both by the title bar and by a split pane's own header, so a tab behaves the
/// same wherever it is. Returns where each one ended up, which the drag resolution needs.
#[allow(clippy::too_many_arguments)]
pub fn tab_strip(
    ui: &mut Ui,
    t: &Theme,
    strip: Rect,
    pane: &Pane,
    focused: bool,
    drag: &Option<TabDrag>,
    icons_cache: &mut crate::shell::icons::Icons,
    out: &mut Vec<Action>,
) -> Vec<Slot> {
    if strip.width() < TAB_MIN * 0.5 {
        return Vec::new();
    }

    let natural = tab_widths(ui, t, pane);
    let wanted: f32 = natural.iter().sum();
    // The `+` button and the gap before it are the strip's fixed cost.
    let available = (strip.width() - TOOL_SIZE - space::S2 * 2.0).max(0.0);

    // Too many tabs: shrink them all in proportion, down to a floor where only the glyph
    // and the close button are left. Browsers do this, and it beats a scrolling strip
    // because every tab stays clickable.
    let scale = if wanted > available && wanted > 0.0 {
        (available / wanted).max(0.0)
    } else {
        1.0
    };

    let mut slots = Vec::with_capacity(natural.len());
    // Which tabs painted no surface of their own, for the dividers below.
    let mut bare = Vec::with_capacity(natural.len());
    let mut x = strip.left();

    for (index, tab) in pane.tabs.iter().enumerate() {
        let want = natural[index];
        // The floor is the *lesser* of the minimum and the natural width, so a tab whose
        // title is already short keeps its size rather than being padded out.
        let width = (want * scale).max(TAB_MIN.min(want));
        // Down to the bottom edge of the strip, and hard against its neighbour: the two
        // things that make a row of tabs read as a browser's rather than as a row of chips.
        let rect = Rect::from_min_max(
            pos2(x.round(), (strip.top() + TAB_TOP).round()),
            pos2((x + width).round(), strip.bottom().round()),
        );
        x += width;
        if rect.right() > strip.right() {
            // No room left; only reachable at absurd tab counts in a narrow pane.
            break;
        }

        let active = pane.active == index;
        let being_dragged = drag
            .as_ref()
            .is_some_and(|d| d.live && d.pane == pane.id && d.tab == index);
        // Windows' icon for the folder this tab is on — the Downloads arrow, the machine
        // for This PC, a drive's own icon — with the painted glyph until it arrives. Tabs
        // come in tens, so a lookup per path is affordable here in a way it would not be
        // in a listing.
        let shell = icons_cache
            .place(&tab.path)
            .and_then(|icon| icons_cache.uv(ui.ctx(), icon));
        let filled = paint_tab(
            ui,
            t,
            rect,
            &tab.title,
            tab.path.as_os_str().is_empty(),
            tab.dir.is_none(),
            active,
            focused,
            being_dragged,
            shell,
            pane.id,
            index,
            out,
        );
        slots.push(Slot {
            pane: pane.id,
            tab: index,
            rect,
        });
        bare.push(!filled);
    }

    // A hairline between two tabs that both have no surface of their own. Tabs touch now, so
    // without this a run of quiet ones reads as a single long row of labels. Where either
    // neighbour is filled its own edge does the job, and a line there would be noise.
    for index in 1..slots.len() {
        if bare[index - 1] && bare[index] {
            let x = slots[index].rect.left().round() - 0.5;
            let inset = space::S2;
            ui.painter().line_segment(
                [
                    pos2(x, slots[index].rect.top() + inset),
                    pos2(x, slots[index].rect.bottom() - inset),
                ],
                Stroke::new(1.0, t.stroke.subtle),
            );
        }
    }

    // The `+` button: a new tab on the same folder this pane is showing.
    let plus = Rect::from_min_size(
        pos2(
            (x + space::S2).round(),
            // Centred on the tabs, not on the strip, now that the tabs no longer are.
            ((strip.top() + TAB_TOP + strip.bottom()) * 0.5 - TOOL_SIZE * 0.5).round(),
        ),
        vec2(TOOL_SIZE, TOOL_SIZE),
    );
    if plus.right() <= strip.right()
        && crate::ui::tool_button(
            ui,
            t,
            plus,
            Id::new(("new-tab", pane.id)),
            &azur_icons::plus,
            // Its key as well as what it does, which is the one thing the menu under the mark said
            // and this button did not — and the two are the same command, so they say it the same
            // way. See `azur_egui_theme::components::shortcut_in` for the bracket going grey.
            "New tab in this folder (Ctrl+T)",
            true,
            false,
            // The strip it sits in, whether that is the title bar or a band of its own.
            t.surfaces.titlebar,
        )
        .clicked()
    {
        out.push(Action::NewTab { pane: pane.id });
    }

    slots
}

/// What each of a pane's tabs would like to be, before anything has to give.
fn tab_widths(ui: &Ui, t: &Theme, pane: &Pane) -> Vec<f32> {
    pane.tabs
        .iter()
        .map(|tab| {
            let label = ui.painter().layout_no_wrap(
                tab.title.clone(),
                t.fonts.body.clone(),
                Color32::PLACEHOLDER,
            );
            (label.size().x + TAB_FURNITURE).clamp(TAB_MIN, TAB_MAX)
        })
        .collect()
}

/// One tab.
#[allow(clippy::too_many_arguments)]
fn paint_tab(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    title: &str,
    is_this_pc: bool,
    loading: bool,
    active: bool,
    pane_focused: bool,
    being_dragged: bool,
    shell: Option<(egui::TextureId, Rect)>,
    pane: PaneId,
    index: usize,
    out: &mut Vec<Action>,
) -> bool {
    let response = ui.interact(
        rect,
        Id::new(("tab", pane, index)),
        Sense::click_and_drag(),
    );

    let corner = TAB_CORNER;
    // Whether this tab has a surface of its own, which is what decides whether it needs a
    // divider from its neighbour: touching tabs with no fill between them would read as one
    // long row of labels.
    let mut filled = true;
    // The strip's whole ladder is pinned to [`crate::ui::seam`] rather than to Azur's control
    // steps, and moved with it when it did. Four fills, in order of how much they claim:
    // nothing, `background-layer-alt` (a hover, or an unfocused pane's active tab), then the seam
    // itself (a press, or the focused pane's active tab). A press taking the *selected* colour is
    // deliberate — it is about to become the selection.
    //
    // Reading them off `control-hover` / `control-active` was right while the bar was
    // `control-active` too, and became an inversion the moment it was not: a hovered inactive tab
    // came out brighter than the selected one, which reads as though the pointer had selected it.
    //
    // A tab that is being dragged stays in place as a ghost outline, so the strip
    // does not reflow under the pointer mid-gesture.
    if being_dragged {
        ui.painter().rect_stroke(
            rect,
            corner,
            Stroke::new(1.0, t.stroke.subtle),
            StrokeKind::Inside,
        );
    } else if active {
        // Grey, and the only thing that says which pane the keyboard is in: a step further from
        // the strip for the focused pane's active tab than for an unfocused one's. That signal
        // used to be a 2px accent line along the bottom edge, which is gone.
        //
        // The focused pane's colour comes from `crate::ui::seam`, which is also the path bar
        // below it and the lines between the panels — this tab is the visible end of that
        // surface, so the two cannot be allowed to drift apart.
        //
        // And it reaches one point past its own rect, to paint over the strip's bottom hairline.
        // That line marks where the title bar or the band ends, which is worth marking
        // everywhere except across a weld: the focused pane's tab and its path bar are one
        // surface, and a `stroke-subtle` line drawn through them cut it in half. An unfocused
        // pane's tab is a different grey from the bar below it and keeps the edge, which is
        // right — it is not welded to a surface it does not match.
        ui.painter().rect_filled(
            if pane_focused {
                Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y + 1.0))
            } else {
                rect
            },
            corner,
            if pane_focused {
                crate::ui::bar(t)
            } else {
                t.bg.layer_alt
            },
        );
    } else if response.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, corner, crate::ui::bar(t));
    } else if response.hovered() {
        ui.painter().rect_filled(rect, corner, t.bg.layer_alt);
    } else {
        filled = false;
    }

    // `text-primary` only for the tab that is actually current — the active tab of the pane the
    // keyboard is in — and for whatever the pointer is over.
    //
    // An unfocused pane's active tab used to get it too, back when its fill was three steps off
    // the strip and could carry the distinction alone. It is one step now that the whole ladder
    // has come down with [`crate::ui::seam`], and one step at the bottom of the dark ramp is
    // `GRAY_2` to `GRAY_3` — barely a change. So the label says it as well, which is the more
    // legible of the two channels anyway.
    let text_color = if being_dragged {
        t.text.disabled
    } else if (active && pane_focused) || response.hovered() {
        t.text.primary
    } else {
        t.text.secondary
    };

    // The glyph: a folder, or the machine for This PC. Dimmed while the listing is
    // still on its way, which is the only loading indicator a fast scan has time
    // to show.
    let glyph_x = rect.left() + 8.0;
    let box_rect = icon_rect(rect, glyph_x, TOOL_ICON);
    match shell {
        Some((texture, uv)) => {
            ui.painter().image(
                texture,
                box_rect,
                uv,
                // Faded while the listing is still on its way, which is the only loading
                // indicator a fast scan has time to show.
                if loading {
                    Color32::from_white_alpha(110)
                } else {
                    Color32::WHITE
                },
            );
        }
        None => {
            let glyph: azur_icons::Icon<'_> = if is_this_pc {
                &icons::this_pc
            } else if active {
                &icons::folder_open
            } else {
                &icons::folder
            };
            glyph(
                ui.painter(),
                box_rect,
                if loading {
                    t.text.disabled
                } else if is_this_pc {
                    text_color
                } else {
                    t.folder
                },
            );
        }
    };

    // The close button, which only appears when there is room for it.
    let mut text_right = rect.right() - space::S2;
    let close_room = rect.width() > TAB_MIN + 8.0;
    let show_close = close_room && (active || response.hovered());
    if close_room {
        let close = Rect::from_min_size(
            pos2(
                rect.right() - space::S2 - TOOL_ICON,
                (rect.center().y - TOOL_ICON * 0.5).round(),
            ),
            vec2(TOOL_ICON, TOOL_ICON),
        );
        text_right = close.left() - space::S2;
        if show_close {
            let close_response =
                ui.interact(close, Id::new(("tab-close", pane, index)), Sense::click());
            if close_response.hovered() {
                ui.painter().rect_filled(
                    close.expand(2.0),
                    CornerRadius::same(radius::SMALL),
                    t.bg.control_active,
                );
            }
            azur_icons::close(
                ui.painter(),
                close,
                if close_response.hovered() {
                    t.status.danger
                } else {
                    t.text.tertiary
                },
            );
            if close_response.clicked() {
                out.push(Action::CloseTab { pane, tab: index });
            }
        }
    }

    let label_left = glyph_x + TOOL_ICON + space::S2;
    if text_right > label_left {
        let galley = truncated(
            ui.painter(),
            title,
            t.fonts.body.clone(),
            text_color,
            text_right - label_left,
        );
        crate::ui::text_left(
            ui.painter(),
            Rect::from_min_max(
                pos2(label_left, rect.top() - TEXT_LIFT),
                pos2(text_right, rect.bottom() - TEXT_LIFT),
            ),
            galley,
        );
    }

    // Clicks. A middle click closes, as everywhere else that has tabs.
    if response.clicked() {
        out.push(Action::ActivateTab { pane, tab: index });
    }
    if response.middle_clicked() {
        out.push(Action::CloseTab { pane, tab: index });
    }
    // A drag starts here and is picked up next frame, which is also when the
    // pointer has moved far enough for it to be a drag at all.
    if response.drag_started() {
        let origin = ui
            .input(|i| i.pointer.press_origin())
            .unwrap_or(rect.left_center());
        out.push(Action::BeginTabDrag {
            pane,
            tab: index,
            grab_dx: origin.x - rect.left(),
        });
    }
    filled
}

/// The application menu: the handful of settings that belong to the window rather
/// than to a pane.
fn app_menu(
    ui: &mut Ui,
    trigger: &egui::Response,
    palette: crate::theme::Palette,
    sidebar: bool,
    win_key: bool,
    out: &mut Vec<Action>,
) {
    use azur_egui_theme::components::{submenu, Menu, MenuItem};

    Menu::new(trigger).min_width(220.0).show(ui.ctx(), |ui| {
        if ui
            .add(MenuItem::new("New tab").shortcut("Ctrl+T").icon(&azur_icons::plus))
            .clicked()
        {
            out.push(Action::NewTabFocused);
        }
        if ui
            .add(
                MenuItem::new("Split to the right")
                    .shortcut("Ctrl+\\")
                    .icon(&icons::split_side),
            )
            .clicked()
        {
            out.push(Action::SplitFocused {
                side: crate::pane::Side::Right,
            });
        }
        if ui
            .add(MenuItem::new("Split below").icon(&icons::split_down))
            .clicked()
        {
            out.push(Action::SplitFocused {
                side: crate::pane::Side::Bottom,
            });
        }
        azur_egui_theme::components::menu_divider(ui);
        // The panel down the left, which is the one piece of furniture in this window with no switch
        // of its own anywhere on screen — there is nowhere to put one that is not inside the thing
        // being hidden. Ticked rather than named twice: one entry that says whether the panel is
        // showing, rather than a `Show` and a `Hide` that are never both true.
        //
        // At the head of the section the window's own shape is in, because that is what it belongs
        // with: what this hides and what the two entries below it resize are the same window, and the
        // section above is about panes and tabs *inside* it.
        //
        // **`Ctrl+B` and not the `Ctrl+Win+←` this was first bound to**, because that combination is
        // Windows' own "previous virtual desktop" and never reaches this program at all — a menu that
        // printed a shortcut which does nothing is worse than one that printed none. Both are read; see
        // [`crate::app::App::window_keys`].
        if ui
            .add(
                MenuItem::new("Left panel")
                    .shortcut("Ctrl+B")
                    .selected(sidebar)
                    .icon(&icons::split_side),
            )
            .clicked()
        {
            out.push(Action::ToggleSidebar);
        }
        if ui
            .add(
                MenuItem::new("Across every screen")
                    .shortcut("Ctrl+Win+↑")
                    .icon(&azur_icons::window_maximize),
            )
            .clicked()
        {
            out.push(Action::Window(WindowAction::SpanScreens));
        }
        if ui
            .add(
                MenuItem::new("Reset window size")
                    .shortcut("Ctrl+Win+↓")
                    .icon(&azur_icons::window_restore),
            )
            .clicked()
        {
            out.push(Action::Window(WindowAction::ResetSize));
        }
        azur_egui_theme::components::menu_divider(ui);
        // The themes, behind one entry rather than side by side in the open. They are a *choice
        // between* things and not a row of switches, which a submenu says by shape: one row naming
        // the question, and the answers a level in. Out here they were the first thing the menu
        // said, which is a great deal of prominence for something set once.
        //
        // **Driven by `Palette::ALL`**, so a new palette appears here by existing. This used to be
        // a literal pair, which is the kind of list that gets forgotten the one time it matters.
        //
        // The parent carries the same `dot` its options do, rather than for want of a better glyph:
        // there is no appearance icon in either set, and a row that opens a choice reading in the
        // same visual family as the choice beats an unrelated shape standing in for one.
        //
        // `ui.close()` after the push, per this helper's own documentation — a click in a nested menu
        // has to bring the whole stack down and not just the level it landed in.
        submenu(ui, MenuItem::new("Theme").icon(&azur_icons::dot), |ui| {
            for wants in crate::theme::Palette::ALL {
                if ui
                    .add(
                        MenuItem::new(wants.label())
                            .selected(palette == wants)
                            .icon(&azur_icons::dot),
                    )
                    .clicked()
                {
                    out.push(Action::SetTheme(wants));
                    ui.close();
                }
            }
        });
        // Windows' own folder key, and **the one entry in this window that changes something outside
        // it**. It is last but for `Close window`, and that is the reasoning: everything above acts
        // on this window and stops existing when the window closes, and this one outlives the
        // process. Ticked when `Win+E` opens this build.
        //
        // See [`crate::shell::winkey`], which is where the setting lives and where what is
        // deliberately *not* claimed alongside it is set out. Worth knowing while reading this line:
        // the label names the outcome and not the gesture, and **the gesture is only `Win+E`** —
        // double-clicking a folder, and "show in folder" from a browser's downloads, still open
        // Explorer.
        //
        // No `shortcut("Win+E")`, deliberately. That column means "this key does this here", and
        // `Win+E` never reaches this program as a keystroke at all — the shell holds it and launches
        // the executable. Printing it there would be the same class of lie the `Ctrl+B` note above
        // is about, one step further on: a key that does something, listed against the thing that is
        // not how it does it.
        if ui
            .add(
                MenuItem::new("Set as default explorer")
                    .selected(win_key)
                    .icon(&icons::folder_open),
            )
            .clicked()
        {
            out.push(Action::SetWinKey(!win_key));
        }
        azur_egui_theme::components::menu_divider(ui);
        if ui
            .add(MenuItem::new("Close window").shortcut("Alt+F4").danger(true))
            .clicked()
        {
            out.push(Action::Window(WindowAction::Close));
        }
    });
}

/// The minimise / maximise / close buttons. Returns their left edge, which is
/// where the tab strip has to stop.
fn window_buttons(
    ui: &mut Ui,
    t: &Theme,
    bar: Rect,
    maximized: bool,
    out: &mut Vec<Action>,
) -> f32 {
    // The platform's own caption buttons are 46×32 with a 10px hairline glyph, and
    // matching that is what makes this window read as a window rather than as an
    // application pretending to be one. Straight out of Azur's `TitleBar`.
    const WIDTH: f32 = CAPTION_WIDTH;
    const GLYPH: f32 = 10.0;

    let buttons: [(WindowAction, azur_icons::Icon<'_>, bool); 3] = [
        (WindowAction::Close, &azur_icons::window_close, true),
        (
            WindowAction::ToggleMaximize,
            if maximized {
                &azur_icons::window_restore
            } else {
                &azur_icons::window_maximize
            },
            false,
        ),
        (WindowAction::Minimize, &azur_icons::window_minimize, false),
    ];

    let mut left = bar.right();
    for (which, glyph, danger) in buttons {
        left -= WIDTH;
        let rect = Rect::from_min_size(pos2(left.round(), bar.top()), vec2(WIDTH, bar.height()));
        // The fill covers the whole button so it reads as one block; only the input
        // rect steps below the resize band.
        let hit = Rect::from_min_max(pos2(rect.left(), rect.top() + TOP_BAND), rect.max);
        let response = ui.interact(hit, Id::new(("caption", which as u8)), Sense::click());

        let (fill, color) = if response.hovered() {
            if danger {
                (Some(t.status.danger), t.text.on_accent)
            } else {
                (Some(t.bg.control_hover), t.text.primary)
            }
        } else {
            (None, t.text.secondary)
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
        }
        glyph(
            ui.painter(),
            Rect::from_center_size(rect.center(), vec2(GLYPH, GLYPH)),
            color,
        );
        if response.clicked() {
            out.push(Action::Window(which));
        }
    }
    left
}

// ---------------------------------------------------------------------------
// The drag
// ---------------------------------------------------------------------------

/// Work out where the dragged tab would land, show it, and act on the release.
///
/// Called after the panes have been drawn, because the slots can now be anywhere: in the
/// title bar with one pane, or inside each pane's own header when split.
pub fn resolve_drag(
    ui: &mut Ui,
    t: &Theme,
    panes: &[Pane],
    slots: &[Slot],
    drag: &mut Option<TabDrag>,
    out: &mut Vec<Action>,
) {
    let Some(state) = drag.as_mut() else { return };
    let ctx = ui.ctx();

    let pointer = ctx.pointer_interact_pos();
    let held = ctx.input(|i| i.pointer.any_down());
    let Some(pointer) = pointer else {
        *drag = None;
        return;
    };

    // A few pixels of slop, so a slightly unsteady click is still a click.
    if !state.live {
        let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(pointer);
        if pointer.distance(origin) < 6.0 {
            if !held {
                *drag = None;
            }
            return;
        }
        state.live = true;
    }

    let target = drop_target(pointer, slots, panes, state);

    // The preview goes in a foreground layer: the panes are drawn after this
    // function runs, and a layer is ordered by rank rather than by when it was
    // added to.
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("yafe-dock-hint")));

    // ---- Where it could go -----------------------------------------------
    //
    // A compass of five in every pane that would take the tab, in grey. Without it the gesture is
    // invisible until the pointer happens to be in the right place: a drag with nothing under it
    // looked like a drag that was not going to work.
    //
    // **It stays up once a target is reached**, including the mark the accent is about to cover.
    // Clearing the marks of the pane being dropped into meant the compass vanished exactly when
    // it was being used, and a reader moving between the five places lost the other four. The
    // accent goes on top, so what says "here" is still the only blue on screen.
    for pane in panes {
        for zone in Zone::ALL {
            if !accepts(pane, panes, state, zone) {
                continue;
            }
            crate::ui::drop_hint(&painter, dock::hint_rect(pane.rect, zone), zone, t);
        }
    }

    // And where it *would* go if the button came up now, over the top of the grey.
    match target {
        Drop::Split { pane, side } => {
            if let Some(rect) = pane_rect(panes, pane) {
                crate::ui::drop_preview(&painter, dock::preview_rect(rect, Zone::Split(side)), t);
            }
        }
        Drop::Into { pane } => {
            if let Some(rect) = pane_rect(panes, pane) {
                crate::ui::drop_preview(&painter, rect, t);
            }
        }
        Drop::Strip { pane, index } => {
            // A caret between the tabs, where the tab would be inserted.
            let (x, band) = caret_at(slots, pane, index);
            painter.rect_filled(
                Rect::from_min_size(pos2(x - 1.0, band.top()), vec2(2.0, band.height())),
                CornerRadius::same(radius::CIRCULAR),
                t.accent.default,
            );
        }
        Drop::Nowhere => {}
    }

    // The ghost, following the pointer.
    ghost(&painter, t, pointer, state);
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);

    if !held {
        let from = (state.pane, state.tab);
        match target {
            Drop::Strip { pane, index } => out.push(Action::MoveTab {
                from: from.0,
                tab: from.1,
                to: pane,
                index,
            }),
            Drop::Into { pane } => out.push(Action::MoveTab {
                from: from.0,
                tab: from.1,
                to: pane,
                index: usize::MAX,
            }),
            Drop::Split { pane, side } => out.push(Action::SplitTab {
                from: from.0,
                tab: from.1,
                target: pane,
                side,
            }),
            Drop::Nowhere => {}
        }
        *drag = None;
    }
}

/// Which drop the pointer is currently over.
fn drop_target(pointer: Pos2, slots: &[Slot], panes: &[Pane], state: &TabDrag) -> Drop {
    // A strip wins over the pane behind it, so reordering never needs the pointer to
    // avoid the listing. Which pane's strip is worked out from the slots themselves, since
    // the bar holds one group per pane and the nearest tab decides which group.
    let over_strip = slots
        .iter()
        .filter(|s| {
            let band = s.rect.expand2(vec2(space::S3, space::S2));
            pointer.y >= band.top() && pointer.y <= band.bottom()
        })
        .min_by(|a, b| {
            let distance = |s: &Slot| (pointer.x - s.rect.center().x).abs();
            distance(a).total_cmp(&distance(b))
        })
        .map(|s| s.pane);

    if let Some(pane) = over_strip {

        // Insert before the first tab of that strip whose midpoint is past the pointer.
        let mut index = 0;
        for slot in slots.iter().filter(|s| s.pane == pane) {
            if pointer.x < slot.rect.center().x {
                break;
            }
            index = slot.tab + 1;
        }

        // Dropping a tab back where it already is means nothing.
        if pane == state.pane && (index == state.tab || index == state.tab + 1) {
            return Drop::Nowhere;
        }
        return Drop::Strip { pane, index };
    }

    // Over the panes: the edges split, the middle moves.
    for pane in panes {
        if pane.rect.contains(pointer) {
            let zone = dock::zone_at(pane.rect, pointer);
            if !accepts(pane, panes, state, zone) {
                return Drop::Nowhere;
            }
            return match zone {
                Zone::Split(side) => Drop::Split { pane: pane.id, side },
                Zone::Into => Drop::Into { pane: pane.id },
            };
        }
    }
    Drop::Nowhere
}

/// Whether dropping the dragged tab in this zone of this pane would do anything.
///
/// Both no-ops are about the pane the tab came *from*: moving it into the strip it is already
/// in, and splitting a pane off itself when that tab is all the pane holds — which would leave
/// an empty pane behind.
///
/// Asked by [`drop_target`] of the one zone the pointer is in, and by the hints of all of them,
/// so a grey rect is never drawn where a release would do nothing.
fn accepts(pane: &Pane, panes: &[Pane], state: &TabDrag, zone: Zone) -> bool {
    if pane.id != state.pane {
        return true;
    }
    match zone {
        Zone::Into => false,
        Zone::Split(_) => panes
            .iter()
            .find(|p| p.id == state.pane)
            .is_some_and(|p| p.tabs.len() > 1),
    }
}

/// Where a pane is this frame, by id.
fn pane_rect(panes: &[Pane], pane: PaneId) -> Option<Rect> {
    panes.iter().find(|p| p.id == pane).map(|p| p.rect)
}

/// Where the insertion caret goes for a strip drop, and how tall to draw it.
fn caret_at(slots: &[Slot], pane: PaneId, index: usize) -> (f32, Rect) {
    let group: Vec<&Slot> = slots.iter().filter(|s| s.pane == pane).collect();
    let band = group
        .first()
        .map(|s| s.rect.expand2(vec2(0.0, 2.0)))
        .unwrap_or(Rect::NOTHING);
    let x = match group.get(index) {
        Some(slot) => slot.rect.left() - space::S2 * 0.5,
        None => group
            .last()
            .map(|s| s.rect.right() + space::S2 * 0.5)
            .unwrap_or(band.left()),
    };
    (x, band)
}

/// The tab that follows the pointer.
fn ghost(painter: &egui::Painter, t: &Theme, pointer: Pos2, state: &TabDrag) {
    let width = 168.0;
    let rect = Rect::from_min_size(
        pos2(
            (pointer.x - state.grab_dx.min(width - 24.0)).round(),
            (pointer.y - TAB_HEIGHT * 0.5).round(),
        ),
        vec2(width, TAB_HEIGHT),
    );
    let corner = CornerRadius::same(radius::SMALL);
    painter.rect_filled(rect, corner, t.bg.layer_alt);
    painter.rect_stroke(rect, corner, Stroke::new(1.0, t.accent.default), StrokeKind::Inside);
    icons::folder(
        painter,
        icon_rect(rect, rect.left() + 8.0, TOOL_ICON),
        t.folder,
    );
    let label_left = rect.left() + 8.0 + TOOL_ICON + space::S2;
    let galley = truncated(
        painter,
        &state.title,
        t.fonts.body.clone(),
        t.text.primary,
        rect.right() - space::S2 - label_left,
    );
    crate::ui::text_left(
        painter,
        Rect::from_min_max(
            pos2(label_left, rect.top() - TEXT_LIFT),
            pos2(rect.right(), rect.bottom() - TEXT_LIFT),
        ),
        galley,
    );
}

// ---------------------------------------------------------------------------
// Resize borders
// ---------------------------------------------------------------------------

/// The eight grab bands a decorated window would have got from the platform.
///
/// # Why not an `Area`
///
/// The obvious implementation — one foreground [`egui::Area`] per band, positioned
/// with `fixed_pos` and filled with `allocate_rect` — is silently, spectacularly
/// wrong, and it is worth recording why.
///
/// An `Area` lays its content out relative to its own origin and then clamps that
/// origin so the area fits on screen. Handing it an *absolute* rect means the content
/// size is measured from wherever the origin currently is to the far edge of the
/// rect; the clamp then moves the origin to fit that size; which makes the content
/// measure larger still. The two chase each other and settle at **half the window**.
/// The east band, asked for six pixels down the right-hand edge, came out as
/// `[600, 16]-[1200, 784]` — silently swallowing every click in the right half of
/// the window, with nothing in the source to suggest it.
///
/// So the bands are plain [`egui::Ui::interact`] calls on the root `Ui`, whose
/// coordinate space *is* screen space. No layout, no clamping, nothing to feed back.
///
/// # Why they never overlap anything
///
/// Registered last, so they win any contest. The top band is [`TOP_BAND`] tall and the title
/// bar's own controls all start below it, so up there is still no contest to win — a grab band
/// that has to fight the close button for a click is a grab band that will one day win.
///
/// The other three do now overlap the panels, since the panels reach the window's edges.
/// [`RESIZE_BAND`] says what that costs and why it is the lesser evil.
pub fn resize_borders(ui: &mut Ui, maximized: bool) {
    use egui::viewport::ResizeDirection as Dir;
    use egui::ViewportCommand as Cmd;

    // A maximised window cannot be resized, and offering the cursor for it is a lie.
    if maximized {
        return;
    }

    let screen = ui.ctx().viewport_rect();
    // A window this small has no margin to put them in; the platform's minimum size
    // keeps this out of reach in practice.
    if screen.width() < CORNER * 4.0 || screen.height() < CORNER * 4.0 {
        return;
    }

    let band = RESIZE_BAND;
    let (left, right) = (screen.left(), screen.right());
    let (top, bottom) = (screen.top(), screen.bottom());

    let bands: [(&str, Rect, Dir, egui::CursorIcon); 8] = [
        (
            "n",
            Rect::from_min_max(pos2(left + CORNER, top), pos2(right - CORNER, top + TOP_BAND)),
            Dir::North,
            egui::CursorIcon::ResizeNorth,
        ),
        (
            "s",
            Rect::from_min_max(pos2(left + CORNER, bottom - band), pos2(right - CORNER, bottom)),
            Dir::South,
            egui::CursorIcon::ResizeSouth,
        ),
        (
            "w",
            Rect::from_min_max(pos2(left, top + CORNER), pos2(left + band, bottom - CORNER)),
            Dir::West,
            egui::CursorIcon::ResizeWest,
        ),
        (
            "e",
            Rect::from_min_max(pos2(right - band, top + CORNER), pos2(right, bottom - CORNER)),
            Dir::East,
            egui::CursorIcon::ResizeEast,
        ),
        // The top corners are only as tall as the top band, so they cannot reach the
        // application mark or the window buttons. The bottom two sit in open canvas
        // and get the full square.
        (
            "nw",
            Rect::from_min_max(pos2(left, top), pos2(left + CORNER, top + TOP_BAND)),
            Dir::NorthWest,
            egui::CursorIcon::ResizeNorthWest,
        ),
        (
            "ne",
            Rect::from_min_max(pos2(right - CORNER, top), pos2(right, top + TOP_BAND)),
            Dir::NorthEast,
            egui::CursorIcon::ResizeNorthEast,
        ),
        (
            "sw",
            Rect::from_min_max(pos2(left, bottom - CORNER), pos2(left + CORNER, bottom)),
            Dir::SouthWest,
            egui::CursorIcon::ResizeSouthWest,
        ),
        (
            "se",
            Rect::from_min_max(pos2(right - CORNER, bottom - CORNER), screen.max),
            Dir::SouthEast,
            egui::CursorIcon::ResizeSouthEast,
        ),
    ];

    // **The left button only.** `Sense::drag()` senses a drag from *any* button, and
    // `Response::drag_started` does not say which — so a middle-button drag on an edge resized the
    // window, and a middle-button drag is a paste on X11 and a scroll gesture everywhere else.
    // Nothing in this program resizes on middle-click, so the button is named here rather than
    // filtered further up. The same restriction on the cursor icon, or an edge would promise a
    // resize under a button that no longer performs one.
    let with = egui::PointerButton::Primary;
    for (name, rect, direction, cursor) in bands {
        let response = ui.interact(rect, Id::new(("yafe-resize", name)), Sense::drag());
        if response.hovered() || response.dragged_by(with) {
            ui.ctx().set_cursor_icon(cursor);
        }
        if response.drag_started_by(with) {
            ui.ctx().send_viewport_cmd(Cmd::BeginResize(direction));
        }
    }
}

#[cfg(test)]
mod tests;
