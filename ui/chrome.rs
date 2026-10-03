1//! The title bar: the tab strips, the window buttons, and the drag that docks a
2//! tab into a split.
3//!
4//! The window asks the platform not to draw its own caption
5//! (`with_decorations(false)`), so everything a title bar does is done here —
6//! including the resize borders, which come free with a decorated window and have
7//! to be built by hand without one.
8//!
9//! # Where the tabs are
10//!
11//! **Above the pane they belong to.** A strip is laid out at its pane's own x-range rather
12//! than packed along the bar, so a tab is directly over the listing it governs and there is
13//! nothing to work out about which is which.
14//!
15//! For the top row of panes that place is the title bar, which is the strip of window
16//! immediately above them. A pane in any row below has nothing above it but another pane,
17//! so its row gets a band of its own — painted like the title bar, because for that row
18//! that is exactly what it is — and the band's height comes off the top of the panes under
19//! it. [`plan_strips`] decides all of this before anything is drawn, which is why the panes
20//! are laid out before the bar rather than after.
21//!
22//! [`tab_strip`] draws a strip at any rect and every strip goes through it, so a tab
23//! behaves identically wherever it ends up.
24//!
25//! # Why the whole drag gesture lives in this file
26//!
27//! [`resolve_drag`] runs after the panes have been drawn, so the rects it tests the
28//! pointer against are this frame's. It paints its preview into a foreground layer,
29//! which is ordered above the panes regardless of when it was added to. One place owns
30//! the gesture, and nothing has to be threaded through the rest of the frame.
31
32use azur_egui_theme::icons as azur_icons;
33use azur_egui_theme::tokens::{radius, row, space};
34use egui::{
35    pos2, vec2, Color32, CornerRadius, Id, LayerId, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
36};
37
38use crate::app::{Action, WindowAction};
39use crate::dock::{self, Zone};
40use crate::icons;
41use crate::pane::{Pane, PaneId};
42use crate::theme::Theme;
43use crate::ui::{icon_rect, truncated, TOOL_ICON, TOOL_SIZE};
44
45/// `tokens::row::TITLE_BAR`.
46pub const HEIGHT: f32 = row::TITLE_BAR;
47
48/// How far down the top edge belongs to the window-resize band rather than to the
49/// title bar.
50///
51/// Everything in the title bar takes its input from below this line, so the two
52/// never compete: Windows' own top edge is four pixels, and it is reliably hittable.
53pub const TOP_BAND: f32 = 4.0;
54
55/// How far along each edge a corner claims.
56const CORNER: f32 = 16.0;
57
58/// How far in from each edge the window-resize bands reach.
59///
60/// This used to be [`crate::ui::GUTTER`], and the bands' whole claim to never stealing a click
61/// was that the panels stopped short of it. They no longer do — the panels reach the window's
62/// edges — so these four points are taken *out of* the panels, which is what every window with
63/// chrome of its own does on this platform: the frame is inside the client area or there is no
64/// frame to grab.
65///
66/// What that costs, measured rather than assumed: the listing's scrollbar is `10` points wide at
67/// the right edge, so `6` of it stays grabbable, which
68/// `the_scrollbar_survives_the_window_resize_band` holds. The other three edges give up a row's
69/// left padding, the status line's bottom, and nothing respectively. A maximised window skips
70/// the bands entirely, because there is nothing to resize.
71const RESIZE_BAND: f32 = space::S2;
72
73/// How much of a strip a tab leaves above itself, and the only air around one.
74///
75/// The same four points the window's top resize edge claims, so a tab in the title bar starts
76/// below that edge rather than racing it for the pointer — and a tab in a band of its own keeps
77/// the same shoulder, so it looks the same wherever it is.
78const TAB_TOP: f32 = TOP_BAND;
79/// A tab's height in a band of its own. In the title bar it takes the bar's height instead;
80/// either way it reaches the bottom edge, which is what makes it read as a browser tab rather
81/// than as a chip floating in a strip.
82const TAB_HEIGHT: f32 = TOOL_SIZE;
83/// Only the top corners, and only just.
84///
85/// A browser tab is a shape with a bottom edge welded to the content below it, so rounding that
86/// edge would be rounding a join that is not there.
87const TAB_CORNER: CornerRadius = CornerRadius {
88    nw: radius::SMALL,
89    ne: radius::SMALL,
90    sw: 0,
91    se: 0,
92};
93const TAB_MIN: f32 = 52.0;
94const TAB_MAX: f32 = 220.0;
95/// Two points off the top of a tab's name, and of every other name on this bar.
96///
97/// The third time this program has needed the same two points — see
98/// [`crate::ui::filelist::CELL_LIFT`] and the breadcrumb's `TEXT_LIFT`, which say it at length.
99/// A line of text centred in a box is centred on its *line box*, which reserves room under the
100/// baseline for descenders, so a name beside a 14px glyph centred on its own ink sits low. Every
101/// name in a tab is x-height and ascenders; the room being reserved is for descenders most of
102/// them do not have.
103const TEXT_LIFT: f32 = 2.0;
104/// Everything in a tab that is not its label: two 14px glyphs, the gaps around
105/// them, and the padding at each end.
106const TAB_FURNITURE: f32 = 8.0 + TOOL_ICON + space::S2 + space::S2 + TOOL_ICON + space::S2;
107
108/// A tab being dragged.
109pub struct TabDrag {
110    pub pane: PaneId,
111    pub tab: usize,
112    /// Where in the tab the pointer took hold, so the ghost does not jump.
113    pub grab_dx: f32,
114    pub title: String,
115    /// Whether the pointer has moved far enough for this to be a drag rather than
116    /// a click that has not finished yet.
117    pub live: bool,
118}
119
120/// Where a dragged tab would land.
121#[derive(Clone, Copy, PartialEq, Debug)]
122enum Drop {
123    /// Into a pane's own strip, at this position.
124    Strip { pane: PaneId, index: usize },
125    /// Onto a pane, splitting it.
126    Split { pane: PaneId, side: crate::pane::Side },
127    /// Onto a pane's centre: move the tab there.
128    Into { pane: PaneId },
129    Nowhere,
130}
131
132/// One tab's place on screen, resolved for this frame.
133pub struct Slot {
134    pub pane: PaneId,
135    pub tab: usize,
136    pub rect: Rect,
137}
138
139/// A caption button's width. The platform's are 46×32, and matching that is what makes
140/// this window read as a window.
141const CAPTION_WIDTH: f32 = 46.0;
142
143/// The title bar's own rect, given the window.
144pub fn bar_rect(screen: Rect) -> Rect {
145    Rect::from_min_size(screen.min, vec2(screen.width(), HEIGHT))
146}
147
148/// Where the title bar's content begins: past the application mark.
149pub fn content_left(bar: Rect) -> f32 {
150    bar.left() + space::S2 + TOOL_SIZE + space::S2
151}
152
153/// Where it has to stop: the left edge of the caption buttons.
154pub fn controls_left(bar: Rect) -> f32 {
155    bar.right() - CAPTION_WIDTH * 3.0
156}
157
158/// The room a row of tab strips needs when it is not in the title bar.
159pub const STRIP_ROW: f32 = TAB_HEIGHT + space::S2 * 2.0;
160
161/// Below this a strip is not worth putting in the title bar: one tab shrunk to its
162/// glyphs, plus the `+`.
163const MIN_STRIP: f32 = TAB_MIN + TOOL_SIZE + space::S2 * 2.0;
164
165/// One row of panes' tab strips, drawn on a band of its own.
166pub struct StripRow {
167    /// The band to fill. Painted like the title bar, because that is what it is.
168    pub band: Rect,
169    /// Where each pane's strip goes inside it.
170    pub strips: Vec<(PaneId, Rect)>,
171}
172
173/// Where every pane's tab strip goes.
174pub struct StripPlan {
175    /// Strips that fit in the title bar — the top row of panes.
176    pub in_bar: Vec<(PaneId, Rect)>,
177    /// A band per row of panes that could not use the title bar: every row below the
178    /// first, and the first as well when the caption buttons leave it too little room.
179    pub rows: Vec<StripRow>,
180}
181
182/// Decide where each pane's tabs go, and take the room they need out of the panes.
183///
184/// A tab belongs directly above the pane it governs, so a strip is laid out at that pane's
185/// own x-range rather than packed from the left. The top row of panes can use the title bar
186/// for that — it is the strip of window directly above them. A pane in any row below has
187/// nothing above it but another pane, so its row gets a band of its own, painted like the
188/// title bar and taking [`STRIP_ROW`] off the top of the panes in that row.
189///
190/// `panes` is adjusted in place, which is why this runs before anything is drawn.
191///
192/// The first row falls back to a band too when the caption buttons would leave one of its
193/// strips under [`MIN_STRIP`] — all of it or none of it, because a window with one pane's
194/// tabs in the bar and another's in a band below reads as a bug rather than as a rule.
195pub fn plan_strips(panes: &mut [(PaneId, Rect)], bar: Rect) -> StripPlan {
196    let mut plan = StripPlan {
197        in_bar: Vec::new(),
198        rows: Vec::new(),
199    };
200    if panes.is_empty() {
201        return plan;
202    }
203
204    // Rows, by top edge. Panes in one row share a top because that is what a row *is*
205    // in a tree of splits; rounding absorbs the half-point a ratio can land on.
206    let mut tops: Vec<i32> = panes.iter().map(|(_, r)| r.top().round() as i32).collect();
207    tops.sort_unstable();
208    tops.dedup();
209
210    let row_of = |top: i32, panes: &[(PaneId, Rect)]| -> Vec<(PaneId, Rect)> {
211        let mut row: Vec<(PaneId, Rect)> = panes
212            .iter()
213            .filter(|(_, r)| r.top().round() as i32 == top)
214            .copied()
215            .collect();
216        row.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
217        row
218    };
219
220    // ---- The top row, if the title bar can hold it ----------------------
221    let (left, right) = (content_left(bar), controls_left(bar) - space::S3);
222    let first = row_of(tops[0], panes);
223    let candidates: Vec<(PaneId, Rect)> = first
224        .iter()
225        .map(|(id, rect)| {
226            let strip = Rect::from_min_max(
227                pos2(rect.left().max(left), bar.top()),
228                pos2(rect.right().min(right), bar.bottom()),
229            );
230            (*id, strip)
231        })
232        .collect();
233    let fits = right > left
234        && candidates
235            .iter()
236            .all(|(_, strip)| strip.width() >= MIN_STRIP.min(right - left));
237
238    let banded: &[i32] = if fits {
239        plan.in_bar = candidates;
240        &tops[1..]
241    } else {
242        &tops[..]
243    };
244
245    // ---- A band for every other row -------------------------------------
246    for &top in banded {
247        let row = row_of(top, panes);
248        let (from, to) = row.iter().fold((f32::MAX, f32::MIN), |(from, to), (_, r)| {
249            (from.min(r.left()), to.max(r.right()))
250        });
251        plan.rows.push(StripRow {
252            band: Rect::from_min_max(pos2(from, top as f32), pos2(to, top as f32 + STRIP_ROW)),
253            strips: row
254                .iter()
255                .map(|(id, rect)| {
256                    (
257                        *id,
258                        Rect::from_min_max(
259                            pos2(rect.left(), top as f32),
260                            pos2(rect.right(), top as f32 + STRIP_ROW),
261                        ),
262                    )
263                })
264                .collect(),
265        });
266        // The band's room comes out of the panes under it. A pane squeezed shorter than
267        // its own strip keeps the strip — losing the tabs would lose the way out.
268        for (id, rect) in panes.iter_mut() {
269            if row.iter().any(|(row_id, _)| row_id == id) {
270                rect.min.y = (rect.top() + STRIP_ROW).min(rect.bottom());
271            }
272        }
273    }
274
275    plan
276}
277
278/// Draw the title bar. Everything it resolves goes into `out`.
279///
280/// Returns where every tab in the window ended up, which the drag resolution needs.
281#[allow(clippy::too_many_arguments)]
282pub fn title_bar(
283    ui: &mut Ui,
284    t: &Theme,
285    panes: &[Pane],
286    strips: &[(PaneId, Rect)],
287    focused: PaneId,
288    maximized: bool,
289    drag: &Option<TabDrag>,
290    icons_cache: &mut crate::shell::icons::Icons,
291    out: &mut Vec<Action>,
292) -> Vec<Slot> {
293    let bar = ui
294        .allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover())
295        .0;
296    ui.painter().rect_filled(bar, CornerRadius::ZERO, t.bg.layer);
297
298    // ---- Dragging and maximising, over the whole bar --------------------
299    //
300    // Registered *first*, so everything below claims its own pixels back: within a layer
301    // egui gives a click to the last widget that asked for it, which means the mark, the
302    // tabs, the `+` and the caption buttons all win over this without any of them having
303    // to be subtracted from it. What is left over — and it is the whole bar minus those —
304    // drags the window and maximises on a double click, which is what a title bar does.
305    //
306    // Below `TOP_BAND`, so the top resize edge and this are different pixels rather than a
307    // race between two gestures.
308    let caption = Rect::from_min_max(pos2(bar.left(), bar.top() + TOP_BAND), bar.max);
309    let caption = ui.interact(caption, Id::new("caption-drag"), Sense::click_and_drag());
310    if caption.double_clicked() {
311        out.push(Action::Window(WindowAction::ToggleMaximize));
312    } else if caption.drag_started() {
313        out.push(Action::Window(WindowAction::Drag));
314    }
315
316    // ---- The application mark, which is also the only global menu --------
317    let mut x = bar.left() + space::S2;
318    let mark = Rect::from_min_size(
319        pos2(x, (bar.center().y - TOOL_SIZE * 0.5).round()),
320        vec2(TOOL_SIZE, TOOL_SIZE),
321    );
322    let mark_response = ui.interact(mark, Id::new("app-menu"), Sense::click());
323    if mark_response.hovered() {
324        ui.painter()
325            .rect_filled(mark, CornerRadius::same(radius::SMALL), t.bg.control_hover);
326    }
327    // In `accent-default` rather than the `text-secondary` the icon brief suggests for a
328    // title bar: this is the one glyph in the window that *is* the brand, and the accent is
329    // the brand. Everywhere else an icon takes the colour of the text it sits beside.
330    crate::brand::mark(
331        ui.painter(),
332        Rect::from_center_size(mark.center(), vec2(16.0, 16.0)),
333        t.accent.default,
334    );
335    app_menu(ui, &mark_response, t.dark, out);
336    x = mark.right() + space::S2;
337
338    // ---- Window buttons, from the right ---------------------------------
339    let controls_left = window_buttons(ui, t, bar, maximized, out);
340
341    // ---- The bar's bottom edge, before the tabs -------------------------
342    //
343    // Before, so a tab with a surface of its own paints over it. That is the whole of what
344    // "a tab is welded to the content below it" means now that the panes reach the bar: the
345    // active tab and the path bar beneath it are the same colour, and a `stroke-subtle`
346    // hairline drawn across them afterwards cut the weld in half — which is precisely what it
347    // used to do, invisibly, because there were four points of canvas under it anyway.
348    //
349    // A *quiet* tab has no fill and still shows the line through it, which is right: a tab
350    // that is not the active one is not welded to anything.
351    ui.painter().line_segment(
352        [bar.left_bottom(), bar.right_bottom()],
353        Stroke::new(1.0, t.stroke.subtle),
354    );
355
356    // ---- The tabs, where the plan put them ------------------------------
357    //
358    // Each strip is at the x-range of the pane it belongs to, so a tab sits above the
359    // listing it governs. `strips` is empty when the top row of panes has its own band
360    // instead, and then the bar names the folder in front — a title bar's usual job.
361    let mut slots = Vec::new();
362    for (pane, strip) in strips {
363        if let Some(pane) = panes.iter().find(|p| p.id == *pane) {
364            slots.extend(tab_strip(
365                ui,
366                t,
367                *strip,
368                pane,
369                pane.id == focused,
370                drag,
371                icons_cache,
372                out,
373            ));
374        }
375    }
376    if strips.is_empty() {
377        let label = panes
378            .iter()
379            .find(|p| p.id == focused)
380            .map(|p| p.tab().title.clone())
381            .unwrap_or_default();
382        if !label.is_empty() {
383            let galley = truncated(
384                ui.painter(),
385                &label,
386                t.fonts.body.clone(),
387                t.text.secondary,
388                (controls_left - space::S3 - x).max(0.0),
389            );
390            crate::ui::text_left(
391                ui.painter(),
392                Rect::from_min_max(
393                    pos2(x, bar.top() - TEXT_LIFT),
394                    pos2(controls_left, bar.bottom() - TEXT_LIFT),
395                ),
396                galley,
397            );
398        }
399    }
400
401    slots
402}
403
404/// Lay out and paint one pane's tabs at `strip`, with its `+` button at the end.
405///
406/// Used both by the title bar and by a split pane's own header, so a tab behaves the
407/// same wherever it is. Returns where each one ended up, which the drag resolution needs.
408#[allow(clippy::too_many_arguments)]
409pub fn tab_strip(
410    ui: &mut Ui,
411    t: &Theme,
412    strip: Rect,
413    pane: &Pane,
414    focused: bool,
415    drag: &Option<TabDrag>,
416    icons_cache: &mut crate::shell::icons::Icons,
417    out: &mut Vec<Action>,
418) -> Vec<Slot> {
419    if strip.width() < TAB_MIN * 0.5 {
420        return Vec::new();
421    }
422
423    let natural = tab_widths(ui, t, pane);
424    let wanted: f32 = natural.iter().sum();
425    // The `+` button and the gap before it are the strip's fixed cost.
426    let available = (strip.width() - TOOL_SIZE - space::S2 * 2.0).max(0.0);
427
428    // Too many tabs: shrink them all in proportion, down to a floor where only the glyph
429    // and the close button are left. Browsers do this, and it beats a scrolling strip
430    // because every tab stays clickable.
431    let scale = if wanted > available && wanted > 0.0 {
432        (available / wanted).max(0.0)
433    } else {
434        1.0
435    };
436
437    let mut slots = Vec::with_capacity(natural.len());
438    // Which tabs painted no surface of their own, for the dividers below.
439    let mut bare = Vec::with_capacity(natural.len());
440    let mut x = strip.left();
441
442    for (index, tab) in pane.tabs.iter().enumerate() {
443        let want = natural[index];
444        // The floor is the *lesser* of the minimum and the natural width, so a tab whose
445        // title is already short keeps its size rather than being padded out.
446        let width = (want * scale).max(TAB_MIN.min(want));
447        // Down to the bottom edge of the strip, and hard against its neighbour: the two
448        // things that make a row of tabs read as a browser's rather than as a row of chips.
449        let rect = Rect::from_min_max(
450            pos2(x.round(), (strip.top() + TAB_TOP).round()),
451            pos2((x + width).round(), strip.bottom().round()),
452        );
453        x += width;
454        if rect.right() > strip.right() {
455            // No room left; only reachable at absurd tab counts in a narrow pane.
456            break;
457        }
458
459        let active = pane.active == index;
460        let being_dragged = drag
461            .as_ref()
462            .is_some_and(|d| d.live && d.pane == pane.id && d.tab == index);
463        // Windows' icon for the folder this tab is on — the Downloads arrow, the machine
464        // for This PC, a drive's own icon — with the painted glyph until it arrives. Tabs
465        // come in tens, so a lookup per path is affordable here in a way it would not be
466        // in a listing.
467        let shell = icons_cache
468            .place(&tab.path)
469            .and_then(|icon| icons_cache.uv(ui.ctx(), icon));
470        let filled = paint_tab(
471            ui,
472            t,
473            rect,
474            &tab.title,
475            tab.path.as_os_str().is_empty(),
476            tab.dir.is_none(),
477            active,
478            focused,
479            being_dragged,
480            shell,
481            pane.id,
482            index,
483            out,
484        );
485        slots.push(Slot {
486            pane: pane.id,
487            tab: index,
488            rect,
489        });
490        bare.push(!filled);
491    }
492
493    // A hairline between two tabs that both have no surface of their own. Tabs touch now, so
494    // without this a run of quiet ones reads as a single long row of labels. Where either
495    // neighbour is filled its own edge does the job, and a line there would be noise.
496    for index in 1..slots.len() {
497        if bare[index - 1] && bare[index] {
498            let x = slots[index].rect.left().round() - 0.5;
499            let inset = space::S2;
500            ui.painter().line_segment(
501                [
502                    pos2(x, slots[index].rect.top() + inset),
503                    pos2(x, slots[index].rect.bottom() - inset),
504                ],
505                Stroke::new(1.0, t.stroke.subtle),
506            );
507        }
508    }
509
510    // The `+` button: a new tab on the same folder this pane is showing.
511    let plus = Rect::from_min_size(
512        pos2(
513            (x + space::S2).round(),
514            // Centred on the tabs, not on the strip, now that the tabs no longer are.
515            ((strip.top() + TAB_TOP + strip.bottom()) * 0.5 - TOOL_SIZE * 0.5).round(),
516        ),
517        vec2(TOOL_SIZE, TOOL_SIZE),
518    );
519    if plus.right() <= strip.right()
520        && crate::ui::tool_button(
521            ui,
522            t,
523            plus,
524            Id::new(("new-tab", pane.id)),
525            &azur_icons::plus,
526            "New tab in this folder",
527            true,
528            false,
529            // The strip it sits in, whether that is the title bar or a band of its own.
530            t.bg.layer,
531        )
532        .clicked()
533    {
534        out.push(Action::NewTab { pane: pane.id });
535    }
536
537    slots
538}
539
540/// What each of a pane's tabs would like to be, before anything has to give.
541fn tab_widths(ui: &Ui, t: &Theme, pane: &Pane) -> Vec<f32> {
542    pane.tabs
543        .iter()
544        .map(|tab| {
545            let label = ui.painter().layout_no_wrap(
546                tab.title.clone(),
547                t.fonts.body.clone(),
548                Color32::PLACEHOLDER,
549            );
550            (label.size().x + TAB_FURNITURE).clamp(TAB_MIN, TAB_MAX)
551        })
552        .collect()
553}
554
555/// One tab.
556#[allow(clippy::too_many_arguments)]
557fn paint_tab(
558    ui: &mut Ui,
559    t: &Theme,
560    rect: Rect,
561    title: &str,
562    is_this_pc: bool,
563    loading: bool,
564    active: bool,
565    pane_focused: bool,
566    being_dragged: bool,
567    shell: Option<(egui::TextureId, Rect)>,
568    pane: PaneId,
569    index: usize,
570    out: &mut Vec<Action>,
571) -> bool {
572    let response = ui.interact(
573        rect,
574        Id::new(("tab", pane, index)),
575        Sense::click_and_drag(),
576    );
577
578    let corner = TAB_CORNER;
579    // Whether this tab has a surface of its own, which is what decides whether it needs a
580    // divider from its neighbour: touching tabs with no fill between them would read as one
581    // long row of labels.
582    let mut filled = true;
583    // The strip's whole ladder is pinned to [`crate::ui::seam`] rather than to Azur's control
584    // steps, and moved with it when it did. Four fills, in order of how much they claim:
585    // nothing, `background-layer-alt` (a hover, or an unfocused pane's active tab), then the seam
586    // itself (a press, or the focused pane's active tab). A press taking the *selected* colour is
587    // deliberate — it is about to become the selection.
588    //
589    // Reading them off `control-hover` / `control-active` was right while the bar was
590    // `control-active` too, and became an inversion the moment it was not: a hovered inactive tab
591    // came out brighter than the selected one, which reads as though the pointer had selected it.
592    //
593    // A tab that is being dragged stays in place as a ghost outline, so the strip
594    // does not reflow under the pointer mid-gesture.
595    if being_dragged {
596        ui.painter().rect_stroke(
597            rect,
598            corner,
599            Stroke::new(1.0, t.stroke.subtle),
600            StrokeKind::Inside,
601        );
602    } else if active {
603        // Grey, and the only thing that says which pane the keyboard is in: a step further from
604        // the strip for the focused pane's active tab than for an unfocused one's. That signal
605        // used to be a 2px accent line along the bottom edge, which is gone.
606        //
607        // The focused pane's colour comes from `crate::ui::seam`, which is also the path bar
608        // below it and the lines between the panels — this tab is the visible end of that
609        // surface, so the two cannot be allowed to drift apart.
610        //
611        // And it reaches one point past its own rect, to paint over the strip's bottom hairline.
612        // That line marks where the title bar or the band ends, which is worth marking
613        // everywhere except across a weld: the focused pane's tab and its path bar are one
614        // surface, and a `stroke-subtle` line drawn through them cut it in half. An unfocused
615        // pane's tab is a different grey from the bar below it and keeps the edge, which is
616        // right — it is not welded to a surface it does not match.
617        ui.painter().rect_filled(
618            if pane_focused {
619                Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y + 1.0))
620            } else {
621                rect
622            },
623            corner,
624            if pane_focused {
625                crate::ui::seam(t)
626            } else {
627                t.bg.layer_alt
628            },
629        );
630    } else if response.is_pointer_button_down_on() {
631        ui.painter().rect_filled(rect, corner, crate::ui::seam(t));
632    } else if response.hovered() {
633        ui.painter().rect_filled(rect, corner, t.bg.layer_alt);
634    } else {
635        filled = false;
636    }
637
638    // `text-primary` only for the tab that is actually current — the active tab of the pane the
639    // keyboard is in — and for whatever the pointer is over.
640    //
641    // An unfocused pane's active tab used to get it too, back when its fill was three steps off
642    // the strip and could carry the distinction alone. It is one step now that the whole ladder
643    // has come down with [`crate::ui::seam`], and one step at the bottom of the dark ramp is
644    // `GRAY_2` to `GRAY_3` — barely a change. So the label says it as well, which is the more
645    // legible of the two channels anyway.
646    let text_color = if being_dragged {
647        t.text.disabled
648    } else if (active && pane_focused) || response.hovered() {
649        t.text.primary
650    } else {
651        t.text.secondary
652    };
653
654    // The glyph: a folder, or the machine for This PC. Dimmed while the listing is
655    // still on its way, which is the only loading indicator a fast scan has time
656    // to show.
657    let glyph_x = rect.left() + 8.0;
658    let box_rect = icon_rect(rect, glyph_x, TOOL_ICON);
659    match shell {
660        Some((texture, uv)) => {
661            ui.painter().image(
662                texture,
663                box_rect,
664                uv,
665                // Faded while the listing is still on its way, which is the only loading
666                // indicator a fast scan has time to show.
667                if loading {
668                    Color32::from_white_alpha(110)
669                } else {
670                    Color32::WHITE
671                },
672            );
673        }
674        None => {
675            let glyph: azur_icons::Icon<'_> = if is_this_pc {
676                &icons::this_pc
677            } else if active {
678                &icons::folder_open
679            } else {
680                &icons::folder
681            };
682            glyph(
683                ui.painter(),
684                box_rect,
685                if loading {
686                    t.text.disabled
687                } else if is_this_pc {
688                    text_color
689                } else {
690                    t.folder
691                },
692            );
693        }
694    };
695
696    // The close button, which only appears when there is room for it.
697    let mut text_right = rect.right() - space::S2;
698    let close_room = rect.width() > TAB_MIN + 8.0;
699    let show_close = close_room && (active || response.hovered());
700    if close_room {
701        let close = Rect::from_min_size(
702            pos2(
703                rect.right() - space::S2 - TOOL_ICON,
704                (rect.center().y - TOOL_ICON * 0.5).round(),
705            ),
706            vec2(TOOL_ICON, TOOL_ICON),
707        );
708        text_right = close.left() - space::S2;
709        if show_close {
710            let close_response =
711                ui.interact(close, Id::new(("tab-close", pane, index)), Sense::click());
712            if close_response.hovered() {
713                ui.painter().rect_filled(
714                    close.expand(2.0),
715                    CornerRadius::same(radius::SMALL),
716                    t.bg.control_active,
717                );
718            }
719            azur_icons::close(
720                ui.painter(),
721                close,
722                if close_response.hovered() {
723                    t.status.danger
724                } else {
725                    t.text.tertiary
726                },
727            );
728            if close_response.clicked() {
729                out.push(Action::CloseTab { pane, tab: index });
730            }
731        }
732    }
733
734    let label_left = glyph_x + TOOL_ICON + space::S2;
735    if text_right > label_left {
736        let galley = truncated(
737            ui.painter(),
738            title,
739            t.fonts.body.clone(),
740            text_color,
741            text_right - label_left,
742        );
743        crate::ui::text_left(
744            ui.painter(),
745            Rect::from_min_max(
746                pos2(label_left, rect.top() - TEXT_LIFT),
747                pos2(text_right, rect.bottom() - TEXT_LIFT),
748            ),
749            galley,
750        );
751    }
752
753    // Clicks. A middle click closes, as everywhere else that has tabs.
754    if response.clicked() {
755        out.push(Action::ActivateTab { pane, tab: index });
756    }
757    if response.middle_clicked() {
758        out.push(Action::CloseTab { pane, tab: index });
759    }
760    // A drag starts here and is picked up next frame, which is also when the
761    // pointer has moved far enough for it to be a drag at all.
762    if response.drag_started() {
763        let origin = ui
764            .input(|i| i.pointer.press_origin())
765            .unwrap_or(rect.left_center());
766        out.push(Action::BeginTabDrag {
767            pane,
768            tab: index,
769            grab_dx: origin.x - rect.left(),
770        });
771    }
772    filled
773}
774
775/// The application menu: the handful of settings that belong to the window rather
776/// than to a pane.
777fn app_menu(ui: &mut Ui, trigger: &egui::Response, dark: bool, out: &mut Vec<Action>) {
778    use azur_egui_theme::components::{Menu, MenuItem};
779
780    Menu::new(trigger).min_width(220.0).show(ui.ctx(), |ui| {
781        if ui
782            .add(
783                MenuItem::new("Dark theme")
784                    .selected(dark)
785                    .icon(&azur_icons::dot),
786            )
787            .clicked()
788        {
789            out.push(Action::SetTheme { dark: true });
790        }
791        if ui
792            .add(
793                MenuItem::new("Light theme")
794                    .selected(!dark)
795                    .icon(&azur_icons::dot),
796            )
797            .clicked()
798        {
799            out.push(Action::SetTheme { dark: false });
800        }
801        azur_egui_theme::components::menu_divider(ui);
802        if ui
803            .add(MenuItem::new("New tab").shortcut("Ctrl+T").icon(&azur_icons::plus))
804            .clicked()
805        {
806            out.push(Action::NewTabFocused);
807        }
808        if ui
809            .add(
810                MenuItem::new("Split to the right")
811                    .shortcut("Ctrl+\\")
812                    .icon(&icons::split_side),
813            )
814            .clicked()
815        {
816            out.push(Action::SplitFocused {
817                side: crate::pane::Side::Right,
818            });
819        }
820        if ui
821            .add(MenuItem::new("Split below").icon(&icons::split_down))
822            .clicked()
823        {
824            out.push(Action::SplitFocused {
825                side: crate::pane::Side::Bottom,
826            });
827        }
828        azur_egui_theme::components::menu_divider(ui);
829        if ui
830            .add(
831                MenuItem::new("Reset window size").icon(&azur_icons::window_restore),
832            )
833            .clicked()
834        {
835            out.push(Action::Window(WindowAction::ResetSize));
836        }
837        if ui
838            .add(MenuItem::new("Close window").shortcut("Alt+F4").danger(true))
839            .clicked()
840        {
841            out.push(Action::Window(WindowAction::Close));
842        }
843    });
844}
845
846/// The minimise / maximise / close buttons. Returns their left edge, which is
847/// where the tab strip has to stop.
848fn window_buttons(
849    ui: &mut Ui,
850    t: &Theme,
851    bar: Rect,
852    maximized: bool,
853    out: &mut Vec<Action>,
854) -> f32 {
855    // The platform's own caption buttons are 46×32 with a 10px hairline glyph, and
856    // matching that is what makes this window read as a window rather than as an
857    // application pretending to be one. Straight out of Azur's `TitleBar`.
858    const WIDTH: f32 = CAPTION_WIDTH;
859    const GLYPH: f32 = 10.0;
860
861    let buttons: [(WindowAction, azur_icons::Icon<'_>, bool); 3] = [
862        (WindowAction::Close, &azur_icons::window_close, true),
863        (
864            WindowAction::ToggleMaximize,
865            if maximized {
866                &azur_icons::window_restore
867            } else {
868                &azur_icons::window_maximize
869            },
870            false,
871        ),
872        (WindowAction::Minimize, &azur_icons::window_minimize, false),
873    ];
874
875    let mut left = bar.right();
876    for (which, glyph, danger) in buttons {
877        left -= WIDTH;
878        let rect = Rect::from_min_size(pos2(left.round(), bar.top()), vec2(WIDTH, bar.height()));
879        // The fill covers the whole button so it reads as one block; only the input
880        // rect steps below the resize band.
881        let hit = Rect::from_min_max(pos2(rect.left(), rect.top() + TOP_BAND), rect.max);
882        let response = ui.interact(hit, Id::new(("caption", which as u8)), Sense::click());
883
884        let (fill, color) = if response.hovered() {
885            if danger {
886                (Some(t.status.danger), t.text.on_accent)
887            } else {
888                (Some(t.bg.control_hover), t.text.primary)
889            }
890        } else {
891            (None, t.text.secondary)
892        };
893        if let Some(fill) = fill {
894            ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
895        }
896        glyph(
897            ui.painter(),
898            Rect::from_center_size(rect.center(), vec2(GLYPH, GLYPH)),
899            color,
900        );
901        if response.clicked() {
902            out.push(Action::Window(which));
903        }
904    }
905    left
906}
907
908// ---------------------------------------------------------------------------
909// The drag
910// ---------------------------------------------------------------------------
911
912/// Work out where the dragged tab would land, show it, and act on the release.
913///
914/// Called after the panes have been drawn, because the slots can now be anywhere: in the
915/// title bar with one pane, or inside each pane's own header when split.
916pub fn resolve_drag(
917    ui: &mut Ui,
918    t: &Theme,
919    panes: &[Pane],
920    slots: &[Slot],
921    drag: &mut Option<TabDrag>,
922    out: &mut Vec<Action>,
923) {
924    let Some(state) = drag.as_mut() else { return };
925    let ctx = ui.ctx();
926
927    let pointer = ctx.pointer_interact_pos();
928    let held = ctx.input(|i| i.pointer.any_down());
929    let Some(pointer) = pointer else {
930        *drag = None;
931        return;
932    };
933
934    // A few pixels of slop, so a slightly unsteady click is still a click.
935    if !state.live {
936        let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(pointer);
937        if pointer.distance(origin) < 6.0 {
938            if !held {
939                *drag = None;
940            }
941            return;
942        }
943        state.live = true;
944    }
945
946    let target = drop_target(pointer, slots, panes, state);
947
948    // The preview goes in a foreground layer: the panes are drawn after this
949    // function runs, and a layer is ordered by rank rather than by when it was
950    // added to.
951    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("yafe-dock-hint")));
952    match target {
953        Drop::Split { pane, side } => {
954            if let Some(rect) = panes.iter().find(|p| p.id == pane).map(|p| p.rect) {
955                crate::ui::drop_preview(&painter, dock::preview_rect(rect, Zone::Split(side)), t);
956            }
957        }
958        Drop::Into { pane } => {
959            if let Some(rect) = panes.iter().find(|p| p.id == pane).map(|p| p.rect) {
960                crate::ui::drop_preview(&painter, rect, t);
961            }
962        }
963        Drop::Strip { pane, index } => {
964            // A caret between the tabs, where the tab would be inserted.
965            let (x, band) = caret_at(slots, pane, index);
966            painter.rect_filled(
967                Rect::from_min_size(pos2(x - 1.0, band.top()), vec2(2.0, band.height())),
968                CornerRadius::same(radius::CIRCULAR),
969                t.accent.default,
970            );
971        }
972        Drop::Nowhere => {}
973    }
974
975    // The ghost, following the pointer.
976    ghost(&painter, t, pointer, state);
977    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
978
979    if !held {
980        let from = (state.pane, state.tab);
981        match target {
982            Drop::Strip { pane, index } => out.push(Action::MoveTab {
983                from: from.0,
984                tab: from.1,
985                to: pane,
986                index,
987            }),
988            Drop::Into { pane } => out.push(Action::MoveTab {
989                from: from.0,
990                tab: from.1,
991                to: pane,
992                index: usize::MAX,
993            }),
994            Drop::Split { pane, side } => out.push(Action::SplitTab {
995                from: from.0,
996                tab: from.1,
997                target: pane,
998                side,
999            }),
1000            Drop::Nowhere => {}
1001        }
1002        *drag = None;
1003    }
1004}
1005
1006/// Which drop the pointer is currently over.
1007fn drop_target(pointer: Pos2, slots: &[Slot], panes: &[Pane], state: &TabDrag) -> Drop {
1008    // A strip wins over the pane behind it, so reordering never needs the pointer to
1009    // avoid the listing. Which pane's strip is worked out from the slots themselves, since
1010    // the bar holds one group per pane and the nearest tab decides which group.
1011    let over_strip = slots
1012        .iter()
1013        .filter(|s| {
1014            let band = s.rect.expand2(vec2(space::S3, space::S2));
1015            pointer.y >= band.top() && pointer.y <= band.bottom()
1016        })
1017        .min_by(|a, b| {
1018            let distance = |s: &Slot| (pointer.x - s.rect.center().x).abs();
1019            distance(a).total_cmp(&distance(b))
1020        })
1021        .map(|s| s.pane);
1022
1023    if let Some(pane) = over_strip {
1024
1025        // Insert before the first tab of that strip whose midpoint is past the pointer.
1026        let mut index = 0;
1027        for slot in slots.iter().filter(|s| s.pane == pane) {
1028            if pointer.x < slot.rect.center().x {
1029                break;
1030            }
1031            index = slot.tab + 1;
1032        }
1033
1034        // Dropping a tab back where it already is means nothing.
1035        if pane == state.pane && (index == state.tab || index == state.tab + 1) {
1036            return Drop::Nowhere;
1037        }
1038        return Drop::Strip { pane, index };
1039    }
1040
1041    // Over the panes: the edges split, the middle moves.
1042    for pane in panes {
1043        if pane.rect.contains(pointer) {
1044            return match dock::zone_at(pane.rect, pointer) {
1045                Zone::Split(side) => {
1046                    // Splitting a single-tab pane away from itself would leave an
1047                    // empty pane behind, so that gesture is a no-op.
1048                    let only_tab = panes
1049                        .iter()
1050                        .find(|p| p.id == state.pane)
1051                        .is_some_and(|p| p.tabs.len() == 1);
1052                    if pane.id == state.pane && only_tab {
1053                        Drop::Nowhere
1054                    } else {
1055                        Drop::Split { pane: pane.id, side }
1056                    }
1057                }
1058                Zone::Into if pane.id == state.pane => Drop::Nowhere,
1059                Zone::Into => Drop::Into { pane: pane.id },
1060            };
1061        }
1062    }
1063    Drop::Nowhere
1064}
1065
1066/// Where the insertion caret goes for a strip drop, and how tall to draw it.
1067fn caret_at(slots: &[Slot], pane: PaneId, index: usize) -> (f32, Rect) {
1068    let group: Vec<&Slot> = slots.iter().filter(|s| s.pane == pane).collect();
1069    let band = group
1070        .first()
1071        .map(|s| s.rect.expand2(vec2(0.0, 2.0)))
1072        .unwrap_or(Rect::NOTHING);
1073    let x = match group.get(index) {
1074        Some(slot) => slot.rect.left() - space::S2 * 0.5,
1075        None => group
1076            .last()
1077            .map(|s| s.rect.right() + space::S2 * 0.5)
1078            .unwrap_or(band.left()),
1079    };
1080    (x, band)
1081}
1082
1083/// The tab that follows the pointer.
1084fn ghost(painter: &egui::Painter, t: &Theme, pointer: Pos2, state: &TabDrag) {
1085    let width = 168.0;
1086    let rect = Rect::from_min_size(
1087        pos2(
1088            (pointer.x - state.grab_dx.min(width - 24.0)).round(),
1089            (pointer.y - TAB_HEIGHT * 0.5).round(),
1090        ),
1091        vec2(width, TAB_HEIGHT),
1092    );
1093    let corner = CornerRadius::same(radius::SMALL);
1094    painter.rect_filled(rect, corner, t.bg.layer_alt);
1095    painter.rect_stroke(rect, corner, Stroke::new(1.0, t.accent.default), StrokeKind::Inside);
1096    icons::folder(
1097        painter,
1098        icon_rect(rect, rect.left() + 8.0, TOOL_ICON),
1099        t.folder,
1100    );
1101    let label_left = rect.left() + 8.0 + TOOL_ICON + space::S2;
1102    let galley = truncated(
1103        painter,
1104        &state.title,
1105        t.fonts.body.clone(),
1106        t.text.primary,
1107        rect.right() - space::S2 - label_left,
1108    );
1109    crate::ui::text_left(
1110        painter,
1111        Rect::from_min_max(
1112            pos2(label_left, rect.top() - TEXT_LIFT),
1113            pos2(rect.right(), rect.bottom() - TEXT_LIFT),
1114        ),
1115        galley,
1116    );
1117}
1118
1119// ---------------------------------------------------------------------------
1120// Resize borders
1121// ---------------------------------------------------------------------------
1122
1123/// The eight grab bands a decorated window would have got from the platform.
1124///
1125/// # Why not an `Area`
1126///
1127/// The obvious implementation — one foreground [`egui::Area`] per band, positioned
1128/// with `fixed_pos` and filled with `allocate_rect` — is silently, spectacularly
1129/// wrong, and it is worth recording why.
1130///
1131/// An `Area` lays its content out relative to its own origin and then clamps that
1132/// origin so the area fits on screen. Handing it an *absolute* rect means the content
1133/// size is measured from wherever the origin currently is to the far edge of the
1134/// rect; the clamp then moves the origin to fit that size; which makes the content
1135/// measure larger still. The two chase each other and settle at **half the window**.
1136/// The east band, asked for six pixels down the right-hand edge, came out as
1137/// `[600, 16]-[1200, 784]` — silently swallowing every click in the right half of
1138/// the window, with nothing in the source to suggest it.
1139///
1140/// So the bands are plain [`egui::Ui::interact`] calls on the root `Ui`, whose
1141/// coordinate space *is* screen space. No layout, no clamping, nothing to feed back.
1142///
1143/// # Why they never overlap anything
1144///
1145/// Registered last, so they win any contest. The top band is [`TOP_BAND`] tall and the title
1146/// bar's own controls all start below it, so up there is still no contest to win — a grab band
1147/// that has to fight the close button for a click is a grab band that will one day win.
1148///
1149/// The other three do now overlap the panels, since the panels reach the window's edges.
1150/// [`RESIZE_BAND`] says what that costs and why it is the lesser evil.
1151pub fn resize_borders(ui: &mut Ui, maximized: bool) {
1152    use egui::viewport::ResizeDirection as Dir;
1153    use egui::ViewportCommand as Cmd;
1154
1155    // A maximised window cannot be resized, and offering the cursor for it is a lie.
1156    if maximized {
1157        return;
1158    }
1159
1160    let screen = ui.ctx().viewport_rect();
1161    // A window this small has no margin to put them in; the platform's minimum size
1162    // keeps this out of reach in practice.
1163    if screen.width() < CORNER * 4.0 || screen.height() < CORNER * 4.0 {
1164        return;
1165    }
1166
1167    let band = RESIZE_BAND;
1168    let (left, right) = (screen.left(), screen.right());
1169    let (top, bottom) = (screen.top(), screen.bottom());
1170
1171    let bands: [(&str, Rect, Dir, egui::CursorIcon); 8] = [
1172        (
1173            "n",
1174            Rect::from_min_max(pos2(left + CORNER, top), pos2(right - CORNER, top + TOP_BAND)),
1175            Dir::North,
1176            egui::CursorIcon::ResizeNorth,
1177        ),
1178        (
1179            "s",
1180            Rect::from_min_max(pos2(left + CORNER, bottom - band), pos2(right - CORNER, bottom)),
1181            Dir::South,
1182            egui::CursorIcon::ResizeSouth,
1183        ),
1184        (
1185            "w",
1186            Rect::from_min_max(pos2(left, top + CORNER), pos2(left + band, bottom - CORNER)),
1187            Dir::West,
1188            egui::CursorIcon::ResizeWest,
1189        ),
1190        (
1191            "e",
1192            Rect::from_min_max(pos2(right - band, top + CORNER), pos2(right, bottom - CORNER)),
1193            Dir::East,
1194            egui::CursorIcon::ResizeEast,
1195        ),
1196        // The top corners are only as tall as the top band, so they cannot reach the
1197        // application mark or the window buttons. The bottom two sit in open canvas
1198        // and get the full square.
1199        (
1200            "nw",
1201            Rect::from_min_max(pos2(left, top), pos2(left + CORNER, top + TOP_BAND)),
1202            Dir::NorthWest,
1203            egui::CursorIcon::ResizeNorthWest,
1204        ),
1205        (
1206            "ne",
1207            Rect::from_min_max(pos2(right - CORNER, top), pos2(right, top + TOP_BAND)),
1208            Dir::NorthEast,
1209            egui::CursorIcon::ResizeNorthEast,
1210        ),
1211        (
1212            "sw",
1213            Rect::from_min_max(pos2(left, bottom - CORNER), pos2(left + CORNER, bottom)),
1214            Dir::SouthWest,
1215            egui::CursorIcon::ResizeSouthWest,
1216        ),
1217        (
1218            "se",
1219            Rect::from_min_max(pos2(right - CORNER, bottom - CORNER), screen.max),
1220            Dir::SouthEast,
1221            egui::CursorIcon::ResizeSouthEast,
1222        ),
1223    ];
1224
1225    for (name, rect, direction, cursor) in bands {
1226        let response = ui.interact(rect, Id::new(("yafe-resize", name)), Sense::drag());
1227        if response.hovered() || response.dragged() {
1228            ui.ctx().set_cursor_icon(cursor);
1229        }
1230        if response.drag_started() {
1231            ui.ctx().send_viewport_cmd(Cmd::BeginResize(direction));
1232        }
1233    }
1234}
1235
1236#[cfg(test)]
1237mod tests {
1238    use super::*;
1239
1240    /// A window 1000 wide, and the panes area a sidebar leaves of it.
1241    fn window() -> (Rect, f32, f32) {
1242        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
1243        let bar = bar_rect(screen);
1244        (bar, 250.0, 996.0)
1245    }
1246
1247    #[test]
1248    fn one_pane_puts_its_tabs_in_the_title_bar_above_itself() {
1249        let (bar, left, right) = window();
1250        let mut panes = vec![(1u32, Rect::from_min_max(pos2(left, 40.0), pos2(right, 690.0)))];
1251        let before = panes[0].1;
1252
1253        let plan = plan_strips(&mut panes, bar);
1254        assert!(plan.rows.is_empty(), "one row needs no band");
1255        assert_eq!(plan.in_bar.len(), 1);
1256        let (id, strip) = plan.in_bar[0];
1257        assert_eq!(id, 1);
1258        assert_eq!(strip.left(), left, "the strip starts where the pane does");
1259        assert!(strip.right() <= controls_left(bar), "and clears the buttons");
1260        assert_eq!(panes[0].1, before, "nothing was taken off the pane");
1261    }
1262
1263    #[test]
1264    fn panes_side_by_side_each_get_a_strip_over_themselves() {
1265        let (bar, left, right) = window();
1266        let middle = 620.0;
1267        let mut panes = vec![
1268            (1u32, Rect::from_min_max(pos2(left, 40.0), pos2(middle, 690.0))),
1269            (2u32, Rect::from_min_max(pos2(middle + 4.0, 40.0), pos2(right, 690.0))),
1270        ];
1271
1272        let plan = plan_strips(&mut panes, bar);
1273        assert!(plan.rows.is_empty());
1274        assert_eq!(plan.in_bar.len(), 2);
1275        assert_eq!(plan.in_bar[0].1.left(), left);
1276        assert_eq!(plan.in_bar[1].1.left(), middle + 4.0);
1277        assert!(
1278            plan.in_bar[0].1.right() <= plan.in_bar[1].1.left(),
1279            "a strip never reaches over its neighbour's pane"
1280        );
1281    }
1282
1283    #[test]
1284    fn a_pane_in_a_row_below_gets_a_band_of_its_own() {
1285        let (bar, left, right) = window();
1286        let split = 370.0;
1287        let mut panes = vec![
1288            (1u32, Rect::from_min_max(pos2(left, 40.0), pos2(right, split))),
1289            (2u32, Rect::from_min_max(pos2(left, split + 4.0), pos2(right, 690.0))),
1290        ];
1291
1292        let plan = plan_strips(&mut panes, bar);
1293        assert_eq!(plan.in_bar.len(), 1, "the top row still uses the title bar");
1294        assert_eq!(plan.in_bar[0].0, 1);
1295        assert_eq!(plan.rows.len(), 1, "the row below gets a band");
1296
1297        let row = &plan.rows[0];
1298        assert_eq!(row.band.top(), split + 4.0, "the band is where the pane was");
1299        assert_eq!(row.band.height(), STRIP_ROW);
1300        assert_eq!(row.strips.len(), 1);
1301        assert_eq!(row.strips[0].0, 2);
1302        assert_eq!(
1303            panes[1].1.top(),
1304            split + 4.0 + STRIP_ROW,
1305            "and the room came out of the pane under it"
1306        );
1307        assert_eq!(panes[0].1.top(), 40.0, "the top pane is untouched");
1308    }
1309
1310    #[test]
1311    fn a_band_covers_only_the_panes_in_its_own_row() {
1312        // A left pane the full height, and the right half split in two: the lower right
1313        // pane's band must not reach across the pane beside it.
1314        let (bar, left, right) = window();
1315        let middle = 620.0;
1316        let split = 370.0;
1317        let mut panes = vec![
1318            (1u32, Rect::from_min_max(pos2(left, 40.0), pos2(middle, 690.0))),
1319            (2u32, Rect::from_min_max(pos2(middle + 4.0, 40.0), pos2(right, split))),
1320            (
1321                3u32,
1322                Rect::from_min_max(pos2(middle + 4.0, split + 4.0), pos2(right, 690.0)),
1323            ),
1324        ];
1325
1326        let plan = plan_strips(&mut panes, bar);
1327        assert_eq!(plan.in_bar.len(), 2, "both top-row panes are in the bar");
1328        assert_eq!(plan.rows.len(), 1);
1329        let row = &plan.rows[0];
1330        assert_eq!(row.band.left(), middle + 4.0, "the band starts at its pane");
1331        assert_eq!(row.band.right(), right);
1332        assert_eq!(panes[0].1.top(), 40.0, "the full-height pane keeps its top");
1333        assert_eq!(panes[1].1.top(), 40.0);
1334        assert_eq!(panes[2].1.top(), split + 4.0 + STRIP_ROW);
1335    }
1336
1337    #[test]
1338    fn a_row_the_caption_buttons_would_squeeze_moves_to_a_band() {
1339        // Four panes across a narrow window: the rightmost has almost no title bar to
1340        // itself, so the whole row moves down rather than one pane being treated
1341        // differently from the rest.
1342        let screen = Rect::from_min_size(Pos2::ZERO, vec2(720.0, 500.0));
1343        let bar = bar_rect(screen);
1344        let mut panes: Vec<(PaneId, Rect)> = (0..4)
1345            .map(|i| {
1346                let x = 150.0 + i as f32 * 140.0;
1347                (i as PaneId + 1, Rect::from_min_max(pos2(x, 40.0), pos2(x + 136.0, 490.0)))
1348            })
1349            .collect();
1350
1351        let plan = plan_strips(&mut panes, bar);
1352        assert!(
1353            plan.in_bar.is_empty(),
1354            "no strip goes in the bar when one of them would not fit"
1355        );
1356        assert_eq!(plan.rows.len(), 1, "the row gets a band instead");
1357        assert_eq!(plan.rows[0].strips.len(), 4, "and every pane is on it");
1358        for (_, rect) in &panes {
1359            assert_eq!(rect.top(), 40.0 + STRIP_ROW);
1360        }
1361    }
1362}
