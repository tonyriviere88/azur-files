1//! The window: chrome, sidebar, breadcrumb, listing.
2//!
3//! Everything here paints at explicit rects rather than laying widgets out. That
4//! is a deliberate choice and the reason a listing of a hundred thousand files
5//! scrolls at the refresh rate: a row is six painter calls against a rect this
6//! module computed, not a `horizontal()` layout with a `Label` per cell that egui
7//! has to measure, allocate, hit-test and register a widget id for. The design
8//! system's own collections do the same thing for the same reason.
9//!
10//! The rule that keeps it honest: **no colour is chosen here.** Every fill, line
11//! and text colour comes from a role on [`crate::theme::Theme`], so the whole
12//! window still re-themes from one place.
13
14pub mod breadcrumb;
15pub mod chrome;
16pub mod deps;
17pub mod filelist;
18pub mod menu;
19pub mod preview;
20pub mod sidebar;
21
22use std::sync::Arc;
23
24use azur_egui_theme::icons::Icon;
25use azur_egui_theme::tokens::{radius, space};
26use egui::{
27    pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Painter, Rect, Response, Sense, Stroke,
28    Ui,
29};
30
31use crate::theme::Theme;
32
33/// The air inside a tab strip: `tokens::space::S2`.
34///
35/// It used to be the canvas showing *around* the panels as well, and the panels stopped short
36/// of the window's edges by it. They no longer do — nothing separates a panel from the window
37/// but the window's own border — so what is left of it is the inset a strip gives its tabs, and
38/// `chrome::RESIZE_BAND` is the four points the resize edges take back out of the panels.
39pub const GUTTER: f32 = space::S2;
40
41/// The line *between* two panels: one point wide, showing [`seam`] through it.
42///
43/// Panels used to be separated by [`GUTTER`] and each ringed in `stroke-subtle`, which
44/// made every boundary three lines wide — two borders and a channel of canvas between
45/// them — and read as a row of loose cards rather than as one window divided up.
46pub const SEAM: f32 = azur_egui_theme::desktop::SEAM;
47
48/// The colour a panel boundary shows, and the fill of the surfaces welded to it.
49///
50/// One colour doing three jobs on purpose: the selected tab, the path bar directly under it,
51/// and the seams between panels are the frame the panels sit in, so they are the same surface
52/// seen in three places rather than three decisions that happen to agree.
53///
54/// `stroke-subtle`, and the design system's decision now — see `azur::desktop::seam`, which
55/// carries the measurements and the floor the column header puts under it. It was
56/// `background-control-active` here, and came down a step because as a filled band across the top
57/// of every pane that was too loud in the dark theme; the dark frame is deliberately the quieter
58/// of the two sides, at ΔL\* 6.1 against the light theme's unchanged `GRAY_14`.
59pub fn seam(t: &Theme) -> Color32 {
60    azur_egui_theme::desktop::seam(t)
61}
62
63/// Draw the widgets `add` puts up with square corners.
64///
65/// **The three text inputs in this program are square**: the rename field over a name, the path
66/// field the breadcrumb turns into, and the filter box. All three appear in place of something
67/// square and take its shape while they are up — a row, the breadcrumb, a bar of flush panels —
68/// and a 4px radius on a 20px-tall box in the middle of that reads as a bubble.
69///
70/// `azur::desktop::squared` is the rule and the mechanism; this is the name the call sites here
71/// already use.
72pub fn squared<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
73    azur_egui_theme::desktop::squared(ui, add)
74}
75
76/// The hover and pressed fills for a control, whatever surface it is painted on.
77///
78/// [`hover_fill`] and then one rung further in the same direction. `surface` is not read, and
79/// that is the point: Azur's `control` / `control-hover` / `control-active` ladder is written for
80/// a control on `background-layer`, so a control on a surface that is *itself* one of those steps
81/// had nowhere to go and painted the surface's own colour onto the surface — the fill vanished at
82/// the moment of the press instead of deepening. That was the path bar in the **light** theme,
83/// where [`seam`] and `control-active` are the same `GRAY_14`.
84///
85/// Both rungs are `azur::desktop`'s now, and so is the check that they step away from every
86/// surface in both themes. `control_active` is left to the two places here that use it as an
87/// inert fill rather than as a press — a sidebar row and a tab go quiet while they are dragged.
88pub fn control_fills(t: &Theme, _surface: Color32) -> (Color32, Color32) {
89    azur_egui_theme::desktop::control_fills(t)
90}
91
92/// A toolbar button's side, and the icon box inside it. `tokens::control::SMALL`
93/// with `ICON_SMALL`, which is the design system's dense-toolbar pairing.
94pub const TOOL_SIZE: f32 = 24.0;
95pub const TOOL_ICON: f32 = 14.0;
96
97/// Lay one line out, cut to `width` with an ellipsis.
98///
99/// The galley is cached by egui on the finished job, so re-laying an unchanged row
100/// every frame costs a hash rather than a shaping pass.
101pub fn truncated(
102    painter: &Painter,
103    text: &str,
104    font: FontId,
105    color: Color32,
106    width: f32,
107) -> Arc<egui::Galley> {
108    let mut job =
109        egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(font, color));
110    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
111    painter.layout_job(job)
112}
113
114/// Snap a text origin to whole *device* pixels.
115///
116/// A glyph atlas is a texture, so a quad landing between texels is sampled between them and
117/// the text comes out soft with a grey fringe down each stem. Both coordinates matter, and
118/// the x as much as the y: panes are split by fractions of a window, so a column's left edge
119/// is fractional about half the time.
120///
121/// `round_to_pixels` rather than `f32::round`, because a logical point is not a pixel. At
122/// 125% scaling, rounding to a whole point leaves the glyph on a quarter-pixel — which is
123/// most of the blur it was meant to remove, on the displays most likely to have it.
124fn snap(painter: &Painter, at: egui::Pos2) -> egui::Pos2 {
125    use egui::emath::GuiRounding as _;
126    at.round_to_pixels(painter.pixels_per_point())
127}
128
129/// Paint a galley vertically centred in `rect`, starting at its left edge.
130///
131/// This is the helper every file name in the listing goes through.
132pub fn text_left(painter: &Painter, rect: Rect, galley: Arc<egui::Galley>) {
133    let at = pos2(rect.left(), rect.center().y - galley.size().y * 0.5);
134    painter.galley(snap(painter, at), galley, Color32::PLACEHOLDER);
135}
136
137/// The same, pushed against the right edge — for the numeric column.
138pub fn text_right(painter: &Painter, rect: Rect, galley: Arc<egui::Galley>) {
139    let at = pos2(
140        rect.right() - galley.size().x,
141        rect.center().y - galley.size().y * 0.5,
142    );
143    painter.galley(snap(painter, at), galley, Color32::PLACEHOLDER);
144}
145
146/// The square an icon of `size` occupies, vertically centred with its left edge at
147/// `x`.
148pub fn icon_rect(row: Rect, x: f32, size: f32) -> Rect {
149    Rect::from_min_size(
150        pos2(x.round(), (row.center().y - size * 0.5).round()),
151        vec2(size, size),
152    )
153}
154
155/// A hand-painted icon button at a rect this caller already knows.
156///
157/// Azur's `IconButton` would do the same job through the layout system, which is
158/// the wrong shape for chrome laid out by arithmetic — a breadcrumb has to know
159/// where its buttons end before it can measure the segments that follow. The states are the
160/// design system's: `desktop::latched` when it is on — the same surface a selected row wears —
161/// and otherwise the two [`control_fills`] for the surface behind it.
162///
163/// `surface` is that surface. It has to be told, because a painter cannot read back what
164/// is already under the rect, and the two toolbars this button appears on are different
165/// colours — the tab strip is `background-layer`, the path bar is [`seam`].
166#[allow(clippy::too_many_arguments)]
167pub fn tool_button(
168    ui: &mut Ui,
169    t: &Theme,
170    rect: Rect,
171    id: Id,
172    glyph: Icon<'_>,
173    tooltip: &str,
174    enabled: bool,
175    active: bool,
176    surface: Color32,
177) -> Response {
178    let response = ui.interact(
179        rect,
180        id,
181        if enabled {
182            Sense::click()
183        } else {
184            Sense::hover()
185        },
186    );
187
188    let corner = CornerRadius::same(radius::SMALL);
189    let (hover, pressed) = control_fills(t, surface);
190    // A latched button wears the surface a selected row wears — `azur::desktop::latched`, which
191    // is where that rule and its numbers live. It used to be `accent-subtle`, Azur's own latch
192    // tint, which in the dark theme is a navy barely off the bar it sits on: the one toggle on
193    // this bar read as *nearly* on.
194    let latched = azur_egui_theme::desktop::latched(t, response.hovered());
195    let fill = if !enabled {
196        None
197    } else if active {
198        Some(latched.0)
199    } else if response.is_pointer_button_down_on() {
200        Some(pressed)
201    } else if response.hovered() {
202        Some(hover)
203    } else {
204        None
205    };
206    if let Some(fill) = fill {
207        ui.painter().rect_filled(rect, corner, fill);
208    }
209
210    let color = if !enabled {
211        t.text.disabled
212    } else if active {
213        latched.1
214    } else if response.hovered() {
215        t.text.primary
216    } else {
217        t.text.secondary
218    };
219    glyph(
220        ui.painter(),
221        Rect::from_center_size(rect.center(), vec2(TOOL_ICON, TOOL_ICON)),
222        color,
223    );
224
225    if response.has_focus() {
226        azur_egui_theme::icons::focus_ring_inset(ui.painter(), rect, corner, t.stroke.focus);
227    }
228    if !tooltip.is_empty() && enabled {
229        azur_egui_theme::components::tooltip(response.clone(), tooltip);
230    }
231    response
232}
233
234/// A horizontal `stroke-subtle` rule across `rect`'s bottom edge.
235pub fn rule_below(painter: &Painter, rect: Rect, t: &Theme) {
236    let y = rect.bottom().round() - 0.5;
237    painter.line_segment(
238        [pos2(rect.left(), y), pos2(rect.right(), y)],
239        Stroke::new(1.0, t.stroke.subtle),
240    );
241}
242
243/// A section label in the sidebar: `body-strong` in `text-secondary`, which is what
244/// `collection_label` gives a group of rows.
245pub fn section_label(painter: &Painter, rect: Rect, t: &Theme, text: &str) {
246    let galley = truncated(
247        painter,
248        text,
249        t.fonts.caption.clone(),
250        t.text.tertiary,
251        rect.width(),
252    );
253    text_left(painter, rect, galley);
254}
255
256/// Paint the accent bar Azur puts down the left edge of a selected row.
257///
258/// `accent.mark` — "the accent as ink rather than as a surface: … a 2px selection bar", which is
259/// the role's own job description. It was `accent.default`, and became indistinguishable from the
260/// row the moment the selected *fill* became `accent.default` too.
261pub fn selection_bar(painter: &Painter, row: Rect, t: &Theme) {
262    azur_egui_theme::desktop::selection_bar(painter, row, t)
263}
264
265/// The fill under the pointer on anything you hover in order to *go* somewhere: a row in the
266/// listing, a row in the sidebar, a segment or a chevron on the path bar.
267///
268/// `background-control-hover`, three rungs along Azur's neutral ramp from the `GRAY_5` it
269/// specifies, which `crate::theme` puts there through `azur::desktop::apply`. Read back from the
270/// theme rather than named here so that there is one value and not two: the same token is what the
271/// context menu, the column headers, the caption buttons, the tab strip, the application mark and
272/// the design system's own `MenuItem` read, and a second constant living here is how Back,
273/// Forward, Up and Refresh came to be the only four hovers in the window still wearing the old
274/// grey.
275///
276/// `GRAY_8` in the dark theme, not the `GRAY_9` first asked for, because on a listing that is
277/// nearly black the band read a shade hot — and darker is *more* legible, not less: a row's name
278/// on it goes from 5.7:1 to 7.2:1 and its metadata from 2.4:1 to 3.0:1.
279pub fn hover_fill(t: &Theme) -> Color32 {
280    azur_egui_theme::desktop::hover_fill(t)
281}
282
283/// The row fill for a state, or `None` to leave the surface showing.
284///
285/// Azur's list vocabulary is `background-card-hover` hovered and `accent-subtle` selected. The
286/// hover is [`hover_fill`], and in the dark theme the selection is the accent's *surface* ramp
287/// instead — `accent.active`, with `accent.default` under the pointer as well.
288///
289/// It got there by walking back down, every figure sampled off the framebuffer. `AZURE_ACCENT`
290/// (`#3aa0ff`) was asked for first and was far too bright: a selected row's name measured 2.5:1
291/// and its Size, Type and Modified columns 1.06:1, which is to say they were gone.
292/// `accent.default` was halfway back at 6.0:1 and 2.5:1; one rung further again is **8.0:1 and
293/// 3.3:1**. Each step down bought legibility rather than cost it, which is the tell that the first
294/// one was three rungs too far.
295///
296/// `azur::desktop::row_fill` is where that lives now, along with what the light theme does
297/// instead — it keeps `accent-subtle`, because the ink on a row does not change when the row is
298/// selected, and a dark fill under near-black metadata is the same mistake pointing the other way.
299pub fn row_fill(t: &Theme, selected: bool, hovered: bool) -> Option<Color32> {
300    azur_egui_theme::desktop::row_fill(t, selected, hovered)
301}
302
303/// The rectangle a file drop would land in: a folder row, or the listing of the folder on show.
304///
305/// Square, and painted *over* the listing rather than under it. Both of those are corrections,
306/// and both are `azur::desktop::drop_target`'s now: a selected row's fill is an opaque surface of
307/// its own drawn after this, so a wash underneath it vanished completely — dropping onto a folder
308/// you had selected showed nothing at all.
309pub fn drop_target(painter: &Painter, rect: Rect, t: &Theme) {
310    azur_egui_theme::desktop::drop_target(painter, rect, t)
311}
312
313/// Draw a translucent accent wash and outline — the drop preview for a docking
314/// gesture, and the "this pane has focus" hint while dragging.
315///
316/// The same mark as [`drop_target`], for the same reason it is square: it is showing where a
317/// rectangle will go. It carried `radius-medium` for as long as the panes did.
318pub fn drop_preview(painter: &Painter, rect: Rect, t: &Theme) {
319    azur_egui_theme::desktop::drop_target(painter, rect, t)
320}
321
322/// Centre a single line in a rect — for the empty and error states.
323pub fn text_center(painter: &Painter, rect: Rect, font: FontId, color: Color32, text: &str) {
324    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, color);
325}
326
327#[cfg(test)]
328mod tests {
329    use super::*;
330    use azur_egui_theme::desktop;
331
332    /// This window is wearing the design system's application-window preset, and every
333    /// colour helper here is the same value the preset decided.
334    ///
335    /// The three things it can catch, none of which the design system's own tests can:
336    /// [`crate::theme`] forgetting to call `desktop::apply`, so the hover token never
337    /// moves and every widget in the window quietly agrees on Azur's `GRAY_5`; a helper
338    /// here drifting back to a value of its own, which is how four toolbar buttons ended
339    /// up a rung and a half off everything around them; and the two hover tokens parting
340    /// company, so a hovered row and a hovered button are different greys.
341    ///
342    /// What the values *are*, and that they step away from every surface in both themes,
343    /// is `azur_egui_theme::desktop`'s to hold — it is the one that knows why.
344    #[test]
345    fn the_window_wears_the_desktop_preset() {
346        for t in [Theme::dark(), Theme::light()] {
347            let name = if t.dark { "dark" } else { "light" };
348            let (hover, pressed) = control_fills(&t, seam(&t));
349            assert_eq!(
350                hover,
351                desktop::hover_fill(t.azur()),
352                "{name}: this window's hover is not the preset's"
353            );
354            assert_eq!(
355                t.bg.card_hover, t.bg.control_hover,
356                "{name}: a hovered row and a hovered button are different greys"
357            );
358            assert_ne!(
359                hover,
360                azur_egui_theme::Theme::of(t.azur().kind()).bg.control_hover,
361                "{name}: `desktop::apply` was never called — the hover is still Azur's own rung"
362            );
363            assert_eq!(pressed, desktop::press_fill(t.azur()));
364            for (selected, hovered) in [(true, true), (true, false), (false, true), (false, false)]
365            {
366                assert_eq!(
367                    row_fill(&t, selected, hovered),
368                    desktop::row_fill(t.azur(), selected, hovered),
369                    "{name}: a row painted {selected}/{hovered} is not the preset's"
370                );
371            }
372        }
373    }
374}
