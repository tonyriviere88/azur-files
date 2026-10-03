1//! The details view: a sticky header over virtualised rows.
2//!
3//! This is the hot loop, and it is built so that the cost of a frame depends on
4//! the size of the *window*, never on the size of the folder:
5//!
6//! - **Only visible rows exist.** The scroll area is told the total height and asked
7//!   for the visible range; a folder of 300,000 files draws the same forty rows as
8//!   a folder of forty.
9//! - **One widget for the whole list.** Not one per row: a single `interact` over
10//!   the viewport, with the row derived from the pointer's `y`. Forty widget ids
11//!   per frame per pane would otherwise be registered, hit-tested and animated for
12//!   nothing.
13//! - **No allocation per cell.** Sizes, dates and type names are written into one
14//!   `String` that is cleared and reused, and egui's galley cache is keyed on the
15//!   finished text — so a row that has not changed is a cache lookup, not a
16//!   shaping pass.
17//! - **Columns measured once.** Modified is a fixed-width format, Size is measured
18//!   from the single longest value, and Type from the handful of distinct labels in
19//!   the folder. None of it is re-measured while scrolling.
20//!
21//! The Name column takes what is left, and the other three are sized to their
22//! content — and draggable, because a measurement is a starting point and not a
23//! decision.
24
25use azur_egui_theme::icons as azur_icons;
26use azur_egui_theme::tokens::{space, typography};
27use egui::{pos2, vec2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};
28
29use crate::app::Action;
30use crate::fs::time::LocalZone;
31use crate::fs::{fmt, Column};
32use crate::icons;
33use crate::pane::{PaneId, Tab, ROW_HEIGHT};
34// `crate::icons` is this program's painted glyphs; this is the shell's icon service.
35use crate::shell::icons as shell_icons;
36use crate::shell::icons::Icons;
37use crate::shell::links;
38use crate::theme::Theme;
39use crate::ui::{
40    icon_rect, row_fill, selection_bar, text_center, text_left, text_right, truncated,
41};
42
43/// The header strip: a body line with `space-2` above and below, which is Azur's
44/// table header at this density.
45pub const HEADER_HEIGHT: f32 = typography::LINE_BODY + space::S2 * 2.0;
46/// The status line at the bottom of a pane.
47const STATUS_HEIGHT: f32 = 22.0;
48/// The glyph in a row.
49///
50/// 16, which is the size the shell small image list is drawn at — so a row does not
51/// reflow when a painted fallback glyph is replaced by the real icon a frame later.
52const GLYPH: f32 = 16.0;
53/// Space either side of a cell's text.
54const CELL_PAD: f32 = space::S3;
55/// Two points off the top of every cell's text in a row.
56///
57/// Text centred in a box is centred on its *line box*, which reserves room under the
58/// baseline for descenders — so a line of mostly-x-height text put beside a 16px icon
59/// centred on its own ink reads two points low. Which it did: a guide drawn through the
60/// icon's centre passed above the x-height of every name in the listing.
61///
62/// Applied to all four cells rather than to the name alone, or the columns of one row would
63/// no longer sit on the same line as each other. The icon stays where it is — it is the
64/// thing the text is being brought level with.
65pub const CELL_LIFT: f32 = 2.0;
66/// How close to a column edge counts as grabbing it.
67const GRIP: f32 = 4.0;
68/// Rows of nothing under the last file.
69///
70/// So that **the folder is always somewhere to right-click.** The menu for the folder itself —
71/// where `New folder`, `Paste` and `Refresh` live — is the one you get by right-clicking a part
72/// of the listing that is not a file, and in a folder taller than the pane there was no such
73/// part: every pixel from the header to the status line was a row. The only way to reach it was
74/// to scroll to the end and find the gap, if the last row happened to leave one.
75///
76/// Three, because it wants to be unmissable rather than merely present, and because the same
77/// space is what makes a rubber band easy to start from below the files.
78///
79/// It is content, not a margin: at the top of a long folder every pixel of the pane is still
80/// rows, and the space appears as you reach the end. The bill for that is a listing which very
81/// nearly fills its pane now has a scrollbar, because there genuinely is more to scroll to.
82const TAIL_ROWS: usize = 3;
83/// Room for the sort triangle beside a header label.
84const SORT_ARROW: f32 = 10.0 + space::S2;
85
86/// What the listing wants the application to do that it cannot do itself.
87pub struct Outcome {
88    /// Prefetch this folder — the cursor has landed on it.
89    pub prefetch: Option<std::path::PathBuf>,
90    /// Where each visible folder row was drawn, so files can be dropped *into* it.
91    ///
92    /// Reported rather than resolved here: a drop target has to be answered synchronously from
93    /// an OLE callback on another stack, so the application publishes rects rather than the
94    /// listing being asked at the moment of the drop. Read one frame later than it is written,
95    /// which is a frame the rows have not moved in.
96    pub drop_rows: Vec<(Rect, std::path::PathBuf)>,
97    /// The rows' own rectangle — where a drop into *this* folder lands, and what lights up
98    /// for one.
99    ///
100    /// Not the pane: the column header sorts and the status line counts, and neither takes a
101    /// drop, so promising them as the destination was promising the wrong thing. `NOTHING` for
102    /// a pane too short to list a single row, which cannot show a highlight either.
103    pub drop_area: Rect,
104}
105
106/// Draw the header, the rows and the status line inside `rect`.
107#[allow(clippy::too_many_arguments)]
108pub fn show(
109    ui: &mut Ui,
110    t: &Theme,
111    zone: &LocalZone,
112    rect: Rect,
113    pane: PaneId,
114    tab: &mut Tab,
115    focused: bool,
116    icons_cache: &mut crate::shell::icons::Icons,
117    links_cache: &mut crate::shell::links::Links,
118    cut: &[std::path::PathBuf],
119    status: Option<&str>,
120    scratch: &mut String,
121    out: &mut Vec<Action>,
122) -> Outcome {
123    // The frame's clock, for the one thing here that depends on how long something has taken:
124    // whether a scan has been slow enough to admit to. See [`crate::pane::SLOW_SCAN`].
125    let now = ui.input(|i| i.time);
126    let mut outcome = Outcome {
127        prefetch: None,
128        drop_rows: Vec::new(),
129        drop_area: Rect::NOTHING,
130    };
131
132    let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER_HEIGHT));
133    let status_rect = Rect::from_min_max(
134        pos2(rect.left(), rect.bottom() - STATUS_HEIGHT),
135        rect.max,
136    );
137    // A pane squeezed shorter than its own furniture would give an inverted body
138    // rect, which turns into an empty visible range and an underflow downstream. The
139    // header and the status line are worth more than a row nobody could read.
140    if status_rect.top() <= header.bottom() + ROW_HEIGHT {
141        header_strip(ui, t, header, pane, tab, &resolved_widths(tab, rect.width()), out);
142        status_line(ui, t, status_rect, tab, status, now, scratch);
143        return outcome;
144    }
145    let body = Rect::from_min_max(header.left_bottom(), status_rect.right_top());
146    outcome.drop_area = body;
147
148    // Columns have to be resolved before the header can be drawn, and measuring
149    // needs a painter — so this happens first, once per listing.
150    if !tab.widths_measured {
151        measure_columns(ui, t, tab, scratch);
152    }
153    let widths = resolved_widths(tab, body.width());
154
155    header_strip(ui, t, header, pane, tab, &widths, out);
156
157    // Whether any rows were drawn, and so whether anything is listening for a click over
158    // the body. See [`bare_body`].
159    let mut listed = false;
160
161    if tab.dir.as_ref().is_some_and(|d| d.error.is_some()) {
162        let message = tab
163            .dir
164            .as_ref()
165            .and_then(|d| d.error.clone())
166            .unwrap_or_default();
167        text_center(
168            ui.painter(),
169            body,
170            t.fonts.body.clone(),
171            t.status.danger,
172            &message,
173        );
174    } else if tab.dir.is_none() {
175        // A scan that has not landed yet, and **only once it has been long enough to notice**.
176        // A local folder comes back in single-digit milliseconds, so this used to be a word that
177        // flashed up and vanished on every navigation, in the place the listing was about to be.
178        // See [`crate::pane::SLOW_SCAN`]; the frame that gets here at the right moment is booked
179        // by `App::start_scans`.
180        if tab.waiting_visibly(now) {
181            text_center(
182                ui.painter(),
183                body,
184                t.fonts.body.clone(),
185                t.text.tertiary,
186                "Reading…",
187            );
188        }
189    } else if tab.order.is_empty() {
190        // "Empty" and "everything is filtered out" are different facts, and telling
191        // them apart is the difference between a dead end and a hint.
192        let message = if tab.dir.as_ref().is_some_and(|d| d.is_empty()) {
193            "This folder is empty"
194        } else if tab.filter.is_empty() {
195            "Everything here is hidden — Ctrl+H shows it"
196        } else {
197            "Nothing matches the filter"
198        };
199        text_center(
200            ui.painter(),
201            body,
202            t.fonts.body.clone(),
203            t.text.tertiary,
204            message,
205        );
206    } else {
207        rows(
208            ui,
209            t,
210            zone,
211            body,
212            pane,
213            tab,
214            focused,
215            &widths,
216            icons_cache,
217            links_cache,
218            cut,
219            scratch,
220            out,
221            &mut outcome,
222        );
223        listed = true;
224    }
225    if !listed {
226        bare_body(ui, body, pane, tab, out);
227    }
228
229    status_line(ui, t, status_rect, tab, status, now, scratch);
230    outcome
231}
232
233// ---------------------------------------------------------------------------
234// Columns
235// ---------------------------------------------------------------------------
236
237/// Measure the three fitted columns against the listing's own content.
238///
239/// Cheap because none of the three needs every entry looked at:
240///
241/// - **Modified** is a fixed-width format, so one measurement of the template does.
242/// - **Size** is measured from the one value whose formatted text is longest, found
243///   by comparing lengths in bytes rather than by laying anything out.
244/// - **Type** has as many distinct labels as the folder has kinds of file, which is
245///   a handful — collected with a small linear scan of already-interned strings.
246fn measure_columns(ui: &Ui, t: &Theme, tab: &mut Tab, scratch: &mut String) {
247    let font = t.fonts.body.clone();
248    let measure = |text: &str| {
249        ui.painter()
250            .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
251            .size()
252            .x
253    };
254    // A header can be wider than everything under it.
255    let header_of = |column: Column| {
256        ui.painter()
257            .layout_no_wrap(
258                column.header().to_owned(),
259                t.fonts.body_strong.clone(),
260                Color32::PLACEHOLDER,
261            )
262            .size()
263            .x
264            + SORT_ARROW
265    };
266
267    let mut size_width: f32 = 0.0;
268    let mut type_width: f32 = 0.0;
269
270    if let Some(dir) = tab.dir.clone() {
271        // Size: the longest formatted string, found without formatting them all.
272        let mut widest_size = 0u64;
273        let mut longest = 0usize;
274        for &i in &tab.order {
275            let entry = &dir.entries[i as usize];
276            if entry.is_dir() {
277                continue;
278            }
279            scratch.clear();
280            fmt::size(entry.size, scratch);
281            if scratch.len() > longest {
282                longest = scratch.len();
283                widest_size = entry.size;
284            }
285        }
286        scratch.clear();
287        fmt::size(widest_size, scratch);
288        size_width = measure(scratch);
289
290        // Type: the distinct *extensions*, which is a much smaller set than the
291        // entries and maps one-to-one onto the labels. Comparing extensions rather
292        // than rendered labels means the inner loop touches no allocated string at
293        // all, and the cap stops a folder of ten thousand unique extensions from
294        // turning this into a quadratic scan.
295        let mut seen: Vec<&str> = Vec::new();
296        for &i in &tab.order {
297            let ext = dir.ext(i as usize);
298            let is_dir = dir.entries[i as usize].is_dir();
299            let key = if is_dir { "\0dir" } else { ext };
300            if seen.contains(&key) {
301                continue;
302            }
303            scratch.clear();
304            fmt::type_label(ext, is_dir, scratch);
305            type_width = type_width.max(measure(scratch));
306            if seen.len() >= 96 {
307                break;
308            }
309            seen.push(key);
310        }
311    }
312
313    let date_width = measure(fmt::DATE_TEMPLATE);
314
315    tab.widths[Column::Size.index()] =
316        (size_width.max(header_of(Column::Size)) + CELL_PAD * 2.0).ceil();
317    tab.widths[Column::Type.index()] =
318        (type_width.max(header_of(Column::Type)) + CELL_PAD * 2.0).ceil();
319    tab.widths[Column::Modified.index()] =
320        (date_width.max(header_of(Column::Modified)) + CELL_PAD * 2.0).ceil();
321    tab.widths_measured = true;
322}
323
324/// Widths for this frame: the three fitted columns as stored, and whatever is left
325/// for Name.
326///
327/// When the pane is too narrow for all four, the fitted columns give way from the
328/// right — Type first, then Modified — because a name you cannot read is worse
329/// than a date you cannot see.
330fn resolved_widths(tab: &Tab, total: f32) -> [f32; 4] {
331    let mut widths = tab.widths;
332    const NAME_MIN: f32 = 120.0;
333
334    let fitted = widths[1] + widths[2] + widths[3];
335    let mut spare = total - fitted;
336    if spare < NAME_MIN {
337        for column in [Column::Type, Column::Modified, Column::Size] {
338            if spare >= NAME_MIN {
339                break;
340            }
341            let index = column.index();
342            spare += widths[index];
343            widths[index] = 0.0;
344        }
345    }
346    widths[0] = (total - widths[1] - widths[2] - widths[3]).max(NAME_MIN);
347    widths
348}
349
350/// The left edge of each column, given the resolved widths.
351fn column_x(rect: Rect, widths: &[f32; 4]) -> [f32; 5] {
352    let mut edges = [rect.left(); 5];
353    for i in 0..4 {
354        edges[i + 1] = edges[i] + widths[i];
355    }
356    edges
357}
358
359/// The header: labels, the sort indicator, and the drag grips between columns.
360fn header_strip(
361    ui: &mut Ui,
362    t: &Theme,
363    rect: Rect,
364    pane: PaneId,
365    tab: &mut Tab,
366    widths: &[f32; 4],
367    out: &mut Vec<Action>,
368) {
369    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg.layer_alt);
370    let edges = column_x(rect, widths);
371
372    for (index, column) in Column::ALL.into_iter().enumerate() {
373        if widths[index] <= 0.0 {
374            continue;
375        }
376        let cell = Rect::from_min_max(
377            pos2(edges[index], rect.top()),
378            pos2(edges[index + 1], rect.bottom()),
379        );
380        let response = ui.interact(cell, Id::new(("th", pane, index)), Sense::click());
381        if response.hovered() {
382            ui.painter()
383                .rect_filled(cell, CornerRadius::ZERO, t.bg.control_hover);
384        }
385        if response.clicked() {
386            out.push(Action::Sort { pane, column });
387        }
388
389        let sorted = tab.sort_by == column;
390        let color = if response.hovered() || sorted {
391            t.text.primary
392        } else {
393            t.text.secondary
394        };
395        let arrow = if sorted { SORT_ARROW } else { 0.0 };
396        let inner = Rect::from_min_max(
397            pos2(cell.left() + CELL_PAD, cell.top()),
398            pos2(cell.right() - CELL_PAD, cell.bottom()),
399        );
400        let galley = truncated(
401            ui.painter(),
402            column.header(),
403            t.fonts.body_strong.clone(),
404            color,
405            (inner.width() - arrow).max(0.0),
406        );
407        let label_width = galley.size().x;
408        if column.numeric() {
409            // Right-aligned, with the arrow tucked inside the padding so the label
410            // stays flush with the numbers below it.
411            let shifted = Rect::from_min_max(
412                inner.min,
413                pos2(inner.right() - arrow, inner.bottom()),
414            );
415            text_right(ui.painter(), shifted, galley);
416            if sorted {
417                let x = shifted.right() + space::S2 * 0.5;
418                sort_glyph(ui, t, icon_rect(cell, x, 10.0), tab.ascending, color);
419            }
420        } else {
421            text_left(ui.painter(), inner, galley);
422            if sorted {
423                let x = inner.left() + label_width + space::S2;
424                sort_glyph(ui, t, icon_rect(cell, x, 10.0), tab.ascending, color);
425            }
426        }
427
428        // The grip on this column's right edge. Not on Name, whose width is whatever
429        // the other three leave behind.
430        if index > 0 {
431            let grip = Rect::from_min_max(
432                pos2(cell.right() - GRIP, rect.top()),
433                pos2(cell.right() + GRIP, rect.bottom()),
434            );
435            let drag = ui.interact(
436                grip,
437                Id::new(("th-grip", pane, index)),
438                Sense::click_and_drag(),
439            );
440            if drag.hovered() || drag.dragged() {
441                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
442                ui.painter().rect_filled(
443                    Rect::from_min_size(pos2(cell.right() - 1.0, rect.top()), vec2(1.0, rect.height())),
444                    CornerRadius::ZERO,
445                    t.accent.default,
446                );
447            }
448            if drag.dragged() {
449                let delta = drag.drag_delta().x;
450                tab.widths[index] = (tab.widths[index] + delta).clamp(48.0, 480.0);
451            }
452            // Double-clicking an edge re-fits the column, which is the gesture every
453            // table in every operating system has.
454            if drag.double_clicked() {
455                tab.widths_measured = false;
456            }
457        }
458    }
459
460    // A `stroke-default` rule under the header, which is what separates a header
461    // strip from the rows in Azur's table.
462    ui.painter().rect_filled(
463        Rect::from_min_size(rect.left_bottom() - vec2(0.0, 1.0), vec2(rect.width(), 1.0)),
464        CornerRadius::ZERO,
465        t.stroke.default,
466    );
467}
468
469/// How wide the rename field should be, given how wide its text has turned out.
470///
471/// The Name column's width, unless the text needs more — and never past the right edge of the
472/// row, because a field whose text is outside the window is no better than a cropped one.
473///
474/// `ink` is the laid-out width of the text, `left..right` the name cell, `row_right` the row's
475/// own right edge.
476fn rename_width(ink: f32, left: f32, right: f32, row_right: f32) -> f32 {
477    /// Room for the caret past the last character. Without it, typing at the end pushes the
478    /// text the field was just widened for straight back out of view.
479    const CARET: f32 = space::S2;
480    let wanted = ink + CARET;
481    let limit = (row_right - CELL_PAD - left).max(MIN_FIELD);
482    wanted.max(right - left).max(MIN_FIELD).min(limit)
483}
484
485/// Narrow enough to type in, on a pane too narrow for anything.
486const MIN_FIELD: f32 = 60.0;
487/// The field's height. The line plus room for a caret above and below it, and enough of a target
488/// to click into. Nothing to do with where the *text* goes — that is `text_row`'s centre.
489const FIELD_HEIGHT: f32 = 20.0;
490
491/// The dashed rectangle that marks the row the keyboard is on but has not selected.
492///
493/// Square corners, because it is drawn one pixel inside a row whose fill has none — a rounded
494/// ring inside a square edge reads as a mistake at this size.
495///
496/// One closed polyline rather than four dashed edges: `dashed_line` walks the points it is
497/// given, so a rectangle handed over as five points comes back with its dashes in step all the
498/// way round instead of restarting at every corner.
499fn cursor_ring(painter: &egui::Painter, rect: Rect, color: Color32) {
500    const DASH: f32 = 2.0;
501    const GAP: f32 = 2.0;
502    // Half-pixel centres, so a one-pixel stroke lands on one row of pixels rather than
503    // straddling two and coming out grey and two wide.
504    let r = Rect::from_min_max(
505        pos2(rect.left() + 0.5, rect.top() + 0.5),
506        pos2(rect.right() - 0.5, rect.bottom() - 0.5),
507    );
508    painter.extend(egui::Shape::dashed_line(
509        &[
510            r.left_top(),
511            r.right_top(),
512            r.right_bottom(),
513            r.left_bottom(),
514            r.left_top(),
515        ],
516        Stroke::new(1.0, color),
517        DASH,
518        GAP,
519    ));
520}
521
522fn sort_glyph(ui: &Ui, _t: &Theme, rect: Rect, ascending: bool, color: Color32) {
523    if ascending {
524        azur_icons::sort_asc(ui.painter(), rect, color);
525    } else {
526        azur_icons::sort_desc(ui.painter(), rect, color);
527    }
528}
529
530// ---------------------------------------------------------------------------
531// Rows
532// ---------------------------------------------------------------------------
533
534#[allow(clippy::too_many_arguments)]
535fn rows(
536    ui: &mut Ui,
537    t: &Theme,
538    zone: &LocalZone,
539    body: Rect,
540    pane: PaneId,
541    tab: &mut Tab,
542    focused: bool,
543    widths: &[f32; 4],
544    icons_cache: &mut crate::shell::icons::Icons,
545    links_cache: &mut crate::shell::links::Links,
546    cut: &[std::path::PathBuf],
547    scratch: &mut String,
548    out: &mut Vec<Action>,
549    outcome: &mut Outcome,
550) {
551    let Some(dir) = tab.dir.clone() else { return };
552    let count = tab.order.len();
553
554    let mut child = ui.new_child(
555        egui::UiBuilder::new()
556            .max_rect(body)
557            .layout(egui::Layout::top_down(egui::Align::Min)),
558    );
559    child.set_clip_rect(body.intersect(ui.clip_rect()));
560    // `ScrollArea::show_rows` reserves `row_height + item_spacing.y` per row, and the
561    // installed style's spacing is `space-3`. Left alone, the scroll extent would be a
562    // third taller than the rows this paints and the visible range would drift out of
563    // step with them the moment anything scrolled — rows landing in the wrong place and
564    // the pointer hit-testing a different one than it looks like. Rows are painted at
565    // exact rects here, so the spacing has to be nothing.
566    child.spacing_mut().item_spacing = egui::Vec2::ZERO;
567
568    let mut scroll = egui::ScrollArea::vertical()
569        .id_salt(("rows", pane))
570        .auto_shrink([false, false]);
571
572    // Keyboard movement has to bring the cursor with it.
573    //
574    // Nudged into view rather than centred: a listing that recentres on every arrow
575    // press is unreadable. `ScrollArea` takes an absolute offset, so the nudge is
576    // computed against last frame's, which the pane records below.
577    if let Some(offset) = tab.scroll_to.take() {
578        // The rubber-band's auto-scroll: an absolute offset, which wins over the
579        // cursor nudge because the pointer is the thing being followed.
580        scroll = scroll.vertical_scroll_offset(offset.max(0.0));
581        tab.scroll_to_cursor = false;
582    } else if tab.scroll_to_cursor {
583        if let Some(at) = tab.cursor {
584            let top = at as f32 * ROW_HEIGHT;
585            let bottom = top + ROW_HEIGHT;
586            let view = body.height();
587            let mut offset = tab.scroll_y;
588            if top < offset {
589                offset = top;
590            } else if bottom > offset + view {
591                offset = bottom - view;
592            }
593            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
594        }
595        tab.scroll_to_cursor = false;
596    }
597
598    // [`TAIL_ROWS`] more than there are rows, and then the range is cut back to the rows that
599    // exist: the extra is scrollable space and not a row, which is the whole point of it —
600    // anything drawn or hit-tested there would be a file, and what is wanted there is the folder.
601    let output = scroll.show_rows(&mut child, ROW_HEIGHT, count + TAIL_ROWS, |ui, range| {
602        let range = range.start.min(count)..range.end.min(count);
603        let first = range.start;
604        let visible = Rect::from_min_max(
605            pos2(body.left(), ui.min_rect().top()),
606            pos2(body.right(), ui.min_rect().top() + range.len() as f32 * ROW_HEIGHT),
607        );
608        // One interaction for the whole visible block, and the row worked out from
609        // the pointer. Forty per-row widgets would cost forty ids, forty hit-tests
610        // and forty animation slots for a hover highlight that arithmetic gives for
611        // nothing.
612        let response = ui.interact(
613            visible,
614            Id::new(("rows-hit", pane)),
615            Sense::click_and_drag(),
616        );
617        let last_visible = range.len().saturating_sub(1);
618        let row_at = |at: egui::Pos2| {
619            (visible.contains(at) && !range.is_empty())
620                .then(|| first + (((at.y - visible.top()) / ROW_HEIGHT) as usize).min(last_visible))
621        };
622        let hovered_row = response.hover_pos().and_then(row_at);
623
624        // Where the button went down, while it is down.
625        //
626        // Not the same row as the one under the pointer, and that difference is the whole
627        // point: egui only calls a press a drag once it has *travelled*, by which time the
628        // pointer is a row or two along. Deciding from the pointer would pick up the file
629        // the drag arrived at rather than the one it started on.
630        let press = ui.input(|i| {
631            i.pointer
632                .any_down()
633                .then(|| i.pointer.press_origin())
634                .flatten()
635        });
636        let pressed_row = press.and_then(row_at);
637
638        let edges = column_x(visible, widths);
639        let name_font = t.fonts.body.clone();
640        let meta_font = t.fonts.caption.clone();
641
642        // Every row's shell icon, drawn in one batch once the rows are done.
643        //
644        // **Why this exists.** egui starts a new draw call whenever the texture changes, and a
645        // shell icon is the only thing in a row that is not the font atlas — the fills, the
646        // rules, the painted glyphs and all four columns of text come out of that one texture.
647        // Drawing an icon inside the row loop therefore cuts the frame's primitive stream in
648        // two at every row: forty rows become eighty-odd draw calls instead of a handful.
649        //
650        // That is not merely slow, it *leaks*, and not in this program: a plain eframe window
651        // drawing forty textured quads holds steady, and the same forty interleaved with text
652        // grows by 190 MB in thirty seconds. `examples/spin.rs` is that measurement. Batching
653        // the icons is the workaround available from here, and it is what a hand-painted
654        // listing should have been doing anyway.
655        //
656        // Drawing them last is invisible: an icon sits in its own column, over the row fill
657        // and clear of the text.
658        let mut deferred: Vec<(egui::TextureId, Rect, Rect, Color32)> = Vec::new();
659        // The row being renamed, if one of the visible ones is, and where its name cell was.
660        //
661        // Drawn after the loop for the same reason the icons are, but to a different end: the
662        // field is allowed to be wider than the Name column, and a widget drawn in the middle
663        // of the loop would have the next three columns of its own row painted on top of it.
664        let mut renaming: Option<(usize, f32, f32, Rect)> = None;
665        // What a row actually has *ink* on, cell by cell, so a click can tell "on the file" from
666        // "on the row it happens to be in". Two gestures ask:
667        //
668        // - A **drag**, which picks the file up from its ink and draws a rubber band from the
669        //   space around it. That asks about the row the button went *down* on.
670        // - A **right click**, which is the file's menu on its ink and the folder's menu off it.
671        //   That asks about the row under the pointer, because a click arrives on the release,
672        //   by which time no button is down and there is no press to ask about.
673        //
674        // So both rows are collected — at most two, and their rects cannot be confused for each
675        // other's, since a row is a horizontal band and the two are at different heights.
676        let mut ink: Vec<Rect> = Vec::with_capacity(10);
677
678        for position in range.clone() {
679            let entry_index = tab.order[position] as usize;
680            let entry = &dir.entries[entry_index];
681            let row = Rect::from_min_size(
682                pos2(
683                    visible.left(),
684                    visible.top() + (position - first) as f32 * ROW_HEIGHT,
685                ),
686                vec2(visible.width(), ROW_HEIGHT),
687            );
688            // Every cell's text is centred in this instead of in the row — see [`CELL_LIFT`].
689            // Fills, the selection bar, the cursor ring and the drag ink all stay on `row`.
690            let text_row = row.translate(vec2(0.0, -CELL_LIFT));
691
692            let selected = tab.selected.get(entry_index).copied().unwrap_or(false);
693            let is_hovered = hovered_row == Some(position);
694            // The two rows whose ink is worth collecting: see where `ink` is declared.
695            let inked = pressed_row == Some(position) || is_hovered;
696            let renaming_here = matches!(tab.renaming, Some((at, _)) if at == entry_index);
697            // The row being renamed wears none of it. The field has a border and a focus ring of
698            // its own, and a selected row's fill and accent bar sit right behind them competing
699            // for the same edge -- so the one row you are actually looking at is the one that
700            // reads worst. Explorer drops the highlight while renaming too.
701            if !renaming_here {
702                if let Some(fill) = row_fill(t, selected, is_hovered) {
703                    ui.painter().rect_filled(row, CornerRadius::ZERO, fill);
704                }
705                if selected {
706                    selection_bar(ui.painter(), row, t);
707                }
708            }
709            // The keyboard cursor, when it is not simply the selection.
710            //
711            // Dashed and grey rather than a solid accent outline. The accent is what this
712            // window says "selected" with — the fill and the bar down the left edge of a
713            // selected row — and spending it on a row that is *not* selected said the opposite
714            // of what it meant. A dashed grey rectangle is what every list on the platform has
715            // marked the focused-not-selected row with since long before any of them had a
716            // theme, and it cannot be confused with a selection at a glance.
717            if focused && tab.cursor == Some(position) && !selected {
718                cursor_ring(ui.painter(), row.shrink(1.0), t.stroke.strong);
719            }
720
721            // A hidden or system entry is dimmed rather than hidden-when-shown:
722            // seeing that it *is* hidden is the point of showing it. A row waiting on a
723            // paste is dimmed for a different reason — it is going somewhere — and
724            // Explorer marks it the same way, so the two share the treatment.
725            let pending_cut = !cut.is_empty()
726                && cut
727                    .iter()
728                    .any(|p| p.file_name().is_some_and(|n| n == dir.name(entry_index)))
729                && cut.iter().any(|p| p.parent() == Some(dir.path.as_path()));
730            let dim = entry.is_hidden() || pending_cut;
731            let name_color = if dim { t.text.tertiary } else { t.text.primary };
732            let meta_color = if dim { t.text.disabled } else { t.text.secondary };
733
734            // ---- Name ----
735            let kind = fmt::kind_of(dir.ext(entry_index), entry.is_dir());
736            let glyph_x = row.left() + CELL_PAD + 2.0;
737            let box_rect = icon_rect(row, glyph_x, GLYPH);
738
739            // The dimmed half of the Name cell: **what this row points at, or where it is.**
740            //
741            // A shortcut's target wins when it is known, because that is what the row *is* — a
742            // name standing for somewhere else — and in an ordinary listing every row shares
743            // the same parent anyway, so the target is the only context there is to give. The
744            // parent is what a flattened listing shows, and it is the answer for every row in
745            // one that is not a shortcut.
746            //
747            // Asked once per row per view and answered on a worker thread: reading a `.lnk`
748            // means COM, and one pointing at a share that is not currently reachable is the
749            // classic Explorer hang. Until the answer lands the row draws its name alone, which
750            // is what it did before this existed. See `crate::shell::links`.
751            let context: Option<std::borrow::Cow<'_, str>> = {
752                let row_index = entry_index as u32;
753                match links::kind_of(dir.ext(entry_index), entry.is_link()) {
754                    Some(kind) => {
755                        if !tab.links.contains_key(&row_index)
756                            && links_cache.request(
757                                tab.view,
758                                row_index,
759                                dir.target(entry_index),
760                                kind,
761                            )
762                        {
763                            // Present means asked, so the row does not ask again next frame.
764                            tab.links.insert(row_index, None);
765                        }
766                        // Cloned rather than borrowed: the map is behind the same `&mut Tab`
767                        // that the next row's request writes to, and one short path per
768                        // shortcut row on screen is not a cost worth threading a lifetime for.
769                        tab.links
770                            .get(&row_index)
771                            .and_then(|target| target.clone())
772                            .map(std::borrow::Cow::Owned)
773                    }
774                    None => None,
775                }
776                // Where the row is, for a flattened listing. `""` in every other one.
777                .or_else(|| match dir.within(entry_index) {
778                    "" => None,
779                    parent => Some(std::borrow::Cow::Borrowed(parent)),
780                })
781            };
782
783            // The shell icon if one is known, and the painted glyph until it is. Nothing
784            // here blocks or allocates: the per-file answer is a lookup in the tab's own
785            // four-bytes-a-row column, and the per-type one is a small map.
786            let shell_icon = match dir.explicit_target(entry_index) {
787                // A row that stands for somewhere else — a volume under This PC. Its icon
788                // is its own, not its type's, and there are at most a couple of dozen of
789                // them, so this is the one listing that can afford to ask per path.
790                Some(target) => icons_cache.place(target),
791                None => {
792                    let ext = dir.ext(entry_index);
793                    // A file whose icon lives inside it: the answer belongs to this view of
794                    // this folder, and the path to ask with is built once per file rather
795                    // than once per file per frame.
796                    let own = if !entry.is_dir() && Icons::is_per_file(ext) {
797                        match tab.file_icons.get(entry_index).copied() {
798                            Some(shell_icons::UNASKED) => {
799                                let path = dir.path.join(dir.name(entry_index));
800                                if icons_cache.request_file(
801                                    tab.view,
802                                    entry_index as u32,
803                                    path,
804                                ) {
805                                    tab.file_icons[entry_index] = shell_icons::ASKED;
806                                }
807                                None
808                            }
809                            Some(index) if index >= 0 => Some(shell_icons::Icon { index }),
810                            _ => None,
811                        }
812                    } else {
813                        None
814                    };
815                    // Falling through to the type's icon means an `.exe` shows the generic
816                    // application glyph rather than nothing while its own is fetched.
817                    own.or_else(|| icons_cache.kind(ext, entry.is_dir()))
818                }
819            }
820            .and_then(|icon| icons_cache.uv(ui.ctx(), icon));
821
822            match shell_icon {
823                Some((texture, uv)) => {
824                    // Held back and drawn after the loop — see `deferred`. A shell icon is a
825                    // textured quad and everything else on a row comes out of the font atlas,
826                    // so drawing it here would split the frame's primitive stream in two at
827                    // every row.
828                    deferred.push((
829                        texture,
830                        uv,
831                        box_rect,
832                        // A hidden entry is faded rather than recoloured: a shell icon
833                        // carries its own colours, and tinting them would misreport
834                        // what kind of file it is.
835                        if dim {
836                            Color32::from_white_alpha(110)
837                        } else {
838                            Color32::WHITE
839                        },
840                    ));
841                }
842                None => {
843                    let glyph: azur_icons::Icon<'_> = if entry.is_dir() && entry.is_link() {
844                        &icons::folder_link
845                    } else {
846                        icons::for_kind(kind)
847                    };
848                    let glyph_color = if dim { t.text.disabled } else { t.kind(kind) };
849                    glyph(ui.painter(), box_rect, glyph_color);
850                }
851            }
852
853            let name_left = glyph_x + GLYPH + space::S3;
854            let name_right = edges[1] - CELL_PAD;
855            // A folder row is somewhere files can be dropped. Recorded for every visible one,
856            // because that is what makes dragging onto a folder mean "into that folder" rather
857            // than "into the folder I am looking at".
858            if entry.is_dir() {
859                outcome.drop_rows.push((row, dir.target(entry_index)));
860            }
861            if inked {
862                // The icon counts as the file too: it is the most obvious thing to take
863                // hold of, and it is what Explorer's own drag handle is.
864                ink.push(box_rect);
865            }
866            if renaming_here {
867                // Held back until every column of every row has been drawn, so that a field
868                // wider than the Name column covers Size, Type and Modified instead of being
869                // painted under them. See where `renaming` is declared.
870                // `text_row`, not `row`: the field has to put its text exactly where the label
871                // would have gone, and the label is centred in `text_row`. See `rename_field`.
872                renaming = Some((entry_index, name_left, name_right, text_row));
873            } else if name_right > name_left {
874                let galley = name_galley(
875                    ui.painter(),
876                    dir.leaf(entry_index),
877                    context.as_deref(),
878                    name_font.clone(),
879                    name_color,
880                    meta_color,
881                    name_right - name_left,
882                );
883                if inked {
884                    ink.push(Rect::from_min_max(
885                        pos2(name_left, row.top()),
886                        pos2((name_left + galley.size().x).min(name_right), row.bottom()),
887                    ));
888                }
889                text_left(
890                    ui.painter(),
891                    Rect::from_min_max(
892                        pos2(name_left, text_row.top()),
893                        pos2(name_right, text_row.bottom()),
894                    ),
895                    galley,
896                );
897            }
898
899            // ---- Size ----
900            if widths[1] > 0.0 && !entry.is_dir() {
901                scratch.clear();
902                fmt::size(entry.size, scratch);
903                let cell = Rect::from_min_max(
904                    pos2(edges[1] + CELL_PAD, text_row.top()),
905                    pos2(edges[2] - CELL_PAD, text_row.bottom()),
906                );
907                let galley = truncated(
908                    ui.painter(),
909                    scratch,
910                    meta_font.clone(),
911                    meta_color,
912                    cell.width(),
913                );
914                if inked {
915                    // Right-aligned, so its ink is against the right edge of the cell.
916                    ink.push(Rect::from_min_max(
917                        pos2((cell.right() - galley.size().x).max(cell.left()), row.top()),
918                        pos2(cell.right(), row.bottom()),
919                    ));
920                }
921                text_right(ui.painter(), cell, galley);
922            }
923
924            // ---- Type ----
925            if widths[2] > 0.0 {
926                scratch.clear();
927                fmt::type_label(dir.ext(entry_index), entry.is_dir(), scratch);
928                let cell = Rect::from_min_max(
929                    pos2(edges[2] + CELL_PAD, text_row.top()),
930                    pos2(edges[3] - CELL_PAD, text_row.bottom()),
931                );
932                let galley = truncated(
933                    ui.painter(),
934                    scratch,
935                    meta_font.clone(),
936                    meta_color,
937                    cell.width(),
938                );
939                if inked {
940                    // The ink is what a drag is measured against, so it stays on the row
941                    // rather than following the text's two-point lift.
942                    ink.push(Rect::from_min_max(
943                        pos2(cell.left(), row.top()),
944                        pos2((cell.left() + galley.size().x).min(cell.right()), row.bottom()),
945                    ));
946                }
947                text_left(ui.painter(), cell, galley);
948            }
949
950            // ---- Modified ----
951            if widths[3] > 0.0 {
952                scratch.clear();
953                fmt::modified(entry.modified, zone, scratch);
954                let cell = Rect::from_min_max(
955                    pos2(edges[3] + CELL_PAD, text_row.top()),
956                    pos2(edges[4] - CELL_PAD, text_row.bottom()),
957                );
958                let galley = truncated(
959                    ui.painter(),
960                    scratch,
961                    meta_font.clone(),
962                    meta_color,
963                    cell.width(),
964                );
965                if inked {
966                    ink.push(Rect::from_min_max(
967                        pos2(cell.left(), row.top()),
968                        pos2((cell.left() + galley.size().x).min(cell.right()), row.bottom()),
969                    ));
970                }
971                text_left(ui.painter(), cell, galley);
972            }
973        }
974
975        // Every visible row's icon, in one run — see where `deferred` is declared.
976        if !deferred.is_empty() {
977            let painter = ui.painter();
978            for (texture, uv, rect, tint) in &deferred {
979                painter.image(*texture, *rect, *uv, *tint);
980            }
981        }
982
983        // And last of all, over everything including its own row's other columns.
984        if let Some((entry_index, left, right, text_row)) = renaming {
985            rename_field(ui, t, pane, tab, entry_index, left, right, text_row, out);
986        }
987
988        // ---- Clicks --------------------------------------------------------
989        let modifiers = ui.input(|i| i.modifiers);
990        if tab.renaming.is_some() {
991            // The field has the keyboard and the pointer; a click that lands outside it
992            // is handled by the field losing focus, not by moving the selection.
993            return;
994        }
995        if response.clicked() || response.secondary_clicked() {
996            out.push(Action::Focus(pane));
997        }
998        // Which row the context menu is *for*, decided below. `None` means the folder's own menu
999        // — the one with `New folder` and `Paste` in it.
1000        let mut menu_row = hovered_row;
1001        if let Some(position) = hovered_row {
1002            if response.clicked() {
1003                if modifiers.command {
1004                    tab.toggle(position);
1005                } else if modifiers.shift {
1006                    tab.select_range_to(position);
1007                } else {
1008                    tab.select_only(position);
1009                }
1010                if tab.is_dir_at(position) {
1011                    outcome.prefetch = tab.target_at(position);
1012                }
1013            }
1014            if response.double_clicked() {
1015                if let Some(path) = tab.target_at(position) {
1016                    if tab.is_dir_at(position) {
1017                        out.push(Action::Navigate { pane, path });
1018                    } else {
1019                        out.push(Action::Open(path));
1020                    }
1021                }
1022            }
1023            // Middle click opens a folder in its own tab — including a folder *shortcut*, which
1024            // is a row this cannot tell apart from a file without reading it. So the reading is
1025            // left to the action, which resolves it the same way opening one does; a shortcut to
1026            // a file lands there and does nothing, which is what a middle click on any other
1027            // file does.
1028            if response.middle_clicked() {
1029                if let Some(path) = tab.target_at(position) {
1030                    if tab.is_dir_at(position) {
1031                        out.push(Action::NavigateNewTab { pane, path });
1032                    } else if tab.is_shortcut_at(position) {
1033                        out.push(Action::OpenNewTab(path));
1034                    }
1035                }
1036            }
1037            // ---- Right click, on the file or merely in its row -------------
1038            //
1039            // A row is mostly space: a 24-point row across a wide pane has ink on perhaps a
1040            // third of it, and the rest is the listing's background as much as the gap below the
1041            // last file is. So a right click that lands on the name, the icon or one of the
1042            // three values is about *that file*, and one that lands in the space around them is
1043            // about the folder — which is where `New folder` and `Paste` are, and which used to
1044            // need finding a gap under the last row to reach.
1045            //
1046            // Only for a row that is not already in the selection. Right-clicking one that is
1047            // means the selection, wherever in the row it lands: the files are picked out
1048            // already, and taking that away because the pointer was between two columns would
1049            // undo work rather than ask a question. That is also the rule the drag follows.
1050            if response.secondary_clicked() && !tab.is_selected(position) {
1051                let at = response
1052                    .interact_pointer_pos()
1053                    .or_else(|| ui.ctx().pointer_interact_pos());
1054                let on_file =
1055                    at.is_some_and(|at| ink.iter().any(|rect| rect.expand(1.0).contains(at)));
1056                if on_file {
1057                    tab.select_only(position);
1058                } else {
1059                    tab.clear_selection();
1060                    menu_row = None;
1061                }
1062            }
1063        } else if response.clicked() {
1064            // A click on the empty space below the rows clears the selection, which
1065            // is how every file manager cancels one.
1066            tab.clear_selection();
1067        }
1068
1069        // ---- Dragging: the file, or a band ---------------------------------
1070        //
1071        // Which one comes from where the button went *down*: on the icon, the name or one
1072        // of the three values — a drag of the file — or in the space around them, which
1073        // bands exactly as it does below the last row. A row is mostly space (a 24-point
1074        // row across a wide pane has ink on maybe a third of it), and treating all of it
1075        // as a drag handle is what makes a band a gesture you can only start by finding
1076        // the bottom of the listing first.
1077        //
1078        // Keyed on the *pressed* row and not the hovered one. egui only calls a press a
1079        // drag once it has travelled, and by then the pointer is a row or two along — so
1080        // the hovered row is the row the drag arrived at, and using it would both test the
1081        // wrong ink and pick up the wrong file.
1082        // Only the two buttons that mean anything here. A middle-button drag scrolls in some
1083        // applications and does nothing in this one, and the thumb buttons navigate — none of
1084        // them should pick a file up or draw a selection box, which is what `drag_started()`
1085        // without a button lets them all do.
1086        let dragging_files = response.drag_started_by(egui::PointerButton::Primary);
1087        let dragging_to_ask = response.drag_started_by(egui::PointerButton::Secondary);
1088        if dragging_files || dragging_to_ask {
1089            if let Some(grabbed) = pressed_row {
1090                let on_file = press
1091                    .is_some_and(|at| ink.iter().any(|rect| rect.expand(1.0).contains(at)));
1092                if on_file {
1093                    // A drag that starts on something already selected takes the whole
1094                    // selection; one that starts anywhere else makes that row the selection
1095                    // first, which is what makes dragging a single file work without clicking
1096                    // it beforehand.
1097                    if !tab.is_selected(grabbed) {
1098                        tab.select_only(grabbed);
1099                    }
1100                    out.push(Action::DragOut {
1101                        pane,
1102                        items: tab.selection_paths(),
1103                    });
1104                } else {
1105                    start_band(ui, body, tab, press);
1106                    out.push(Action::Focus(pane));
1107                }
1108            }
1109        }
1110
1111        context_menu(ui, &response, pane, tab, menu_row, out);
1112    });
1113
1114    // Remembered so the next scroll-into-view can nudge rather than jump, and so a
1115    // tab keeps its place when the pane it lives in is redrawn elsewhere.
1116    tab.scroll_y = output.state.offset.y;
1117
1118    // Everything below the last row: the canvas a short listing leaves above the status line, and
1119    // the [`TAIL_ROWS`] a long one leaves once it is scrolled to the end. That space is part of
1120    // the list — clicking it cancels the selection, as it does in every file manager, and
1121    // right-clicking it is how the folder's own menu is reached — but `show_rows` hands out a
1122    // viewport of rows and nothing else, so nothing was listening there.
1123    //
1124    // Measured from the rows rather than from `content_size`, which now includes the tail.
1125    let rows_bottom = body.top() + count as f32 * ROW_HEIGHT - output.state.offset.y;
1126    let empty = Rect::from_min_max(
1127        pos2(body.left(), rows_bottom.clamp(body.top(), body.bottom())),
1128        pos2(output.inner_rect.right(), body.bottom()),
1129    );
1130    if empty.height() > 1.0 && empty.width() > 1.0 {
1131        let response = child.interact(
1132            empty,
1133            Id::new(("rows-empty", pane)),
1134            Sense::click_and_drag(),
1135        );
1136        if response.clicked() {
1137            tab.clear_selection();
1138            out.push(Action::Focus(pane));
1139        }
1140        if response.secondary_clicked() {
1141            out.push(Action::Focus(pane));
1142        }
1143        // The folder's own menu, so right-clicking the space below the files offers
1144        // Refresh and the rest rather than nothing.
1145        context_menu(&child, &response, pane, tab, None, out);
1146
1147        // ---- The rubber band ------------------------------------------------
1148        //
1149        // It starts here rather than on a row because a drag *from* a row is how you
1150        // pick files up and move them — which is the other half of this gesture and
1151        // the reason the two have to start in different places, exactly as they do in
1152        // Explorer.
1153        if response.drag_started_by(egui::PointerButton::Primary)
1154            || response.drag_started_by(egui::PointerButton::Secondary)
1155        {
1156            let origin = child.input(|i| i.pointer.press_origin());
1157            start_band(&child, body, tab, origin);
1158            out.push(Action::Focus(pane));
1159        }
1160    }
1161
1162    if tab.band.is_some() {
1163        band(&mut child, t, body, pane, tab);
1164    }
1165}
1166
1167/// Begin a rubber band at where the button went down.
1168///
1169/// `press_origin` rather than the current position: those are different pixels — a drag has
1170/// to travel before egui calls it one — and anchoring at the later of the two loses whatever
1171/// the pointer crossed on the way, so a quick flick would select nothing.
1172fn start_band(ui: &Ui, body: Rect, tab: &mut Tab, origin: Option<egui::Pos2>) {
1173    let anchor = content_pos(ui, body, tab, origin);
1174    let modifiers = ui.input(|i| i.modifiers);
1175    tab.band = Some(crate::pane::Band {
1176        anchor,
1177        current: anchor,
1178        // A plain drag replaces the selection; Ctrl or Shift builds on it.
1179        base: if modifiers.command || modifiers.shift {
1180            tab.selected.clone()
1181        } else {
1182            Vec::new()
1183        },
1184    });
1185}
1186
1187/// A pointer position as a distance from the top of the whole listing.
1188fn content_pos(ui: &Ui, body: Rect, tab: &Tab, at: Option<egui::Pos2>) -> egui::Pos2 {
1189    let at = at.or_else(|| ui.ctx().pointer_interact_pos()).unwrap_or(body.min);
1190    pos2(at.x, at.y - body.top() + tab.scroll_y)
1191}
1192
1193/// Track, apply and paint a rubber-band selection.
1194fn band(ui: &mut Ui, t: &Theme, body: Rect, pane: PaneId, tab: &mut Tab) {
1195    let held = ui.input(|i| i.pointer.any_down());
1196    let pointer = ui.ctx().pointer_interact_pos();
1197
1198    if !held || pointer.is_none() {
1199        tab.band = None;
1200        return;
1201    }
1202
1203    // Auto-scroll when the band is dragged past an edge, at a rate that grows with
1204    // how far past it the pointer is — so a small overshoot creeps and a big one
1205    // moves, without a separate "fast" mode to discover.
1206    let at = pointer.unwrap_or(body.min);
1207    let overshoot = if at.y < body.top() {
1208        at.y - body.top()
1209    } else if at.y > body.bottom() {
1210        at.y - body.bottom()
1211    } else {
1212        0.0
1213    };
1214    if overshoot != 0.0 {
1215        let rows = tab.order.len() as f32 * ROW_HEIGHT;
1216        let limit = (rows - body.height()).max(0.0);
1217        let step = (overshoot * 0.35).clamp(-ROW_HEIGHT * 3.0, ROW_HEIGHT * 3.0);
1218        let next = (tab.scroll_y + step).clamp(0.0, limit);
1219        if next != tab.scroll_y {
1220            tab.scroll_to = Some(next);
1221            tab.scroll_y = next;
1222        }
1223        // A drag held still outside the view has to keep scrolling, and egui only
1224        // redraws on demand.
1225        ui.ctx().request_repaint();
1226    }
1227
1228    let current = content_pos(ui, body, tab, pointer);
1229    if let Some(band) = &mut tab.band {
1230        band.current = current;
1231    }
1232    tab.apply_band();
1233
1234    // Painted last, so it lies over the rows it is selecting. Clipped to the body, or
1235    // a band dragged past the edge would spill onto the status line.
1236    let Some(band) = &tab.band else { return };
1237    let to_screen = |p: egui::Pos2| pos2(p.x, p.y + body.top() - tab.scroll_y);
1238    let rect = Rect::from_two_pos(to_screen(band.anchor), to_screen(band.current))
1239        .intersect(body);
1240    if rect.width() < 1.0 && rect.height() < 1.0 {
1241        return;
1242    }
1243    let painter = ui.painter().with_clip_rect(body);
1244    let accent = t.accent.default;
1245    painter.rect_filled(
1246        rect,
1247        CornerRadius::ZERO,
1248        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 36),
1249    );
1250    painter.rect_stroke(
1251        rect,
1252        CornerRadius::ZERO,
1253        Stroke::new(1.0, accent),
1254        StrokeKind::Inside,
1255    );
1256    let _ = pane;
1257}
1258
1259/// How much of a name a rename should start with selected: the stem, not the extension.
1260///
1261/// Typing over `report.docx` means replacing `report`, and an editor that hands you the
1262/// extension as well is an editor that turns every rename into a file with no type. Explorer
1263/// selects the stem; so does this.
1264///
1265/// `file_stem` gets the awkward cases right without help. `.gitignore` has no stem to speak of
1266/// and comes back whole, which is what Explorer selects too; `archive.tar.gz` comes back as
1267/// `archive.tar`, because only the last extension is one.
1268fn stem_chars(name: &str, is_dir: bool) -> usize {
1269    let whole = name.chars().count();
1270    if is_dir {
1271        return whole;
1272    }
1273    match std::path::Path::new(name).file_stem().and_then(|s| s.to_str()) {
1274        Some(stem) if !stem.is_empty() => stem.chars().count(),
1275        // No stem, or a name the platform will not split: select all of it.
1276        _ => whole,
1277    }
1278}
1279
1280/// Put the caret over the stem of the name in a freshly opened rename field.
1281fn select_stem(ctx: &egui::Context, id: Id, name: &str, is_dir: bool) {
1282    use egui::text::{CCursor, CCursorRange};
1283
1284    let Some(mut state) = egui::TextEdit::load_state(ctx, id) else {
1285        return;
1286    };
1287    let end = stem_chars(name, is_dir);
1288    state
1289        .cursor
1290        .set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(end))));
1291    state.store(ctx, id);
1292}
1293
1294/// The in-place rename field, drawn over the name cell.
1295///
1296/// Explorer renames where the name sits rather than in a dialog, which keeps the
1297/// neighbouring names on screen — usually the reason you are renaming in the first
1298/// place. `Enter` commits, `Escape` abandons, and clicking away commits, which is
1299/// what every other in-place editor on the platform does.
1300///
1301/// # It is allowed to be wider than the column
1302///
1303/// The Name column is as wide as the other three leave it, which on a narrow pane is narrower
1304/// than plenty of names. A field pinned to that width scrolls a long name sideways under the
1305/// caret, so renaming `2026-04-report-final-v3.xlsx` meant editing eleven characters of it
1306/// through a letterbox and guessing at the rest. So it grows to fit its text and runs on over
1307/// Size, Type and Modified — which are nothing anybody needs to read while typing a name, and
1308/// come back the moment the rename ends. It stops at the right edge of the row, because past
1309/// that it would be a field with its text outside the window.
1310///
1311/// # The name does not move when you start typing
1312///
1313/// Which sounds like nothing and was the most obvious thing wrong with it. A field is a box with
1314/// its own padding and its own idea of where a line sits inside it, so the name jumped **two
1315/// points right and one point down** the instant a rename opened — measured, off a screenshot,
1316/// against the same row drawn as a label. It reads as a flinch on the one thing you are looking
1317/// at.
1318///
1319/// So the box is given no padding at all, and both sides now centre the line by the same rule:
1320/// `text_left` centres a label's galley in `text_row`, and the field is centred on that same
1321/// `text_row` with `Align::Center`. The name stays exactly where it was, which is what makes it
1322/// look like the row itself became editable rather than like a widget appeared over it.
1323///
1324/// `background-layer`, the panel's own fill, for the same reason: not `background-control`, which
1325/// is a *control's* colour and drew a grey slab over the row. Opaque rather than transparent
1326/// though, because covering the Size, Type and Modified cells it runs over is the whole point of
1327/// being wider than the column.
1328#[allow(clippy::too_many_arguments)]
1329fn rename_field(
1330    ui: &mut Ui,
1331    t: &Theme,
1332    pane: PaneId,
1333    tab: &mut Tab,
1334    entry: usize,
1335    left: f32,
1336    right: f32,
1337    text_row: Rect,
1338    out: &mut Vec<Action>,
1339) {
1340    let fresh = tab.rename_fresh;
1341    // A folder keeps its whole name selected even when it has a dot in it, which is what
1342    // Explorer does with `my.folder`.
1343    let is_dir = tab
1344        .dir
1345        .as_ref()
1346        .and_then(|dir| dir.entries.get(entry))
1347        .is_some_and(|entry| entry.is_dir());
1348    let Some(text) = tab.rename_text(entry) else {
1349        return;
1350    };
1351    let whole = text.clone();
1352
1353    // What the text will take, measured with the font the field will lay it out in.
1354    let font = egui::TextStyle::Body.resolve(ui.style());
1355    let line = ui
1356        .painter()
1357        .layout_no_wrap(whole.clone(), font, Color32::PLACEHOLDER)
1358        .size();
1359    let width = rename_width(line.x, left, right, text_row.right());
1360
1361    // Where `text_left` would have put the label's first pixel — the same arithmetic, including
1362    // the snap to *device* pixels, which is the part that matters and the part that is easy to
1363    // miss. Centring the field on `text_row` and leaving egui to divide by two got within half a
1364    // point, and half a point is a blurred word rather than a sharp one.
1365    //
1366    // Then the field is placed so that `Align::Center` inside it lands the line exactly there:
1367    // egui centres the galley in the field's inner rect, and the margin is nothing, so the line
1368    // sits `(FIELD_HEIGHT - line) / 2` below the top.
1369    use egui::emath::GuiRounding as _;
1370    let baseline = (text_row.center().y - line.y * 0.5).round_to_pixels(ui.painter().pixels_per_point());
1371    let field = Rect::from_min_size(
1372        pos2(left, baseline - (FIELD_HEIGHT - line.y) * 0.5),
1373        vec2(width, FIELD_HEIGHT),
1374    );
1375
1376    let id = Id::new(("rename", pane, entry));
1377    // Not the accent ring egui puts round a focused field. The field is already an obviously
1378    // editable box — a filled rectangle with a caret in it, sitting on a row that has had its
1379    // own highlight taken away for exactly this reason — and the ring on top of that was the
1380    // loudest thing in the window while renaming. Swapped rather than scoped, so nothing about
1381    // the layout of this row changes with it.
1382    //
1383    // The *width* and not the whole stroke, and the difference is the whole name being
1384    // unreadable. `Visuals::selection::stroke` is two unrelated things in one field: the frame
1385    // round a focused `TextEdit` — `builder.rs` reads all of it — and, in
1386    // `text_selection::visuals`, `stroke.color` alone, which is **the colour selected text is
1387    // drawn in**. `Stroke::NONE` is transparent, so zeroing the field took the frame off and
1388    // painted the selected stem in nothing: a solid accent block with the name invisible inside
1389    // it, which is what a rename opens with every time. A zero-width stroke draws no frame and
1390    // leaves the colour where it was.
1391    //
1392    // The colour it leaves is right because the design system was wrong too, and got fixed: it
1393    // pointed that field at `stroke-focus`, for the border, which put selected text at 2.3:1 on
1394    // the selection behind it. It is `text-on_accent` now — see
1395    // `azur_egui_theme::style::tests::selected_text_reads_on_the_selection_behind_it`.
1396    let ring = ui.visuals().selection.stroke;
1397    ui.visuals_mut().selection.stroke = Stroke::new(0.0, ring.color);
1398    // Square, like the row it is standing in for. See [`crate::ui::squared`].
1399    let response = crate::ui::squared(ui, |ui| {
1400        ui.put(
1401            field,
1402            egui::TextEdit::singleline(text)
1403                .id(id)
1404                .margin(egui::Margin::ZERO)
1405                .vertical_align(egui::Align::Center)
1406                .background_color(t.bg.layer)
1407                .desired_width(width),
1408        )
1409    });
1410    ui.visuals_mut().selection.stroke = ring;
1411    if fresh {
1412        response.request_focus();
1413        select_stem(ui.ctx(), id, &whole, is_dir);
1414        tab.rename_fresh = false;
1415    }
1416
1417    let (enter, escape) = ui.input(|i| {
1418        (
1419            i.key_pressed(egui::Key::Enter),
1420            i.key_pressed(egui::Key::Escape),
1421        )
1422    });
1423    let name = tab
1424        .rename_text(entry)
1425        .map(|text| text.clone())
1426        .unwrap_or_default();
1427
1428    if escape {
1429        out.push(Action::CancelRename(pane));
1430    } else if enter || response.lost_focus() {
1431        out.push(Action::CommitRename { pane, name });
1432    }
1433}
1434
1435/// Ask for the shell context menu on a right click.
1436///
1437/// The menu itself is Explorer's — see [`crate::shell::menu`] — so this only has to
1438/// A body with no rows in it still answers a right click.
1439///
1440/// [`rows`] is what wires the listing's clicks up, and a listing with nothing in it never gets
1441/// there: an empty folder, a folder filtered down to nothing, one still being read, and one
1442/// that refused to be read all draw a line of text over a body that nothing was listening to.
1443/// So right-clicking an empty folder did nothing at all — and an empty folder is precisely
1444/// where `New folder` and `Paste` are most wanted.
1445///
1446/// The folder's own menu, the same one the space below a short listing gives.
1447fn bare_body(ui: &Ui, body: Rect, pane: PaneId, tab: &Tab, out: &mut Vec<Action>) {
1448    if body.height() <= 1.0 || body.width() <= 1.0 {
1449        return;
1450    }
1451    // Clicks only. A drag over an empty body is a rubber band around nothing, and leaving
1452    // dragging alone keeps the drop zone underneath able to take files dropped in.
1453    let response = ui.interact(body, Id::new(("bare-body", pane)), Sense::click());
1454    if response.clicked() || response.secondary_clicked() {
1455        out.push(Action::Focus(pane));
1456    }
1457    context_menu(ui, &response, pane, tab, None, out);
1458}
1459
1460/// decide *what* it is for and *where* it goes. Showing it is deferred to the
1461/// application through an action, because `TrackPopupMenuEx` is modal: it must not run
1462/// with the listing borrowed and half-drawn.
1463fn context_menu(
1464    ui: &Ui,
1465    response: &egui::Response,
1466    pane: PaneId,
1467    tab: &Tab,
1468    row: Option<usize>,
1469    out: &mut Vec<Action>,
1470) {
1471    if !response.secondary_clicked() {
1472        return;
1473    }
1474    // Where the pointer was when the button went down, in screen pixels — a Win32 menu
1475    // is positioned in physical coordinates, and egui works in points.
1476    let at = response
1477        .interact_pointer_pos()
1478        .or_else(|| ui.ctx().pointer_interact_pos())
1479        .unwrap_or(response.rect.center());
1480    let scale = ui.ctx().pixels_per_point();
1481
1482    // `row` is what the menu is for, and by the time this is called the selection already agrees
1483    // with it — the caller selected the row the click landed on, or cleared the selection and
1484    // passed `None`. So the selection is the answer, with the row itself as the fallback for the
1485    // case that cannot happen: a row that is not selected and not selectable, because the listing
1486    // went away between the click and here.
1487    let items = match row {
1488        Some(_) if tab.selected_count > 0 => tab.selection_paths(),
1489        Some(row) => tab.target_at(row).into_iter().collect(),
1490        // The background: the folder's own menu, where New and Paste live.
1491        None => Vec::new(),
1492    };
1493
1494    out.push(Action::ShellMenu {
1495        pane,
1496        items,
1497        at: ((at.x * scale) as i32, (at.y * scale) as i32),
1498    });
1499}
1500
1501// ---------------------------------------------------------------------------
1502// Status line
1503// ---------------------------------------------------------------------------
1504
1505/// `"s"` unless there is exactly one.
1506fn plural(count: u32) -> &'static str {
1507    if count == 1 {
1508        ""
1509    } else {
1510        "s"
1511    }
1512}
1513
1514/// The separator between a row's name and the context after it.
1515///
1516/// Spaces either side, and both of them belong to the *dimmed* half: what the eye should find
1517/// first is where the name ends, and a gap in body-coloured text before a gap in secondary
1518/// makes that edge fuzzy.
1519const CONTEXT: &str = " > ";
1520
1521/// A row's name, and — dimmed, after a `>` — where it is or what it points at.
1522///
1523/// Two sections of one galley rather than two galleys, so the pair share a baseline, a
1524/// truncation and a single draw. The name comes first and is what survives: everything after
1525/// the separator is context, and context is the thing to give up when the column is narrow.
1526///
1527/// `context` is `None` for the ordinary case — a file in the folder you are looking at, which
1528/// is not a shortcut — and then this is [`crate::ui::truncated`] and nothing else.
1529///
1530/// **The context is elided from its front, not its back.** For a flattened row that means
1531/// `translations_fr.json > …\Resources\Lang` rather than `> PluginGeosystem\Resour…`: the
1532/// folder immediately holding the file is what identifies it, and the same is true of a
1533/// shortcut's target, where the last component is the program it runs. A component at a time,
1534/// never mid-name, because a path cut mid-component reads as a different path.
1535///
1536/// At most one extra layout per level, only for the rows that do not fit, and only for the ~40
1537/// on screen; egui caches finished galleys, so a row that has not changed costs a hash lookup
1538/// on every frame after the first.
1539fn name_galley(
1540    painter: &egui::Painter,
1541    name: &str,
1542    context: Option<&str>,
1543    font: egui::FontId,
1544    color: egui::Color32,
1545    dim: egui::Color32,
1546    width: f32,
1547) -> std::sync::Arc<egui::Galley> {
1548    let Some(context) = context.filter(|text| !text.is_empty()) else {
1549        return truncated(painter, name, font, color, width);
1550    };
1551
1552    let lay = |context: &str| {
1553        let mut job = egui::text::LayoutJob::default();
1554        job.append(name, 0.0, egui::TextFormat::simple(font.clone(), color));
1555        job.append(
1556            &format!("{CONTEXT}{context}"),
1557            0.0,
1558            egui::TextFormat::simple(font.clone(), dim),
1559        );
1560        job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
1561        painter.layout_job(job)
1562    };
1563
1564    let whole = lay(context);
1565    if !whole.elided {
1566        return whole;
1567    }
1568    let mut shortest = None;
1569    let mut cut = 0;
1570    while let Some(at) = context[cut..].find(['\\', '/']) {
1571        cut += at + 1;
1572        let galley = lay(&format!("…\\{}", &context[cut..]));
1573        let fits = !galley.elided;
1574        shortest = Some(galley);
1575        if fits {
1576            break;
1577        }
1578    }
1579    // Not even the last component fits. The name is still first, so what is on screen is the
1580    // name and as much of the context as there was room for, which is the right way round.
1581    shortest.unwrap_or(whole)
1582}
1583
1584/// The count, the selection, and how long the scan took.
1585///
1586/// The timing is not decoration: a file manager that claims to be fast should be
1587/// willing to be checked, and a folder that suddenly takes 200ms is how you find
1588/// out something is wrong.
1589fn status_line(
1590    ui: &Ui,
1591    t: &Theme,
1592    rect: Rect,
1593    tab: &Tab,
1594    override_text: Option<&str>,
1595    now: f64,
1596    scratch: &mut String,
1597) {
1598    use std::fmt::Write as _;
1599
1600    ui.painter()
1601        .rect_filled(rect, CornerRadius::ZERO, t.bg.layer_alt);
1602    ui.painter().line_segment(
1603        [rect.left_top(), rect.right_top()],
1604        Stroke::new(1.0, t.stroke.subtle),
1605    );
1606
1607    // A copy in progress or something that went wrong displaces the counts: it is the
1608    // more urgent fact, and the counts have not changed anyway.
1609    if let Some(text) = override_text {
1610        let inner = Rect::from_min_max(
1611            pos2(rect.left() + space::S3, rect.top()),
1612            pos2(rect.right() - space::S3, rect.bottom()),
1613        );
1614        let galley = truncated(
1615            ui.painter(),
1616            text,
1617            t.fonts.caption.clone(),
1618            t.text.primary,
1619            inner.width(),
1620        );
1621        text_left(ui.painter(), inner, galley);
1622        return;
1623    }
1624
1625    scratch.clear();
1626    match &tab.dir {
1627        // Nothing until the wait is worth mentioning, and then the same word the body uses.
1628        None if tab.waiting_visibly(now) => scratch.push_str("Reading…"),
1629        None => {}
1630        Some(dir) if dir.error.is_some() => scratch.push_str("Could not be read"),
1631        Some(dir) => {
1632            let shown = tab.order.len();
1633            if shown == dir.len() {
1634                // Nothing is being held back, so the breakdown the scan already
1635                // counted is more use than a total.
1636                let (folders, files) = (dir.dir_count, dir.file_count);
1637                match (folders, files) {
1638                    (0, n) => {
1639                        let _ = write!(scratch, "{n} file{}", plural(n));
1640                    }
1641                    (n, 0) => {
1642                        let _ = write!(scratch, "{n} folder{}", plural(n));
1643                    }
1644                    (d, f) => {
1645                        let _ = write!(
1646                            scratch,
1647                            "{d} folder{}, {f} file{}",
1648                            plural(d),
1649                            plural(f)
1650                        );
1651                    }
1652                }
1653            } else {
1654                let _ = write!(scratch, "{shown} of {} items", dir.len());
1655            }
1656            if tab.selected_count > 0 {
1657                let _ = write!(scratch, "   ·   {} selected", tab.selected_count);
1658            }
1659            if dir.total_size > 0 {
1660                scratch.push_str("   ·   ");
1661                fmt::size(dir.total_size, scratch);
1662            }
1663            // A flatten that hit its limit has to say so. A listing quietly missing rows is
1664            // the one wrong answer a file manager must never give: everything else here can
1665            // be checked against the folder, and this cannot.
1666            if dir.truncated {
1667                scratch.push_str("   ·   stopped at the limit — not all of the tree is here");
1668            }
1669        }
1670    }
1671
1672    let inner = Rect::from_min_max(
1673        pos2(rect.left() + space::S3, rect.top()),
1674        pos2(rect.right() - space::S3, rect.bottom()),
1675    );
1676    let galley = truncated(
1677        ui.painter(),
1678        scratch,
1679        t.fonts.caption.clone(),
1680        t.text.tertiary,
1681        inner.width() - 70.0,
1682    );
1683    text_left(ui.painter(), inner, galley);
1684
1685    if let Some(dir) = &tab.dir {
1686        if dir.error.is_none() {
1687            scratch.clear();
1688            let millis = dir.scan_micros as f64 / 1000.0;
1689            let _ = if millis < 10.0 {
1690                write!(scratch, "{millis:.1} ms")
1691            } else {
1692                write!(scratch, "{millis:.0} ms")
1693            };
1694            let galley = truncated(
1695                ui.painter(),
1696                scratch,
1697                t.fonts.caption.clone(),
1698                t.text.disabled,
1699                70.0,
1700            );
1701            text_right(ui.painter(), inner, galley);
1702        }
1703    }
1704}
1705
1706#[cfg(test)]
1707mod tests {
1708    use super::*;
1709
1710    /// A rename field is as wide as its text, not as wide as the column.
1711    ///
1712    /// The Name column is whatever the other three leave, which on a narrow pane is narrower
1713    /// than plenty of names — and a field pinned to it scrolls the name sideways under the
1714    /// caret, so renaming a long one meant editing it through a letterbox. It grows instead,
1715    /// over Size, Type and Modified, and stops at the row's right edge.
1716    #[test]
1717    fn a_rename_field_grows_past_its_column_but_not_past_the_row() {
1718        // A name cell from 40 to 160 in a row 600 wide.
1719        let (left, right, row_right) = (40.0, 160.0, 600.0);
1720        let column = right - left;
1721
1722        // Short text: the column's width, unchanged. Nothing overlaps that does not need to.
1723        assert_eq!(rename_width(30.0, left, right, row_right), column);
1724        // Text that just fits stays put too.
1725        assert_eq!(rename_width(column - 12.0, left, right, row_right), column);
1726
1727        // Longer than the column: wide enough for the text, and wider than the column.
1728        let long = rename_width(300.0, left, right, row_right);
1729        assert!(
1730            long > column,
1731            "a name wider than its column got a field the width of the column ({long})"
1732        );
1733        assert!(
1734            long >= 300.0,
1735            "the field is narrower than the text it has to show ({long})"
1736        );
1737
1738        // Never past the row. A 5000px name gets the rest of the row and no more.
1739        let huge = rename_width(5_000.0, left, right, row_right);
1740        assert!(
1741            left + huge <= row_right,
1742            "the field runs {} past the right edge of the row",
1743            left + huge - row_right
1744        );
1745        // And on a pane too narrow for any of this, still something to type in.
1746        assert!(rename_width(400.0, 40.0, 44.0, 60.0) >= MIN_FIELD);
1747    }
1748
1749    /// The Name cell: the name, then its context in secondary ink, and the context is what
1750    /// gives way when the column is narrow.
1751    ///
1752    /// Measured against the real font, because every claim here is about what fits: the widths
1753    /// below are found by laying the text out, not assumed.
1754    #[test]
1755    fn a_row_says_where_it_is_after_its_name_and_gives_that_up_first() {
1756        let ctx = egui::Context::default();
1757        azur_egui_theme::fonts::install(&ctx);
1758        let font = egui::FontId::proportional(14.0);
1759        let (ink, dim) = (egui::Color32::WHITE, egui::Color32::GRAY);
1760        let mut checked = false;
1761        let _ = ctx.run_ui(Default::default(), |ui| {
1762            let painter = ui.painter();
1763            let width_of = |text: &str| {
1764                crate::ui::truncated(painter, text, font.clone(), ink, f32::INFINITY)
1765                    .size()
1766                    .x
1767            };
1768            let cell = |context: Option<&str>, width: f32| {
1769                name_galley(
1770                    painter,
1771                    "translations_fr.json",
1772                    context,
1773                    font.clone(),
1774                    ink,
1775                    dim,
1776                    width,
1777                )
1778            };
1779
1780            // No context — a file in the folder you are looking at, which is not a shortcut.
1781            // One section, in body ink, and nothing else drawn.
1782            let plain = cell(None, 400.0);
1783            assert_eq!(plain.text(), "translations_fr.json");
1784            assert_eq!(plain.job.sections.len(), 1);
1785            assert_eq!(plain.job.sections[0].format.color, ink);
1786
1787            // With context: `name > where`, and **everything from the separator on is
1788            // secondary**, which is the whole of what makes the name the thing you read.
1789            const WHERE: &str = "PluginGeosystem\\Resources\\Lang";
1790            let whole = cell(Some(WHERE), 900.0);
1791            assert_eq!(whole.text(), format!("translations_fr.json > {WHERE}"));
1792            assert!(!whole.elided);
1793            assert_eq!(whole.job.sections.len(), 2);
1794            assert_eq!(whole.job.sections[0].format.color, ink);
1795            assert_eq!(
1796                whole.job.sections[1].format.color, dim,
1797                "the context is drawn in the same ink as the name"
1798            );
1799            let dimmed = whole.job.sections[1].byte_range.clone();
1800            let separator = &whole.text()[dimmed.start.0..dimmed.end.0];
1801            assert!(
1802                separator.starts_with(" > "),
1803                "the separator belongs to the dimmed half, and got {separator:?}"
1804            );
1805
1806            // Narrower: the context loses its *leading* folders, not its last one. The folder
1807            // immediately holding the file is what identifies it.
1808            let tail = "translations_fr.json > …\\Lang";
1809            let cut = cell(Some(WHERE), width_of(tail) + 4.0);
1810            assert_eq!(
1811                cut.text(),
1812                tail,
1813                "the folders furthest from the file are what should have gone"
1814            );
1815
1816            // Narrower still: the name survives and the context is whatever fits. The name is
1817            // first in the job, so it is the last thing egui cuts.
1818            let squeezed = cell(Some(WHERE), width_of("translations_fr.json") + 6.0);
1819            assert!(
1820                squeezed.text().starts_with("translations_fr.json"),
1821                "what survived was {:?}, which is not the name",
1822                squeezed.text()
1823            );
1824            checked = true;
1825        });
1826        assert!(checked, "the pass never ran, so nothing was checked");
1827    }
1828
1829    /// What a rename starts with selected.
1830    ///
1831    /// Explorer selects the stem, so that typing replaces the name and leaves the extension
1832    /// alone. The awkward cases are the point: a dotfile has no extension to speak of, a
1833    /// double extension only counts the last one, and a folder is a folder whatever is in its
1834    /// name.
1835    #[test]
1836    fn a_rename_selects_the_name_and_not_the_extension() {
1837        assert_eq!(stem_chars("report.docx", false), "report".len());
1838        assert_eq!(stem_chars("archive.tar.gz", false), "archive.tar".len());
1839        // A dotfile is all name.
1840        assert_eq!(stem_chars(".gitignore", false), ".gitignore".chars().count());
1841        // No extension at all.
1842        assert_eq!(stem_chars("Makefile", false), "Makefile".len());
1843        // A folder called `my.folder` is not a `folder` file.
1844        assert_eq!(stem_chars("my.folder", true), "my.folder".chars().count());
1845        // Counted in characters, not bytes, or the caret lands mid-glyph: `réunion` is seven
1846        // characters and eight bytes, so this number is the whole point of the assertion.
1847        assert_eq!(stem_chars("réunion.txt", false), 7);
1848        assert_eq!("réunion".len(), 8, "and it would be 8 if this counted bytes");
1849        assert_eq!(stem_chars("", false), 0);
1850    }
1851}
