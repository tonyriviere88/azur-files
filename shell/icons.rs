1//! Explorer's own file icons, at Explorer's speed.
2//!
3//! # The problem this has to solve
4//!
5//! The obvious way to get a file's icon is `SHGetFileInfoW` with `SHGFI_ICON` for
6//! each row. [`crate::fs::scan`]'s benchmark measures that path at **over a
7//! millisecond per file** — 67 seconds for a folder of 60,000 — because it opens the
8//! file, consults its association, may extract a resource from a DLL, and returns a
9//! fresh `HICON` to be destroyed. It is exactly what makes a shell-backed file
10//! manager show an empty window for a second before its rows appear.
11//!
12//! Three things together make this cost nothing instead:
13//!
14//! 1. **`SHGFI_USEFILEATTRIBUTES`.** With it, the shell answers from the *name and
15//!    the attribute word* and never touches the file. That removes the I/O, and with
16//!    it the reason a folder of 60,000 was slow.
17//! 2. **`SHGFI_SYSICONINDEX`, not `SHGFI_ICON`.** An index into the one system image
18//!    list, rather than a handle to own and free. Nothing is allocated per row.
19//! 3. **A cache keyed by extension.** Every `.rs` in a folder resolves to the same
20//!    index, so a folder of ten thousand source files performs *one* lookup. This is
21//!    the step that turns a per-file cost into a per-*type* one, and there are only
22//!    ever a few dozen types on screen.
23//!
24//! The bitmap itself is then pulled out of the system image list once per icon index
25//! and uploaded to one egui texture atlas, so a row costs a textured quad — the same
26//! as the painted glyphs it replaces.
27//!
28//! # What still has to be per-file
29//!
30//! Executables, shortcuts and anything with its own embedded icon: `.exe` files do
31//! not share an icon the way `.txt` files do. Those are resolved per file, off the
32//! UI thread, and until the answer arrives the row shows the generic icon for its
33//! kind. A folder full of executables therefore fills in over a few frames rather
34//! than all at once, which is the same thing Explorer does and for the same reason.
35//!
36//! # Where a per-file answer lives, and why it matters
37//!
38//! **In the view that asked, keyed by row.** Not here, and not keyed by path.
39//!
40//! It used to be a `HashMap<PathBuf, i32>` on this struct, which is the obvious design and
41//! is a memory leak with a cache's manners: scrolling `C:\Windows\System32` put **4,345
42//! entries** in it — a heap-allocated path per executable file ever seen — and none of them
43//! went away when you left the folder. Twenty folders like that is 87,000 paths, and the
44//! shell-side cost of every question behind them.
45//!
46//! So a request carries the *view* that asked and the *row* it asked about, and the answer
47//! comes back addressed the same way. [`crate::pane::Tab`] holds a `Vec<i32>` — four bytes a
48//! row, sized when the listing lands, dropped when the tab moves. Leave the folder and
49//! everything the folder cost goes with it; an answer that arrives afterwards finds no view
50//! to belong to and is thrown away. Measured, the same scroll of System32 went from
51//! **+8.7 MB to +1.4 MB**.
52//!
53//! What stays here is only what is genuinely shared and genuinely small: one entry per file
54//! *type*, one per sidebar *place*, and the uploaded bitmaps — of which a folder of 4,910
55//! executables produced twenty, because thousands of files share a handful of icons.
56//!
57//! The questions go to **one worker thread with a bounded queue**. That used to be a thread
58//! per question, so the same scroll spawned 4,910 of them, each with a stack; the burst was
59//! visible as megabytes arriving and leaving. A dropped question costs a row the generic icon
60//! until it comes round again, which is cheaper than remembering every path in order to
61//! avoid asking twice.
62
63use std::collections::HashMap;
64use std::path::{Path, PathBuf};
65use std::sync::mpsc::{channel, Receiver, Sender};
66use std::sync::{Arc, Mutex};
67
68use egui::{ColorImage, TextureHandle, TextureOptions};
69
70/// The size the shell's small image list is drawn at.
71///
72/// 16×16 on every Windows to date. Only the tests name it — the drawing code takes
73/// whatever size the bitmap turns out to be and scales it into the row, so a future
74/// Windows that changes this needs no code change here.
75#[cfg_attr(not(test), allow(dead_code))]
76const SMALL: usize = 16;
77
78/// Extensions whose icon is stored in the file rather than shared by the type.
79///
80/// A `.exe` carries its own; a `.txt` does not. Only these pay a per-file lookup.
81/// Dropping `dll` and `ocx` from this list was tried, on the grounds that scrolling all of
82/// System32 asked about 4,345 files and got 20 distinct icons back. It cut the questions to
83/// 710 and did not measurably cut the memory — the cost is the shell's own cache, which it
84/// keeps whether or not this program does — so the fidelity was not worth spending. The
85/// [`Recent`] cap is what bounds it instead.
86#[cfg(windows)]
87fn has_own_icon(ext: &str) -> bool {
88    matches!(
89        ext.to_ascii_lowercase().as_str(),
90        "exe" | "dll" | "ico" | "lnk" | "url" | "cpl" | "msc" | "scr" | "ocx" | "msi"
91    )
92}
93
94/// One atlas cell, and the grid: 16x16 icons, 32 across and 8 down.
95const CELL: usize = 16;
96const ATLAS_COLUMNS: usize = 32;
97const ATLAS_ROWS: usize = 8;
98const ATLAS_SLOTS: usize = ATLAS_COLUMNS * ATLAS_ROWS;
99
100/// A map that remembers when each entry was last read, so the least useful can go when
101/// there are too many of them or when nothing has wanted them for a while.
102///
103/// The two caches below are keyed by *path* and by *texture*, and both are unbounded in the
104/// thing that grows: files, and GL objects. Measured, scrolling `C:\Windows\System32` — 4,910
105/// files, nearly all of them `.dll` — put **4,345 entries** in the path map and cost 8.7 MB,
106/// and twenty folders like it would be most of a memory budget. The same measurement is the
107/// argument for the size of the caps: those 4,345 lookups produced **20 distinct icons**, so
108/// the path map is a lookup table for a handful of answers and holding thousands of its keys
109/// buys nothing.
110///
111/// A plain `HashMap` plus a stamp rather than an intrusive list: an eviction happens once
112/// every few hundred lookups, and one sort of a thousand stamps is cheaper to run than a
113/// doubly-linked list is to maintain — or to read.
114struct Recent<K, V> {
115    items: HashMap<K, (V, std::time::Instant)>,
116    /// Trim back to three-quarters of this when it is passed.
117    cap: usize,
118}
119
120impl<K: std::hash::Hash + Eq + Clone, V> Recent<K, V> {
121    fn new(cap: usize) -> Self {
122        Self {
123            items: HashMap::new(),
124            cap,
125        }
126    }
127
128    /// Read an entry, marking it as used now.
129    fn get(&mut self, key: &K) -> Option<&V> {
130        let (value, used) = self.items.get_mut(key)?;
131        *used = std::time::Instant::now();
132        Some(value)
133    }
134
135    fn insert(&mut self, key: K, value: V) {
136        self.items.insert(key, (value, std::time::Instant::now()));
137        if self.items.len() > self.cap {
138            self.evict_oldest(self.items.len() - self.cap * 3 / 4);
139        }
140    }
141
142    fn len(&self) -> usize {
143        self.items.len()
144    }
145
146    /// Drop the `count` least recently read entries.
147    fn evict_oldest(&mut self, count: usize) {
148        let mut stamps: Vec<(std::time::Instant, K)> = self
149            .items
150            .iter()
151            .map(|(key, (_, used))| (*used, key.clone()))
152            .collect();
153        stamps.sort_unstable_by_key(|(used, _)| *used);
154        for (_, key) in stamps.into_iter().take(count) {
155            self.items.remove(&key);
156        }
157    }
158
159}
160
161/// What a row needs to draw an icon: which texture, and where in it.
162#[derive(Clone, Copy, Debug)]
163pub struct Icon {
164    /// Index into the system image list. The cache maps this to a texture region.
165    pub index: i32,
166}
167
168/// An answer from the worker.
169enum Ready {
170    /// An extension (or `"\0dir"`) now known to map to this image-list index.
171    Kind { key: String, index: i32 },
172    /// One file's own icon, addressed by the view that asked and the row it asked about.
173    ///
174    /// No path comes back. The question was "row 412 of view 9", and the answer is stored in
175    /// view 9's own column — so when that view is gone, so is everything it collected. This
176    /// is the whole of the folder-scoped rule for icons: nothing here is keyed by a path,
177    /// because a path is a per-file allocation that outlives the folder it came from.
178    File { view: u64, row: u32, index: i32 },
179    /// A *place* — a sidebar row. Keyed by path, and deliberately: there are a dozen of them
180    /// for the life of the window and they belong to no folder.
181    Place { path: PathBuf, index: i32 },
182    /// One icon's pixels, pulled out of the shell's image list.
183    ///
184    /// **This is the one that used to freeze the window.** Getting a bitmap out of the image
185    /// list looks like a local operation and is not: for an icon the shell resolved from a file
186    /// on a network share, `IImageList::GetIcon` goes back over the network to extract it.
187    /// Measured on a mapped share: **2.54 seconds**, on the UI thread, for one `.ico` file —
188    /// with the other four icons in the same folder costing 2 to 31 ms each. The window sat
189    /// there until it returned.
190    Bitmap { index: i32, image: ColorImage },
191}
192
193/// What the worker is asked to find out.
194enum Job {
195    Kind { key: String, is_dir: bool },
196    File { view: u64, row: u32, path: PathBuf },
197}
198
199/// A bitmap to fetch, on a thread of its own. See [`Ready::Bitmap`].
200struct Wanted(i32);
201
202/// A row's icon has not been asked about yet.
203pub const UNASKED: i32 = -1;
204/// It has been asked about and the answer has not come back.
205pub const ASKED: i32 = -2;
206
207/// The icon service.
208///
209/// Lookups are answered from memory when they can be and requested in the
210/// background when they cannot, so drawing a row never blocks on the shell.
211pub struct Icons {
212    /// Extension → image-list index. `"\0dir"` is the folder entry.
213    ///
214    /// Not bounded, because it cannot grow: it is one entry per *type*, and a machine has a
215    /// few hundred file types on it. Measured at 29 after scrolling all of System32.
216    kinds: HashMap<String, i32>,
217    /// Sidebar places, which are keyed by path because they are not part of any folder:
218    /// the drives, the shell's own folders, the Recycle Bin. A dozen or so, for the session.
219    places_seen: HashMap<PathBuf, i32>,
220    /// File answers waiting to be handed to the view that asked. Drained every frame.
221    answers: Vec<(u64, u32, i32)>,
222    /// The one worker thread, and how many jobs are outstanding on it.
223    ///
224    /// It used to be a thread *per file*: scrolling a folder of 4,910 executables spawned
225    /// 4,910 of them, each with its own stack, and the burst was visible as megabytes
226    /// arriving and leaving. One thread with a queue does the same work with one stack.
227    jobs: Option<Sender<Job>>,
228    queued: Arc<std::sync::atomic::AtomicUsize>,
229    /// Diagnostics for `--trace`: how many bitmaps have been pulled out of the image list and
230    /// how many textures uploaded. Both should stop climbing almost immediately.
231    pub bitmaps: u64,
232    pub uploads: u64,
233    /// Image-list index → its slot in the atlas, least recently drawn first.
234    slots: Recent<i32, u32>,
235    /// The one texture every shell icon is drawn from. See [`Icons::uv`].
236    atlas: Option<TextureHandle>,
237    /// The next never-used slot, until the grid is full.
238    next_slot: u32,
239    /// Requests already in flight, so a folder of ten thousand `.exe` files does not
240    /// queue ten thousand duplicates per frame.
241    pending: Arc<Mutex<std::collections::HashSet<String>>>,
242    tx: Sender<Ready>,
243    rx: Receiver<Ready>,
244    /// Turned off when the shell cannot be reached at all, so nothing retries per row
245    /// for the rest of the session.
246    available: bool,
247    /// The worker that answers *place* lookups, started on first use. One thread for the
248    /// session, and the only lookup here that needs a thread of its own — see
249    /// [`place_worker`].
250    places: Option<Sender<PathBuf>>,
251    /// The worker that pulls bitmaps out of the image list. A thread of its own rather than a
252    /// share of the lookup thread, because one slow extraction must not hold up the type
253    /// lookups every ordinary row is waiting on — and on a network share one of them took
254    /// two and a half seconds.
255    pixels: Option<Sender<Wanted>>,
256    /// Bitmaps already asked for, so a row does not re-ask every frame while one is in flight.
257    /// Bounded by the number of distinct icons on screen, which is what the atlas is sized for.
258    asked_bitmaps: std::collections::HashSet<i32>,
259    /// The views on screen. A per-file question for a view that has gone is dropped by the
260    /// worker before it costs anything — which is what makes changing folder cancel the old
261    /// folder's questions rather than queue behind them.
262    live: Arc<Mutex<std::collections::HashSet<u64>>>,
263}
264
265impl Icons {
266    pub fn new() -> Self {
267        let (tx, rx) = channel();
268        Self {
269            kinds: HashMap::new(),
270            places_seen: HashMap::new(),
271            answers: Vec::new(),
272            jobs: None,
273            queued: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
274            bitmaps: 0,
275            uploads: 0,
276            // As many as the atlas has slots. Shared across folders, because an icon is not
277            // folder-scoped: one bitmap serves every `.dll` on the machine.
278            slots: Recent::new(ATLAS_SLOTS),
279            atlas: None,
280            next_slot: 0,
281            pending: Arc::new(Mutex::new(std::collections::HashSet::new())),
282            tx,
283            rx,
284            available: cfg!(windows),
285            places: None,
286            pixels: None,
287            asked_bitmaps: std::collections::HashSet::new(),
288            live: Arc::new(Mutex::new(std::collections::HashSet::new())),
289        }
290    }
291
292    /// Take delivery of anything resolved since the last frame, and let go of anything
293    /// nothing has asked about for a while.
294    pub fn poll(&mut self, ctx: &egui::Context) {
295        while let Ok(ready) = self.rx.try_recv() {
296            match ready {
297                Ready::Kind { key, index } => {
298                    self.kinds.insert(key, index);
299                }
300                Ready::File { view, row, index } => self.answers.push((view, row, index)),
301                Ready::Place { path, index } => {
302                    self.places_seen.insert(path, index);
303                }
304                Ready::Bitmap { index, image } => {
305                    // The upload is the only part of this that has to be here: it is a memcpy
306                    // into a texture, with none of the shell behind it.
307                    self.asked_bitmaps.remove(&index);
308                    let slot = self.claim_slot(index);
309                    self.blit(ctx, slot, &image);
310                    self.uploads += 1;
311                }
312            }
313        }
314        // The atlas is one texture for the session and its slots are reused, so there is
315        // nothing here to sweep.
316    }
317
318    /// File answers that arrived, for the views that asked. Drained.
319    ///
320    /// The application hands each to the tab whose `view` matches and drops the rest, which
321    /// is what makes a folder you have left cost nothing: its answers are not stored
322    /// anywhere on the way past.
323    pub fn answers(&mut self) -> Vec<(u64, u32, i32)> {
324        std::mem::take(&mut self.answers)
325    }
326
327    /// The icon for a file *type* — one answer shared by every file of that extension.
328    ///
329    /// Returns `None` on the first sighting of a type and asks about it; the row draws its
330    /// painted glyph for a frame or two instead. That is the whole trade: never block a
331    /// frame, and accept that a brand-new type is generic for an instant.
332    pub fn kind(&mut self, ext: &str, is_dir: bool) -> Option<Icon> {
333        if !self.available {
334            return None;
335        }
336        let key = if is_dir {
337            "\0dir"
338        } else if ext.is_empty() {
339            "\0none"
340        } else {
341            // The only allocation on this path, and only for a type never seen before.
342            return self.kind_owned(ext.to_ascii_lowercase(), is_dir);
343        };
344        if let Some(&index) = self.kinds.get(key) {
345            return Some(Icon { index });
346        }
347        self.request_kind(key.to_owned(), is_dir);
348        None
349    }
350
351    fn kind_owned(&mut self, key: String, is_dir: bool) -> Option<Icon> {
352        if let Some(&index) = self.kinds.get(&key) {
353            return Some(Icon { index });
354        }
355        self.request_kind(key, is_dir);
356        None
357    }
358
359    /// Whether a file of this extension carries an icon of its own.
360    ///
361    /// Public so the listing can decide whether a row is worth a per-file question before
362    /// building the path to ask it with.
363    pub fn is_per_file(ext: &str) -> bool {
364        #[cfg(windows)]
365        return has_own_icon(ext);
366        #[cfg(not(windows))]
367        {
368            let _ = ext;
369            false
370        }
371    }
372
373    /// Ask for one file's own icon, to be delivered to `view` at `row`.
374    ///
375    /// The queue is capped rather than the requests deduplicated: a dropped question costs a
376    /// row the generic icon for as long as it takes to come round again, and the alternative
377    /// is a set of every path ever asked about — which is the per-file allocation this whole
378    /// arrangement exists to avoid.
379    pub fn request_file(&mut self, view: u64, row: u32, path: PathBuf) -> bool {
380        if !self.available {
381            return false;
382        }
383        const QUEUE_CAP: usize = 64;
384        if self.queued.load(std::sync::atomic::Ordering::Relaxed) >= QUEUE_CAP {
385            return false;
386        }
387        let jobs = self.worker().clone();
388        self.queued
389            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
390        if jobs.send(Job::File { view, row, path }).is_err() {
391            self.queued
392                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
393            return false;
394        }
395        true
396    }
397
398    /// The icon for a *place* — a sidebar row rather than a listing row.
399    ///
400    /// Different from [`Icons::lookup`] in two ways that matter, and both are the reason
401    /// the sidebar cannot just use it:
402    ///
403    /// - **The answer is per path, never per type.** The whole point is that Downloads
404    ///   does not look like a folder, and it only does not because the shell is allowed
405    ///   to read that folder's own `desktop.ini`. Asking about the *type* "directory"
406    ///   would give the generic folder for every row.
407    /// - **A place need not be a file at all.** The Recycle Bin and This PC are shell
408    ///   namespace items with no path behind them; those go through a PIDL.
409    ///
410    /// There are a dozen or so of these for a session, all resolved once, so the
411    /// per-path cost the listing goes to such lengths to avoid is not a cost here.
412    pub fn place(&mut self, path: &Path) -> Option<Icon> {
413        if !self.available {
414            return None;
415        }
416        if let Some(&index) = self.places_seen.get(path) {
417            return Some(Icon { index });
418        }
419        self.request_place(path);
420        None
421    }
422
423    /// Where an icon lives in the atlas: the one texture, and the patch of it to draw.
424    ///
425    /// **One texture for every shell icon in the window**, which is not an optimisation but a
426    /// correctness fix. egui begins a new draw call whenever the texture changes between
427    /// primitives, and `egui_glow` leaks memory per draw call — megabytes a second at 60fps,
428    /// reproduced with none of this program in the frame by `examples/spin.rs`. A texture per
429    /// icon meant a frame's primitive stream broke at every distinct icon on screen; a single
430    /// atlas means it breaks once.
431    ///
432    /// A slot is 16×16, the size of the shell's small image list, in a 32×8 grid — 256 of
433    /// them, which is more distinct icons than a window has ever shown at once. Slots are
434    /// reused least-recently-drawn first, so a long session overwrites rather than grows, and
435    /// the texture is one 512×128 upload for the life of the process.
436    /// Returns `None` until the bitmap has arrived, the same as [`Icons::kind`] does for a type
437    /// never seen before: the caller draws its painted glyph for a frame or two. That is not a
438    /// nicety — pulling the bitmap out of the image list is a shell call that reaches the
439    /// network, and doing it here is what froze the window for two and a half seconds on a
440    /// mapped share. See [`Ready::Bitmap`].
441    pub fn uv(&mut self, ctx: &egui::Context, icon: Icon) -> Option<(egui::TextureId, egui::Rect)> {
442        let _ = ctx;
443        let slot = match self.slots.get(&icon.index) {
444            Some(&slot) => slot,
445            None => {
446                self.request_bitmap(icon.index);
447                return None;
448            }
449        };
450        let atlas = self.atlas.as_ref()?;
451        Some((atlas.id(), Self::slot_uv(slot)))
452    }
453
454    /// Ask for one icon's pixels, once.
455    ///
456    /// Once per index for the life of the process, including when the answer never comes: an
457    /// index the shell cannot produce a bitmap for sends nothing back and stays in
458    /// `asked_bitmaps`, so the row keeps its painted glyph rather than asking again on every
459    /// frame forever. On a share that is the difference between one failed extraction and one
460    /// per frame.
461    fn request_bitmap(&mut self, index: i32) {
462        if !self.available || !self.asked_bitmaps.insert(index) {
463            return;
464        }
465        self.bitmaps += 1;
466        if self.pixels().send(Wanted(index)).is_err() {
467            self.asked_bitmaps.remove(&index);
468        }
469    }
470
471    /// Which views are on screen, so the worker can drop questions from the ones that are not.
472    ///
473    /// Called every frame. Changing folder leaves a queue of per-file questions about files
474    /// nothing is showing any more, and each of them is a blocking shell call — on a network
475    /// share, a slow one. Skipping them is the difference between a new folder's icons arriving
476    /// now and arriving after the old folder's have all been answered.
477    pub fn only(&mut self, views: &[u64]) {
478        if let Ok(mut live) = self.live.lock() {
479            if live.len() == views.len() && views.iter().all(|view| live.contains(view)) {
480                return;
481            }
482            live.clear();
483            live.extend(views.iter().copied());
484        }
485    }
486
487    /// The next slot for an icon: an unused one, or the one drawn longest ago.
488    fn claim_slot(&mut self, index: i32) -> u32 {
489        let slot = if (self.next_slot as usize) < ATLAS_SLOTS {
490            let slot = self.next_slot;
491            self.next_slot += 1;
492            slot
493        } else {
494            // Full: take the slot of the icon nobody has drawn for the longest, and let its
495            // owner ask again if it reappears.
496            let victim = self
497                .slots
498                .items
499                .iter()
500                .min_by_key(|(_, (_, used))| *used)
501                .map(|(key, (slot, _))| (*key, *slot));
502            match victim {
503                Some((key, slot)) => {
504                    self.slots.items.remove(&key);
505                    slot
506                }
507                // Cannot happen: the grid is full, so something is in it.
508                None => 0,
509            }
510        };
511        self.slots.insert(index, slot);
512        slot
513    }
514
515    /// Copy one icon into its slot, creating the atlas on first use.
516    fn blit(&mut self, ctx: &egui::Context, slot: u32, image: &ColorImage) {
517        let atlas = self.atlas.get_or_insert_with(|| {
518            ctx.load_texture(
519                "shell-icons",
520                ColorImage::filled(
521                    [ATLAS_COLUMNS * CELL, ATLAS_ROWS * CELL],
522                    egui::Color32::TRANSPARENT,
523                ),
524                // Linear, because a 16px icon drawn at 15.8 device pixels on a scaled
525                // display is the normal case and nearest would shimmer.
526                TextureOptions::LINEAR,
527            )
528        });
529        // Padded into the cell rather than scaled: the shell's small list is 16×16, and a
530        // bitmap of another size is rare enough that clipping it beats resampling every one.
531        let mut cell = ColorImage::filled([CELL, CELL], egui::Color32::TRANSPARENT);
532        let [w, h] = image.size;
533        for y in 0..h.min(CELL) {
534            for x in 0..w.min(CELL) {
535                cell[(x, y)] = image[(x, y)];
536            }
537        }
538        let (column, row) = ((slot as usize) % ATLAS_COLUMNS, (slot as usize) / ATLAS_COLUMNS);
539        atlas.set_partial([column * CELL, row * CELL], cell, TextureOptions::LINEAR);
540    }
541
542    /// The patch of the atlas a slot occupies, in 0..1 texture coordinates.
543    fn slot_uv(slot: u32) -> egui::Rect {
544        let (column, row) = ((slot as usize) % ATLAS_COLUMNS, (slot as usize) / ATLAS_COLUMNS);
545        let (w, h) = (
546            1.0 / ATLAS_COLUMNS as f32,
547            1.0 / ATLAS_ROWS as f32,
548        );
549        egui::Rect::from_min_size(
550            egui::pos2(column as f32 * w, row as f32 * h),
551            egui::vec2(w, h),
552        )
553    }
554
555    /// How many distinct icons are held. Read by the tests, which is how the
556    /// cache-once-per-type promise is checked.
557    #[cfg_attr(not(test), allow(dead_code))]
558    pub fn len(&self) -> usize {
559        self.kinds.len() + self.places_seen.len()
560    }
561
562    /// What the cache is holding: known types, known paths, and uploaded textures.
563    ///
564    /// For `--trace`. A texture is the expensive one — it is a GL object with the driver's
565    /// own per-object overhead behind it, which is invisible to a Rust allocator counter and
566    /// is exactly the kind of thing that turns "memory grows as I browse" into a number
567    /// nobody can find.
568    pub fn held(&self) -> (usize, usize, usize) {
569        (self.kinds.len(), self.places_seen.len(), self.slots.len())
570    }
571
572    fn request_kind(&mut self, key: String, is_dir: bool) {
573        // One type is asked about once, and there are a few dozen of them in a session, so
574        // this set is the one place a claim is still worth keeping.
575        if !self.claim(&key) {
576            return;
577        }
578        let jobs = self.worker();
579        let _ = jobs.send(Job::Kind { key, is_dir });
580    }
581
582    fn request_place(&mut self, path: &Path) {
583        if !self.claim(&place_key(path)) {
584            return;
585        }
586        let worker = self
587            .places
588            .get_or_insert_with(|| place_worker(self.tx.clone(), self.pending.clone()));
589        let _ = worker.send(path.to_path_buf());
590    }
591
592    /// The worker that answers type and file lookups, started on first use.
593    ///
594    /// One thread for the session. It used to be one thread per question — 4,910 of them for
595    /// a folder of that many executables, each with a stack of its own — which is both the
596    /// spike this program was accused of and pure waste: the work is a blocking syscall, and
597    /// a queue in front of one thread serialises it just as well.
598    fn worker(&mut self) -> &Sender<Job> {
599        let tx = self.tx.clone();
600        let pending = self.pending.clone();
601        let queued = self.queued.clone();
602        let live = self.live.clone();
603        self.jobs.get_or_insert_with(|| {
604            let (send, receive) = channel::<Job>();
605            let _ = std::thread::Builder::new()
606                .name("shell-icons".to_owned())
607                .spawn(move || {
608                    // Any lookup can touch an empty removable drive, and the syscall would
609                    // otherwise raise "Please insert a disk into drive E:" from inside it.
610                    crate::fs::scan::silence_device_dialogs();
611                    while let Ok(job) = receive.recv() {
612                        match job {
613                            Job::Kind { key, is_dir } => {
614                                // A representative name of that type. The shell is told not
615                                // to look at the file, so nothing of this name need exist.
616                                let sample = if is_dir {
617                                    PathBuf::from("C:\\folder")
618                                } else if key.starts_with('\0') {
619                                    PathBuf::from("C:\\file")
620                                } else {
621                                    PathBuf::from(format!("C:\\file.{key}"))
622                                };
623                                if let Some(index) = index_of(&sample, is_dir, true) {
624                                    let _ = tx.send(Ready::Kind {
625                                        key: key.clone(),
626                                        index,
627                                    });
628                                }
629                                if let Ok(mut pending) = pending.lock() {
630                                    pending.remove(&key);
631                                }
632                            }
633                            Job::File { view, row, path } => {
634                                // Nothing is showing that view any more: the folder was left
635                                // while this was in the queue. Dropped here rather than
636                                // answered and discarded, because the answering is the
637                                // expensive part — it opens the file, and on a network share
638                                // that is the whole of the cost.
639                                let wanted = live
640                                    .lock()
641                                    .map(|live| live.contains(&view))
642                                    .unwrap_or(true);
643                                if wanted {
644                                    // Not `use_attributes`: the whole point of this branch is
645                                    // that the icon is inside the file, so it has to be opened.
646                                    if let Some(index) = index_of(&path, false, false) {
647                                        let _ = tx.send(Ready::File { view, row, index });
648                                    }
649                                }
650                                queued.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
651                            }
652                        }
653                    }
654                });
655            send
656        })
657    }
658
659    /// The thread that pulls bitmaps out of the shell's image list, started on first use.
660    ///
661    /// Its own thread, and its own apartment. `SHGetImageList` hands back a COM interface, so
662    /// this needs one — and it is entered once for the life of the thread rather than around
663    /// each call, for the reason set out on [`place_worker`]: the last `CoUninitialize` in a
664    /// process frees shell state other threads are still using.
665    fn pixels(&mut self) -> &Sender<Wanted> {
666        let tx = self.tx.clone();
667        self.pixels.get_or_insert_with(|| {
668            let (send, receive) = channel::<Wanted>();
669            let _ = std::thread::Builder::new()
670                .name("shell-bitmaps".to_owned())
671                .spawn(move || {
672                    crate::shell::init();
673                    crate::fs::scan::silence_device_dialogs();
674                    while let Ok(Wanted(index)) = receive.recv() {
675                        if let Some(image) = bitmap(index) {
676                            let _ = tx.send(Ready::Bitmap { index, image });
677                        }
678                    }
679                });
680            send
681        })
682    }
683
684    /// Reserve a key, returning whether this caller got it.
685    fn claim(&self, key: &str) -> bool {
686        self.pending
687            .lock()
688            .map(|mut pending| pending.insert(key.to_owned()))
689            .unwrap_or(false)
690    }
691}
692
693impl Default for Icons {
694    fn default() -> Self {
695        Self::new()
696    }
697}
698
699/// The key a place lookup is claimed under.
700///
701/// Apart from the key space [`Icons::request_path`] uses, so a folder that is both a place
702/// and a listed file cannot answer one question with the other's icon.
703fn place_key(path: &Path) -> String {
704    format!(" place {}", path.to_string_lossy())
705}
706
707/// The thread that answers place lookups, with an apartment of its own that lasts.
708///
709/// **Why a thread at all.** `SHParseDisplayName` — the only way to reach something that is
710/// not a file, which This PC and the Recycle Bin both are — goes through the desktop's
711/// `IShellFolder`, and on a thread with no apartment it fails with `CO_E_NOTINITIALIZED`.
712/// That failure is silent and indistinguishable from "the shell has no icon for this", which
713/// is how those two rows came to draw their painted glyph while every real folder beside
714/// them had the shell's icon. Every other lookup here calls `SHGetFileInfoW`, which is
715/// documented as needing no apartment, and gets none.
716///
717/// **Why one thread, and why it never uninitialises.** The obvious fix — COM around each
718/// lookup on its own short-lived thread — is worse than the bug: the *last*
719/// `CoUninitialize` in a process frees shell state other threads are still using, and it
720/// showed up at once as `SHGetFileInfoW` on an unrelated thread returning nothing. So the
721/// apartment is entered once, on a thread that outlives the requests, exactly as
722/// [`crate::shell::Modal`] does.
723fn place_worker(
724    tx: Sender<Ready>,
725    pending: Arc<Mutex<std::collections::HashSet<String>>>,
726) -> Sender<PathBuf> {
727    let (send, requests) = channel::<PathBuf>();
728    let spawned = std::thread::Builder::new()
729        .name("shell-place".to_owned())
730        .spawn(move || {
731            crate::shell::init();
732            // Ends when the `Icons` holding the sender goes away.
733            while let Ok(path) = requests.recv() {
734                if let Some(index) = index_of_place(&path) {
735                    let _ = tx.send(Ready::Place {
736                        path: path.clone(),
737                        index,
738                    });
739                }
740                if let Ok(mut pending) = pending.lock() {
741                    pending.remove(&place_key(&path));
742                }
743            }
744        });
745    let _ = spawned;
746    send
747}
748
749// ---------------------------------------------------------------------------
750// Windows
751// ---------------------------------------------------------------------------
752
753/// Ask the shell for an image-list index.
754///
755/// `use_attributes` is the difference between a microsecond and a millisecond: with
756/// it the shell answers from the name and the attribute word alone.
757#[cfg(windows)]
758fn index_of(path: &Path, is_dir: bool, use_attributes: bool) -> Option<i32> {
759    use windows::core::PCWSTR;
760    use windows::Win32::Storage::FileSystem::{
761        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
762    };
763    use windows::Win32::UI::Shell::{
764        SHGetFileInfoW, SHFILEINFOW, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
765    };
766
767    let wide: Vec<u16> = wide(path);
768    let attributes = if is_dir {
769        FILE_ATTRIBUTE_DIRECTORY
770    } else {
771        FILE_ATTRIBUTE_NORMAL
772    };
773    let mut flags = SHGFI_SYSICONINDEX;
774    if use_attributes {
775        flags |= SHGFI_USEFILEATTRIBUTES;
776    }
777
778    let mut info = SHFILEINFOW::default();
779    // SAFETY: `wide` is null-terminated and outlives the call; `info` is sized by
780    // `size_of`. A zero return means the shell declined, which is not an error here.
781    let ok = unsafe {
782        SHGetFileInfoW(
783            PCWSTR(wide.as_ptr()),
784            attributes,
785            Some(&mut info),
786            std::mem::size_of::<SHFILEINFOW>() as u32,
787            flags,
788        )
789    };
790    (ok != 0).then_some(info.iIcon)
791}
792
793#[cfg(not(windows))]
794fn index_of(_path: &Path, _is_dir: bool, _use_attributes: bool) -> Option<i32> {
795    None
796}
797
798/// The image-list index for a place: a real folder, or a namespace item.
799///
800/// An empty path is This PC, and anything starting with `shell:` or `::{` is a moniker
801/// for something that is not a file — both of which have to be parsed into a PIDL first,
802/// because `SHGetFileInfoW` on the text would go looking for a file of that name.
803/// A real folder goes by name with *no* `SHGFI_USEFILEATTRIBUTES`, which is what lets the
804/// shell read its `desktop.ini` and hand back the Downloads icon rather than a folder.
805#[cfg(windows)]
806fn index_of_place(path: &Path) -> Option<i32> {
807    let text = path.to_string_lossy();
808    let moniker = if path.as_os_str().is_empty() {
809        Some(std::borrow::Cow::Borrowed("shell:MyComputerFolder"))
810    } else if text.starts_with("shell:") || text.starts_with("::{") {
811        Some(text.clone())
812    } else {
813        None
814    };
815
816    match moniker {
817        Some(moniker) => index_of_moniker(&moniker),
818        None => index_of(path, true, false),
819    }
820}
821
822/// The icon of something named by a shell moniker rather than by a path.
823#[cfg(windows)]
824fn index_of_moniker(moniker: &str) -> Option<i32> {
825    use windows::core::PCWSTR;
826    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
827    use windows::Win32::UI::Shell::{
828        ILFree, SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW, SHGFI_PIDL, SHGFI_SYSICONINDEX,
829    };
830
831    let wide: Vec<u16> = moniker.encode_utf16().chain(std::iter::once(0)).collect();
832    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
833    // SAFETY: `wide` is null-terminated and outlives the call. `pidl` is written only on
834    // success and freed on both paths out below.
835    unsafe {
836        SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).ok()?;
837    }
838    if pidl.is_null() {
839        return None;
840    }
841
842    let mut info = SHFILEINFOW::default();
843    // SAFETY: with `SHGFI_PIDL` the first argument is a PIDL cast to `PCWSTR`, which is
844    // the shape this API has always had. `info` is sized by `size_of`.
845    let ok = unsafe {
846        SHGetFileInfoW(
847            PCWSTR(pidl as *const u16),
848            Default::default(),
849            Some(&mut info),
850            std::mem::size_of::<SHFILEINFOW>() as u32,
851            SHGFI_SYSICONINDEX | SHGFI_PIDL,
852        )
853    };
854    // SAFETY: allocated by `SHParseDisplayName`, freed exactly once.
855    unsafe { ILFree(Some(pidl)) };
856    (ok != 0).then_some(info.iIcon)
857}
858
859#[cfg(not(windows))]
860fn index_of_place(_path: &Path) -> Option<i32> {
861    None
862}
863
864/// Pull one icon out of the system small image list as RGBA.
865#[cfg(windows)]
866fn bitmap(index: i32) -> Option<ColorImage> {
867    use windows::Win32::Graphics::Gdi::DeleteObject;
868    use windows::Win32::UI::Shell::{SHGetImageList, SHIL_SMALL};
869    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
870    use windows::Win32::UI::Controls::IImageList;
871
872    // SAFETY: every raw call below is checked, and every handle it hands back is
873    // released on all paths out.
874    unsafe {
875        // The shell's own list, so these are the exact bitmaps Explorer draws.
876        let list: IImageList = SHGetImageList(SHIL_SMALL as i32).ok()?;
877        let icon: HICON = list.GetIcon(index, 0u32).ok()?;
878
879        let mut info = ICONINFO::default();
880        if GetIconInfo(icon, &mut info).is_err() {
881            let _ = DestroyIcon(icon);
882            return None;
883        }
884        let colour = info.hbmColor;
885        let mask = info.hbmMask;
886
887        let result = read_bgra(colour, mask);
888
889        if !colour.is_invalid() {
890            let _ = DeleteObject(colour.into());
891        }
892        if !mask.is_invalid() {
893            let _ = DeleteObject(mask.into());
894        }
895        let _ = DestroyIcon(icon);
896        result
897    }
898}
899
900/// One GDI bitmap as RGBA, for the icons the shell puts on its menu items.
901///
902/// No mask: a menu bitmap is 32-bit with real alpha, unlike the paired colour-and-mask
903/// pair an `HICON` is built from.
904#[cfg(windows)]
905pub fn bitmap_of(bitmap: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<ColorImage> {
906    // SAFETY: the handle belongs to the caller and is only read; nothing is freed here.
907    unsafe { read_bgra(bitmap, windows::Win32::Graphics::Gdi::HBITMAP::default()) }
908}
909
910/// Read a 32-bit icon bitmap into an egui image, using the mask for anything that
911/// has no alpha channel of its own.
912///
913/// Monochrome and 24-bit icons still exist in the wild — a lot of shell extensions
914/// ship them — and without the mask they come out as opaque black rectangles.
915#[cfg(windows)]
916unsafe fn read_bgra(
917    colour: windows::Win32::Graphics::Gdi::HBITMAP,
918    mask: windows::Win32::Graphics::Gdi::HBITMAP,
919) -> Option<ColorImage> {
920    use windows::Win32::Graphics::Gdi::{
921        CreateCompatibleDC, DeleteDC, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
922        BI_RGB, DIB_RGB_COLORS,
923    };
924
925    if colour.is_invalid() {
926        return None;
927    }
928
929    let mut header = BITMAP::default();
930    let read = GetObjectW(
931        colour.into(),
932        std::mem::size_of::<BITMAP>() as i32,
933        Some((&mut header) as *mut BITMAP as *mut std::ffi::c_void),
934    );
935    if read == 0 || header.bmWidth <= 0 || header.bmHeight <= 0 {
936        return None;
937    }
938    let (w, h) = (header.bmWidth as usize, header.bmHeight as usize);
939    if w > 512 || h > 512 {
940        return None;
941    }
942
943    let dc = CreateCompatibleDC(None);
944    if dc.is_invalid() {
945        return None;
946    }
947
948    let mut info = BITMAPINFO {
949        bmiHeader: BITMAPINFOHEADER {
950            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
951            biWidth: w as i32,
952            // Negative, so the rows come back top-down and no flip is needed.
953            biHeight: -(h as i32),
954            biPlanes: 1,
955            biBitCount: 32,
956            biCompression: BI_RGB.0,
957            ..Default::default()
958        },
959        ..Default::default()
960    };
961
962    let mut pixels = vec![0u8; w * h * 4];
963    let lines = GetDIBits(
964        dc,
965        colour,
966        0,
967        h as u32,
968        Some(pixels.as_mut_ptr().cast()),
969        &mut info,
970        DIB_RGB_COLORS,
971    );
972
973    // Does this icon carry real transparency, or does it need the mask?
974    let opaque = pixels.chunks_exact(4).all(|p| p[3] == 0);
975    if lines != 0 && opaque && !mask.is_invalid() {
976        let mut mask_bits = vec![0u8; w * h * 4];
977        let read = GetDIBits(
978            dc,
979            mask,
980            0,
981            h as u32,
982            Some(mask_bits.as_mut_ptr().cast()),
983            &mut info,
984            DIB_RGB_COLORS,
985        );
986        if read != 0 {
987            // In an icon mask, white means "transparent here".
988            for (pixel, m) in pixels.chunks_exact_mut(4).zip(mask_bits.chunks_exact(4)) {
989                pixel[3] = if m[0] > 127 { 0 } else { 255 };
990            }
991        }
992    } else if lines != 0 && opaque {
993        for pixel in pixels.chunks_exact_mut(4) {
994            pixel[3] = 255;
995        }
996    }
997
998    let _ = DeleteDC(dc);
999    if lines == 0 {
1000        return None;
1001    }
1002
1003    // GDI hands back BGRA; egui wants RGBA.
1004    for pixel in pixels.chunks_exact_mut(4) {
1005        pixel.swap(0, 2);
1006    }
1007    Some(ColorImage::from_rgba_unmultiplied([w, h], &pixels))
1008}
1009
1010#[cfg(not(windows))]
1011fn bitmap(_index: i32) -> Option<ColorImage> {
1012    None
1013}
1014
1015/// A path as a null-terminated wide string.
1016#[cfg(windows)]
1017fn wide(path: &Path) -> Vec<u16> {
1018    use std::os::windows::ffi::OsStrExt as _;
1019    path.as_os_str()
1020        .encode_wide()
1021        .chain(std::iter::once(0))
1022        .collect()
1023}
1024
1025#[cfg(all(test, windows))]
1026mod tests {
1027    use super::*;
1028    use std::time::{Duration, Instant};
1029
1030    /// A file that carries an icon of its own, on any Windows.
1031    fn notepad() -> PathBuf {
1032        PathBuf::from(r"C:\Windows\System32
1033otepad.exe")
1034    }
1035
1036    #[test]
1037    fn a_type_icon_costs_no_io() {
1038        crate::shell::init();
1039        // `SHGFI_USEFILEATTRIBUTES` means the shell answers from the name alone, so a
1040        // path that does not exist still resolves — which is the whole point, and the
1041        // reason a folder of 60,000 files does not pay per file.
1042        let index = index_of(Path::new(r"C:\nothing-here.txt"), false, true);
1043        assert!(
1044            index.is_some(),
1045            "a type icon has to resolve without the file existing"
1046        );
1047
1048        let started = Instant::now();
1049        for _ in 0..200 {
1050            let _ = index_of(Path::new(r"C:\nothing-here.txt"), false, true);
1051        }
1052        let each = started.elapsed() / 200;
1053        assert!(
1054            each < Duration::from_micros(500),
1055            "{each:?} per type lookup -- this is meant to be the cheap path"
1056        );
1057    }
1058
1059    #[test]
1060    fn a_file_with_its_own_icon_gets_its_own_index() {
1061        crate::shell::init();
1062        let notepad = notepad();
1063        if !notepad.exists() {
1064            return;
1065        }
1066        let own = index_of(&notepad, false, false).expect("notepad has an icon");
1067        let generic = index_of(Path::new(r"C:\anything.exe"), false, true)
1068            .expect("the generic application icon");
1069        assert_ne!(
1070            own, generic,
1071            "an executable carries its own icon, which is why it is worth a per-file \
1072             lookup at all"
1073        );
1074    }
1075
1076    #[test]
1077    fn an_icon_index_yields_a_visible_bitmap() {
1078        crate::shell::init();
1079        let index = index_of(Path::new(r"C:\folder"), true, true).expect("a folder icon");
1080        let image = bitmap(index).expect("the system image list has a bitmap for it");
1081        assert_eq!(image.size, [SMALL, SMALL], "the small list is 16 square");
1082
1083        let lit = image
1084            .pixels
1085            .iter()
1086            .filter(|p| p.a() > 0 && (p.r(), p.g(), p.b()) != (0, 0, 0))
1087            .count();
1088        assert!(
1089            lit > 20,
1090            "only {lit} pixels are both opaque and coloured -- the mask or the channel \
1091             order is wrong, and the icon would draw as a black square"
1092        );
1093        let clear = image.pixels.iter().filter(|p| p.a() == 0).count();
1094        assert!(
1095            clear > 20,
1096            "nothing is transparent, so the icon would draw as a filled block"
1097        );
1098    }
1099
1100    /// Time every shell call an icon costs, against a folder given on the command line.
1101    ///
1102    /// `YAFE_PROBE=H:\some\folder cargo test probe_icon_costs -- --ignored --nocapture`
1103    ///
1104    /// Not a test of anything: a measurement, kept because the answer is entirely different on
1105    /// a network share and guessing which of these calls is the slow one is how a whole
1106    /// afternoon gets spent on the wrong one.
1107    #[test]
1108    #[ignore]
1109    #[cfg(windows)]
1110    fn probe_icon_costs() {
1111        let Some(folder) = std::env::var_os("YAFE_PROBE") else {
1112            eprintln!("set YAFE_PROBE to a folder");
1113            return;
1114        };
1115        crate::shell::init();
1116        let folder = std::path::PathBuf::from(folder);
1117        let mut entries: Vec<(std::path::PathBuf, bool)> = Vec::new();
1118        for entry in std::fs::read_dir(&folder).expect("readable").flatten() {
1119            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
1120            entries.push((entry.path(), is_dir));
1121        }
1122        eprintln!("{} entries in {}", entries.len(), folder.display());
1123
1124        let timed = |label: &str, f: &mut dyn FnMut() -> Option<i32>| {
1125            let at = Instant::now();
1126            let got = f();
1127            (label.to_owned(), at.elapsed(), got)
1128        };
1129
1130        // 1. The type lookup, which is what every ordinary row goes through.
1131        let mut rows: Vec<(String, Duration, Option<i32>)> = Vec::new();
1132        let mut seen = std::collections::HashSet::new();
1133        for (path, is_dir) in &entries {
1134            let ext = path
1135                .extension()
1136                .map(|e| e.to_string_lossy().to_ascii_lowercase())
1137                .unwrap_or_default();
1138            if !seen.insert((ext.clone(), *is_dir)) {
1139                continue;
1140            }
1141            let p = path.clone();
1142            rows.push(timed(
1143                &format!("kind .{ext}{}", if *is_dir { " (dir)" } else { "" }),
1144                &mut || index_of(&p, *is_dir, true),
1145            ));
1146        }
1147
1148        // 2. The per-file lookup, for the extensions that carry their own icon.
1149        for (path, is_dir) in entries.iter().filter(|(_, d)| !d).take(12) {
1150            let ext = path
1151                .extension()
1152                .map(|e| e.to_string_lossy().to_ascii_lowercase())
1153                .unwrap_or_default();
1154            if !has_own_icon(&ext) {
1155                continue;
1156            }
1157            let p = path.clone();
1158            rows.push(timed(
1159                &format!("file {}", p.file_name().unwrap_or_default().to_string_lossy()),
1160                &mut || index_of(&p, *is_dir, false),
1161            ));
1162        }
1163
1164        // 3. The place lookup, which is what the sidebar and every tab does.
1165        for path in std::iter::once(folder.clone())
1166            .chain(entries.iter().filter(|(_, d)| *d).map(|(p, _)| p.clone()).take(6))
1167        {
1168            let p = path.clone();
1169            rows.push(timed(
1170                &format!("place {}", p.display()),
1171                &mut || index_of_place(&p),
1172            ));
1173        }
1174
1175        for (label, took, got) in &rows {
1176            eprintln!("{:>9.2?}  {label} -> {got:?}", took);
1177        }
1178
1179        // 4. Pulling the bitmap out of the image list, which happens on the UI thread.
1180        let mut indices: Vec<i32> = rows.iter().filter_map(|(_, _, got)| *got).collect();
1181        indices.sort_unstable();
1182        indices.dedup();
1183        eprintln!("-- bitmaps, {} distinct indices --", indices.len());
1184        let mut worst = Duration::ZERO;
1185        let mut total = Duration::ZERO;
1186        for index in &indices {
1187            let at = Instant::now();
1188            let got = bitmap(*index);
1189            let took = at.elapsed();
1190            total += took;
1191            worst = worst.max(took);
1192            eprintln!("{:>9.2?}  bitmap {index} -> {}", took, got.is_some());
1193        }
1194        eprintln!("bitmaps: {total:.2?} total, {worst:.2?} worst");
1195    }
1196
1197    /// A bitmap is fetched off the UI thread, and drawn once it arrives.
1198    ///
1199    /// The shape of this test is the point. `uv` answers `None` first and something later,
1200    /// because in between a worker thread did the only part of this that can block — pulling
1201    /// the icon out of the shell's image list, which reaches the network for an icon that came
1202    /// from a file on a share and was measured at **2.54 seconds** for one `.ico`. It used to
1203    /// happen here, in the frame, and that is what froze the window.
1204    #[test]
1205    fn a_bitmap_is_fetched_off_the_ui_thread() {
1206        crate::shell::init();
1207        let ctx = egui::Context::default();
1208        let mut icons = Icons::new();
1209
1210        // A real image-list index to ask about: the folder icon.
1211        let deadline = Instant::now() + Duration::from_secs(10);
1212        let icon = loop {
1213            icons.poll(&ctx);
1214            if let Some(icon) = icons.kind("", true) {
1215                break icon;
1216            }
1217            assert!(Instant::now() < deadline, "the folder icon never arrived");
1218            std::thread::sleep(Duration::from_millis(10));
1219        };
1220
1221        // Asked here, not fetched here.
1222        assert!(
1223            icons.uv(&ctx, icon).is_none(),
1224            "the first ask must not go to the shell"
1225        );
1226        assert_eq!(icons.bitmaps, 1, "and it must not ask twice");
1227        assert!(icons.uv(&ctx, icon).is_none());
1228        assert_eq!(icons.bitmaps, 1);
1229
1230        let deadline = Instant::now() + Duration::from_secs(10);
1231        loop {
1232            icons.poll(&ctx);
1233            if icons.uv(&ctx, icon).is_some() {
1234                break;
1235            }
1236            assert!(Instant::now() < deadline, "the bitmap never arrived");
1237            std::thread::sleep(Duration::from_millis(10));
1238        }
1239    }
1240
1241    /// A question about a folder nobody is looking at any more is dropped, not answered.
1242    #[test]
1243    fn leaving_a_folder_cancels_its_questions() {
1244        crate::shell::init();
1245        let ctx = egui::Context::default();
1246        let mut icons = Icons::new();
1247        let exe = std::env::current_exe().expect("this test binary is an executable");
1248
1249        // View 1 asks, and then goes away before the worker gets to it.
1250        icons.only(&[1]);
1251        assert!(icons.request_file(1, 0, exe.clone()));
1252        icons.only(&[2]);
1253
1254        // Nothing addressed to view 1 comes back. Waited on rather than asserted at once,
1255        // because the point is that the worker *reached* the job and skipped it.
1256        let deadline = Instant::now() + Duration::from_secs(5);
1257        while Instant::now() < deadline {
1258            icons.poll(&ctx);
1259            assert!(
1260                icons.answers().is_empty(),
1261                "a question from a view that has gone should not be answered"
1262            );
1263            std::thread::sleep(Duration::from_millis(10));
1264        }
1265
1266        // And a live view still gets its answer, so the skip is about the view and not about
1267        // the worker having stopped.
1268        assert!(icons.request_file(2, 0, exe));
1269        let deadline = Instant::now() + Duration::from_secs(10);
1270        let mut got = Vec::new();
1271        while Instant::now() < deadline && got.is_empty() {
1272            icons.poll(&ctx);
1273            got = icons.answers();
1274            std::thread::sleep(Duration::from_millis(10));
1275        }
1276        assert!(
1277            got.iter().any(|(view, _, _)| *view == 2),
1278            "the live view's question should be answered: {got:?}"
1279        );
1280    }
1281
1282    #[test]
1283    fn the_cache_asks_once_per_type() {
1284        crate::shell::init();
1285        let mut icons = Icons::new();
1286
1287        // First sighting: nothing yet, and a request queued.
1288        assert!(icons.kind("", true).is_none());
1289
1290        // Give the worker a moment, then it is cached and every later ask is free.
1291        let deadline = Instant::now() + Duration::from_secs(5);
1292        while Instant::now() < deadline {
1293            icons.poll(&egui::Context::default());
1294            if icons.kind("", true).is_some() {
1295                break;
1296            }
1297            std::thread::sleep(Duration::from_millis(10));
1298        }
1299        assert!(
1300            icons.kind("", true).is_some(),
1301            "the folder icon never arrived"
1302        );
1303        let held = icons.len();
1304        for _ in 0..100 {
1305            let _ = icons.kind("", true);
1306        }
1307        assert_eq!(icons.len(), held, "a cached type is not looked up again");
1308    }
1309}
