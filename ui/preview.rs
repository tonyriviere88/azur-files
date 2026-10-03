1//! The preview panel: what is *in* the file the keyboard is on.
2//!
3//! **Inside the pane**, not across the window, and that is the whole design. A preview belongs to
4//! the folder you are looking at — two panes side by side each get their own, showing their own
5//! selection, which is what makes comparing two builds of the same DLL a matter of looking left
6//! and right rather than clicking back and forth. So [`Preview`] hangs off [`crate::pane::Tab`]:
7//! each folder has one, shut by default.
8//!
9//! # Right, bottom, or whichever fits
10//!
11//! [`Where`] is the window's preference — one setting, not one per folder, because "where the
12//! preview goes" is a habit and "is it showing for this folder" is not. [`Where::Auto`] picks from
13//! the pane's own shape every frame: **a pane wider than it is tall gets a panel down the right,
14//! and anything squarer or taller gets one along the bottom.**
15//!
16//! The threshold is 1.25 rather than 1.0, and it leans that way deliberately: width is the scarcer
17//! thing in a listing. Four columns and a path bar need it, and taking 40% of a 500-point pane
18//! away leaves a Name column with room for `translations_f…`. Height costs a listing nothing but
19//! rows, and rows scroll.
20//!
21//! # Four views
22//!
23//! A picture, **two** pictures compared, some text, or — for a binary — [`crate::ui::deps`]'s
24//! dependency tree. Which one is [`crate::preview::kind_of`]'s answer and the *selection*'s (two
25//! pictures selected at once is the comparison), the reading is [`crate::preview::Previews`]'s, and
26//! this module is where each of them is drawn. Anything else says so rather than showing an empty
27//! box.
28//!
29//! # What goes on the bar, and what goes first when it will not fit
30//!
31//! Left to right: the name, then a comment, then the size, then the controls — and the controls are
32//! the last thing to go. What gives way, in order, is **the comment, then the size, then the name**,
33//! which is [`header`]'s one non-obvious rule: a long name never crops while a detail could have
34//! been dropped instead, because the name is what identifies the file and the size is a nicety.
35
36use std::path::Path;
37
38use azur_egui_theme::components::{galley_on_baseline, ink_baseline};
39use azur_egui_theme::icons as azur_icons;
40use azur_egui_theme::tokens::space;
41use egui::{pos2, vec2, Color32, Id, Rect, Sense, Ui, Vec2};
42
43use crate::app::Action;
44use crate::pane::PaneId;
45use crate::preview::{self, Ask, Payload};
46use crate::theme::Theme;
47use crate::ui::{deps, icon_rect, seam, text_center, tool_button, truncated, SEAM, TOOL_SIZE};
48
49/// The panel's own bar: the file's name, what it turned out to be, and the controls.
50///
51/// Shorter than a pane's path bar. This is furniture *inside* a pane, and a second 32-point bar
52/// under the first one made the pane look like two panes.
53pub const HEADER: f32 = 28.0;
54
55/// How much of the pane the panel takes to begin with, along whichever axis it is split on.
56pub const SHARE: f32 = 0.42;
57
58/// What the listing keeps, whatever the panel is dragged to.
59const MIN_LIST: f32 = 180.0;
60
61/// What a panel down the side keeps: enough for its bar's controls and a little canvas.
62///
63/// Wider than it looks like it needs to be, and the reason is the bar: the zoom field and the four
64/// buttons are a fixed cost before the name gets anything, and a panel too narrow to show its own
65/// controls is a panel with no way back to 100%. See [`ACTIONS`].
66const MIN_PANEL_W: f32 = 240.0;
67
68/// And what a panel along the bottom keeps: its bar and a little canvas. Height only — its width is
69/// the pane's, which nothing here gets to choose.
70const MIN_PANEL_H: f32 = HEADER + 64.0;
71
72/// A pane has to be wider than this multiple of its height before [`Where::Auto`] puts the panel
73/// down the side. See the module header for why it is not 1.0.
74const AUTO_RATIO: f32 = 1.25;
75
76/// The air inside the panel's bar and around its canvas.
77const PAD: f32 = space::S2;
78
79/// The glyph in the bar, saying which of the views this is.
80const GLYPH: f32 = 14.0;
81
82/// The mark beside a count that must not itself be coloured.
83const MARK: f32 = 12.0;
84
85/// The zoom field.
86///
87/// Enough for `400%` and a chevron. It is a combo box rather than a label because a zoom you can
88/// only reach through `+` and `−` is a zoom you cannot ask for: 100% from 874% is eight clicks.
89const ZOOM_W: f32 = 64.0;
90
91/// What the bar's controls cost when they are all there — the zoom field, Fit, out, in, and close.
92///
93/// Only used to size [`MIN_PANEL_W`], but worth naming: it is the number that decides how narrow a
94/// panel is allowed to be.
95const ACTIONS: f32 = ZOOM_W + TOOL_SIZE * 4.0 + PAD * 4.0;
96
97/// The strip above each image in a comparison, holding which file it is.
98const CAPTION: f32 = 16.0;
99
100/// How long the selection has to sit still before the panel follows it.
101///
102/// The same quarter second the filter waits, for the same reason and with more at stake: holding
103/// the down arrow through a folder of photographs would otherwise decode thirty of them,
104/// twenty-nine of which nobody is going to look at. See [`crate::pane::FILTER_DELAY`].
105pub const FOLLOW_DELAY: f64 = 0.25;
106
107/// Where the panel goes.
108#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
109pub enum Where {
110    #[default]
111    Right,
112    Bottom,
113    /// Whichever suits the pane's shape. See the module header.
114    Auto,
115}
116
117/// Which side it ends up on, once [`Where::Auto`] has been resolved.
118#[derive(Clone, Copy, PartialEq, Eq, Debug)]
119pub enum Side {
120    Right,
121    Bottom,
122}
123
124impl Where {
125    pub fn label(self) -> &'static str {
126        match self {
127            Self::Right => "Right",
128            Self::Bottom => "Bottom",
129            Self::Auto => "Auto",
130        }
131    }
132
133    /// The three, in the order the menu lists them.
134    pub const ALL: [Self; 3] = [Self::Right, Self::Bottom, Self::Auto];
135
136    /// Which side the panel goes on in a pane of this shape.
137    pub fn side(self, pane: Rect) -> Side {
138        match self {
139            Self::Right => Side::Right,
140            Self::Bottom => Side::Bottom,
141            Self::Auto => {
142                if pane.width() >= pane.height() * AUTO_RATIO {
143                    Side::Right
144                } else {
145                    Side::Bottom
146                }
147            }
148        }
149    }
150
151    /// For the settings file, which is a `key=value` text file people are meant to be able to fix
152    /// by hand — so the values are words rather than numbers.
153    pub fn as_str(self) -> &'static str {
154        match self {
155            Self::Right => "right",
156            Self::Bottom => "bottom",
157            Self::Auto => "auto",
158        }
159    }
160
161    pub fn parse(text: &str) -> Option<Self> {
162        Self::ALL
163            .into_iter()
164            .find(|at| at.as_str().eq_ignore_ascii_case(text))
165    }
166}
167
168/// Where the panel goes, how much room it takes, and how its text view reads: the window's
169/// preferences, one of each.
170#[derive(Clone, Copy, Debug)]
171pub struct Layout {
172    pub at: Where,
173    /// How much of the pane the panel takes along the split axis, before the clamps in
174    /// [`split`].
175    pub share: f32,
176    /// Number the lines in the text view.
177    ///
178    /// A preference and not per-file, because it is a way of reading rather than a fact about a
179    /// document: somebody who wants line numbers wants them in the next file too.
180    pub numbers: bool,
181}
182
183impl Default for Layout {
184    fn default() -> Self {
185        Self {
186            at: Where::default(),
187            share: SHARE,
188            numbers: false,
189        }
190    }
191}
192
193/// What is on the canvas.
194enum Content {
195    /// Nothing has been asked for: no selection, or one with no preview.
196    Nothing,
197    /// A selection with nothing to show, and what it was — `"zip"`, `"mp4"`.
198    Unsupported(String),
199    /// Asked for, on its way.
200    Reading,
201    Picture(Box<Picture>),
202    Text(Text),
203    Binary(deps::View),
204    /// Something went wrong, in a few words.
205    Failed(String),
206}
207
208/// One image on the canvas.
209struct Frame {
210    texture: egui::TextureHandle,
211    /// Its own size in pixels, which need not be the group's — two files being compared can be
212    /// different shapes.
213    pixels: Vec2,
214    /// What the caption above it says. Empty when there is only one frame and no caption.
215    label: String,
216    /// It is a difference mask rather than a picture, so it is drawn in the status hue over the
217    /// board rather than as it is. See [`crate::preview::Diff::mask`].
218    mask: bool,
219}
220
221/// One or three images, and how they are being looked at.
222///
223/// Three when two files are being compared: the two of them and the difference. The zoom and the
224/// pan are **shared**, which is the whole point of a comparison — three views that scrolled
225/// independently would be three views of nothing in particular.
226struct Picture {
227    frames: Vec<Frame>,
228    /// The size all frames are placed against: the larger of the two, so the same pixel lands at
229    /// the same offset in each view.
230    pixels: Vec2,
231    /// What the first file is on disk, for the bar.
232    natural: [u32; 2],
233    /// And the second's, when there is one and it differs.
234    other: Option<[u32; 2]>,
235    scaled: bool,
236    vector: bool,
237    /// The share of pixels that differ, for a comparison.
238    differing: Option<f32>,
239    /// Show both sources as well as the difference. Only meaningful with three frames.
240    all: bool,
241    /// `None` is *fit*, recomputed from the canvas every frame; `Some` is an absolute scale
242    /// somebody chose. That distinction is the whole zoom model: resizing the pane while fitted
243    /// re-fits, and resizing it while zoomed leaves the zoom alone.
244    zoom: Option<f32>,
245    /// The scale that fits the canvas, as of the last frame drawn.
246    ///
247    /// Derived rather than chosen, and stored only so the bar's buttons and field have a number to
248    /// work from — they run before the canvas is measured. Until a frame has been drawn it is 1.0,
249    /// which is a sane picture rather than a blank one.
250    fit: f32,
251    /// How far the images are dragged from where they would sit, in points.
252    pan: Vec2,
253    /// The zoom field's text, and which preset was last taken from its list.
254    ///
255    /// The text is the field's to own while it has focus — that is what makes typing `137` possible
256    /// — and is rewritten from [`Picture::percent`] on every frame it does not.
257    zoom_text: String,
258    zoom_pick: Option<usize>,
259}
260
261struct Text {
262    body: String,
263    truncated: bool,
264    /// Its columns mean something, so it is set in the monospace role.
265    code: bool,
266}
267
268/// One folder's preview panel.
269pub struct Preview {
270    pub open: bool,
271    /// What is on show, or being read.
272    of: Option<Ask>,
273    /// A selection that has not sat still long enough yet: what, and since when.
274    pending: Option<(Ask, f64)>,
275    /// The read this is waiting for, if it is waiting for one.
276    awaiting: Option<u64>,
277    content: Content,
278}
279
280impl Default for Preview {
281    fn default() -> Self {
282        Self {
283            open: false,
284            of: None,
285            pending: None,
286            awaiting: None,
287            content: Content::Nothing,
288        }
289    }
290}
291
292impl Preview {
293    /// A duplicate for a new tab of the same folder: open the same way, showing nothing yet.
294    ///
295    /// The content is deliberately not copied. A decoded picture is up to 16 MB — three of them for
296    /// a comparison — and a graph is a megabyte of names; handing a copy to every `Ctrl+T` would
297    /// make duplicating a tab the most expensive thing in the window. The panel is following the
298    /// selection anyway, so it fills itself in a quarter of a second.
299    pub fn duplicate(&self) -> Self {
300        Self {
301            open: self.open,
302            ..Self::default()
303        }
304    }
305
306    /// Whether a read has been asked for, or is about to be, and has not landed yet.
307    pub fn busy(&self) -> bool {
308        self.awaiting.is_some() || self.pending.is_some()
309    }
310
311    /// Whether this panel is the one waiting for `token`.
312    ///
313    /// Asked before the payload is handed over rather than after, because a payload is a decoded
314    /// picture or a walked graph and there is only one of it — every other panel would have to be
315    /// given a copy in order to reject it.
316    pub fn wants(&self, token: u64) -> bool {
317        self.awaiting == Some(token)
318    }
319
320    /// What the panel is looking at. For the tests; the panel reads the field.
321    #[cfg(test)]
322    pub fn showing(&self) -> Option<&Path> {
323        self.of.as_ref().map(Ask::first)
324    }
325
326    /// How many rows the dependency tree is showing, if that is what this is. For the tests.
327    #[cfg(test)]
328    pub fn dependency_rows(&self) -> Option<usize> {
329        match &self.content {
330            Content::Binary(view) => Some(view.shown()),
331            _ => None,
332        }
333    }
334
335    /// How many images this is holding, and how many are on show. For the tests, which is where
336    /// the difference — the comparison's toggle — can be seen from.
337    #[cfg(test)]
338    pub fn frames(&self) -> Option<(usize, usize)> {
339        match &self.content {
340            Content::Picture(picture) => Some((picture.frames.len(), picture.showing().len())),
341            _ => None,
342        }
343    }
344
345    /// Flip a comparison between all three views and the difference alone. For the tests; the bar's
346    /// button does it directly.
347    #[cfg(test)]
348    pub fn toggle_all(&mut self) {
349        if let Content::Picture(picture) = &mut self.content {
350            picture.all = !picture.all;
351        }
352    }
353
354    /// Shut it, and let go of everything it was holding.
355    ///
356    /// Up to three decoded pictures, or a dependency graph. There is nothing to be gained by
357    /// keeping any of it for a panel nobody is looking at, and re-reading is a quarter of a second
358    /// — which is also what somebody who has just rebuilt the file means by opening the panel
359    /// again.
360    pub fn close(&mut self) {
361        self.open = false;
362        self.forget();
363    }
364
365    fn forget(&mut self) {
366        self.of = None;
367        self.pending = None;
368        self.awaiting = None;
369        self.content = Content::Nothing;
370    }
371
372    /// What the keyboard is on, offered every frame while the panel is open.
373    ///
374    /// `None` means the selection is not something with a preview — a folder, an archive, nothing
375    /// at all — and then the panel says so rather than keeping the last answer. **Clearing is the
376    /// right answer precisely because the panel is inside the pane**: it sits beside the row it is
377    /// about, so a picture left next to a different selection would be read as being that
378    /// selection's. A panel across the whole window could keep the last thing it was shown; this
379    /// one cannot.
380    pub fn follow(&mut self, what: Option<Ask>, now: f64) {
381        let Some(ask) = what else {
382            // The extension is the useful half of "nothing to show here".
383            let ext = self
384                .of
385                .as_ref()
386                .map(|ask| preview::extension_of(ask.first()));
387            if self.of.is_some() || self.pending.is_some() {
388                self.forget();
389                self.content = Content::Unsupported(ext.unwrap_or_default());
390            }
391            return;
392        };
393        // Already the one on show, or the one being read: nothing to do, and in particular
394        // nothing to ask for again. Without this the answer arriving would immediately queue
395        // another read of the same file, for ever.
396        if self.of.as_ref() == Some(&ask) {
397            self.pending = None;
398            return;
399        }
400        // Restarted on every change, so arrowing down a folder of images decodes the one you
401        // stop on rather than each one on the way past.
402        if self.pending.as_ref().map(|(pending, _)| pending) != Some(&ask) {
403            self.pending = Some((ask, now));
404        }
405    }
406
407    /// Ask for something at once, with no waiting — the keyboard asked for the panel itself.
408    pub fn ask_for(&mut self, ask: Ask) {
409        self.open = true;
410        self.pending = Some((ask, f64::NEG_INFINITY));
411    }
412
413    /// Has the selection sat still long enough? What to read, or how long is left to wait.
414    ///
415    /// Both, because the caller has to book the frame that would notice: this program is idle
416    /// between events, so a wait that nothing asks to be woken from is a wait that never ends.
417    pub fn settle(&mut self, now: f64) -> (Option<Ask>, Option<f64>) {
418        let Some((_, at)) = &self.pending else {
419            return (None, None);
420        };
421        let left = FOLLOW_DELAY - (now - at);
422        if left > 0.0 {
423            return (None, Some(left));
424        }
425        let (ask, _) = self.pending.take().expect("just matched");
426        (Some(ask), None)
427    }
428
429    /// A read has been started for what [`Self::settle`] handed back.
430    pub fn asked(&mut self, ask: Ask, token: u64) {
431        self.of = Some(ask);
432        self.awaiting = Some(token);
433        self.content = Content::Reading;
434    }
435
436    /// A read has come back. Ignored unless it is the one being waited for.
437    pub fn arrived(&mut self, token: u64, payload: Payload, ctx: &egui::Context) {
438        if self.awaiting != Some(token) {
439            return;
440        }
441        self.awaiting = None;
442        // The names the captions use, for a comparison.
443        let (first, second) = match &self.of {
444            Some(Ask::Pair(a, b)) => (leaf(a), leaf(b)),
445            _ => (String::new(), String::new()),
446        };
447        self.content = match payload {
448            // The one thing that has to happen on the UI thread: a texture is the graphics
449            // device's, and the worker has no idea one exists.
450            Payload::Picture(picture) => {
451                let only = frame(ctx, picture.pixels, String::new(), false);
452                Content::Picture(Box::new(Picture {
453                    pixels: only.pixels,
454                    frames: vec![only],
455                    natural: picture.natural,
456                    scaled: picture.scaled,
457                    vector: picture.vector,
458                    ..Picture::fresh()
459                }))
460            }
461            Payload::Diff(diff) => {
462                let a = frame(ctx, diff.a.pixels, first, false);
463                let b = frame(ctx, diff.b.pixels, second, false);
464                let mask = frame(ctx, diff.mask.pixels, "differences".to_owned(), true);
465                Content::Picture(Box::new(Picture {
466                    // The mask is the larger of the two by construction, so it is the space all
467                    // three are placed against — which is what lines corresponding pixels up
468                    // across the views.
469                    pixels: mask.pixels,
470                    frames: vec![a, b, mask],
471                    natural: diff.a.natural,
472                    other: (diff.b.natural != diff.a.natural).then_some(diff.b.natural),
473                    scaled: diff.a.scaled || diff.b.scaled,
474                    vector: diff.a.vector || diff.b.vector,
475                    differing: Some(diff.differing),
476                    ..Picture::fresh()
477                }))
478            }
479            Payload::Text(text) => Content::Text(Text {
480                body: text.body,
481                truncated: text.truncated,
482                code: text.code,
483            }),
484            Payload::Binary(graph) => Content::Binary(deps::View::new(graph)),
485            Payload::Failed(why) => Content::Failed(why),
486        };
487    }
488}
489
490/// A file's own name.
491fn leaf(path: &Path) -> String {
492    path.file_name()
493        .map(|name| name.to_string_lossy().into_owned())
494        .unwrap_or_default()
495}
496
497/// Upload one image and describe it.
498fn frame(ctx: &egui::Context, pixels: egui::ColorImage, label: String, mask: bool) -> Frame {
499    let size = pixels.size;
500    Frame {
501        texture: ctx.load_texture(
502            "preview",
503            pixels,
504            // Linear, because a preview is nearly always shown at some scale other than 1:1 and
505            // nearest would make every one of them a staircase.
506            egui::TextureOptions::LINEAR,
507        ),
508        pixels: vec2(size[0] as f32, size[1] as f32),
509        label,
510        mask,
511    }
512}
513
514impl Picture {
515    /// The fields that are the same however many frames there are.
516    fn fresh() -> Self {
517        Self {
518            frames: Vec::new(),
519            pixels: Vec2::ZERO,
520            natural: [0, 0],
521            other: None,
522            scaled: false,
523            vector: false,
524            differing: None,
525            all: true,
526            zoom: None,
527            fit: 1.0,
528            pan: Vec2::ZERO,
529            zoom_text: String::new(),
530            zoom_pick: None,
531        }
532    }
533
534    /// The scale the canvas is showing it at, fit included.
535    ///
536    /// `fit` is not the stored answer: it depends on the canvas, the canvas depends on the pane,
537    /// and a stored one would be a frame stale every time anything moved. So it is recomputed and
538    /// the *choice* — fitted, or a number somebody picked — is what is kept.
539    fn scale(&self) -> f32 {
540        self.zoom.unwrap_or(self.fit)
541    }
542
543    /// What the bar reports: the scale against the file's own pixels, not against the texture's —
544    /// which are not the same thing for a picture scaled down to fit memory, or for vector art
545    /// rasterised at a size of this program's choosing.
546    fn percent(&self) -> f32 {
547        self.scale() * self.pixels.x / self.natural[0].max(1) as f32 * 100.0
548    }
549
550    /// And the inverse, for a percentage somebody typed or picked.
551    fn scale_for(&self, percent: f32) -> f32 {
552        percent / 100.0 * self.natural[0].max(1) as f32 / self.pixels.x.max(1.0)
553    }
554
555    /// The frames on show: all of them, or the difference alone.
556    fn showing(&self) -> &[Frame] {
557        if self.frames.len() == 3 && !self.all {
558            &self.frames[2..]
559        } else {
560            &self.frames
561        }
562    }
563}
564
565/// Take the panel's room off the pane's body, leaving the listing what is left.
566///
567/// The returned panel rect **includes the seam before it**, which is what makes the boundary one
568/// line neither side draws — exactly as between two panes. `None` when the panel is shut, or when
569/// the pane is too small to give it room without squeezing the listing past being usable.
570pub fn split(body: Rect, open: bool, layout: Layout) -> (Rect, Option<Rect>) {
571    if !open {
572        return (body, None);
573    }
574    let side = layout.at.side(body);
575    let (span, least) = match side {
576        Side::Right => (body.width(), MIN_PANEL_W),
577        Side::Bottom => (body.height(), MIN_PANEL_H),
578    };
579    let room = span - MIN_LIST - SEAM;
580    if room < least {
581        return (body, None);
582    }
583    let take = (span * layout.share).clamp(least, room);
584    match side {
585        Side::Right => {
586            let edge = body.right() - take;
587            (
588                Rect::from_min_max(body.min, pos2(edge - SEAM, body.bottom())),
589                Some(Rect::from_min_max(pos2(edge - SEAM, body.top()), body.max)),
590            )
591        }
592        Side::Bottom => {
593            let edge = body.bottom() - take;
594            (
595                Rect::from_min_max(body.min, pos2(body.right(), edge - SEAM)),
596                Some(Rect::from_min_max(pos2(body.left(), edge - SEAM), body.max)),
597            )
598        }
599    }
600}
601
602/// Draw the panel, and answer the pointer.
603#[allow(clippy::too_many_arguments)]
604pub fn show(
605    ui: &mut Ui,
606    t: &Theme,
607    rect: Rect,
608    pane: PaneId,
609    preview: &mut Preview,
610    layout: &mut Layout,
611    body: Rect,
612    scratch: &mut String,
613    out: &mut Vec<Action>,
614) {
615    let side = layout.at.side(body);
616
617    // The seam before the panel, and the panel's own surface after it.
618    ui.painter()
619        .rect_filled(rect, egui::CornerRadius::ZERO, seam(t));
620    let inside = match side {
621        Side::Right => Rect::from_min_max(pos2(rect.left() + SEAM, rect.top()), rect.max),
622        Side::Bottom => Rect::from_min_max(pos2(rect.left(), rect.top() + SEAM), rect.max),
623    };
624    ui.painter()
625        .rect_filled(inside, egui::CornerRadius::ZERO, t.bg.layer);
626
627    grip(ui, rect, pane, side, layout, body);
628
629    let bar = Rect::from_min_size(inside.min, vec2(inside.width(), HEADER));
630    let canvas = Rect::from_min_max(bar.left_bottom(), inside.max);
631    header(ui, t, bar, pane, preview, layout, scratch, out);
632
633    if canvas.height() < 24.0 || canvas.width() < 48.0 {
634        return;
635    }
636    match &mut preview.content {
637        Content::Nothing => note(ui, t, canvas, "Nothing selected"),
638        Content::Unsupported(ext) => {
639            let what = if ext.is_empty() {
640                "No preview for this".to_owned()
641            } else {
642                format!("No preview for a .{ext}")
643            };
644            note(ui, t, canvas, &what);
645        }
646        // Nothing is drawn but the word. A read takes tens of milliseconds and a spinner that
647        // appears and vanishes inside three frames is worse than nothing.
648        Content::Reading => note(ui, t, canvas, "Reading…"),
649        Content::Failed(why) => {
650            let why = why.clone();
651            note(ui, t, canvas, &why)
652        }
653        Content::Picture(picture) => pictures(ui, t, canvas, pane, picture),
654        Content::Text(text) => text_canvas(ui, t, canvas, pane, text, layout.numbers),
655        Content::Binary(view) => deps::show(ui, t, canvas, view),
656    }
657}
658
659/// A line of secondary text in the middle of the canvas, for the states that have nothing to draw.
660fn note(ui: &Ui, t: &Theme, rect: Rect, text: &str) {
661    text_center(
662        ui.painter(),
663        rect,
664        t.fonts.body.clone(),
665        t.text.secondary,
666        text,
667    );
668}
669
670/// The panel's edge, which is also how much room it has.
671///
672/// The same gesture as the sidebar's splitter and the column edges in the listing: drag to size,
673/// double click to put it back. Reaching a few points either side of the one-point seam, because
674/// a one-point grab target is a one-point grab target.
675fn grip(ui: &mut Ui, rect: Rect, pane: PaneId, side: Side, layout: &mut Layout, body: Rect) {
676    let band = match side {
677        Side::Right => Rect::from_min_max(
678            pos2(rect.left() - 3.0, rect.top()),
679            pos2(rect.left() + SEAM + 3.0, rect.bottom()),
680        ),
681        Side::Bottom => Rect::from_min_max(
682            pos2(rect.left(), rect.top() - 3.0),
683            pos2(rect.right(), rect.top() + SEAM + 3.0),
684        ),
685    };
686    let response = ui.interact(
687        band,
688        Id::new(("preview-grip", pane)),
689        Sense::click_and_drag(),
690    );
691    if response.hovered() || response.dragged() {
692        ui.ctx().set_cursor_icon(match side {
693            Side::Right => egui::CursorIcon::ResizeHorizontal,
694            Side::Bottom => egui::CursorIcon::ResizeVertical,
695        });
696    }
697    if response.dragged() {
698        // As a share rather than as points, so the panel keeps its proportion when the window is
699        // resized — which is what makes `Auto` bearable: the panel does not have to be re-dragged
700        // every time a split changes its pane's shape.
701        let (delta, span) = match side {
702            Side::Right => (-response.drag_delta().x, body.width()),
703            Side::Bottom => (-response.drag_delta().y, body.height()),
704        };
705        layout.share = (layout.share + delta / span.max(1.0)).clamp(0.1, 0.9);
706    }
707    if response.double_clicked() {
708        layout.share = SHARE;
709    }
710}
711
712/// What is on show, what it turned out to be, and the controls.
713///
714/// Laid out **right to left**, because that is the order the priorities run in: the controls take
715/// what they need first, then the two details in turn, and the name gets whatever is left. See the
716/// module header for the rule.
717#[allow(clippy::too_many_arguments)]
718fn header(
719    ui: &mut Ui,
720    t: &Theme,
721    rect: Rect,
722    pane: PaneId,
723    preview: &mut Preview,
724    layout: &mut Layout,
725    scratch: &mut String,
726    out: &mut Vec<Action>,
727) {
728    use std::fmt::Write as _;
729
730    // One baseline for the whole bar, taken from its principal font, so the title, the details and
731    // the glyph beside them are on one line. `crate::ui::deps` says why at length.
732    let baseline = ink_baseline(ui.painter(), &t.fonts.body, rect.top(), rect.height());
733    let surface = t.bg.layer;
734    let button = |right: f32| {
735        Rect::from_center_size(
736            pos2(right - TOOL_SIZE * 0.5, rect.center().y),
737            vec2(TOOL_SIZE, TOOL_SIZE),
738        )
739    };
740
741    // ---- The controls, from the right edge inwards ------------------------
742    let close = button(rect.right() - PAD);
743    if tool_button(
744        ui,
745        t,
746        close,
747        Id::new(("preview-close", pane)),
748        &azur_icons::close,
749        "Close the preview (Ctrl+P)",
750        true,
751        false,
752        surface,
753    )
754    .clicked()
755    {
756        out.push(Action::ClosePreview(pane));
757    }
758    let mut right = close.left() - PAD;
759
760    // The view's own toggle, immediately inside the close button: showing both sources of a
761    // comparison, or numbering the lines of a text file. Never both — they belong to different
762    // views — so they share the slot.
763    match &mut preview.content {
764        Content::Picture(picture) if picture.frames.len() == 3 => {
765            let at = button(right);
766            if tool_button(
767                ui,
768                t,
769                at,
770                Id::new(("preview-all", pane)),
771                &crate::icons::columns,
772                "Show both images as well as the difference",
773                true,
774                picture.all,
775                surface,
776            )
777            .clicked()
778            {
779                picture.all = !picture.all;
780            }
781            right = at.left() - PAD;
782        }
783        Content::Text(_) => {
784            let at = button(right);
785            if tool_button(
786                ui,
787                t,
788                at,
789                Id::new(("preview-numbers", pane)),
790                &crate::icons::line_numbers,
791                "Number the lines",
792                true,
793                layout.numbers,
794                surface,
795            )
796            .clicked()
797            {
798                layout.numbers = !layout.numbers;
799                out.push(Action::RememberLayout);
800            }
801            right = at.left() - PAD;
802        }
803        _ => {}
804    }
805
806    // The zoom group: in, out, Fit, and the field — laid out right to left, so on screen it reads
807    // `[field] [fit] [−] [+]`, which is the order asked for.
808    //
809    // Dropped whole if the bar is narrower than they are. That is the one case the priority rule
810    // does not cover, and the alternative is drawing them on top of each other: a panel down the
811    // side is never this narrow — `MIN_PANEL_W` is sized from `ACTIONS` for exactly this reason —
812    // but a panel along the bottom is as wide as its pane, and a pane can be squeezed.
813    if let Content::Picture(picture) = &mut preview.content {
814        if right - rect.left() > ACTIONS {
815            for (glyph, tip, step) in [
816                (
817                    &azur_icons::plus as azur_egui_theme::icons::Icon<'_>,
818                    "Zoom in",
819                    Some(ZOOM_STEP),
820                ),
821                (&azur_icons::minus, "Zoom out", Some(1.0 / ZOOM_STEP)),
822                (&crate::icons::fit, "Fit the panel", None),
823            ] {
824                let at = button(right);
825                if tool_button(
826                    ui,
827                    t,
828                    at,
829                    Id::new(("preview-zoom", pane, tip)),
830                    glyph,
831                    tip,
832                    true,
833                    false,
834                    surface,
835                )
836                .clicked()
837                {
838                    match step {
839                        // Zoomed about the middle of the canvas, which is where the eye is when a
840                        // button rather than the wheel was used.
841                        Some(by) => {
842                            picture.zoom = Some((picture.scale() * by).clamp(ZOOM_MIN, ZOOM_MAX));
843                        }
844                        None => {
845                            picture.zoom = None;
846                            picture.pan = Vec2::ZERO;
847                        }
848                    }
849                }
850                right = at.left() - PAD;
851            }
852            right = zoom_field(ui, rect, right, pane, picture) - PAD;
853        }
854    }
855
856    // ---- The two details, in the order they give way ----------------------
857    //
858    // The comment first, then the size, and only once both are gone does the name start to crop.
859    let mut comment = String::new();
860    scratch.clear();
861    let size = scratch;
862    let mut mark = None;
863    match &preview.content {
864        Content::Picture(picture) => {
865            // The dimensions, and both of them when two files are being compared and disagree —
866            // because that *is* a difference.
867            let mut dimensions = format!("{} × {}", picture.natural[0], picture.natural[1]);
868            if let Some(other) = picture.other {
869                let _ = write!(dimensions, " / {} × {}", other[0], other[1]);
870            }
871            match picture.differing {
872                // **For a comparison the headline is the share that differs**, so it takes the
873                // slot that survives and the dimensions take the one that goes first. The two
874                // slots are priorities and not captions: "0.04% differs" is the answer somebody
875                // opened the comparison for, and `300 × 200` is the nicety.
876                Some(differing) => {
877                    let _ = write!(size, "{:.2}% differs", differing * 100.0);
878                    if differing > 0.0 {
879                        mark = Some(t.status.danger);
880                    }
881                    comment.push_str(&dimensions);
882                }
883                None => {
884                    size.push_str(&dimensions);
885                    if picture.vector {
886                        comment.push_str("vector, no text");
887                    } else if picture.scaled {
888                        comment.push_str("scaled to fit memory");
889                    }
890                }
891            }
892        }
893        Content::Text(text) => {
894            crate::fs::fmt::size(text.body.len() as u64, size);
895            if text.truncated {
896                comment.push_str("first part only");
897            }
898        }
899        Content::Binary(view) => {
900            let (files, api_sets, missing) = view.graph().tally();
901            if missing > 0 {
902                let _ = write!(size, "{missing} missing");
903                // The count in `text-primary` with the status hue on a **glyph** beside it, rather
904                // than in `status.danger` as text: measured on this surface the danger role is
905                // under the floor for 12-point text and over the one for a shape, so the mark
906                // carries the colour and the number stays legible. `crate::ui::deps` has the table.
907                mark = Some(t.status.danger);
908            } else {
909                let _ = write!(
910                    size,
911                    "{files} file{}, {api_sets} API set{}",
912                    if files == 1 { "" } else { "s" },
913                    if api_sets == 1 { "" } else { "s" },
914                );
915            }
916            if view.graph().truncated {
917                comment.push_str("stopped early");
918            }
919        }
920        _ => {}
921    }
922
923    // ---- The title, and what the details have left it --------------------
924    let mut x = rect.left() + PAD;
925    let (glyph, ink): (azur_egui_theme::icons::Icon<'_>, Color32) = match &preview.content {
926        Content::Picture(_) => (&crate::icons::image, t.image),
927        Content::Text(_) => (&crate::icons::document, t.document),
928        Content::Binary(_) => (&crate::icons::executable, t.executable),
929        Content::Failed(_) => (&azur_icons::error, t.status.danger),
930        _ => (&crate::icons::file, t.text.secondary),
931    };
932    let box_rect = icon_rect(rect, x, GLYPH);
933    glyph(ui.painter(), box_rect, ink);
934    x = box_rect.right() + PAD;
935
936    let name = preview
937        .of
938        .as_ref()
939        .map(Ask::title)
940        .unwrap_or_else(|| "Preview".to_owned());
941    // How wide the name would like to be, which is what decides whether a detail has to go: the
942    // name is never cropped while a detail could have been dropped instead.
943    let wanted = ui
944        .painter()
945        .layout_no_wrap(name.clone(), t.fonts.body.clone(), t.text.primary)
946        .size()
947        .x;
948    let costs = |text: &str| {
949        if text.is_empty() {
950            0.0
951        } else {
952            ui.painter()
953                .layout_no_wrap(text.to_owned(), t.fonts.caption.clone(), t.text.secondary)
954                .size()
955                .x
956                + PAD * 2.0
957        }
958    };
959    let mark_w = if mark.is_some() { MARK + PAD } else { 0.0 };
960    let (show_comment, show_size) = what_fits(
961        wanted,
962        costs(&comment),
963        costs(size) + mark_w,
964        (right - x).max(0.0),
965    );
966
967    {
968        let mut detail = |ui: &Ui, text: &str, with_mark: bool| {
969            if text.is_empty() {
970                return;
971            }
972            let galley = truncated(
973                ui.painter(),
974                text,
975                t.fonts.caption.clone(),
976                // The one carrying the mark is the headline, so it is the one in the readable ink.
977                if with_mark {
978                    t.text.primary
979                } else {
980                    t.text.secondary
981                },
982                (right - x).max(0.0),
983            );
984            let w = galley.size().x;
985            galley_on_baseline(ui.painter(), right - w, baseline, galley);
986            right -= w + PAD;
987            if with_mark {
988                if let Some(ink) = mark {
989                    azur_icons::error(ui.painter(), icon_rect(rect, right - MARK, MARK), ink);
990                    right -= MARK + PAD;
991                }
992            }
993        };
994        if show_size {
995            detail(ui, size, mark.is_some());
996        }
997        if show_comment {
998            detail(ui, &comment, false);
999        }
1000    }
1001
1002    let galley = truncated(
1003        ui.painter(),
1004        &name,
1005        t.fonts.body.clone(),
1006        t.text.primary,
1007        (right - PAD - x).max(0.0),
1008    );
1009    galley_on_baseline(ui.painter(), x, baseline, galley);
1010
1011    // A binary's title carries the whole answer: what the walk found, how long it took, and
1012    // where every name was looked for — which is the one thing a location column cannot say for
1013    // itself, and does not fit on a bar this narrow.
1014    if let Content::Binary(view) = &preview.content {
1015        let response = ui.interact(
1016            Rect::from_min_max(pos2(x, rect.top()), pos2(right, rect.bottom())),
1017            Id::new(("preview-title", pane)),
1018            Sense::hover(),
1019        );
1020        if response.hovered() {
1021            azur_egui_theme::components::tooltip(response, &deps::about(view.graph()));
1022        }
1023    }
1024}
1025
1026/// Which of the two details survive: the comment, and the size.
1027///
1028/// **The rule the bar is built around**, and the only surprising thing about it: the name is never
1029/// cropped while a detail could have been dropped instead. So the comment goes first, then the
1030/// size, and the name starts losing characters only once there is nothing else left to give.
1031///
1032/// The consequence worth knowing is that a *long name* takes the details away even in a wide panel.
1033/// That is the right way round — the name is what identifies the file and `300 × 200` is a nicety —
1034/// but it does mean the details come and go as you arrow down a folder of mixed names, which is a
1035/// thing somebody reading this will otherwise take for a bug.
1036fn what_fits(name: f32, comment: f32, size: f32, room: f32) -> (bool, bool) {
1037    if name + comment + size <= room {
1038        (true, true)
1039    } else if name + size <= room {
1040        (false, true)
1041    } else {
1042        (false, false)
1043    }
1044}
1045
1046/// The zoom field: a subtle, editable combo box. Returns its left edge.
1047///
1048/// **Editable and not a menu**, because a zoom you can only reach through `+` and `−` is a zoom you
1049/// cannot ask for — 100% from 874% is eight clicks. The presets are what people actually want and
1050/// the field is there for the time they want 137%.
1051///
1052/// `Variant::Subtle` gives up the fill and the border until the pointer is over it, which is the
1053/// right treatment on a bar this crowded: a field's border round every control is more lines than
1054/// there is information, and the affordance arrives when the pointer does.
1055fn zoom_field(ui: &mut Ui, bar: Rect, right: f32, pane: PaneId, picture: &mut Picture) -> f32 {
1056    use azur_egui_theme::components::{ComboBox, Size, Variant};
1057
1058    /// What the list offers. `Fit` first, because it is where the panel starts and what a
1059    /// double click puts it back to.
1060    const PRESETS: [&str; 10] = [
1061        "Fit", "25%", "33%", "50%", "75%", "100%", "150%", "200%", "300%", "400%",
1062    ];
1063
1064    let field = Rect::from_center_size(
1065        pos2(right - ZOOM_W * 0.5, bar.center().y),
1066        vec2(ZOOM_W, TOOL_SIZE),
1067    );
1068    let was = picture.zoom_pick;
1069    // A child `Ui` with an id of its own, rather than `Ui::put`: the widget takes its id from the
1070    // `Ui`'s auto-id counter, so two panes drawing their bars in the same frame would otherwise
1071    // depend on the order they were drawn in for their fields to stay apart.
1072    let mut child = ui.new_child(
1073        egui::UiBuilder::new()
1074            .id_salt(("preview-zoom-field", pane))
1075            .max_rect(field)
1076            .layout(egui::Layout::left_to_right(egui::Align::Center)),
1077    );
1078    let response = child.add(
1079        ComboBox::new(&mut picture.zoom_text, &mut picture.zoom_pick)
1080            .options(PRESETS)
1081            .variant(Variant::Subtle)
1082            .size(Size::Small)
1083            .width(ZOOM_W),
1084    );
1085
1086    let typing = response.has_focus();
1087    let entered = typing && child.input(|i| i.key_pressed(egui::Key::Enter));
1088    if picture.zoom_pick != was {
1089        // Taken from the list.
1090        if let Some(picked) = picture.zoom_pick.and_then(|at| PRESETS.get(at)) {
1091            apply_zoom(picture, picked);
1092        }
1093        picture.zoom_pick = None;
1094    } else if entered || response.lost_focus() {
1095        let text = picture.zoom_text.clone();
1096        apply_zoom(picture, &text);
1097    } else if !typing {
1098        // Not being edited: the field reports what the canvas is actually showing, which the wheel
1099        // and the buttons and a resize all change without going through here.
1100        let now = format!("{:.0}%", picture.percent());
1101        if picture.zoom_text != now {
1102            picture.zoom_text = now;
1103        }
1104    }
1105    field.left()
1106}
1107
1108/// Read a percentage — or the word `Fit` — and go there.
1109///
1110/// Anything unparseable is ignored rather than reset to something: the field is rewritten from the
1111/// canvas on the next frame it does not have focus, so a typo puts the old value back by itself.
1112fn apply_zoom(picture: &mut Picture, text: &str) {
1113    let text = text.trim();
1114    if text.eq_ignore_ascii_case("fit") {
1115        picture.zoom = None;
1116        picture.pan = Vec2::ZERO;
1117        return;
1118    }
1119    let number: String = text
1120        .chars()
1121        .filter(|c| c.is_ascii_digit() || *c == '.')
1122        .collect();
1123    if let Ok(percent) = number.parse::<f32>() {
1124        if percent > 0.0 {
1125            let scale = picture.scale_for(percent);
1126            picture.zoom = Some(scale.clamp(ZOOM_MIN, ZOOM_MAX));
1127        }
1128    }
1129}
1130
1131/// What a zoom step multiplies by.
1132///
1133/// A quarter, which takes four steps to double. Photo viewers use anything from 1.1 to 2; a
1134/// quarter is fine enough to land near what you wanted and coarse enough that getting from fit to
1135/// 4:1 is not a dozen clicks.
1136const ZOOM_STEP: f32 = 1.25;
1137
1138/// How far a picture may be zoomed, as a scale on the texture rather than a percentage of the file.
1139const ZOOM_MIN: f32 = 0.01;
1140const ZOOM_MAX: f32 = 32.0;
1141
1142/// One picture, or three, on a checkerboard.
1143///
1144/// The interactions are the ones every image viewer has: **the wheel zooms about the pointer**,
1145/// **dragging pans**, and **a double click goes back to fit**. Zooming about the pointer rather
1146/// than the middle is the one that matters — it is what makes it possible to get to a corner of a
1147/// large image without a dozen alternating zooms and drags.
1148///
1149/// With three frames all of that is **shared**: one zoom, one pan, one gesture over the whole
1150/// canvas. Three views that scrolled independently would be three views of nothing in particular.
1151fn pictures(ui: &mut Ui, t: &Theme, canvas: Rect, pane: PaneId, picture: &mut Picture) {
1152    let count = picture.showing().len().max(1);
1153    let n = count as f32;
1154    // Along the canvas's longer axis, so three views of a wide panel are three columns and three
1155    // of a tall one are three rows — the same question `Where::Auto` answers, one level down.
1156    let across = canvas.width() >= canvas.height();
1157    let cells: Vec<Rect> = (0..count)
1158        .map(|i| {
1159            let at = i as f32;
1160            if across {
1161                let w = (canvas.width() - SEAM * (n - 1.0)) / n;
1162                Rect::from_min_size(
1163                    pos2(canvas.left() + at * (w + SEAM), canvas.top()),
1164                    vec2(w, canvas.height()),
1165                )
1166            } else {
1167                let h = (canvas.height() - SEAM * (n - 1.0)) / n;
1168                Rect::from_min_size(
1169                    pos2(canvas.left(), canvas.top() + at * (h + SEAM)),
1170                    vec2(canvas.width(), h),
1171                )
1172            }
1173        })
1174        .collect();
1175
1176    // One interaction over the whole canvas, so a drag anywhere moves every view together.
1177    let response = ui.interact(
1178        canvas,
1179        Id::new(("preview-canvas", pane)),
1180        Sense::click_and_drag(),
1181    );
1182
1183    // Where the images go inside each cell, once the captions have had their strip.
1184    let captioned = count > 1;
1185    let areas: Vec<Rect> = cells
1186        .iter()
1187        .map(|cell| {
1188            if captioned {
1189                Rect::from_min_max(pos2(cell.left(), cell.top() + CAPTION), cell.max)
1190            } else {
1191                *cell
1192            }
1193        })
1194        .collect();
1195
1196    // Fit: recomputed every frame, because the canvas moves — and against the *smallest* area, so
1197    // every view fits rather than the first one fitting and the rest overflowing. **Never enlarged
1198    // past 1:1**: a 16-pixel icon blown up to fill a 400-point panel is not a preview of it, it is
1199    // a mosaic, and fitting is an upper bound.
1200    let smallest = areas
1201        .iter()
1202        .fold(Vec2::splat(f32::INFINITY), |acc, area| acc.min(area.size()));
1203    picture.fit = (smallest.x / picture.pixels.x.max(1.0))
1204        .min(smallest.y / picture.pixels.y.max(1.0))
1205        .min(1.0);
1206
1207    // ---- The gestures ----
1208    if response.dragged() {
1209        picture.pan += response.drag_delta();
1210        // A drag is a choice to look at part of it, so it pins the scale as well: without this,
1211        // panning a fitted image would move something that cannot move.
1212        picture.zoom = Some(picture.scale());
1213    }
1214    if response.double_clicked() {
1215        picture.zoom = None;
1216        picture.pan = Vec2::ZERO;
1217    }
1218    let wheel = ui.input(|i| i.smooth_scroll_delta.y);
1219    if response.hovered() && wheel != 0.0 {
1220        let was = picture.scale();
1221        let now = (was * ZOOM_STEP.powf(wheel / 50.0)).clamp(ZOOM_MIN, ZOOM_MAX);
1222        // **Zoom about the pointer**: the point under the cursor stays under it. The images sit at
1223        // their cell's centre plus the pan, so the offset from that centre to the pointer scales
1224        // with them and the pan takes up the difference. Measured against the cell the pointer is
1225        // in, which is what makes it work in a three-view comparison too.
1226        if let Some(at) = response.hover_pos() {
1227            let cell = areas
1228                .iter()
1229                .find(|area| area.contains(at))
1230                .copied()
1231                .unwrap_or(canvas);
1232            let middle = cell.center() + picture.pan;
1233            picture.pan += (middle - at) * (now / was - 1.0);
1234        }
1235        picture.zoom = Some(now);
1236    }
1237
1238    let scale = picture.scale();
1239    let group = picture.pixels * scale;
1240    // Panning is bounded so the images cannot be dragged out of view altogether: at least a
1241    // quarter stays. One smaller than its cell is simply centred — there is nowhere for it to go,
1242    // and letting it wander would be a gesture with no meaning.
1243    let slack = ((group - smallest) * 0.5 + smallest * 0.25).max(Vec2::ZERO);
1244    picture.pan = picture.pan.clamp(-slack, slack);
1245
1246    let checker = checkerboard(ui.ctx(), t);
1247    for (frame, (cell, area)) in picture.showing().iter().zip(cells.iter().zip(&areas)) {
1248        if captioned {
1249            let strip = Rect::from_min_max(cell.min, pos2(cell.right(), cell.top() + CAPTION));
1250            let galley = truncated(
1251                ui.painter(),
1252                &frame.label,
1253                t.fonts.caption.clone(),
1254                t.text.secondary,
1255                (strip.width() - PAD * 2.0).max(0.0),
1256            );
1257            let baseline =
1258                ink_baseline(ui.painter(), &t.fonts.caption, strip.top(), strip.height());
1259            galley_on_baseline(ui.painter(), strip.left() + PAD, baseline, galley);
1260        }
1261
1262        // The checkerboard, so an alpha channel is visible as absence rather than as whatever the
1263        // panel's surface happens to be. One tiled quad, not a grid of rects: a 400-point canvas
1264        // at 8-point squares would be two and a half thousand rectangles a frame.
1265        ui.painter().add(egui::Shape::image(
1266            checker.id(),
1267            *area,
1268            Rect::from_min_size(pos2(0.0, 0.0), area.size() / (CHECKER * 2.0)),
1269            Color32::WHITE,
1270        ));
1271
1272        // Placed against the *group's* rect and not its own, so the same pixel of two files of
1273        // different shapes lands at the same offset in each view — which is the only way a
1274        // comparison means anything.
1275        let whole = Rect::from_center_size(area.center() + picture.pan, group);
1276        let where_ = Rect::from_min_size(whole.min, frame.pixels * scale);
1277        let painter = ui.painter().with_clip_rect(*area);
1278        painter.add(egui::Shape::image(
1279            frame.texture.id(),
1280            where_,
1281            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
1282            // A mask is tinted here rather than coloured on the worker, which is what keeps the
1283            // rule that no colour is chosen outside the theme: what came back is a measurement.
1284            if frame.mask {
1285                t.status.danger
1286            } else {
1287                Color32::WHITE
1288            },
1289        ));
1290    }
1291
1292    if response.hovered() {
1293        ui.ctx()
1294            .set_cursor_icon(if group.x > smallest.x || group.y > smallest.y {
1295                egui::CursorIcon::Grab
1296            } else {
1297                egui::CursorIcon::Default
1298            });
1299    }
1300}
1301
1302/// The air either side of a line number.
1303///
1304/// Wider than [`PAD`], and the reason is that this gap is between two runs of *text* rather than
1305/// between a control and its edge: at four points the number and the first character of the line
1306/// read as one word.
1307const GUTTER_GAP: f32 = space::S3;
1308
1309/// One square of the checkerboard, in points.
1310const CHECKER: f32 = 8.0;
1311
1312/// The two-by-two texture the checkerboard is tiled from.
1313///
1314/// Built once per theme and kept in the context's own cache: it is one 2×2 image, it never
1315/// changes, and a texture uploaded per frame would be a texture uploaded per frame.
1316///
1317/// `Repeat` is the whole trick — it is what lets one quad with a `uv` of many tiles stand in for
1318/// a grid of rectangles.
1319fn checkerboard(ctx: &egui::Context, t: &Theme) -> egui::TextureHandle {
1320    let id = Id::new(("preview-checker", t.dark));
1321    if let Some(cached) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
1322        return cached;
1323    }
1324    // Two surfaces from the ramp rather than two greys named here, and *this* pair rather than an
1325    // adjacent one, because the two themes have to agree: measured, `canvas`/`control` is 11.5 ΔL*
1326    // apart in the dark theme and 3.6 in the light one, where the board all but disappeared and
1327    // with it the whole point of having one. `layer`/`control-active` is 15.4 and 13.9 — balanced,
1328    // and about what Photoshop's white-and-light-grey board measures, which is the value everyone
1329    // already reads as "nothing here". See `the_checkerboard_reads_as_a_checkerboard`.
1330    let (a, b) = (t.bg.layer, t.bg.control_active);
1331    let image = egui::ColorImage {
1332        size: [2, 2],
1333        pixels: vec![a, b, b, a],
1334        source_size: vec2(2.0, 2.0),
1335    };
1336    let handle = ctx.load_texture(
1337        "preview-checker",
1338        image,
1339        egui::TextureOptions {
1340            magnification: egui::TextureFilter::Nearest,
1341            minification: egui::TextureFilter::Nearest,
1342            wrap_mode: egui::TextureWrapMode::Repeat,
1343            mipmap_mode: None,
1344        },
1345    );
1346    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
1347    handle
1348}
1349
1350/// Text, wrapped and scrolled, optionally numbered.
1351///
1352/// **Monospace only where the columns mean something** — see [`crate::preview::is_code`], which
1353/// asks a question with an answer rather than a matter of taste: does moving a character sideways
1354/// change what the file means? In a log, a table, a diff or any source file it does. A `.md` or a
1355/// `.txt` is paragraphs, and paragraphs are what the proportional face is for.
1356///
1357/// Wrapped rather than scrolled sideways, which is what was asked for and also the only thing that
1358/// works in a panel this narrow. Which makes the **line numbers** the interesting part: a wrapped
1359/// paragraph is several visual rows of one logical line, so the gutter cannot simply count rows.
1360/// It walks the galley and numbers the rows that *begin* a line, which is a fact only the finished
1361/// layout knows — and the reason the galley is laid out here by hand and then handed to a `Label`
1362/// rather than left to the widget.
1363fn text_canvas(ui: &mut Ui, t: &Theme, canvas: Rect, pane: PaneId, text: &Text, numbers: bool) {
1364    let font = if text.code {
1365        t.fonts.mono.clone()
1366    } else {
1367        t.fonts.body.clone()
1368    };
1369    let mut child = ui.new_child(
1370        egui::UiBuilder::new()
1371            .max_rect(canvas.shrink2(vec2(PAD, 0.0)))
1372            .layout(egui::Layout::top_down(egui::Align::Min)),
1373    );
1374    child.set_clip_rect(canvas.intersect(ui.clip_rect()));
1375    egui::ScrollArea::vertical()
1376        .id_salt(("preview-text", pane))
1377        .auto_shrink([false, false])
1378        .show(&mut child, |ui| {
1379            // The gutter, wide enough for the last line's number. Measured from the count rather
1380            // than guessed, so a 12,000-line file does not have its numbers clipped and a 12-line
1381            // one does not carry a gutter for four digits it will never use.
1382            let lines = text.body.lines().count().max(1);
1383            let gutter = if numbers {
1384                ui.painter()
1385                    .layout_no_wrap(
1386                        "0".repeat(lines.to_string().len()),
1387                        font.clone(),
1388                        t.text.secondary,
1389                    )
1390                    .size()
1391                    .x
1392                    + GUTTER_GAP * 2.0
1393            } else {
1394                0.0
1395            };
1396
1397            // One galley for the whole thing, laid out once and cached by egui on the job. The
1398            // body is capped at `preview::TEXT_CAP` for exactly this reason: egui lays out every
1399            // line whether or not it is on screen.
1400            let mut job = egui::text::LayoutJob::single_section(
1401                text.body.clone(),
1402                egui::TextFormat::simple(font.clone(), t.text.primary),
1403            );
1404            job.wrap = egui::text::TextWrapping {
1405                max_width: (ui.available_width() - gutter).max(16.0),
1406                ..Default::default()
1407            };
1408            let galley = ui.painter().layout_job(job);
1409            let shown = ui
1410                .horizontal(|ui| {
1411                    ui.add_space(gutter);
1412                    ui.add(egui::Label::new(galley.clone()).selectable(true))
1413                })
1414                .inner;
1415
1416            if numbers {
1417                let origin = shown.rect.min;
1418                let clip = ui.clip_rect();
1419                let mut line = 1usize;
1420                let mut starts = true;
1421                for row in &galley.rows {
1422                    if starts {
1423                        let y = origin.y + row.pos.y;
1424                        // Only the rows on screen get a galley of their own. A 15,000-line file is
1425                        // 15,000 numbers, and laying out the ones nobody can see would undo the
1426                        // whole reason the body is one galley.
1427                        if y + row.row.size.y >= clip.top() && y <= clip.bottom() {
1428                            let number = ui.painter().layout_no_wrap(
1429                                line.to_string(),
1430                                // The **same font as the body**, which is what guarantees the two
1431                                // share a baseline: a caption-sized number beside a body-sized
1432                                // line would sit a point above it, and a gutter that does not line
1433                                // up is worse than no gutter.
1434                                font.clone(),
1435                                t.text.secondary,
1436                            );
1437                            ui.painter().galley(
1438                                pos2(origin.x - GUTTER_GAP - number.size().x, y),
1439                                number,
1440                                Color32::PLACEHOLDER,
1441                            );
1442                        }
1443                        line += 1;
1444                    }
1445                    starts = row.ends_with_newline;
1446                }
1447            }
1448
1449            if text.truncated {
1450                ui.add_space(space::S2);
1451                let mut how_much = String::new();
1452                crate::fs::fmt::size(preview::TEXT_CAP as u64, &mut how_much);
1453                ui.label(
1454                    egui::RichText::new(format!("— the first {how_much} of a longer file —"))
1455                        .font(t.fonts.caption.clone())
1456                        .color(t.text.secondary),
1457                );
1458            }
1459            ui.add_space(space::S3);
1460        });
1461}
1462
1463#[cfg(test)]
1464mod tests {
1465    use super::*;
1466    use std::path::PathBuf;
1467
1468    /// A pane wider than it is tall gets the panel down the side; anything squarer gets it along
1469    /// the bottom. Which is the whole of `Auto`, and it is the reason it exists: two panes side by
1470    /// side are each half as wide, and a preview taking 40% of *that* leaves no listing at all.
1471    #[test]
1472    fn auto_puts_the_panel_where_the_pane_has_room_for_it() {
1473        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(820.0, 540.0));
1474        let narrow = Rect::from_min_size(pos2(0.0, 0.0), vec2(410.0, 540.0));
1475        assert_eq!(Where::Auto.side(wide), Side::Right, "one pane in a window");
1476        assert_eq!(Where::Auto.side(narrow), Side::Bottom, "two side by side");
1477        // A square pane goes to the bottom, because width is the scarcer thing in a listing.
1478        let square = Rect::from_min_size(pos2(0.0, 0.0), vec2(500.0, 500.0));
1479        assert_eq!(Where::Auto.side(square), Side::Bottom);
1480        // And the two fixed ones do not care what shape anything is.
1481        for pane in [wide, narrow, square] {
1482            assert_eq!(Where::Right.side(pane), Side::Right);
1483            assert_eq!(Where::Bottom.side(pane), Side::Bottom);
1484        }
1485    }
1486
1487    #[test]
1488    fn a_position_survives_a_trip_through_the_settings_file() {
1489        for at in Where::ALL {
1490            assert_eq!(Where::parse(at.as_str()), Some(at));
1491            assert_eq!(Where::parse(&at.as_str().to_uppercase()), Some(at));
1492        }
1493        assert_eq!(Where::parse("sideways"), None);
1494        assert_eq!(Where::parse(""), None);
1495    }
1496
1497    /// A shut panel takes nothing, an open one leaves the listing usable, and the seam between
1498    /// them belongs to neither.
1499    #[test]
1500    fn the_panel_leaves_the_listing_usable_on_both_sides() {
1501        let body = Rect::from_min_size(pos2(0.0, 32.0), vec2(900.0, 620.0));
1502        let shut = Layout::default();
1503        assert_eq!(split(body, false, shut), (body, None));
1504
1505        type Span = fn(Rect) -> f32;
1506        for (at, span, least) in [
1507            (Where::Right, (|r: Rect| r.width()) as Span, MIN_PANEL_W),
1508            (Where::Bottom, |r: Rect| r.height(), MIN_PANEL_H),
1509        ] {
1510            let layout = Layout {
1511                at,
1512                ..Default::default()
1513            };
1514            let (list, panel) = split(body, true, layout);
1515            let panel = panel.expect("an open panel is somewhere");
1516            assert!(
1517                span(list) >= MIN_LIST,
1518                "{at:?}: the listing got {} of {}",
1519                span(list),
1520                span(body)
1521            );
1522            assert!(span(panel) >= least + SEAM, "{at:?}: the panel is too small");
1523            // They abut, with the seam inside the panel's first point.
1524            match at.side(body) {
1525                Side::Right => assert_eq!(list.right(), panel.left()),
1526                Side::Bottom => assert_eq!(list.bottom(), panel.top()),
1527            }
1528            assert_eq!(
1529                span(list) + span(panel),
1530                span(body),
1531                "{at:?}: room went missing"
1532            );
1533
1534            // Dragged past either stop, both sides keep their minimum.
1535            for share in [0.001, 0.999] {
1536                let (list, panel) = split(
1537                    body,
1538                    true,
1539                    Layout {
1540                        at,
1541                        share,
1542                        ..Default::default()
1543                    },
1544                );
1545                let panel = panel.expect("still open");
1546                assert!(span(list) >= MIN_LIST, "{at:?} at {share}: the listing");
1547                assert!(span(panel) >= least, "{at:?} at {share}: the panel");
1548            }
1549
1550            // And a pane with no room for it does not get one, rather than getting one over the
1551            // top of the listing it is about.
1552            let cramped = match at.side(body) {
1553                Side::Right => Rect::from_min_size(body.min, vec2(MIN_LIST + 8.0, 400.0)),
1554                Side::Bottom => Rect::from_min_size(body.min, vec2(400.0, MIN_LIST + 8.0)),
1555            };
1556            assert_eq!(split(cramped, true, layout), (cramped, None), "{at:?}");
1557        }
1558    }
1559
1560    /// **The comment goes first, then the size, and only then does the name crop.**
1561    ///
1562    /// The bar's one non-obvious rule, stated as a table. What it is really guarding is the third
1563    /// case: a name long enough to need cropping has already cost both details, so nothing is ever
1564    /// abbreviated while something droppable is still on the bar.
1565    #[test]
1566    fn the_details_give_way_before_the_name_does() {
1567        // Everything fits.
1568        assert_eq!(what_fits(100.0, 80.0, 60.0, 300.0), (true, true));
1569        // Exactly enough is enough.
1570        assert_eq!(what_fits(100.0, 80.0, 60.0, 240.0), (true, true));
1571        // One point short: the comment goes, and the name is untouched.
1572        assert_eq!(what_fits(100.0, 80.0, 60.0, 239.0), (false, true));
1573        // Shorter still: the size goes too.
1574        assert_eq!(what_fits(100.0, 80.0, 60.0, 159.0), (false, false));
1575        // And a name that cannot fit even alone still keeps the controls — it crops instead, which
1576        // is what the `(false, false)` answer leaves the caller to do.
1577        assert_eq!(what_fits(400.0, 80.0, 60.0, 120.0), (false, false));
1578        // A view with no details of its own: whichever way the flags fall there is nothing to
1579        // draw, and a name that does fit is reported as fitting.
1580        assert_eq!(what_fits(40.0, 0.0, 0.0, 50.0), (true, true));
1581    }
1582
1583    /// **A panel down the side is never too narrow for its own controls.**
1584    ///
1585    /// The bar's zoom field and four buttons are a fixed cost, and a panel that cannot show them is
1586    /// a panel whose only way back to 100% has gone. `MIN_PANEL_W` is what guarantees it, so this
1587    /// is the assertion that ties the two constants together — change either and it says so.
1588    #[test]
1589    fn the_narrowest_panel_still_has_room_for_its_controls() {
1590        // A compile-time assertion, because both sides are constants: this is a statement about the
1591        // source rather than about a run, and `const` is where clippy rightly insists it goes.
1592        const _: () = assert!(MIN_PANEL_W >= ACTIONS + GLYPH + PAD * 3.0);
1593        // And there is something left over for a name, or the bar is controls and nothing else.
1594        const _: () = assert!(MIN_PANEL_W - ACTIONS - GLYPH - PAD * 3.0 >= 24.0);
1595    }
1596
1597    /// The panel follows the keyboard, but only once it stops moving — and it *does* let go when
1598    /// the keyboard moves onto something with no preview, which is the half of [`Preview::follow`]
1599    /// that being inside the pane makes necessary.
1600    #[test]
1601    fn the_panel_waits_for_the_selection_to_stop_moving() {
1602        let mut it = Preview {
1603            open: true,
1604            ..Default::default()
1605        };
1606        let one = Ask::One(PathBuf::from(r"C:\pics\one.png"), preview::Kind::Picture);
1607        let two = Ask::One(PathBuf::from(r"C:\pics\two.png"), preview::Kind::Picture);
1608
1609        it.follow(Some(one.clone()), 10.0);
1610        let (ready, left) = it.settle(10.0 + FOLLOW_DELAY * 0.6);
1611        assert!(ready.is_none(), "it read before the wait was up");
1612        assert!((left.expect("waiting") - FOLLOW_DELAY * 0.4).abs() < 1e-6);
1613
1614        // Moved on before the wait was up: the clock restarts, and the first one is never read
1615        // at all — which is the point of the wait rather than a side effect of it.
1616        it.follow(Some(two.clone()), 10.0 + FOLLOW_DELAY * 0.6);
1617        assert!(
1618            it.settle(10.0 + FOLLOW_DELAY * 1.2).0.is_none(),
1619            "the wait did not restart"
1620        );
1621        let (ready, left) = it.settle(10.0 + FOLLOW_DELAY * 2.0);
1622        assert_eq!(ready.as_ref(), Some(&two));
1623        assert_eq!(left, None);
1624
1625        // The file already on show is never asked for again, or the answer arriving would queue
1626        // another read of it for ever.
1627        it.asked(two.clone(), 1);
1628        it.follow(Some(two.clone()), 40.0);
1629        assert_eq!(it.settle(41.0), (None, None));
1630        assert_eq!(it.showing(), Some(two.first()));
1631
1632        // Selecting a *second* picture is a different question, so it is asked afresh.
1633        let pair = Ask::Pair(
1634            PathBuf::from(r"C:\pics\one.png"),
1635            PathBuf::from(r"C:\pics\two.png"),
1636        );
1637        it.follow(Some(pair.clone()), 50.0);
1638        assert_eq!(it.settle(50.0 + FOLLOW_DELAY * 2.0).0.as_ref(), Some(&pair));
1639
1640        // And the keyboard moving onto something with no preview clears it: this panel is beside
1641        // the row it is about, so a stale picture next to a different selection would be a lie.
1642        it.asked(pair, 2);
1643        it.follow(None, 60.0);
1644        assert_eq!(it.showing(), None);
1645        assert!(matches!(it.content, Content::Unsupported(_)));
1646
1647        // The keyboard asking for the panel itself does not wait at all.
1648        it.ask_for(one.clone());
1649        assert_eq!(it.settle(70.0).0, Some(one));
1650    }
1651
1652    /// Closing lets go of what the panel was holding, and a duplicated tab does not inherit it.
1653    #[test]
1654    fn a_shut_panel_holds_nothing() {
1655        let mut it = Preview {
1656            open: true,
1657            ..Default::default()
1658        };
1659        it.asked(
1660            Ask::One(PathBuf::from(r"C:\a.png"), preview::Kind::Picture),
1661            3,
1662        );
1663        assert!(it.busy());
1664        // A duplicate opens the same way and reads for itself: a copy of three 16 MB textures per
1665        // `Ctrl+T` would make duplicating a tab the most expensive thing in the window.
1666        let copy = it.duplicate();
1667        assert!(copy.open && !copy.busy() && copy.showing().is_none());
1668
1669        it.close();
1670        assert!(!it.open && !it.busy() && it.showing().is_none());
1671        assert!(matches!(it.content, Content::Nothing));
1672    }
1673
1674    /// The bar's title says what is on show, and a comparison says both names.
1675    #[test]
1676    fn a_comparison_is_titled_with_both_names() {
1677        let one = Ask::One(PathBuf::from(r"C:\pics\a.png"), preview::Kind::Picture);
1678        assert_eq!(one.title(), "a.png");
1679        let pair = Ask::Pair(
1680            PathBuf::from(r"C:\pics\a.png"),
1681            PathBuf::from(r"D:\other\b.png"),
1682        );
1683        assert_eq!(pair.title(), "a.png ↔ b.png");
1684        // The first is what anything needing one path uses — the folder for the tooltip, the
1685        // staleness test in `follow`.
1686        assert_eq!(pair.first(), Path::new(r"C:\pics\a.png"));
1687    }
1688
1689    /// The checkerboard reads as a checkerboard: its two squares are far enough apart to see and
1690    /// close enough together not to compete with the picture on top of them.
1691    ///
1692    /// Both halves matter. A board whose squares are the same colour says nothing about an alpha
1693    /// channel, and one in black and white would be the loudest thing in the window — the point
1694    /// of it is to be recognisably *absence*.
1695    #[test]
1696    fn the_checkerboard_reads_as_a_checkerboard() {
1697        use azur_egui_theme::contrast::apart;
1698
1699        for t in [Theme::dark(), Theme::light()] {
1700            let name = if t.dark { "dark" } else { "light" };
1701            let got = apart(t.bg.layer, t.bg.control_active);
1702            assert!(
1703                (8.0..=18.0).contains(&got),
1704                "{name}: the checkerboard's squares are {got:.1} ΔL* apart"
1705            );
1706        }
1707        // And the two themes have to agree about it, which is what ruled out the first pair this
1708        // used: a board that is plain in one theme and invisible in the other is not one rule, it
1709        // is two.
1710        let dark = Theme::dark();
1711        let light = Theme::light();
1712        let gap = apart(dark.bg.layer, dark.bg.control_active)
1713            - apart(light.bg.layer, light.bg.control_active);
1714        assert!(
1715            gap.abs() < 5.0,
1716            "the board is {gap:.1} ΔL* stronger in one theme than the other"
1717        );
1718    }
1719}
