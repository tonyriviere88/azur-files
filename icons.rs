1//! The application's glyphs, painted rather than loaded.
2//!
3//! Same shape as [`azur_egui_theme::icons`] — `fn(&Painter, Rect, Color32)` — so
4//! these are interchangeable with the design system's own and can be handed to any
5//! Azur component that takes an icon.
6//!
7//! Painted, for the same reason the theme paints its own: nothing to embed,
8//! nothing to rasterise, and a glyph that is sharp at any size. It also sidesteps
9//! the thing that makes shell-based file managers slow — asking the operating
10//! system for a per-file icon is a registry walk, an image load and a cache miss
11//! per row, and it is why Explorer's window can be visible for a second before its
12//! icons are.
13//!
14//! # The one convention
15//!
16//! **Folders are filled; files are outlined.** A folder is a place you can go and
17//! a file is a thing that is there, and at 14 pixels that difference has to be
18//! legible before any colour is. Every glyph is laid out on a 16-unit grid mapped
19//! onto whatever rect it is handed, so the whole set stays visually consistent as
20//! the row height changes.
21
22use egui::{pos2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};
23
24/// A 16-unit design grid mapped onto a rect.
25struct Grid {
26    origin: Pos2,
27    scale: f32,
28}
29
30impl Grid {
31    fn new(rect: Rect) -> Self {
32        // The largest centred square, so a glyph in a non-square box is not
33        // stretched.
34        let side = rect.width().min(rect.height());
35        let square = Rect::from_center_size(rect.center(), Vec2::splat(side));
36        Self {
37            origin: square.min,
38            scale: side / 16.0,
39        }
40    }
41
42    /// A point in grid units.
43    #[inline]
44    fn at(&self, x: f32, y: f32) -> Pos2 {
45        pos2(self.origin.x + x * self.scale, self.origin.y + y * self.scale)
46    }
47
48    /// A rect in grid units.
49    #[inline]
50    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
51        Rect::from_min_max(self.at(x0, y0), self.at(x1, y1))
52    }
53
54    /// Corner radius in grid units, as egui's `u8` of pixels.
55    #[inline]
56    fn radius(&self, units: f32) -> CornerRadius {
57        CornerRadius::same((units * self.scale).round().clamp(0.0, 255.0) as u8)
58    }
59
60    /// The stroke an outlined glyph is drawn with: 1.5 units, floored at one
61    /// device pixel so a small icon stays visible rather than fading.
62    #[inline]
63    fn stroke(&self, color: Color32) -> Stroke {
64        Stroke::new((self.scale * 1.5).max(1.0), color)
65    }
66
67    /// A thinner line, for interior detail that must not compete with the outline.
68    #[inline]
69    fn hairline(&self, color: Color32) -> Stroke {
70        Stroke::new((self.scale * 1.15).max(1.0), color)
71    }
72}
73
74fn path(p: &Painter, points: Vec<Pos2>, stroke: Stroke) {
75    p.add(Shape::line(points, stroke));
76}
77
78fn closed(p: &Painter, points: Vec<Pos2>, stroke: Stroke) {
79    p.add(Shape::closed_line(points, stroke));
80}
81
82fn fill(p: &Painter, points: Vec<Pos2>, color: Color32) {
83    p.add(Shape::convex_polygon(points, color, Stroke::NONE));
84}
85
86// ---------------------------------------------------------------------------
87// Folders — filled, because a place is not a thing
88// ---------------------------------------------------------------------------
89
90/// A folder: the tab behind, the body in front.
91pub fn folder(p: &Painter, rect: Rect, color: Color32) {
92    let g = Grid::new(rect);
93    // The tab, drawn first so the body's top edge cuts across it cleanly.
94    p.rect_filled(g.rect(1.5, 2.6, 7.5, 5.6), g.radius(0.9), color);
95    p.rect_filled(g.rect(1.0, 4.4, 15.0, 13.6), g.radius(1.3), color);
96}
97
98/// An open folder — the front face tilted forward. For the folder you are in.
99pub fn folder_open(p: &Painter, rect: Rect, color: Color32) {
100    let g = Grid::new(rect);
101    p.rect_filled(g.rect(1.5, 2.6, 7.5, 5.6), g.radius(0.9), color);
102    p.rect_filled(g.rect(1.0, 4.4, 13.0, 11.0), g.radius(1.3), color);
103    // The front flap, sheared right so it reads as leaning open.
104    fill(
105        p,
106        vec![
107            g.at(3.0, 7.0),
108            g.at(16.0, 7.0),
109            g.at(13.6, 13.6),
110            g.at(0.6, 13.6),
111        ],
112        color,
113    );
114}
115
116/// A folder with a link badge — a junction or a symlinked directory.
117pub fn folder_link(p: &Painter, rect: Rect, color: Color32) {
118    folder(p, rect, color);
119    let g = Grid::new(rect);
120    // An arrow in the bottom-right corner, knocked through the fill.
121    let stroke = Stroke::new((g.scale * 1.6).max(1.2), color);
122    path(
123        p,
124        vec![g.at(9.0, 12.0), g.at(14.0, 12.0), g.at(14.0, 7.0)],
125        stroke,
126    );
127}
128
129// ---------------------------------------------------------------------------
130// Files — outlined
131// ---------------------------------------------------------------------------
132
133/// The page every file glyph is built on: a sheet with the corner turned down.
134fn page(p: &Painter, g: &Grid, color: Color32) {
135    let stroke = g.stroke(color);
136    closed(
137        p,
138        vec![
139            g.at(3.5, 1.5),
140            g.at(9.5, 1.5),
141            g.at(12.5, 4.5),
142            g.at(12.5, 14.5),
143            g.at(3.5, 14.5),
144        ],
145        stroke,
146    );
147    // The fold, which is what stops the shape reading as a plain rectangle.
148    path(
149        p,
150        vec![g.at(9.5, 1.5), g.at(9.5, 4.5), g.at(12.5, 4.5)],
151        g.hairline(color),
152    );
153}
154
155/// A file of no particular kind.
156pub fn file(p: &Painter, rect: Rect, color: Color32) {
157    page(p, &Grid::new(rect), color);
158}
159
160/// A document: a page with writing on it.
161pub fn document(p: &Painter, rect: Rect, color: Color32) {
162    let g = Grid::new(rect);
163    page(p, &g, color);
164    let stroke = g.hairline(color);
165    for (i, width) in [4.5, 4.5, 3.0].into_iter().enumerate() {
166        let y = 7.5 + i as f32 * 2.3;
167        p.line_segment([g.at(5.5, y), g.at(5.5 + width, y)], stroke);
168    }
169}
170
171/// An image: a frame with a horizon and a sun.
172pub fn image(p: &Painter, rect: Rect, color: Color32) {
173    let g = Grid::new(rect);
174    let stroke = g.stroke(color);
175    p.rect_stroke(
176        g.rect(1.5, 2.5, 14.5, 13.5),
177        g.radius(1.3),
178        stroke,
179        StrokeKind::Middle,
180    );
181    p.circle_filled(g.at(5.4, 6.2), g.scale * 1.15, color);
182    // The hill, clipped by the frame it sits in.
183    path(
184        p,
185        vec![
186            g.at(2.0, 12.0),
187            g.at(6.2, 8.2),
188            g.at(9.4, 11.0),
189            g.at(11.4, 9.2),
190            g.at(14.0, 11.8),
191        ],
192        g.hairline(color),
193    );
194}
195
196/// A quaver: the note head and its stem.
197pub fn audio(p: &Painter, rect: Rect, color: Color32) {
198    let g = Grid::new(rect);
199    let stroke = g.stroke(color);
200    p.circle_stroke(g.at(5.0, 11.6), g.scale * 2.2, stroke);
201    path(p, vec![g.at(7.2, 11.6), g.at(7.2, 3.0)], stroke);
202    // The flag, which is what makes it a note rather than a lollipop.
203    path(
204        p,
205        vec![g.at(7.2, 3.0), g.at(12.6, 4.6), g.at(12.6, 7.0)],
206        g.hairline(color),
207    );
208}
209
210/// Video: a play triangle in a frame.
211pub fn video(p: &Painter, rect: Rect, color: Color32) {
212    let g = Grid::new(rect);
213    p.rect_stroke(
214        g.rect(1.5, 3.0, 14.5, 13.0),
215        g.radius(1.3),
216        g.stroke(color),
217        StrokeKind::Middle,
218    );
219    fill(
220        p,
221        vec![g.at(6.6, 5.6), g.at(11.0, 8.0), g.at(6.6, 10.4)],
222        color,
223    );
224}
225
226/// An archive: a box with a band and a clasp.
227pub fn archive(p: &Painter, rect: Rect, color: Color32) {
228    let g = Grid::new(rect);
229    let stroke = g.stroke(color);
230    p.rect_stroke(
231        g.rect(2.0, 2.5, 14.0, 13.5),
232        g.radius(1.3),
233        stroke,
234        StrokeKind::Middle,
235    );
236    p.line_segment([g.at(2.0, 6.2), g.at(14.0, 6.2)], g.hairline(color));
237    p.rect_filled(g.rect(7.0, 8.2, 9.0, 10.6), g.radius(0.5), color);
238}
239
240/// Code: the angle brackets, which is what a source file looks like.
241pub fn code(p: &Painter, rect: Rect, color: Color32) {
242    let g = Grid::new(rect);
243    let stroke = g.stroke(color);
244    path(
245        p,
246        vec![g.at(5.6, 4.6), g.at(1.8, 8.0), g.at(5.6, 11.4)],
247        stroke,
248    );
249    path(
250        p,
251        vec![g.at(10.4, 4.6), g.at(14.2, 8.0), g.at(10.4, 11.4)],
252        stroke,
253    );
254    p.line_segment([g.at(9.2, 3.4), g.at(6.8, 12.6)], g.hairline(color));
255}
256
257/// An executable: a chip, legs and all.
258pub fn executable(p: &Painter, rect: Rect, color: Color32) {
259    let g = Grid::new(rect);
260    let stroke = g.stroke(color);
261    p.rect_stroke(
262        g.rect(4.0, 4.0, 12.0, 12.0),
263        g.radius(1.0),
264        stroke,
265        StrokeKind::Middle,
266    );
267    p.rect_filled(g.rect(6.6, 6.6, 9.4, 9.4), g.radius(0.4), color);
268    let leg = g.hairline(color);
269    for offset in [6.0, 8.0, 10.0] {
270        p.line_segment([g.at(offset, 2.0), g.at(offset, 4.0)], leg);
271        p.line_segment([g.at(offset, 12.0), g.at(offset, 14.0)], leg);
272        p.line_segment([g.at(2.0, offset), g.at(4.0, offset)], leg);
273        p.line_segment([g.at(12.0, offset), g.at(14.0, offset)], leg);
274    }
275}
276
277/// A font: a serifed A on a baseline.
278pub fn font(p: &Painter, rect: Rect, color: Color32) {
279    let g = Grid::new(rect);
280    let stroke = g.stroke(color);
281    path(p, vec![g.at(4.0, 12.0), g.at(8.0, 3.2), g.at(12.0, 12.0)], stroke);
282    p.line_segment([g.at(5.6, 9.4), g.at(10.4, 9.4)], g.hairline(color));
283    p.line_segment([g.at(2.6, 14.2), g.at(13.4, 14.2)], g.hairline(color));
284}
285
286/// A model: a wireframe cube. Meshes, point clouds, CAD.
287pub fn model(p: &Painter, rect: Rect, color: Color32) {
288    let g = Grid::new(rect);
289    let stroke = g.stroke(color);
290    // The silhouette is a hexagon; the three interior edges make it a solid.
291    closed(
292        p,
293        vec![
294            g.at(8.0, 1.8),
295            g.at(14.2, 5.2),
296            g.at(14.2, 10.8),
297            g.at(8.0, 14.2),
298            g.at(1.8, 10.8),
299            g.at(1.8, 5.2),
300        ],
301        stroke,
302    );
303    let inner = g.hairline(color);
304    p.line_segment([g.at(8.0, 8.0), g.at(8.0, 14.2)], inner);
305    p.line_segment([g.at(8.0, 8.0), g.at(1.8, 5.2)], inner);
306    p.line_segment([g.at(8.0, 8.0), g.at(14.2, 5.2)], inner);
307}
308
309/// Data: the database cylinder.
310pub fn data(p: &Painter, rect: Rect, color: Color32) {
311    let g = Grid::new(rect);
312    let stroke = g.stroke(color);
313    let radius = g.scale * 5.0;
314    for y in [3.6, 8.0, 12.4] {
315        // A flat ellipse out of a scaled circle, which is cheaper than a path and
316        // indistinguishable at this size.
317        ellipse(p, g.at(8.0, y), Vec2::new(radius, g.scale * 1.7), stroke);
318    }
319    p.line_segment([g.at(3.0, 3.6), g.at(3.0, 12.4)], stroke);
320    p.line_segment([g.at(13.0, 3.6), g.at(13.0, 12.4)], stroke);
321}
322
323fn ellipse(p: &Painter, center: Pos2, radii: Vec2, stroke: Stroke) {
324    const STEPS: usize = 20;
325    let points = (0..=STEPS)
326        .map(|i| {
327            let a = i as f32 / STEPS as f32 * std::f32::consts::TAU;
328            center + Vec2::new(a.cos() * radii.x, a.sin() * radii.y)
329        })
330        .collect();
331    p.add(Shape::line(points, stroke));
332}
333
334/// The glyph for a file kind.
335pub fn for_kind(kind: crate::fs::fmt::Kind) -> azur_egui_theme::icons::Icon<'static> {
336    use crate::fs::fmt::Kind;
337    match kind {
338        Kind::Folder => &folder,
339        Kind::Image => &image,
340        Kind::Audio => &audio,
341        Kind::Video => &video,
342        Kind::Archive => &archive,
343        Kind::Code => &code,
344        Kind::Document => &document,
345        Kind::Executable => &executable,
346        Kind::Font => &font,
347        Kind::Model => &model,
348        Kind::Data => &data,
349        Kind::Other => &file,
350    }
351}
352
353// ---------------------------------------------------------------------------
354// Places and volumes
355// ---------------------------------------------------------------------------
356
357/// This PC: a desktop tower.
358pub fn this_pc(p: &Painter, rect: Rect, color: Color32) {
359    let g = Grid::new(rect);
360    let stroke = g.stroke(color);
361    p.rect_stroke(
362        g.rect(4.0, 1.8, 12.0, 14.2),
363        g.radius(1.2),
364        stroke,
365        StrokeKind::Middle,
366    );
367    let detail = g.hairline(color);
368    p.line_segment([g.at(6.2, 4.4), g.at(9.8, 4.4)], detail);
369    p.line_segment([g.at(6.2, 6.4), g.at(9.8, 6.4)], detail);
370    p.circle_filled(g.at(8.0, 11.4), g.scale * 1.1, color);
371}
372
373/// A monitor on a stand.
374pub fn desktop(p: &Painter, rect: Rect, color: Color32) {
375    let g = Grid::new(rect);
376    let stroke = g.stroke(color);
377    p.rect_stroke(
378        g.rect(1.6, 2.6, 14.4, 11.0),
379        g.radius(1.2),
380        stroke,
381        StrokeKind::Middle,
382    );
383    p.line_segment([g.at(8.0, 11.0), g.at(8.0, 13.4)], g.hairline(color));
384    p.line_segment([g.at(5.0, 13.6), g.at(11.0, 13.6)], stroke);
385}
386
387/// A house.
388pub fn home(p: &Painter, rect: Rect, color: Color32) {
389    let g = Grid::new(rect);
390    let stroke = g.stroke(color);
391    path(
392        p,
393        vec![
394            g.at(1.8, 7.6),
395            g.at(8.0, 2.2),
396            g.at(14.2, 7.6),
397        ],
398        stroke,
399    );
400    path(
401        p,
402        vec![
403            g.at(3.4, 7.0),
404            g.at(3.4, 14.0),
405            g.at(12.6, 14.0),
406            g.at(12.6, 7.0),
407        ],
408        stroke,
409    );
410    p.line_segment([g.at(6.6, 14.0), g.at(6.6, 10.2)], g.hairline(color));
411    p.line_segment([g.at(6.6, 10.2), g.at(9.4, 10.2)], g.hairline(color));
412    p.line_segment([g.at(9.4, 10.2), g.at(9.4, 14.0)], g.hairline(color));
413}
414
415/// An arrow into a tray.
416pub fn downloads(p: &Painter, rect: Rect, color: Color32) {
417    let g = Grid::new(rect);
418    let stroke = g.stroke(color);
419    p.line_segment([g.at(8.0, 1.8), g.at(8.0, 9.6)], stroke);
420    path(
421        p,
422        vec![g.at(4.8, 6.6), g.at(8.0, 9.8), g.at(11.2, 6.6)],
423        stroke,
424    );
425    path(
426        p,
427        vec![
428            g.at(2.2, 11.0),
429            g.at(2.2, 14.0),
430            g.at(13.8, 14.0),
431            g.at(13.8, 11.0),
432        ],
433        stroke,
434    );
435}
436
437/// A waste bin.
438pub fn trash(p: &Painter, rect: Rect, color: Color32) {
439    let g = Grid::new(rect);
440    let stroke = g.stroke(color);
441    p.line_segment([g.at(2.2, 4.4), g.at(13.8, 4.4)], stroke);
442    path(
443        p,
444        vec![g.at(6.2, 4.4), g.at(6.2, 2.4), g.at(9.8, 2.4), g.at(9.8, 4.4)],
445        g.hairline(color),
446    );
447    path(
448        p,
449        vec![
450            g.at(3.6, 4.4),
451            g.at(4.4, 14.0),
452            g.at(11.6, 14.0),
453            g.at(12.4, 4.4),
454        ],
455        stroke,
456    );
457    let bar = g.hairline(color);
458    p.line_segment([g.at(6.6, 6.8), g.at(6.9, 11.8)], bar);
459    p.line_segment([g.at(9.4, 6.8), g.at(9.1, 11.8)], bar);
460}
461
462/// A five-pointed star, for a bookmark. `filled` is the bookmarked state.
463fn star_shape(p: &Painter, rect: Rect, color: Color32, filled: bool) {
464    let g = Grid::new(rect);
465    let center = g.at(8.0, 8.4);
466    let (outer, inner) = (g.scale * 6.4, g.scale * 2.6);
467    let points: Vec<Pos2> = (0..10)
468        .map(|i| {
469            // Start at the top, then alternate outer and inner vertices.
470            let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
471            let radius = if i % 2 == 0 { outer } else { inner };
472            center + Vec2::new(angle.cos(), angle.sin()) * radius
473        })
474        .collect();
475    if filled {
476        // A star is not convex, so it has to be triangulated by hand: a fan from
477        // the centre, which is exact for a regular star polygon.
478        for pair in points.windows(2) {
479            p.add(Shape::convex_polygon(
480                vec![center, pair[0], pair[1]],
481                color,
482                Stroke::NONE,
483            ));
484        }
485        p.add(Shape::convex_polygon(
486            vec![center, points[9], points[0]],
487            color,
488            Stroke::NONE,
489        ));
490    } else {
491        closed(p, points, g.stroke(color));
492    }
493}
494
495/// An outlined star — not bookmarked.
496pub fn star(p: &Painter, rect: Rect, color: Color32) {
497    star_shape(p, rect, color, false);
498}
499
500/// A filled star — bookmarked.
501pub fn star_filled(p: &Painter, rect: Rect, color: Color32) {
502    star_shape(p, rect, color, true);
503}
504
505/// A fixed disk: the drive body and its activity light.
506pub fn drive(p: &Painter, rect: Rect, color: Color32) {
507    let g = Grid::new(rect);
508    let stroke = g.stroke(color);
509    p.rect_stroke(
510        g.rect(1.6, 4.6, 14.4, 11.4),
511        g.radius(1.4),
512        stroke,
513        StrokeKind::Middle,
514    );
515    p.circle_filled(g.at(11.6, 8.0), g.scale * 1.1, color);
516    p.line_segment([g.at(4.0, 8.0), g.at(8.4, 8.0)], g.hairline(color));
517}
518
519/// A USB stick.
520pub fn drive_removable(p: &Painter, rect: Rect, color: Color32) {
521    let g = Grid::new(rect);
522    let stroke = g.stroke(color);
523    p.rect_stroke(
524        g.rect(5.0, 5.6, 11.0, 14.2),
525        g.radius(1.0),
526        stroke,
527        StrokeKind::Middle,
528    );
529    p.rect_stroke(
530        g.rect(6.4, 1.8, 9.6, 5.6),
531        g.radius(0.5),
532        g.hairline(color),
533        StrokeKind::Middle,
534    );
535    p.line_segment([g.at(7.0, 9.0), g.at(9.0, 9.0)], g.hairline(color));
536}
537
538/// An optical disc.
539pub fn drive_optical(p: &Painter, rect: Rect, color: Color32) {
540    let g = Grid::new(rect);
541    p.circle_stroke(g.at(8.0, 8.0), g.scale * 6.2, g.stroke(color));
542    p.circle_stroke(g.at(8.0, 8.0), g.scale * 1.9, g.hairline(color));
543}
544
545/// A network share: a box with a plug lead.
546pub fn drive_network(p: &Painter, rect: Rect, color: Color32) {
547    let g = Grid::new(rect);
548    let stroke = g.stroke(color);
549    p.rect_stroke(
550        g.rect(1.8, 3.0, 14.2, 8.0),
551        g.radius(1.0),
552        stroke,
553        StrokeKind::Middle,
554    );
555    p.circle_filled(g.at(11.8, 5.5), g.scale * 0.9, color);
556    let lead = g.hairline(color);
557    p.line_segment([g.at(8.0, 8.0), g.at(8.0, 11.0)], lead);
558    p.line_segment([g.at(3.6, 11.0), g.at(12.4, 11.0)], lead);
559    p.line_segment([g.at(3.6, 11.0), g.at(3.6, 13.6)], lead);
560    p.line_segment([g.at(12.4, 11.0), g.at(12.4, 13.6)], lead);
561}
562
563/// The glyph for a volume kind.
564pub fn for_drive(kind: crate::fs::drives::DriveKind) -> azur_egui_theme::icons::Icon<'static> {
565    use crate::fs::drives::DriveKind;
566    match kind {
567        DriveKind::Removable => &drive_removable,
568        DriveKind::Optical => &drive_optical,
569        DriveKind::Network => &drive_network,
570        _ => &drive,
571    }
572}
573
574/// The glyph for a sidebar place.
575pub fn for_place(icon: crate::fs::places::PlaceIcon) -> azur_egui_theme::icons::Icon<'static> {
576    use crate::fs::places::PlaceIcon;
577    match icon {
578        PlaceIcon::ThisPc => &this_pc,
579        PlaceIcon::Home => &home,
580        PlaceIcon::Desktop => &desktop,
581        PlaceIcon::Documents => &document,
582        PlaceIcon::Downloads => &downloads,
583        PlaceIcon::Music => &audio,
584        PlaceIcon::Pictures => &image,
585        PlaceIcon::Videos => &video,
586        PlaceIcon::Trash => &trash,
587    }
588}
589
590// ---------------------------------------------------------------------------
591// Navigation and commands
592// ---------------------------------------------------------------------------
593
594/// An arrow with a shaft, pointing in `turns` quarter-turns clockwise from up.
595fn arrow(p: &Painter, rect: Rect, color: Color32, turns: f32) {
596    let g = Grid::new(rect);
597    let center = g.at(8.0, 8.0);
598    let (sin, cos) = (turns * std::f32::consts::FRAC_PI_2).sin_cos();
599    let turn = |x: f32, y: f32| {
600        let (dx, dy) = (x - 8.0, y - 8.0);
601        pos2(
602            center.x + (dx * cos - dy * sin) * g.scale,
603            center.y + (dx * sin + dy * cos) * g.scale,
604        )
605    };
606    let stroke = g.stroke(color);
607    p.line_segment([turn(8.0, 13.0), turn(8.0, 3.4)], stroke);
608    path(
609        p,
610        vec![turn(3.8, 7.6), turn(8.0, 3.4), turn(12.2, 7.6)],
611        stroke,
612    );
613}
614
615/// Back.
616pub fn arrow_left(p: &Painter, rect: Rect, color: Color32) {
617    arrow(p, rect, color, 3.0);
618}
619
620/// Forward.
621pub fn arrow_right(p: &Painter, rect: Rect, color: Color32) {
622    arrow(p, rect, color, 1.0);
623}
624
625/// Up one level.
626pub fn arrow_up(p: &Painter, rect: Rect, color: Color32) {
627    arrow(p, rect, color, 0.0);
628}
629
630/// Refresh: a nearly-closed circle with an arrowhead.
631pub fn refresh(p: &Painter, rect: Rect, color: Color32) {
632    let g = Grid::new(rect);
633    let stroke = g.stroke(color);
634    let center = g.at(8.0, 8.0);
635    let radius = g.scale * 5.4;
636
637    // Three-quarters of a turn, leaving the gap at the top right for the head.
638    const STEPS: usize = 24;
639    let start = -std::f32::consts::FRAC_PI_2 + 0.5;
640    let sweep = std::f32::consts::TAU * 0.82;
641    let points = (0..=STEPS)
642        .map(|i| {
643            let a = start + sweep * i as f32 / STEPS as f32;
644            center + Vec2::new(a.cos(), a.sin()) * radius
645        })
646        .collect();
647    p.add(Shape::line(points, stroke));
648
649    let tip = center + Vec2::new(start.cos(), start.sin()) * radius;
650    fill(
651        p,
652        vec![
653            tip + Vec2::new(-g.scale * 0.4, -g.scale * 2.6),
654            tip + Vec2::new(g.scale * 3.0, -g.scale * 0.9),
655            tip + Vec2::new(-g.scale * 0.9, g.scale * 1.4),
656        ],
657        color,
658    );
659}
660
661/// A pen lying at 45° — the path bar's "this can be typed into" hint.
662///
663/// Filled rather than outlined, which is the one place this set's own convention bends and it
664/// bends for legibility: at 14 pixels an outlined pencil is three near-parallel hairlines a
665/// pixel apart, and what reads at that size is the silhouette. It is not a file or a folder,
666/// so the rule those two divide has nothing to say about it.
667///
668/// Two shapes, not one: the barrel is a quadrilateral and the nib is the triangle that finishes
669/// it, and keeping them apart is what lets the nib be sharper than a fifth corner on a convex
670/// polygon would be.
671/// It fills the grid corner to corner, which is the other half of being legible at this size:
672/// a pen inset the way a folder is would be drawing a 10-unit glyph in a 16-unit box.
673///
674/// **The ink is symmetric about the centre of the grid — `2..14` on both axes** — because this
675/// is drawn into a row, and a glyph in a row is centred by what you can see rather than by the
676/// box it was handed. The first version of it spanned `4..16` vertically: the box was centred on
677/// the path bar to the pixel and the pen still sat two units low, which is the one kind of
678/// mistake in a row that everything says is correct and nothing looks it.
679pub fn pencil(p: &Painter, rect: Rect, color: Color32) {
680    let g = Grid::new(rect);
681    // The barrel: a 3.4-unit-wide bar at 45°, from the nib end (bottom left) to the butt.
682    fill(
683        p,
684        vec![
685            g.at(3.1, 10.5),
686            g.at(11.6, 2.0),
687            g.at(14.0, 4.4),
688            g.at(5.5, 12.9),
689        ],
690        color,
691    );
692    // The nib: the tip, and the two barrel corners it comes to a point from — so the two
693    // polygons share an edge and the join cannot show.
694    fill(
695        p,
696        vec![g.at(2.0, 14.0), g.at(3.1, 10.5), g.at(5.5, 12.9)],
697        color,
698    );
699}
700
701/// A funnel — the filter box's affordance.
702pub fn filter(p: &Painter, rect: Rect, color: Color32) {
703    let g = Grid::new(rect);
704    let stroke = g.stroke(color);
705    path(
706        p,
707        vec![
708            g.at(2.2, 3.2),
709            g.at(13.8, 3.2),
710            g.at(9.4, 8.4),
711            g.at(9.4, 13.6),
712            g.at(6.6, 12.2),
713            g.at(6.6, 8.4),
714        ],
715        stroke,
716    );
717    p.line_segment([g.at(2.2, 3.2), g.at(6.6, 8.4)], stroke);
718}
719
720/// Three rows stepping in — a hierarchy. The path bar's flatten toggle.
721///
722/// The art is the *tree*, not the flat list the button produces, for the same reason the
723/// hidden-files toggle shows an eye rather than a revealed file: what a toggle names is the
724/// thing it is about, and the two states are said by the fill and the colour
725/// [`crate::ui::tool_button`] gives a latched button. A flat list drawn as three flush bars
726/// would be indistinguishable from a generic "list" glyph in any state.
727///
728/// Three strokes and nothing else, because this is drawn at 14 pixels: an arrowhead small
729/// enough to fit beside them would be under three pixels wide, which at this weight is a blot
730/// rather than a shape. The rows are right-aligned and step in by 3.6 units, which is what makes
731/// the shape read as indentation rather than as a list.
732///
733/// The ink spans 2..14 on both axes, symmetric about the centre of the grid, so centring the
734/// box centres the drawing — see [`pencil`], where getting that wrong cost two units.
735pub fn flatten(p: &Painter, rect: Rect, color: Color32) {
736    let g = Grid::new(rect);
737    let stroke = g.stroke(color);
738    for (indent, y) in [(2.0, 3.4), (5.6, 8.0), (9.2, 12.6)] {
739        p.line_segment([g.at(indent, y), g.at(14.0, y)], stroke);
740    }
741}
742
743/// An eye — the path bar's preview toggle.
744///
745/// A lens rather than a panel-with-content, and the choice is the same one [`flatten`] makes:
746/// **what a toggle draws is the thing it is about, not the shape of what it produces.** The
747/// preview panel can be down the side or along the bottom, so a glyph of a panel would be wrong
748/// half the time; the eye says "look at what is in this" whichever way the panel opens, and the
749/// two states are said by the fill and the colour [`crate::ui::tool_button`] gives a latched
750/// button.
751///
752/// Two arcs and a pupil, at 14 pixels. The lid is a pair of quadratic curves rather than one
753/// ellipse, because an ellipse's ends are round and an eye's are points — and at this size that
754/// difference is what tells it from a circle. Eight segments a side is enough that the curve reads
755/// as smooth after the pixel snap.
756///
757/// **The bulge is 3.0 units, which is most of the drawing.** The first version used 1.8, and the
758/// arithmetic below is why that was wrong: the curve's deviation from the midline peaks at
759/// `lift × 4t(1−t)`, whose maximum is `lift` — so ±1.8 gave an eye 3.6 units tall in a 16-unit
760/// grid against 12.8 units wide, a 3.6:1 slit rather than an eye. ±3.0 is 6 units, and 12.8:6 is
761/// about the proportion an eye actually has.
762///
763/// The ink spans 1.6..14.4 across and 5.0..11.0 down, symmetric about the centre of the grid, so
764/// centring the box centres the drawing — see [`pencil`], where getting that wrong cost two units.
765pub fn eye(p: &Painter, rect: Rect, color: Color32) {
766    let g = Grid::new(rect);
767    let stroke = g.stroke(color);
768    // A quadratic through the two corners of the eye, bulging by `lift` at its middle.
769    let arc = |lift: f32| -> Vec<Pos2> {
770        const STEPS: usize = 8;
771        (0..=STEPS)
772            .map(|i| {
773                let t = i as f32 / STEPS as f32;
774                // The Bézier's own basis, written out: one control point, so this is two
775                // multiplies rather than a curve library.
776                let x = 1.6 + 12.8 * t;
777                let y = 8.0 + lift * 4.0 * t * (1.0 - t);
778                g.at(x, y)
779            })
780            .collect()
781    };
782    for lift in [-3.0, 3.0] {
783        p.add(egui::Shape::line(arc(lift), stroke));
784    }
785    p.circle_filled(g.at(8.0, 8.0), 1.9 * g.scale, color);
786}
787
788/// Four arrows leaving the middle for the corners — *fit this in the panel*.
789///
790/// It was `window_maximize`, which is a rectangle and says "a window" rather than "make this fill
791/// that". The arrows are what turn it into a verb.
792///
793/// **The corner bracket is the arrowhead**, and that is the whole trick at this size. The first
794/// version drew a box with four headed arrows inside it, and at 14 pixels it came out as a blot:
795/// each head was two 2.2-unit segments, which is about two device pixels, and the box's edge was
796/// one more pixel a unit away from them. [`flatten`] has the same note. Here every element is at
797/// least three units and there is nothing for them to collide with, so the shape resolves — and a
798/// bracket at the end of a diagonal reads as an arrow anyway, which is why every media player's
799/// fullscreen button is drawn this way.
800///
801/// The ink spans 2.4..13.6 on both axes, symmetric about the centre.
802pub fn fit(p: &Painter, rect: Rect, color: Color32) {
803    let g = Grid::new(rect);
804    let stroke = g.stroke(color);
805    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
806        let out = |along: f32, across: f32| g.at(8.0 + dx * along, 8.0 + dy * across);
807        // The shaft, from a clear gap at the middle out to the corner.
808        p.line_segment([out(1.8, 1.8), out(5.6, 5.6)], stroke);
809        // And the bracket: one arm along each axis, so the pair points where the shaft does.
810        p.line_segment([out(5.6, 5.6), out(2.2, 5.6)], stroke);
811        p.line_segment([out(5.6, 5.6), out(5.6, 2.2)], stroke);
812    }
813}
814
815/// Three panels side by side — *show both images and the difference*, rather than the difference
816/// alone.
817///
818/// Latched while all three are on show, which is what says the two you are comparing are still
819/// there. Drawn as a box divided twice rather than as three separate boxes: three boxes at 14
820/// pixels is three outlines and two gaps in 12 units, which is one pixel each and reads as hatching.
821pub fn columns(p: &Painter, rect: Rect, color: Color32) {
822    let g = Grid::new(rect);
823    let stroke = g.stroke(color);
824    p.rect_stroke(
825        g.rect(1.8, 2.8, 14.2, 13.2),
826        g.radius(1.2),
827        stroke,
828        StrokeKind::Middle,
829    );
830    for x in [6.0, 10.0] {
831        p.line_segment([g.at(x, 2.8), g.at(x, 13.2)], g.hairline(color));
832    }
833}
834
835/// A gutter with three lines beside it — *number the lines*.
836///
837/// Not digits. A `1` and a `2` at 14 pixels are three pixels wide and would be two smudges; what
838/// carries the meaning at this size is the *gutter* — a rule with short marks to its left and full
839/// lines to its right, which is the shape of a numbered listing whether or not the numerals resolve.
840pub fn line_numbers(p: &Painter, rect: Rect, color: Color32) {
841    let g = Grid::new(rect);
842    let rule = g.hairline(color);
843    p.line_segment([g.at(5.6, 2.6), g.at(5.6, 13.4)], rule);
844    for y in [4.2, 8.0, 11.8] {
845        // The number, as a mark: short, and clear of the rule.
846        p.line_segment([g.at(2.4, y), g.at(4.2, y)], g.stroke(color));
847        p.line_segment([g.at(7.2, y), g.at(14.0, y)], g.stroke(color));
848    }
849}
850
851/// Split the pane left/right. Also the drop hint for a side dock.
852pub fn split_side(p: &Painter, rect: Rect, color: Color32) {
853    let g = Grid::new(rect);
854    p.rect_stroke(
855        g.rect(1.8, 3.0, 14.2, 13.0),
856        g.radius(1.2),
857        g.stroke(color),
858        StrokeKind::Middle,
859    );
860    p.line_segment([g.at(8.0, 3.0), g.at(8.0, 13.0)], g.stroke(color));
861    p.rect_filled(g.rect(8.8, 4.6, 13.0, 11.4), g.radius(0.6), color);
862}
863
864/// Split the pane top/bottom.
865pub fn split_down(p: &Painter, rect: Rect, color: Color32) {
866    let g = Grid::new(rect);
867    p.rect_stroke(
868        g.rect(1.8, 3.0, 14.2, 13.0),
869        g.radius(1.2),
870        g.stroke(color),
871        StrokeKind::Middle,
872    );
873    p.line_segment([g.at(1.8, 8.0), g.at(14.2, 8.0)], g.stroke(color));
874    p.rect_filled(g.rect(3.2, 8.8, 12.8, 11.8), g.radius(0.6), color);
875}
876
