1//! Tabs and panes: what is being looked at, and where.
2//!
3//! A [`Tab`] is a place plus how it is being viewed — the sort, the filter, the
4//! selection, the scroll position and its own navigation history. A [`Pane`] is a
5//! stack of tabs with one of them showing. The tree that arranges panes on screen
6//! lives in [`crate::dock`].
7//!
8//! The listing itself is an `Arc<Dir>` borrowed from the loader's cache, so two
9//! tabs on the same folder share one copy of the data and still sort it
10//! differently: the order is a per-tab `Vec<u32>` of indices into a `Dir` neither
11//! of them owns.
12
13use std::path::PathBuf;
14use std::sync::Arc;
15
16use crate::fs::{display_name, sort, Column, Dir};
17
18/// Identifies a pane for the lifetime of the window. Never reused, so a stale
19/// reference resolves to nothing rather than to the wrong pane.
20pub type PaneId = u32;
21
22/// How tall a row is, in points.
23///
24/// A deliberate departure from `tokens::row::TABLE` (36px). Azur's table row is
25/// sized for a form; a file listing is read by scanning hundreds of lines, and
26/// every point of row height is a file that did not fit on screen. 24 is dense
27/// enough to show ~40 rows in a half-height pane and still leave the 14px glyph
28/// and 14px body text their breathing room.
29pub const ROW_HEIGHT: f32 = 24.0;
30
31/// A rubber-band selection in progress.
32///
33/// Both corners are in *content* coordinates — distance from the top of the whole
34/// listing, not from the top of the window — so the band stays anchored to the rows
35/// it was drawn over while the view auto-scrolls underneath it.
36pub struct Band {
37    pub anchor: egui::Pos2,
38    pub current: egui::Pos2,
39    /// What was selected before the band started, for an additive gesture. Empty when
40    /// the band replaces the selection.
41    ///
42    /// Holding Ctrl or Shift *adds* what the band covers to what was already there —
43    /// it does not invert it. Inverting is what a Ctrl-*click* does, and Explorer keeps
44    /// the two distinct, so a band dragged back over something already selected does
45    /// not quietly turn it off.
46    pub base: Vec<bool>,
47}
48
49impl Band {
50    /// The rows the band currently covers, as positions in the display order.
51    ///
52    /// Only the vertical span matters: a row in the details view spans the full width,
53    /// so a band that crosses its `y` crosses the row, however narrow it is. This is
54    /// what Explorer's details view does too.
55    pub fn rows(&self, count: usize) -> std::ops::Range<usize> {
56        if count == 0 {
57            return 0..0;
58        }
59        let (top, bottom) = (
60            self.anchor.y.min(self.current.y),
61            self.anchor.y.max(self.current.y),
62        );
63        let first = (top / ROW_HEIGHT).floor().max(0.0) as usize;
64        let last = (bottom / ROW_HEIGHT).floor().max(0.0) as usize;
65        if first >= count {
66            return 0..0;
67        }
68        first..(last + 1).min(count)
69    }
70}
71
72/// One place being looked at.
73pub struct Tab {
74    /// The folder. Empty means "This PC".
75    pub path: PathBuf,
76    /// The deepest folder the breadcrumb still shows: [`Tab::path`], or something under it.
77    ///
78    /// Going **up** leaves this alone, so the trail you came down stays on the bar and the
79    /// folder you just left is one click away instead of something to go and find again.
80    /// Going anywhere off the trail replaces it.
81    ///
82    /// Explorer trims the bar to the folder you are in, and then the only way back down is
83    /// the chevron beside it — which means reading a menu to find a name that was on screen
84    /// a moment ago. This keeps it on screen. The folder actually being *shown* is still
85    /// obvious: it is the bold segment, wherever along the trail it sits.
86    pub trail: PathBuf,
87    /// What the tab strip shows.
88    pub title: String,
89
90    /// Everywhere this tab has been, and where in that trail it is. Back and
91    /// Forward move `at`; navigating anywhere else truncates the future, which is
92    /// how every browser and shell behaves.
93    pub history: Vec<PathBuf>,
94    pub at: usize,
95
96    /// The listing, once it arrives.
97    pub dir: Option<Arc<Dir>>,
98    /// The request this tab is waiting for. Any other token is a stale answer.
99    pub awaiting: Option<u64>,
100    /// When the folder was asked for, so a wait long enough to notice can say so and a wait
101    /// nobody could notice says nothing. See [`SLOW_SCAN`].
102    pub asked_at: Option<f64>,
103
104    /// This view of this folder, so an answer that arrives after the folder is gone can be
105    /// recognised and thrown away. A fresh one on every move.
106    pub view: u64,
107    /// The icon each row's own file carries, by entry index — [`crate::shell::icons::UNASKED`]
108    /// until asked about.
109    ///
110    /// **This is the folder-scoped rule in one field.** A per-file icon used to live in a map
111    /// keyed by `PathBuf` in the icon service, which meant browsing a folder of executables
112    /// left one heap-allocated path per file behind for the rest of the session. Here it is
113    /// four bytes per row in a `Vec` the tab owns: sized when the listing lands, dropped when
114    /// the tab moves. Leave the folder and every byte of it goes with it.
115    pub file_icons: Vec<i32>,
116    /// Where a shortcut row points, by entry index — see [`crate::shell::links`].
117    ///
118    /// A map rather than a column, because unlike an icon this is only ever wanted for a
119    /// handful of rows: a `.lnk` or a reparse point. A `Vec` would be an `Option<String>` per
120    /// row, which on a flattened tree of 200,000 is megabytes to say "not a shortcut" 199,990
121    /// times.
122    ///
123    /// **A key that is present means asked.** `None` is an answer as much as `Some` is — an
124    /// unreadable shortcut, or one pointing at something with no path — and both stop the row
125    /// asking again. Dropped with the folder, exactly like [`Tab::file_icons`].
126    pub links: std::collections::HashMap<u32, Option<String>>,
127
128    /// Display order: indices into `dir.entries`, sorted and filtered.
129    pub order: Vec<u32>,
130    pub sort_by: Column,
131    pub ascending: bool,
132    pub filter: String,
133    /// When the filter text last changed, if the change has not been applied yet.
134    ///
135    /// See [`Tab::settle_filter`] and [`FILTER_DELAY`]. `None` means the order on screen is the
136    /// order this filter asks for.
137    pub filter_at: Option<f64>,
138    pub show_hidden: bool,
139    /// Show everything under this folder as one flat listing rather than its own
140    /// children. See [`Tab::toggle_flat`] and [`crate::fs::scan::scan_deep`].
141    pub flat: bool,
142
143    /// Selection, indexed by *entry* index so it survives a re-sort.
144    pub selected: Vec<bool>,
145    pub selected_count: usize,
146    /// The keyboard cursor, as a position in `order`.
147    pub cursor: Option<usize>,
148    /// Where a shift-click range started, as a position in `order`.
149    pub anchor: Option<usize>,
150
151    /// Column widths. The first is Name, which flexes; the rest are measured from
152    /// their content the first time a listing is drawn.
153    pub widths: [f32; 4],
154    /// Cleared whenever the listing changes, so the fitted columns are re-measured.
155    pub widths_measured: bool,
156
157    /// Select this name as soon as the listing lands.
158    ///
159    /// Set when going Up, so the folder you came out of is highlighted where you
160    /// left it rather than making you find it again.
161    pub reveal: Option<String>,
162    /// Scroll to the cursor on the next frame, for keyboard movement and reveal.
163    pub scroll_to_cursor: bool,
164
165    /// Keystrokes typed recently, for type-ahead find, and when the last one was.
166    pub typeahead: String,
167    pub typeahead_at: f64,
168
169    /// The breadcrumb has been turned into an editable path field.
170    pub editing_path: bool,
171    pub edit_text: String,
172
173    /// Last frame's scroll offset, so a scroll-into-view can nudge rather than jump.
174    pub scroll_y: f32,
175    /// An absolute offset to scroll to on the next frame, for the auto-scroll a
176    /// rubber-band does when it is dragged past the edge of the view.
177    pub scroll_to: Option<f32>,
178
179    /// A rubber-band selection being dragged.
180    pub band: Option<Band>,
181
182    /// A row being renamed in place: which entry, and the text so far.
183    ///
184    /// Explorer edits the name where it sits rather than in a dialog, which keeps the
185    /// surrounding names visible — usually the whole reason you are renaming.
186    pub renaming: Option<(usize, String)>,
187    /// Whether the rename field still needs the caret put in it.
188    pub rename_fresh: bool,
189    /// What was selected when the listing was dropped for a refresh, to be selected again when
190    /// the new one lands.
191    ///
192    /// Needed because a refresh happens in two steps with nothing in between: the listing is
193    /// dropped so the folder is re-read, and the answer arrives frames later. By then the old
194    /// listing is gone, and with it any way to say what the selection *was* -- which is why this
195    /// is captured at the moment it is dropped rather than worked out on arrival.
196    pub keep_selected: Vec<String>,
197    /// Start renaming whatever [`Tab::reveal`] finds, as soon as it is found.
198    ///
199    /// Set when this program has just made a folder: the shell creates it, the folder is
200    /// re-read, and the new row arrives selected and ready to be named — which is the whole
201    /// gesture in Explorer, where `New folder` and typing its name are one action rather than
202    /// two.
203    pub rename_revealed: bool,
204
205    /// **This folder's preview panel**, shut by default.
206    ///
207    /// Per tab rather than per pane or per window, because a preview belongs to the folder it is
208    /// beside: two panes each showing a build of the same DLL get their own, and switching tabs
209    /// puts back the one that tab had open. Where the panel *goes* is the window's preference and
210    /// lives on `App` — see [`crate::ui::preview::Layout`].
211    pub preview: crate::ui::preview::Preview,
212}
213
214/// Widths for the three fitted columns before anything has been measured. Only
215/// visible for the frame between a listing arriving and being drawn.
216const DEFAULT_WIDTHS: [f32; 4] = [240.0, 88.0, 130.0, 132.0];
217
218/// How long a folder may take to read before the listing admits to waiting for it.
219///
220/// **Under this, saying anything is worse than saying nothing.** A local folder comes back in
221/// single-digit milliseconds — [`crate::fs::scan`]'s whole design is about that — so a
222/// `Reading…` drawn the moment a folder is asked for is a word that flashes up and vanishes on
223/// every single navigation, in the exact place the listing is about to be. Over half a second,
224/// silence is the thing that misleads: a window showing an empty folder that is not empty looks
225/// like a program that has finished and got it wrong.
226///
227/// Half a second is the usual floor for "worth telling somebody about" and it is comfortably
228/// past every local case; what it catches is the disconnected share and the sleeping drive,
229/// which are the two this program has always had to be honest about.
230pub const SLOW_SCAN: f64 = 0.5;
231
232/// How long the filter waits for the typing to stop before it is applied.
233///
234/// **A filter is re-applied from scratch on every change** — one pass over every entry, then a
235/// sort of whatever survived — so on a listing big enough it is not the filter that is slow, it
236/// is *typing*. Measured by [`crate::fs::sort::tests::filter_speed`] over a flattened
237/// `C:\Program Files`, 188,729 entries, release build:
238///
239/// | filter | rows | one pass |
240/// | --- | --- | --- |
241/// | `""` | 188,685 | 240 ms |
242/// | `"e"` | 187,961 | 239 ms |
243/// | `"ex"` | 62,296 | 107 ms |
244/// | `"exe"` | 3,072 | 18 ms |
245///
246/// Note which passes are the expensive ones: the *intermediate* needles, because they are the
247/// ones that leave enough rows to sort. Typing `exe` cost 364 ms of frozen window in three
248/// stalls to arrive at an answer that costs 18 ms to compute — and the two stalls in front of it
249/// were for orders nobody was going to read.
250///
251/// So each keystroke restarts this timer and only the last one does any work.
252///
253/// A quarter of a second is longer than the ~150 ms at which a pause starts to read as lag, and
254/// that is the trade being made deliberately: the pause is not being hidden, it is being spent
255/// on *not* running the two passes above it. A keystroke gap of 250 ms is slow typing — a word
256/// typed at any ordinary speed collapses into one pass — and where the answer costs a fifth of a
257/// second to compute, waiting a quarter for the typing to finish is cheaper than computing three
258/// answers nobody reads. On a small folder, where a pass is microseconds, all this costs is that
259/// the listing settles a beat after you stop.
260pub const FILTER_DELAY: f64 = 0.25;
261
262/// A number no view has had before.
263///
264/// Not a hash of the path: two views of the *same* folder, and the same folder revisited
265/// after a refresh, are different views, and an answer for one is not an answer for the
266/// other. Wrapping is irrelevant at one per navigation.
267fn next_view() -> u64 {
268    use std::sync::atomic::{AtomicU64, Ordering};
269    static NEXT: AtomicU64 = AtomicU64::new(1);
270    NEXT.fetch_add(1, Ordering::Relaxed)
271}
272
273impl Tab {
274    pub fn new(path: impl Into<PathBuf>) -> Self {
275        let path = path.into();
276        Self {
277            title: display_name(&path),
278            history: vec![path.clone()],
279            at: 0,
280            trail: path.clone(),
281            path,
282            dir: None,
283            awaiting: None,
284            asked_at: None,
285            view: next_view(),
286            file_icons: Vec::new(),
287            links: std::collections::HashMap::new(),
288            order: Vec::new(),
289            // Type, not Name: a folder read by type comes up grouped — every source
290            // file together, every image together — and within a group it is still in
291            // name order, so nothing is harder to find than it would have been.
292            sort_by: Column::Type,
293            ascending: true,
294            filter: String::new(),
295            filter_at: None,
296            show_hidden: false,
297            flat: false,
298            selected: Vec::new(),
299            selected_count: 0,
300            cursor: None,
301            anchor: None,
302            widths: DEFAULT_WIDTHS,
303            widths_measured: false,
304            reveal: None,
305            keep_selected: Vec::new(),
306            rename_revealed: false,
307            scroll_to_cursor: false,
308            typeahead: String::new(),
309            typeahead_at: 0.0,
310            editing_path: false,
311            edit_text: String::new(),
312            scroll_y: 0.0,
313            scroll_to: None,
314            band: None,
315            renaming: None,
316            rename_fresh: false,
317            preview: crate::ui::preview::Preview::default(),
318        }
319    }
320
321    /// A copy of this tab pointing at the same place — what the `+` button and a
322    /// duplicate both make. The listing comes along, so a new tab on the current
323    /// folder is populated in the frame it is created.
324    pub fn duplicate(&self) -> Self {
325        let mut tab = Self::new(self.path.clone());
326        // The bar comes across as it looks, or a duplicate of a tab you had walked up
327        // would silently lose the trail the original still shows.
328        tab.trail = self.trail.clone();
329        tab.sort_by = self.sort_by;
330        tab.ascending = self.ascending;
331        tab.show_hidden = self.show_hidden;
332        // Including flattened, and the listing with it: a duplicate is the same place as
333        // it currently *looks*, and a copy that quietly walked the tree again — for
334        // seconds, on a big one — would be a worse answer than either keeping it or
335        // dropping it.
336        tab.flat = self.flat;
337        tab.widths = self.widths;
338        tab.widths_measured = self.widths_measured;
339        // Open the same way, but reading for itself — see `Preview::duplicate`, which explains
340        // why the decoded content is deliberately not carried across.
341        tab.preview = self.preview.duplicate();
342        if let Some(dir) = &self.dir {
343            tab.apply(dir.clone());
344        }
345        tab
346    }
347
348    // ---- Navigation ----------------------------------------------------
349
350    /// Go somewhere, recording it in the history.
351    pub fn navigate(&mut self, path: impl Into<PathBuf>) {
352        let path = path.into();
353        if path == self.path {
354            return;
355        }
356        // Anything you do after going Back replaces the forward trail.
357        self.history.truncate(self.at + 1);
358        self.history.push(path.clone());
359        self.at = self.history.len() - 1;
360        // A history that grows all session is a leak nobody notices until it is
361        // one; 256 places back is more than anyone walks.
362        if self.history.len() > 256 {
363            let excess = self.history.len() - 256;
364            self.history.drain(..excess);
365            self.at -= excess;
366        }
367        self.go_to(path);
368    }
369
370    pub fn can_go_back(&self) -> bool {
371        self.at > 0
372    }
373
374    pub fn can_go_forward(&self) -> bool {
375        self.at + 1 < self.history.len()
376    }
377
378    pub fn go_back(&mut self) {
379        if self.can_go_back() {
380            self.at -= 1;
381            let path = self.history[self.at].clone();
382            // Where you were is highlighted by `go_to`, from the trail, for every arrival rather
383            // than only for the one that lands on the parent.
384            self.go_to(path);
385        }
386    }
387
388    pub fn go_forward(&mut self) {
389        if self.can_go_forward() {
390            self.at += 1;
391            let path = self.history[self.at].clone();
392            self.go_to(path);
393        }
394    }
395
396    /// Up one level. The folder just left is highlighted by [`Tab::go_to`], off the trail.
397    pub fn go_up(&mut self) {
398        if let Some(parent) = crate::fs::parent_of(&self.path) {
399            self.navigate(parent);
400        }
401    }
402
403    /// Point the tab at a path without touching the history.
404    fn go_to(&mut self, path: PathBuf) {
405        // The one place [`Tab::trail`] is decided, because this is the one place the path
406        // changes — `navigate`, `go_back`, `go_forward` and `go_up` all come through here.
407        //
408        // `Path::starts_with` compares whole components, so `C:\Users` is a prefix of
409        // `C:\Users\tony` and `C:\Use` is not. The empty path is a prefix of everything,
410        // which is the answer this wants: This PC is where the breadcrumb starts, so going
411        // there is going up rather than going somewhere else.
412        if !self.trail.starts_with(&path) {
413            self.trail = path.clone();
414        }
415        // **The child of this folder that the breadcrumb still shows, selected on arrival.**
416        // Standing in `a/b` with `a/b/c` on the bar, `c` is the one thing you are most likely to
417        // want next — it is where you just came from, or where you were heading before you
418        // stopped off here — and the bar is already pointing at it. So the listing selects it and
419        // scrolls to it, which for a folder of five thousand names is the difference between
420        // going up a level and losing your place.
421        //
422        // Asked of [`crate::fs::breadcrumb_segments`] rather than worked out from components, so
423        // that "what the breadcrumb shows" means literally that: the same walk, with the same
424        // answer for a drive root and for This PC, where a raw component walk gives `C:` without
425        // its root and no place at all above it.
426        //
427        // This subsumes what `go_up` and `go_back` each used to do for themselves, and covers
428        // what neither did: clicking a segment three levels up now selects the segment below it
429        // too, rather than only a single step back.
430        let trail = crate::fs::breadcrumb_segments(&self.trail);
431        self.reveal = trail
432            .iter()
433            .position(|(_, at)| *at == path)
434            .and_then(|here| trail.get(here + 1))
435            .map(|(_, child)| display_name(child));
436        self.title = display_name(&path);
437        self.path = path;
438        self.dir = None;
439        self.awaiting = None;
440        self.asked_at = None;
441        // A new view of a new folder: any icon answer still in flight for the old one is now
442        // addressed to a view that no longer exists, and the column it would have gone in is
443        // released here.
444        self.view = next_view();
445        self.file_icons = Vec::new();
446        self.file_icons.shrink_to_fit();
447        self.links = std::collections::HashMap::new();
448        self.order.clear();
449        self.selected.clear();
450        self.selected_count = 0;
451        self.cursor = None;
452        self.anchor = None;
453        self.filter.clear();
454        self.filter_at = None;
455        // **A flatten does not come along to the next folder**, for the same reason the
456        // filter does not: both are a question asked of the folder you were looking at,
457        // and the answer to a question about somewhere else is not the same answer. It
458        // also means opening a row in a flattened listing lands in an ordinary folder,
459        // which is the only way out of the view that does not need the button again — and
460        // it is what stops a click on a deep folder from silently starting a second tree
461        // walk. `Tab::refresh` keeps it, because that is the same question again.
462        self.flat = false;
463        self.widths_measured = false;
464        self.editing_path = false;
465        // **A different folder opens at the top.** Row 200 of the folder you just left is not
466        // row 200 of anything, and the scroll offset does not belong to this tab as far as egui
467        // is concerned — it belongs to the pane's one scroll area, which goes on showing
468        // whatever it was showing unless something asks it not to. Setting `scroll_y` alone
469        // records where the listing *is*; `scroll_to` is what moves it.
470        self.scroll_y = 0.0;
471        self.scroll_to = Some(0.0);
472        self.band = None;
473        self.renaming = None;
474        self.keep_selected.clear();
475    }
476
477    /// Flatten this folder, or stop flattening it.
478    ///
479    /// The listing goes, because the two are different listings of the same folder —
480    /// a flattened one is [`crate::fs::scan::scan_deep`]'s and holds relative paths.
481    /// Turning it *off* comes straight back out of the cache, which still has the
482    /// folder's own children; turning it *on* is a fresh walk every time, which is the
483    /// point of the gesture.
484    ///
485    /// The selection is not carried across. It is kept by name, and a name means two
486    /// different things on the two sides of this: `file.txt` on one and
487    /// `sub\file.txt` on the other, so nothing would match anyway — and a selection
488    /// that half-survived would be worse than one that plainly did not.
489    pub fn toggle_flat(&mut self) {
490        // "This PC" is not a folder and has no tree: its rows are volumes, each of which is a
491        // place to flatten of its own. There is nothing for the button to do here, so it is
492        // drawn disabled and this refuses — rather than latching over a listing that would not
493        // have changed.
494        if self.path.as_os_str().is_empty() {
495            return;
496        }
497        self.flat = !self.flat;
498        self.keep_selected.clear();
499        self.selected.clear();
500        self.selected_count = 0;
501        self.cursor = None;
502        self.anchor = None;
503        self.renaming = None;
504        self.dir = None;
505        self.awaiting = None;
506        self.asked_at = None;
507        self.order.clear();
508        self.widths_measured = false;
509        // The top, because row 200 of a folder's own children is not row 200 of its
510        // whole tree — the same reason a new folder opens at the top in [`Tab::go_to`].
511        self.scroll_y = 0.0;
512        self.scroll_to = Some(0.0);
513    }
514
515    /// Drop the listing so the folder is read again, keeping the selection by name.
516    ///
517    /// This is a *refresh*, not a navigation: the same folder, read afresh because something
518    /// changed it. Every file operation ends here, so losing the selection here means copying a
519    /// file and then having nothing selected to copy again.
520    pub fn refresh(&mut self) {
521        self.keep_selected = match &self.dir {
522            Some(dir) => self
523                .selected
524                .iter()
525                .enumerate()
526                .filter(|(_, &on)| on)
527                .filter(|&(entry, _)| entry < dir.len())
528                .map(|(entry, _)| dir.name(entry).to_owned())
529                .collect(),
530            None => std::mem::take(&mut self.keep_selected),
531        };
532        self.dir = None;
533        self.awaiting = None;
534        self.asked_at = None;
535    }
536
537    /// Whether this tab has been waiting for its listing long enough to say so.
538    ///
539    /// See [`SLOW_SCAN`]. `false` while nothing has been asked for, which is also the
540    /// answer for the frame between a folder being wanted and being requested.
541    pub fn waiting_visibly(&self, now: f64) -> bool {
542        self.asked_at.is_some_and(|asked| now - asked >= SLOW_SCAN)
543    }
544
545    // ---- The listing ---------------------------------------------------
546
547    /// Take a finished listing and build the view over it.
548    ///
549    /// A *re-read* of the folder already being shown keeps the selection, by name. This is what
550    /// a refresh has to do -- every file operation ends in one, so a selection that did not
551    /// survive it meant copying a file and then finding nothing selected to copy again. Which is
552    /// exactly what Ctrl+C, Ctrl+V, Ctrl+C did: the second copy had nothing to work with.
553    /// [`Tab::go_to`] still clears everything, because a different folder is a different set of
554    /// files.
555    pub fn apply(&mut self, dir: Arc<Dir>) {
556        // Whatever [`Tab::refresh`] put aside, plus the live selection when the listing is being
557        // replaced under a tab that still has one.
558        let mut held = std::mem::take(&mut self.keep_selected);
559        if held.is_empty() {
560            if let Some(old) = self.dir.as_ref().filter(|_| self.selected_count > 0) {
561                held = self
562                    .selected
563                    .iter()
564                    .enumerate()
565                    .filter(|(_, &on)| on)
566                    .filter(|&(entry, _)| entry < old.len())
567                    .map(|(entry, _)| old.name(entry).to_owned())
568                    .collect();
569            }
570        }
571
572        self.selected = vec![false; dir.len()];
573        self.file_icons = vec![crate::shell::icons::UNASKED; dir.len()];
574        self.links.clear();
575        self.selected_count = 0;
576        self.cursor = None;
577        self.anchor = None;
578        self.widths_measured = false;
579        self.dir = Some(dir);
580        self.awaiting = None;
581        self.asked_at = None;
582        self.rebuild_order();
583
584        // Whatever was selected and is still there. Anything that has gone -- moved, renamed,
585        // deleted -- simply is not selected any more, which is the only sensible answer.
586        if !held.is_empty() {
587            if let Some(dir) = self.dir.clone() {
588                for (position, &entry) in self.order.iter().enumerate() {
589                    if held.iter().any(|name| name == dir.name(entry as usize)) {
590                        self.selected[entry as usize] = true;
591                        self.selected_count += 1;
592                        self.cursor.get_or_insert(position);
593                        self.anchor.get_or_insert(position);
594                    }
595                }
596            }
597        }
598
599        // Highlight and scroll to whatever we came out of.
600        let rename = std::mem::take(&mut self.rename_revealed);
601        if let Some(name) = self.reveal.take() {
602            if let Some(dir) = self.dir.clone() {
603                // By leaf, so that a row revealed after a rename or a new folder is still
604                // found in a flattened listing, where the name it is stored under carries
605                // the folders in front of it. Identical to the name in every other
606                // listing. Two files of the same name in different folders are two matches
607                // and the first one wins, which is the same rule a duplicate name in one
608                // folder would meet if the filesystem allowed one.
609                if let Some(at) = self
610                    .order
611                    .iter()
612                    .position(|&i| dir.leaf(i as usize) == name)
613                {
614                    self.select_only(at);
615                    self.scroll_to_cursor = true;
616                    // And it beats the top of the listing, which is where [`Tab::go_to`] asks a
617                    // new folder to open. This is Back or Up, or a folder just created: the row
618                    // being revealed is the whole reason for the move, and it can be row 500.
619                    self.scroll_to = None;
620                    if rename {
621                        self.begin_rename();
622                    }
623                }
624            }
625        }
626    }
627
628    /// Note that the filter text has changed, without applying it yet.
629    ///
630    /// Called on every keystroke; the last one before the pause is the only one that ends up
631    /// doing any work. See [`FILTER_DELAY`] for what a pass costs and why that matters.
632    pub fn filter_changed(&mut self, now: f64) {
633        self.filter_at = Some(now);
634    }
635
636    /// Apply a filter whose keystrokes have stopped, or say how long is left until they have.
637    ///
638    /// Called once a frame. `Some(seconds)` means a rebuild is owed but not due, and the caller
639    /// has to make sure there *is* a frame then — this program is idle between events, so
640    /// without a repaint asked for, the filter would be applied whenever something else next
641    /// happened to want a frame. `None` means there is nothing owed, either because nothing
642    /// changed or because this call has just done it.
643    pub fn settle_filter(&mut self, now: f64) -> Option<f64> {
644        let at = self.filter_at?;
645        let left = FILTER_DELAY - (now - at);
646        if left > 0.0 {
647            return Some(left);
648        }
649        self.filter_at = None;
650        self.rebuild_order();
651        None
652    }
653
654    /// Re-apply the sort and the filter from scratch.
655    pub fn rebuild_order(&mut self) {
656        // Whatever was owed is paid by this: every path into here builds the order from the
657        // filter as it stands, so a deadline left behind would only buy a second identical pass.
658        self.filter_at = None;
659        let Some(dir) = self.dir.clone() else {
660            self.order.clear();
661            return;
662        };
663        // Remember what the cursor was pointing at, since its position moves.
664        let cursor_entry = self.cursor.and_then(|at| self.order.get(at).copied());
665        sort::build_order(
666            &dir,
667            &mut self.order,
668            self.sort_by,
669            self.ascending,
670            self.show_hidden,
671            &self.filter,
672        );
673        self.cursor = cursor_entry.and_then(|entry| self.order.iter().position(|&i| i == entry));
674        self.anchor = self.cursor;
675    }
676
677    /// Click a column header: toggle the direction if it is already the sort,
678    /// otherwise switch to it in its natural direction.
679    pub fn sort_by_column(&mut self, column: Column) {
680        if self.sort_by == column {
681            self.ascending = !self.ascending;
682        } else {
683            self.sort_by = column;
684            self.ascending = column.starts_ascending();
685        }
686        self.rebuild_order();
687    }
688
689    /// The entry index at a position in the display order.
690    #[inline]
691    pub fn entry_at(&self, position: usize) -> Option<usize> {
692        self.order.get(position).map(|&i| i as usize)
693    }
694
695    /// Where the row at `position` leads.
696    pub fn target_at(&self, position: usize) -> Option<PathBuf> {
697        let dir = self.dir.as_ref()?;
698        Some(dir.target(self.entry_at(position)?))
699    }
700
701    /// Whether the row at `position` is a shortcut of either kind — a `.lnk` or a reparse
702    /// point.
703    ///
704    /// From the enumeration alone, so it costs nothing: it says the row is *worth* reading,
705    /// not what it leads to. [`crate::shell::links::folder_target`] is what answers that, and
706    /// it is only ever asked of a row this returns `true` for.
707    pub fn is_shortcut_at(&self, position: usize) -> bool {
708        let Some(entry) = self.entry_at(position) else {
709            return false;
710        };
711        let Some(dir) = &self.dir else { return false };
712        crate::shell::links::kind_of(dir.ext(entry), dir.entries[entry].is_link()).is_some()
713    }
714
715    /// Whether the row at `position` is a folder — which decides whether opening
716    /// it navigates or hands it to the shell.
717    pub fn is_dir_at(&self, position: usize) -> bool {
718        self.entry_at(position)
719            .and_then(|i| self.dir.as_ref().map(|d| d.entries[i].is_dir()))
720            .unwrap_or(false)
721    }
722
723    // ---- Selection -----------------------------------------------------
724
725    #[inline]
726    pub fn is_selected(&self, position: usize) -> bool {
727        self.entry_at(position)
728            .and_then(|i| self.selected.get(i).copied())
729            .unwrap_or(false)
730    }
731
732    pub fn clear_selection(&mut self) {
733        if self.selected_count > 0 {
734            self.selected.iter_mut().for_each(|s| *s = false);
735            self.selected_count = 0;
736        }
737    }
738
739    /// One row, alone. A plain click.
740    pub fn select_only(&mut self, position: usize) {
741        self.clear_selection();
742        if let Some(entry) = self.entry_at(position) {
743            self.selected[entry] = true;
744            self.selected_count = 1;
745        }
746        self.cursor = Some(position);
747        self.anchor = Some(position);
748    }
749
750    /// Flip one row. Ctrl-click.
751    pub fn toggle(&mut self, position: usize) {
752        if let Some(entry) = self.entry_at(position) {
753            let now = !self.selected[entry];
754            self.selected[entry] = now;
755            self.selected_count = if now {
756                self.selected_count + 1
757            } else {
758                self.selected_count.saturating_sub(1)
759            };
760        }
761        self.cursor = Some(position);
762        self.anchor = Some(position);
763    }
764
765    /// Everything between the anchor and here. Shift-click.
766    pub fn select_range_to(&mut self, position: usize) {
767        let from = self.anchor.unwrap_or(position);
768        self.clear_selection();
769        let (lo, hi) = (from.min(position), from.max(position));
770        for at in lo..=hi.min(self.order.len().saturating_sub(1)) {
771            if let Some(entry) = self.entry_at(at) {
772                if !self.selected[entry] {
773                    self.selected[entry] = true;
774                    self.selected_count += 1;
775                }
776            }
777        }
778        self.cursor = Some(position);
779    }
780
781    /// Apply a rubber-band's coverage to the selection.
782    ///
783    /// Recomputed from the band's base on every frame of the drag rather than
784    /// accumulated, so shrinking the band deselects again — which is what makes a
785    /// rubber band feel like a rubber band instead of a paintbrush.
786    pub fn apply_band(&mut self) {
787        let Some(band) = &self.band else { return };
788        let covered = band.rows(self.order.len());
789        let base = &band.base;
790
791        self.selected_count = 0;
792        for (entry, selected) in self.selected.iter_mut().enumerate() {
793            *selected = base.get(entry).copied().unwrap_or(false);
794            if *selected {
795                self.selected_count += 1;
796            }
797        }
798        for position in covered {
799            let Some(&entry) = self.order.get(position) else {
800                continue;
801            };
802            let entry = entry as usize;
803            if !self.selected[entry] {
804                self.selected[entry] = true;
805                self.selected_count += 1;
806            }
807        }
808    }
809
810    /// Begin renaming the row under the cursor.
811    pub fn begin_rename(&mut self) {
812        let Some(position) = self.cursor else { return };
813        let Some(entry) = self.entry_at(position) else {
814            return;
815        };
816        let Some(dir) = &self.dir else { return };
817        // The file's own name, not the whole of what the row shows: in a flattened
818        // listing a name is a relative path, and a rename field holding `sub\file.txt`
819        // would be inviting the user to type a path into a rename.
820        self.renaming = Some((entry, dir.leaf(entry).to_owned()));
821        self.rename_fresh = true;
822    }
823
824    /// The name being edited, if this row is the one being renamed.
825    pub fn rename_text(&mut self, entry: usize) -> Option<&mut String> {
826        match &mut self.renaming {
827            Some((at, text)) if *at == entry => Some(text),
828            _ => None,
829        }
830    }
831
832    pub fn select_all(&mut self) {
833        self.clear_selection();
834        for &entry in &self.order {
835            self.selected[entry as usize] = true;
836        }
837        self.selected_count = self.order.len();
838    }
839
840    /// The paths of everything selected, in display order.
841    pub fn selection_paths(&self) -> Vec<PathBuf> {
842        let Some(dir) = &self.dir else { return Vec::new() };
843        self.order
844            .iter()
845            .filter(|&&i| self.selected.get(i as usize).copied().unwrap_or(false))
846            .map(|&i| dir.target(i as usize))
847            .collect()
848    }
849
850    /// Move the cursor, taking the selection with it unless `extend` is set.
851    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
852        if self.order.is_empty() {
853            return;
854        }
855        let last = self.order.len() as isize - 1;
856        let from = self.cursor.map(|c| c as isize).unwrap_or(-1);
857        let to = (from + delta).clamp(0, last) as usize;
858        if extend {
859            self.select_range_to(to);
860        } else {
861            self.select_only(to);
862        }
863        self.scroll_to_cursor = true;
864    }
865
866    pub fn move_cursor_to(&mut self, position: usize, extend: bool) {
867        if self.order.is_empty() {
868            return;
869        }
870        let position = position.min(self.order.len() - 1);
871        if extend {
872            self.select_range_to(position);
873        } else {
874            self.select_only(position);
875        }
876        self.scroll_to_cursor = true;
877    }
878
879    /// Jump to the next row whose name starts with what has been typed.
880    ///
881    /// `now` is the frame time; a gap longer than a second starts a fresh word,
882    /// which is what makes typing `re` find `readme` but typing `r` a minute later
883    /// start again from `r`.
884    pub fn type_ahead(&mut self, ch: char, now: f64) {
885        if now - self.typeahead_at > 1.0 {
886            self.typeahead.clear();
887        }
888        self.typeahead_at = now;
889        self.typeahead.extend(ch.to_lowercase());
890
891        let Some(dir) = self.dir.clone() else { return };
892        let needle = self.typeahead.as_str();
893        // Start from the row after the cursor, so repeating a letter walks through
894        // the matches instead of sticking on the first.
895        let start = self.cursor.map(|c| c + 1).unwrap_or(0);
896        let found = (0..self.order.len())
897            .map(|offset| (start + offset) % self.order.len())
898            .find(|&at| {
899                let name = dir.name(self.order[at] as usize);
900                name.len() >= needle.len()
901                    && name
902                        .chars()
903                        .zip(needle.chars())
904                        .all(|(a, b)| a.to_ascii_lowercase() == b)
905            });
906        if let Some(at) = found {
907            self.select_only(at);
908            self.scroll_to_cursor = true;
909        }
910    }
911}
912
913/// A stack of tabs, one of them showing.
914pub struct Pane {
915    pub id: PaneId,
916    pub tabs: Vec<Tab>,
917    pub active: usize,
918    /// Where this pane was last drawn. Needed because the tab strip is drawn
919    /// before the panes are, so a drop target is resolved against the previous
920    /// frame's geometry.
921    pub rect: egui::Rect,
922    /// Where the folder rows were, so files can be dropped into one of them. Written by the
923    /// listing each frame and read by the next, for the same reason `rect` is.
924    pub drop_rows: Vec<(egui::Rect, std::path::PathBuf)>,
925    /// Where the rows were, which is where a drop into this pane's own folder goes. Written and
926    /// read the same way, and for the same reason, as `drop_rows`.
927    pub drop_area: egui::Rect,
928}
929
930impl Pane {
931    pub fn new(id: PaneId, tab: Tab) -> Self {
932        Self {
933            id,
934            tabs: vec![tab],
935            active: 0,
936            rect: egui::Rect::NOTHING,
937            drop_rows: Vec::new(),
938            drop_area: egui::Rect::NOTHING,
939        }
940    }
941
942    /// Show tab `index`, bringing its own scroll position with it.
943    ///
944    /// **Every change of active tab goes through here.** The listing's scroll offset lives in
945    /// the pane's one scroll area, which egui keys by where it is drawn rather than by what it
946    /// shows — so a tab that comes to the front inherits wherever the tab before it had got to
947    /// unless it asks for its own place back. [`Tab::scroll_y`] is where the tab keeps it, and
948    /// [`Tab::scroll_to`] is what moves the listing there.
949    pub fn show_tab(&mut self, index: usize) {
950        self.active = index.min(self.tabs.len().saturating_sub(1));
951        let tab = self.tab_mut();
952        tab.scroll_to = Some(tab.scroll_y);
953    }
954
955    pub fn tab(&self) -> &Tab {
956        &self.tabs[self.active.min(self.tabs.len() - 1)]
957    }
958
959    pub fn tab_mut(&mut self) -> &mut Tab {
960        let at = self.active.min(self.tabs.len() - 1);
961        &mut self.tabs[at]
962    }
963
964    /// Close a tab and keep the selection somewhere sensible: the tab to the
965    /// right, as every browser does. Returns whether the pane still has tabs.
966    pub fn close_tab(&mut self, at: usize) -> bool {
967        if at >= self.tabs.len() {
968            return true;
969        }
970        self.tabs.remove(at);
971        if self.tabs.is_empty() {
972            return false;
973        }
974        if self.active > at || self.active >= self.tabs.len() {
975            self.show_tab(self.active.saturating_sub(1));
976        }
977        true
978    }
979}
980
981/// Which side of a pane something is being dropped on.
982#[derive(Clone, Copy, PartialEq, Eq, Debug)]
983pub enum Side {
984    Left,
985    Right,
986    Top,
987    Bottom,
988}
989
990impl Side {
991    /// Whether a split on this side arranges its children side by side.
992    pub fn is_horizontal(self) -> bool {
993        matches!(self, Self::Left | Self::Right)
994    }
995
996    /// Whether the dropped pane becomes the *first* child.
997    pub fn is_first(self) -> bool {
998        matches!(self, Self::Left | Self::Top)
999    }
1000}
1001
1002#[cfg(test)]
1003mod tests {
1004    use super::*;
1005
1006    /// Arriving somewhere the breadcrumb still runs past selects the segment below it.
1007    #[test]
1008    fn arriving_at_a_folder_selects_the_child_the_breadcrumb_shows() {
1009        // One step up: `c` is on the bar and is where you just came from.
1010        let mut tab = Tab::new(r"C:\a\b\c");
1011        tab.navigate(r"C:\a\b");
1012        assert_eq!(tab.reveal.as_deref(), Some("c"));
1013
1014        // Two at once, which is what clicking a segment does. The old rule only ever managed a
1015        // single step, because it asked whether the place it landed was the parent.
1016        let mut tab = Tab::new(r"C:\a\b\c");
1017        tab.navigate(r"C:\a");
1018        assert_eq!(tab.reveal.as_deref(), Some("b"));
1019
1020        // Going *down* leaves nothing behind on the bar to point at.
1021        let mut tab = Tab::new(r"C:\a");
1022        tab.navigate(r"C:\a\b");
1023        assert_eq!(tab.reveal, None);
1024
1025        // Sideways: the trail is replaced, so again there is nothing.
1026        let mut tab = Tab::new(r"C:\a\b\c");
1027        tab.navigate(r"D:\elsewhere");
1028        assert_eq!(tab.reveal, None);
1029    }
1030
1031    /// The three ways of arriving all get it, because they all go through `go_to`.
1032    #[test]
1033    fn up_back_and_forward_all_highlight_off_the_trail() {
1034        let mut tab = Tab::new(r"C:\a\b\c");
1035        tab.go_up();
1036        assert_eq!(tab.path, PathBuf::from(r"C:\a\b"));
1037        assert_eq!(tab.reveal.as_deref(), Some("c"), "up left `c` behind");
1038
1039        tab.go_up();
1040        assert_eq!(tab.reveal.as_deref(), Some("b"), "and `b` above that");
1041        // Going up *records* the arrival, so there is nothing forward of here to go to — the
1042        // history reads `c`, `b`, `a` and the cursor is on its last entry.
1043        assert!(!tab.can_go_forward());
1044
1045        // Back down it, which is where `go_back` used to do this for itself.
1046        tab.go_back();
1047        assert_eq!(tab.path, PathBuf::from(r"C:\a\b"));
1048        assert_eq!(tab.reveal.as_deref(), Some("c"));
1049        tab.go_back();
1050        assert_eq!(tab.path, PathBuf::from(r"C:\a\b\c"));
1051        assert_eq!(tab.reveal, None, "arriving at the end of the trail");
1052
1053        // And forward, which never highlighted anything before and now does.
1054        tab.go_forward();
1055        assert_eq!(tab.path, PathBuf::from(r"C:\a\b"));
1056        assert_eq!(tab.reveal.as_deref(), Some("c"));
1057    }
1058
1059    /// A re-read of the same folder keeps what was selected; going somewhere else does not.
1060    ///
1061    /// Every file operation ends in a re-read, so a selection that does not survive one means
1062    /// copying a file and then having nothing selected to copy again. That is what made Ctrl+C
1063    /// then Ctrl+V work exactly once: the second Ctrl+C had an empty selection and copied nothing.
1064    #[test]
1065    fn a_re_read_keeps_the_selection_and_a_new_folder_does_not() {
1066        use crate::fs::dir::DirBuilder;
1067
1068        let folder = PathBuf::from(r"C:\somewhere");
1069        let listing = |at: &std::path::Path, names: &[&str]| {
1070            let mut build = DirBuilder::new(at.to_path_buf());
1071            for name in names {
1072                build.push(name, 1, 0, 0);
1073            }
1074            Arc::new(build.finish(0))
1075        };
1076        /// The display position of a name, which is what the selection is indexed by.
1077        fn position_of(tab: &Tab, name: &str) -> usize {
1078            (0..tab.order.len())
1079                .find(|at| {
1080                    tab.entry_at(*at)
1081                        .and_then(|e| tab.dir.as_ref().map(|d| d.name(e) == name))
1082                        .unwrap_or(false)
1083                })
1084                .unwrap_or_else(|| panic!("`{name}` is not in the listing"))
1085        }
1086
1087        let mut tab = Tab::new(folder.clone());
1088        tab.apply(listing(&folder, &["one.txt", "two.txt", "three.txt"]));
1089        tab.select_only(position_of(&tab, "two.txt"));
1090        assert_eq!(tab.selection_paths().len(), 1);
1091
1092        // The same folder read again -- a refresh, or the tail of a file operation.
1093        tab.apply(listing(&folder, &["one.txt", "two.txt", "three.txt", "four.txt"]));
1094        let still: Vec<String> = tab
1095            .selection_paths()
1096            .iter()
1097            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
1098            .collect();
1099        assert_eq!(still, ["two.txt"], "the selection has to survive a re-read");
1100        assert!(tab.cursor.is_some(), "and the cursor has to go with it");
1101
1102        // Something that has gone is simply not selected any more.
1103        tab.apply(listing(&folder, &["one.txt", "three.txt"]));
1104        assert_eq!(tab.selection_paths().len(), 0);
1105
1106        // A different folder is a different set of files, even when a name matches.
1107        tab.apply(listing(&folder, &["one.txt", "two.txt"]));
1108        tab.select_only(position_of(&tab, "two.txt"));
1109        let elsewhere = PathBuf::from(r"C:\elsewhere");
1110        tab.go_to(elsewhere.clone());
1111        tab.apply(listing(&elsewhere, &["two.txt"]));
1112        assert_eq!(
1113            tab.selection_paths().len(),
1114            0,
1115            "a name that happens to match somewhere else is not the same file"
1116        );
1117    }
1118
1119    #[test]
1120    fn navigating_after_back_drops_the_future() {
1121        let mut tab = Tab::new("/a");
1122        tab.navigate("/a/b");
1123        tab.navigate("/a/b/c");
1124        assert!(tab.can_go_back());
1125
1126        tab.go_back();
1127        assert_eq!(tab.path, PathBuf::from("/a/b"));
1128        assert!(tab.can_go_forward());
1129
1130        tab.navigate("/a/b/d");
1131        assert!(
1132            !tab.can_go_forward(),
1133            "going somewhere new has to replace the forward trail"
1134        );
1135        assert_eq!(tab.history, ["/a", "/a/b", "/a/b/d"].map(PathBuf::from));
1136    }
1137
1138    #[test]
1139    fn walking_up_keeps_the_trail_on_the_breadcrumb() {
1140        let mut tab = Tab::new("/a/b/c");
1141        assert_eq!(tab.trail, PathBuf::from("/a/b/c"));
1142
1143        // Up: the folder just left is still on the bar, which is the point.
1144        tab.go_up();
1145        assert_eq!(tab.path, PathBuf::from("/a/b"));
1146        assert_eq!(tab.trail, PathBuf::from("/a/b/c"));
1147
1148        // And again -- two levels up still shows all three.
1149        tab.go_up();
1150        assert_eq!(tab.path, PathBuf::from("/a"));
1151        assert_eq!(tab.trail, PathBuf::from("/a/b/c"));
1152
1153        // Back down onto the trail: the trail is unchanged, so nothing on the bar moves
1154        // while the bold segment walks along it.
1155        tab.navigate("/a/b");
1156        assert_eq!(tab.trail, PathBuf::from("/a/b/c"));
1157
1158        // Deeper than the trail extends it.
1159        tab.navigate("/a/b/c/d");
1160        assert_eq!(tab.trail, PathBuf::from("/a/b/c/d"));
1161    }
1162
1163    #[test]
1164    fn stepping_off_the_trail_replaces_it() {
1165        let mut tab = Tab::new("/a/b/c");
1166        tab.go_up();
1167
1168        // A sibling is not an ancestor, however much of the path it shares.
1169        tab.navigate("/a/b/x");
1170        assert_eq!(tab.trail, PathBuf::from("/a/b/x"));
1171
1172        // Nor is a folder whose name merely starts the same way: `starts_with` compares
1173        // components, so `/a/bb` is not under `/a/b`.
1174        tab.navigate("/a/bb");
1175        assert_eq!(tab.trail, PathBuf::from("/a/bb"));
1176    }
1177
1178    #[test]
1179    fn this_pc_is_up_from_everywhere() {
1180        // The empty path is This PC, and the breadcrumb always starts there -- so going to
1181        // it is walking up, and the trail stays.
1182        let mut tab = Tab::new("/a/b");
1183        tab.navigate(PathBuf::new());
1184        assert!(tab.path.as_os_str().is_empty());
1185        assert_eq!(tab.trail, PathBuf::from("/a/b"));
1186
1187        // Out of This PC to a different root: nothing shared, so the trail goes.
1188        tab.navigate("/z");
1189        assert_eq!(tab.trail, PathBuf::from("/z"));
1190    }
1191
1192    #[test]
1193    fn a_duplicate_shows_the_same_bar() {
1194        let mut tab = Tab::new("/a/b/c");
1195        tab.go_up();
1196        let copy = tab.duplicate();
1197        assert_eq!(copy.path, PathBuf::from("/a/b"));
1198        assert_eq!(
1199            copy.trail,
1200            PathBuf::from("/a/b/c"),
1201            "a duplicate that lost the trail would show a different bar from the tab it \
1202             was copied from"
1203        );
1204    }
1205
1206    #[test]
1207    fn navigating_to_where_you_are_is_not_history() {
1208        let mut tab = Tab::new("/a");
1209        tab.navigate("/a");
1210        assert_eq!(tab.history.len(), 1);
1211    }
1212
1213    #[test]
1214    fn history_stops_growing() {
1215        let mut tab = Tab::new("/0");
1216        for i in 1..400 {
1217            tab.navigate(format!("/{i}"));
1218        }
1219        assert_eq!(tab.history.len(), 256);
1220        assert_eq!(tab.at, 255, "the cursor has to follow the truncation");
1221        assert_eq!(tab.history[tab.at], tab.path);
1222    }
1223
1224    #[test]
1225    fn closing_the_last_tab_reports_the_pane_is_empty() {
1226        let mut pane = Pane::new(0, Tab::new("/a"));
1227        assert!(!pane.close_tab(0));
1228    }
1229
1230    #[test]
1231    fn closing_a_tab_keeps_the_active_one_active() {
1232        let mut pane = Pane::new(0, Tab::new("/a"));
1233        pane.tabs.push(Tab::new("/b"));
1234        pane.tabs.push(Tab::new("/c"));
1235        pane.active = 2;
1236
1237        assert!(pane.close_tab(0));
1238        assert_eq!(pane.active, 1, "still looking at /c");
1239        assert_eq!(pane.tab().path, PathBuf::from("/c"));
1240
1241        assert!(pane.close_tab(1));
1242        assert_eq!(pane.tab().path, PathBuf::from("/b"));
1243    }
1244}
