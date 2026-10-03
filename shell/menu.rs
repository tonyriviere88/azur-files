1//! Explorer's context menu content, read out and handed over to be drawn.
2//!
3//! The menu the user sees is this program's own — [`crate::ui::menu`] draws it with the
4//! design system, at the pointer, keyboard-navigable, themed with everything else.
5//! What is *in* it comes from the shell, so 7-Zip, TortoiseGit, Open With, Send To,
6//! Properties and whatever else is installed are all there and all work.
7//!
8//! # How the content is obtained without showing a Win32 menu
9//!
10//! `IContextMenu::QueryContextMenu` does not display anything: it *populates an
11//! `HMENU`*. So this creates one, lets the shell and every extension fill it, and then
12//! reads it back out with `GetMenuItemInfoW` — labels, separators, disabled and checked
13//! states, the default (bold) item, the accelerator text, the item bitmaps and the
14//! submenus. `TrackPopupMenuEx` is never called, and no native menu ever appears.
15//!
16//! Two things make that read faithful rather than approximate:
17//!
18//! **Submenus have to be asked for.** An extension fills its submenu lazily, when the
19//! menu is about to pop up, via `IContextMenu2::HandleMenuMsg(WM_INITMENUPOPUP)`. A
20//! menu that is never shown never gets that message — so [`Live::fill`] sends it itself
21//! before reading a submenu. Without it, Send To and New come back empty, which is the
22//! usual way a re-drawn shell menu ends up looking finished and being broken.
23//!
24//! **Commands are invoked by verb where there is one.** `GetCommandString` gives the
25//! canonical name — `open`, `copy`, `delete`, `properties` — which is stable and can be
26//! used from any thread against a freshly obtained `IContextMenu`. That matters because
27//! invoking a command can put up a dialog, which has to happen off the UI thread; see
28//! [`crate::shell::Modal`]. Where an extension offers no canonical verb, the numeric id
29//! is used instead, against a menu re-queried with identical flags and items so the
30//! numbering is identical.
31//!
32//! # Why none of it happens in a frame
33//!
34//! `QueryContextMenu` shows nothing, so for a long time it was called during the frame
35//! that opened the menu. It is also **slow**, and not only the first time. Measured on this
36//! machine with a fairly ordinary set of extensions installed, by
37//! `what_the_shell_menu_takes_to_build --nocapture`:
38//!
39//! | | `QueryContextMenu` | submenu prefill | reading the `HMENU` | bitmaps | verbs |
40//! | --- | --- | --- | --- | --- | --- |
41//! | a file, first of a session | 690 ms | 116 ms | 0.2 ms | 0.2 ms | 0.0 ms |
42//! | a file, after that | 130–570 ms | 41–51 ms | 0.2 ms | 0.1 ms | 0.0 ms |
43//! | a folder, or empty space | 130–200 ms | 3 ms | 0.2 ms | 0.3 ms | 0.0 ms |
44//!
45//! So a right click froze the window for a sixth of a second at best and most of a second
46//! at worst: the shell asks every installed extension to contribute, and each one goes to
47//! the registry and the disk to decide what to offer. The spread is wide and it is not
48//! ours — the same call on the same file measured 567 ms in one session and 140 ms in the
49//! next — so the figures above are worth reading as an order of magnitude and not as a
50//! benchmark. What is *not* wide is everything this file does with the answer: reading the
51//! `HMENU` back and converting the item bitmaps come to under half a millisecond together,
52//! every time, and are not worth moving anywhere.
53//!
54//! And those are the *cheap* menus. On an executable on a mapped network share the same call
55//! takes twenty-four seconds, every time — one extension reading the whole file — which is
56//! measured, and is the reason for the shape of [`Builder`]. Read the note there before
57//! changing anything about how it is threaded.
58//!
59//! Two things follow, and both are here:
60//!
61//! **The query runs on a thread of its own, one per menu.** [`Builder`] gives each menu a
62//! worker with an STA of its own and answers by channel, so the window keeps running frames
63//! while the shell takes its time. Whatever an extension does on the way, it does it where no
64//! frame is waiting for it — and a worker that is taking too long is *left*, rather than
65//! becoming the thing every later menu queues behind. The menu itself is not drawn until the
66//! answer is in — see [`crate::ui::menu`] for why it is not shown early and grown.
67//!
68//! **Submenus are filled when they are opened.** Prefilling all of them cost 41–116 ms on
69//! a file — Open With alone was nearly all of it — for menus the user usually never opens.
70//! [`Live`] keeps the `HMENU` and the `IContextMenu` alive for as long as the menu is on
71//! screen, so a submenu can be filled on the hover that opens it.
72//!
73//! # What the first menu of a session is missing
74//!
75//! Some submenus populate themselves *after* `WM_INITMENUPOPUP` and are not finished when it
76//! returns. Read immediately, Send To comes back with only `Desktop (create shortcut)` in it
77//! and Include in library with the shell's own `Retrieving libraries...` placeholder. Read
78//! from the *next* `IContextMenu` in the same process, both are complete — Send To with seven
79//! entries — because the shell has cached its enumeration by then.
80//!
81//! Waiting does not fix it: measured, the same submenu re-read 300 ms and 1 s later, and
82//! re-sent `WM_INITMENUPOPUP` a second and third time, gives the same short answer every
83//! time. It is fixed at the moment that `IContextMenu` first initialised it. So the first
84//! right click of a session has a short Send To and every one after it does not.
85//!
86//! Both ways out cost more than the symptom. Building a throwaway menu at startup to warm the
87//! shell loads every installed extension into the process — 11 MB of private bytes, measured
88//! by `what_the_shell_menu_costs` — whether or not anybody ever right-clicks. Warming on the
89//! first menu instead doubles the wait for that one menu. Neither is worth it for one short
90//! submenu once per run, so this is left as it is, and written down.
91
92use std::path::{Path, PathBuf};
93
94/// One of this program's own entries, which the shell knows nothing about.
95///
96/// There used to be a dozen of these above every shell menu — Open in new tab, Refresh, Select
97/// all, Copy path and the rest. They are gone: a context menu on a file or a folder now shows
98/// Windows' own menu and nothing else, which is what it claims to be. Every command they carried
99/// is still on its keyboard shortcut, and most of them are in the shell's menu anyway under the
100/// name Explorer gives it.
101///
102/// What is left is the one menu Windows has no answer for, because it is not the shell's question:
103/// where a right-button drag has just landed.
104#[derive(Clone, Copy, PartialEq, Eq, Debug)]
105pub enum Own {
106    /// The three a right-button drag offers when it lands, which is how Windows has asked
107    /// "copy or move?" since it stopped guessing.
108    CopyHere,
109    MoveHere,
110    Cancel,
111}
112
113impl Own {
114    pub fn label(self) -> &'static str {
115        match self {
116            Self::CopyHere => "Copy here",
117            Self::MoveHere => "Move here",
118            Self::Cancel => "Cancel",
119        }
120    }
121}
122
123/// What activating an entry does.
124#[derive(Clone, Debug)]
125pub enum Command {
126    /// One of this program's own.
127    Own(Own),
128    /// A shell command: its canonical verb if it has one, and its id either way.
129    Shell { verb: Option<String>, id: u32 },
130}
131
132/// What an entry is.
133#[derive(Clone, Debug)]
134pub enum Kind {
135    Command(Command),
136    Separator,
137    /// A submenu, which starts out empty.
138    ///
139    /// Filling one means asking the extension that owns it to populate its `HMENU`, and
140    /// that costs real time — up to a tenth of a second for Open With. So it is left
141    /// unfilled until the user opens it.
142    ///
143    /// `source` is what the shell knows this submenu by, and `None` means there is nothing
144    /// to ask: either it has been filled already, or it never had a shell behind it.
145    ///
146    /// It is a bare number and not a path through the entries on purpose. It *was* a path,
147    /// and that was wrong the moment the drawing code put this program's own entries above
148    /// the shell's: the path the menu on screen would have asked with was six entries and a
149    /// divider further along than the one the shell had filed the submenu under, so every
150    /// lookup missed and every submenu in the program came back empty. An opaque id cannot
151    /// go wrong that way, because neither side can compute it.
152    Submenu {
153        children: Vec<Entry>,
154        source: Option<u32>,
155    },
156}
157
158impl Kind {
159    /// A submenu nobody has asked the shell about yet.
160    pub fn unfilled(source: u32) -> Self {
161        Self::Submenu {
162            children: Vec::new(),
163            source: Some(source),
164        }
165    }
166
167    /// A submenu with everything in it, which nothing will be asked about.
168    pub fn complete(children: Vec<Entry>) -> Self {
169        Self::Submenu {
170            children,
171            source: None,
172        }
173    }
174
175    /// What the shell knows this submenu by, if it is still waiting to be filled.
176    pub fn unasked(&self) -> Option<u32> {
177        match self {
178            Self::Submenu { source, .. } => *source,
179            _ => None,
180        }
181    }
182}
183
184/// One line of the menu.
185#[derive(Clone, Debug)]
186pub struct Entry {
187    /// Ready to draw: accelerator ampersands removed, the tab-separated shortcut split
188    /// off into `shortcut`.
189    pub label: String,
190    pub shortcut: String,
191    pub kind: Kind,
192    pub enabled: bool,
193    pub checked: bool,
194    /// The bold one — what a double click would have done.
195    pub default: bool,
196    /// The item's own bitmap, as RGBA, when the shell gave one.
197    pub icon: Option<egui::ColorImage>,
198}
199
200impl Entry {
201    fn separator() -> Self {
202        Self {
203            label: String::new(),
204            shortcut: String::new(),
205            kind: Kind::Separator,
206            enabled: false,
207            checked: false,
208            default: false,
209            icon: None,
210        }
211    }
212
213    /// One of this program's own entries.
214    pub fn own(which: Own) -> Self {
215        Self {
216            label: which.label().to_owned(),
217            shortcut: String::new(),
218            kind: Kind::Command(Command::Own(which)),
219            enabled: true,
220            checked: false,
221            default: false,
222            icon: None,
223        }
224    }
225}
226
227
228#[cfg(test)]
229/// The whole menu, submenus and all, on the calling thread.
230///
231/// Half a second of it, for a file — see the note at the top of this file. Nothing in the
232/// program calls this: the app goes through [`Builder`], which does the same work off the
233/// UI thread and fills submenus only when they are opened. It is kept because it is the
234/// shape the fidelity tests want — one call, everything present, nothing to wait for.
235pub fn build(parent: &Path, items: &[PathBuf]) -> Vec<Entry> {
236    #[cfg(windows)]
237    {
238        let Some((mut live, entries)) = win::Live::open(parent, items, Depth::Full) else {
239            return Vec::new();
240        };
241        fn deepen(live: &mut win::Live, entries: &mut Vec<Entry>) {
242            for entry in entries.iter_mut() {
243                let Some(source) = entry.kind.unasked() else {
244                    continue;
245                };
246                let mut children = live.fill(source);
247                deepen(live, &mut children);
248                entry.kind = Kind::complete(children);
249            }
250            // A submenu with nothing in it is worse than no entry at all: it looks like
251            // something that failed rather than something absent.
252            entries.retain(|e| !matches!(&e.kind, Kind::Submenu { children, .. } if children.is_empty()));
253        }
254        let mut entries = entries;
255        deepen(&mut live, &mut entries);
256        entries
257    }
258    #[cfg(not(windows))]
259    {
260        let _ = (parent, items);
261        Vec::new()
262    }
263}
264
265// ---------------------------------------------------------------------------
266// The builder thread
267// ---------------------------------------------------------------------------
268
269/// How much of a menu to ask the shell for.
270///
271/// # Why there is a choice at all
272///
273/// `QueryContextMenu` is one call that lets every installed extension contribute, and the whole
274/// cost is paid inside it — before a single entry exists. So there is nothing to filter
275/// afterwards: an entry that took twenty-four seconds to decide on has already taken them by the
276/// time this program can see it, and dropping it saves nothing. The only lever is asking for
277/// less, and `CMF_` flags are the whole of that lever.
278///
279/// Measured back to back on a 6.4 MB executable on a mapped share, by `probe_menu_costs`:
280///
281/// | flags | `QueryContextMenu` | what came back |
282/// | --- | --- | --- |
283/// | `CMF_NORMAL \| CMF_EXPLORE` | 19.2 s | 29 entries — everything |
284/// | `CMF_OPTIMIZEFORINVOKE` | 10.3 s | 20 entries, labelled `open`, `runas`, `pintohomefile` |
285/// | `CMF_NORMAL \| CMF_EXPLORE \| CMF_DONOTPICKDEFAULT` | 21.0 s | 29 entries |
286/// | **`CMF_DEFAULTONLY`** | **0.49 s** | Open, Run as administrator, Cut, Copy, Paste, Create shortcut, Delete, Rename, Properties |
287/// | `CMF_NOVERBS` | 1.3 ms | Cut, Copy, Create shortcut, Delete, Properties |
288///
289/// `CMF_OPTIMIZEFORINVOKE` is the flag whose documented job is exactly this — "do not do work
290/// that is only needed to display the menu" — and it is no use for a menu that will be
291/// displayed: the labels come back as raw verb names because extensions skip building display
292/// strings, and it is still ten seconds, because whichever extension reads the whole file does
293/// not honour it.
294///
295/// `CMF_DEFAULTONLY` does, forty times over, and what it leaves is the shell's own verbs. That
296/// is a real menu — everything anybody does to a file is in it — minus the third-party extras
297/// (7-Zip, Send To, Open With, Copy as path, Previous Versions, and the several installed
298/// "open with" entries). It is a request for the default verb rather than for a short menu, so
299/// this is leaning on it a little sideways; what it does empirically is skip the extensions that
300/// have to look at the file to decide what to offer, which is precisely the thing that is slow.
301#[derive(Clone, Copy, PartialEq, Eq, Debug)]
302pub enum Depth {
303    /// Everything the machine has to offer. What a local file gets.
304    Full,
305    /// The shell's own verbs, and nothing that has to read the file to decide.
306    Fast,
307}
308
309/// What the UI thread wants from the shell.
310enum Ask {
311    /// Query the shell for a selection, or for the folder when `items` is empty.
312    Build {
313        token: u64,
314        parent: PathBuf,
315        items: Vec<PathBuf>,
316        depth: Depth,
317    },
318    /// Fill the submenu the shell knows by this id.
319    Fill { token: u64, id: u32 },
320    /// The menu is gone: let the `HMENU` and the `IContextMenu` go with it.
321    Close { token: u64 },
322}
323
324/// What came back.
325pub enum Said {
326    /// The shell's entries, and how much of a menu they are.
327    Built {
328        token: u64,
329        entries: Vec<Entry>,
330        depth: Depth,
331    },
332    /// One submenu's contents. Empty means the extension really had nothing.
333    Filled {
334        token: u64,
335        id: u32,
336        children: Vec<Entry>,
337    },
338}
339
340/// Builds context menus off the UI thread, on a worker that can be walked away from.
341///
342/// One menu at a time, which is all a pointer can be pointing at. Every request carries a
343/// token; answers for a token the caller has stopped caring about are simply dropped,
344/// which is what makes a right click during a slow build safe — the old menu's answer
345/// arrives, does not match, and goes in the bin.
346///
347/// # Why a worker per menu, and not one thread with a queue
348///
349/// It *was* one long-lived thread reading a channel, and that is fine right up until the
350/// shell takes a really long time over one menu. Measured by `probe_menu_costs`, on a 14 MB
351/// executable on a mapped SMB share:
352///
353/// | | `QueryContextMenu` |
354/// | --- | --- |
355/// | a folder on the share | 0.20 s |
356/// | a 41 MB `.lib` on the share | 1.8 s |
357/// | a 6.4 MB `.exe` on the share | 11.7 s |
358/// | a 11.5 MB `.exe` on the share | 19.8 s |
359/// | a 14.7 MB `.exe` on the share | **23.9 s** |
360///
361/// Every time, not just the first. That is one extension reading the whole executable — the
362/// time is linear in its size at about 570 kB/s, while a 41 MB file that is *not* an
363/// executable comes back in under two seconds, so the link is doing 20 MB/s and the slow read
364/// is small-chunk and latency-bound rather than short of bandwidth. Nothing else in a menu
365/// costs anything at all: on that same file, parsing the path was 0.26 s, binding to the
366/// parent 0.19 s, and reading the whole `HMENU` back out, item bitmaps and all twenty-six
367/// canonical verbs included, came to **0.4 ms**.
368///
369/// None of that is ours to make faster. What *was* ours is that the queue made it everybody
370/// else's problem: a second right click — on a local file, on anything — sat behind the first
371/// for the rest of those twenty-four seconds, so one slow menu meant no menus at all until it
372/// finished. `QueryContextMenu` is a blocking call into somebody else's code and there is no
373/// asking it to stop, so the only cancellation available is to stop waiting: [`Builder::build`]
374/// **retires** a worker that has not answered yet and serves the new menu on a fresh one. The
375/// abandoned thread finishes whenever the shell lets go, drops its `Live`, and exits; its
376/// answer arrives with a stale token and is discarded.
377///
378/// The cost is that two workers can briefly hold two sets of extension interfaces, which is
379/// the thing the single thread was carefully avoiding. That is the right way round: a few
380/// megabytes for a few seconds, against a program whose context menu stops working.
381///
382/// # Why not [`crate::shell::Modal`]
383///
384/// The modal thread blocks for as long as a Properties sheet is open. A menu that queued
385/// behind one would arrive when the user closed a dialog, which is not when they asked for
386/// it. And the two have opposite lifetimes: a modal request is over when it returns,
387/// whereas a menu's `IContextMenu` has to stay alive — on the thread that made it — for as
388/// long as a submenu might still be opened.
389pub struct Builder {
390    /// The worker serving the menu asked for most recently. Spawned on the first menu, so a
391    /// session where nobody right-clicks has no thread and no shell extensions in it.
392    current: Option<Worker>,
393    /// Workers abandoned inside a call nobody is waiting for. Reaped as they end.
394    retired: Vec<std::thread::JoinHandle<()>>,
395    /// Handed to every worker, live and retired. Answers are told apart by token.
396    says: std::sync::mpsc::Sender<Said>,
397    rx: std::sync::mpsc::Receiver<Said>,
398    ctx: egui::Context,
399    next: u64,
400    /// The token of the build that has not been answered yet — which is the same thing as
401    /// "the current worker is inside `QueryContextMenu` and will be for as long as it takes".
402    waiting: Option<u64>,
403}
404
405/// One thread with one apartment and at most one menu open on it.
406struct Worker {
407    /// Dropping this ends the thread's loop, which is how a worker is told to finish.
408    tx: std::sync::mpsc::Sender<Ask>,
409    thread: Option<std::thread::JoinHandle<()>>,
410}
411
412/// Ends the threads, and waits for them — but not for ever.
413///
414/// The waiting is the point. A menu still open when the program quits leaves a worker holding
415/// an `IContextMenu`, and through it a live object inside every shell extension that
416/// contributed to it. Joining releases all of that on the thread that owns it, while that
417/// thread is still there — rather than leaving it to process teardown, where an `HMENU` and
418/// a set of apartment-threaded interfaces are freed by nobody in particular.
419///
420/// The bound is the point too, and it is new. An unconditional join here hands the
421/// twenty-four seconds straight back, this time to closing the window: measured, closing while
422/// a worker was still inside `QueryContextMenu` on that network executable took **19.9 s**
423/// joining and **1.9 s** with the bound, which is the rest of shutting down and not this. So
424/// the wait is brief, and it is spent in [`crate::shell::answering_calls`] rather than asleep,
425/// because this is the UI thread's own apartment and a worker on the way out may yet call into
426/// it. Whatever has not finished by then is left to the process, which is where it was going
427/// anyway.
428///
429/// It does *not* fix the intermittent failure to exit that a `--shot --menu` run shows about
430/// one time in eight. That was the first guess, and it was wrong: the same run against the
431/// commit before any of this hangs at the same rate, and a `--shot` run with no menu in it
432/// does not hang at all. So the cause is the extensions being in the process at all, which
433/// predates all of this and is not this file's to fix.
434impl Drop for Builder {
435    fn drop(&mut self) {
436        // The sender goes with the `Worker`; the handle is kept so it can be waited for.
437        let live = self.current.take().and_then(|mut w| w.thread.take());
438        let mut threads = std::mem::take(&mut self.retired);
439        threads.extend(live);
440
441        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
442        while !threads.is_empty() {
443            threads.retain(|thread| !thread.is_finished());
444            if threads.is_empty() || std::time::Instant::now() >= deadline {
445                break;
446            }
447            crate::shell::answering_calls(5);
448        }
449    }
450}
451
452impl Builder {
453    pub fn new(ctx: &egui::Context) -> Self {
454        let (says, rx) = std::sync::mpsc::channel::<Said>();
455        Self {
456            current: None,
457            retired: Vec::new(),
458            says,
459            rx,
460            ctx: ctx.clone(),
461            next: 1,
462            waiting: None,
463        }
464    }
465
466    /// A fresh worker with an apartment of its own.
467    fn spawn(&self) -> Worker {
468        let (tx, asks) = std::sync::mpsc::channel::<Ask>();
469        let says = self.says.clone();
470        let ctx = self.ctx.clone();
471        let thread = std::thread::Builder::new()
472            .name("shell-menu".to_owned())
473            .spawn(move || serve(asks, says, ctx))
474            .ok();
475        Worker { tx, thread }
476    }
477
478    /// Send an ask, if there is a worker to hear it.
479    fn ask(&self, ask: Ask) {
480        if let Some(worker) = &self.current {
481            let _ = worker.tx.send(ask);
482        }
483    }
484
485    /// Ask for a menu. The token identifies its answers.
486    ///
487    /// If the previous menu has not come back yet, its worker is abandoned rather than queued
488    /// behind — see the note on [`Builder`]. So this returns immediately and the new menu is
489    /// built immediately, whatever the shell is still doing about the old one.
490    ///
491    /// The app happens to reach [`Builder::abandon`] first, because opening a menu closes
492    /// whatever was on its way. That does not make this redundant: a build queued behind a
493    /// worker that has twenty seconds left to run is never the right thing, and whether it can
494    /// happen should not depend on a caller elsewhere getting the order right.
495    pub fn build(&mut self, parent: &Path, items: &[PathBuf], depth: Depth) -> u64 {
496        let token = self.next;
497        self.next += 1;
498        if self.waiting.is_some() {
499            self.retire();
500        }
501        if self.current.is_none() {
502            self.current = Some(self.spawn());
503        }
504        self.waiting = Some(token);
505        self.ask(Ask::Build {
506            token,
507            parent: parent.to_owned(),
508            items: items.to_vec(),
509            depth,
510        });
511        token
512    }
513
514    /// Stop waiting for the menu that is still being built.
515    ///
516    /// Not the same as [`Builder::close`], which tells a worker to let go of a menu it has
517    /// already made. There is nothing to tell here: the thread is inside somebody else's
518    /// code. So it is let go of instead — the loop ends when the call returns, the `Live`
519    /// goes with it, and the answer lands on a channel nobody is reading.
520    pub fn abandon(&mut self) {
521        if self.waiting.take().is_some() {
522            self.retire();
523        }
524    }
525
526    /// Whether a build is outstanding, which is the one state a worker cannot be talked out of.
527    ///
528    /// The app has no use for this — it knows whether it is waiting, because it is holding the
529    /// `Asking`. It is here for the test that has to establish that the slow build really was
530    /// still running when the second one was asked for.
531    #[cfg(test)]
532    pub fn busy(&self) -> bool {
533        self.waiting.is_some()
534    }
535
536    fn retire(&mut self) {
537        if let Some(mut worker) = self.current.take() {
538            if let Some(thread) = worker.thread.take() {
539                self.retired.push(thread);
540            }
541            // And `worker` drops here, taking its sender with it — which is what ends the
542            // thread's loop once the shell finally returns.
543        }
544        self.reap();
545    }
546
547    /// Join whichever abandoned workers have since finished, so their handles do not pile up.
548    fn reap(&mut self) {
549        if self.retired.is_empty() {
550            return;
551        }
552        let mut still = Vec::with_capacity(self.retired.len());
553        for thread in std::mem::take(&mut self.retired) {
554            if thread.is_finished() {
555                let _ = thread.join();
556            } else {
557                still.push(thread);
558            }
559        }
560        self.retired = still;
561    }
562
563    /// Ask for one submenu's contents.
564    pub fn fill(&self, token: u64, id: u32) {
565        self.ask(Ask::Fill { token, id });
566    }
567
568    /// The menu has closed; nothing more will be asked of it.
569    pub fn close(&self, token: u64) {
570        self.ask(Ask::Close { token });
571    }
572
573    /// Whatever has arrived, without waiting.
574    pub fn poll(&mut self) -> Option<Said> {
575        self.reap();
576        let said = self.rx.try_recv().ok()?;
577        if let Said::Built { token, .. } = &said {
578            if self.waiting == Some(*token) {
579                self.waiting = None;
580            }
581        }
582        Some(said)
583    }
584}
585
586/// The builder thread.
587fn serve(
588    asks: std::sync::mpsc::Receiver<Ask>,
589    says: std::sync::mpsc::Sender<Said>,
590    ctx: egui::Context,
591) {
592    // Its own apartment: the `HMENU` and every interface reached through it belong to this
593    // thread and are only ever touched from here.
594    super::init();
595    let mut held = Held::default();
596
597    while let Ok(ask) = asks.recv() {
598        let said = match ask {
599            Ask::Build {
600                token,
601                parent,
602                items,
603                depth,
604            } => Some(Said::Built {
605                token,
606                entries: held.open(token, &parent, &items, depth),
607                depth,
608            }),
609            Ask::Fill { token, id } => Some(Said::Filled {
610                token,
611                id,
612                children: held.fill(token, id),
613            }),
614            Ask::Close { token } => {
615                held.close(token);
616                None
617            }
618        };
619        if let Some(said) = said {
620            if says.send(said).is_err() {
621                return;
622            }
623            ctx.request_repaint();
624        }
625    }
626}
627
628/// Makes the next build take this long instead of asking the shell anything at all.
629///
630/// For `a_slow_menu_does_not_hold_up_the_next_one`, which needs a build that is still running
631/// when the second one is asked for. A stall touches the shell not at all, deliberately: the
632/// worker abandoned in the middle of it must not be able to collide with another test over the
633/// process-wide clipboard lock — see [`crate::shell::serialised`].
634///
635/// Every build stalls while it is set, so a test can stall the retry as well as the first
636/// attempt. Whichever test sets it must put it back, and must be holding
637/// [`crate::shell::serialised`] while it does.
638#[cfg(test)]
639pub(crate) static STALL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
640
641/// How many builds have stalled, so the test can wait until one really has.
642#[cfg(test)]
643pub(crate) static STALLED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
644
645/// The one menu the builder thread currently has open, if any.
646#[derive(Default)]
647struct Held {
648    #[cfg(windows)]
649    live: Option<(u64, win::Live)>,
650    #[cfg(not(windows))]
651    live: Option<u64>,
652}
653
654impl Held {
655    fn open(&mut self, token: u64, parent: &Path, items: &[PathBuf], depth: Depth) -> Vec<Entry> {
656        #[cfg(test)]
657        {
658            use std::sync::atomic::Ordering;
659            let ms = STALL_MS.load(Ordering::SeqCst);
660            if ms > 0 {
661                STALLED.fetch_add(1, Ordering::SeqCst);
662                std::thread::sleep(std::time::Duration::from_millis(ms));
663                return Vec::new();
664            }
665        }
666        // The previous menu goes first, so this worker holds at most one `HMENU` and one set
667        // of extension interfaces at a time.
668        self.live = None;
669        #[cfg(windows)]
670        {
671            match win::Live::open(parent, items, depth) {
672                Some((live, entries)) => {
673                    self.live = Some((token, live));
674                    entries
675                }
676                None => Vec::new(),
677            }
678        }
679        #[cfg(not(windows))]
680        {
681            let _ = (token, parent, items, depth);
682            Vec::new()
683        }
684    }
685
686    fn fill(&mut self, token: u64, id: u32) -> Vec<Entry> {
687        #[cfg(windows)]
688        {
689            match &mut self.live {
690                Some((held, live)) if *held == token => live.fill(id),
691                _ => Vec::new(),
692            }
693        }
694        #[cfg(not(windows))]
695        {
696            let _ = (token, id);
697            Vec::new()
698        }
699    }
700
701    fn close(&mut self, token: u64) {
702        #[cfg(windows)]
703        if matches!(&self.live, Some((held, _)) if *held == token) {
704            self.live = None;
705        }
706        #[cfg(not(windows))]
707        let _ = token;
708    }
709}
710
711/// Run a shell command. Puts up dialogs, so it belongs on the modal thread.
712pub fn invoke(parent: &Path, items: &[PathBuf], command: &Command, owner: super::Owner) {
713    #[cfg(windows)]
714    if let Command::Shell { verb, id } = command {
715        win::invoke(parent, items, verb.as_deref(), *id, owner);
716    }
717    #[cfg(not(windows))]
718    let _ = (parent, items, command, owner);
719}
720
721#[cfg(windows)]
722mod win {
723    use super::*;
724    use windows::core::{Interface, PCSTR, PCWSTR, PSTR, PWSTR};
725    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
726    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
727    use windows::Win32::UI::Shell::{
728        IContextMenu, IContextMenu2, IShellFolder, SHBindToParent, SHParseDisplayName, CMF_EXPLORE,
729        CMF_NORMAL, CMF_OPTIMIZEFORINVOKE, CMINVOKECOMMANDINFOEX, GCS_VERBW,
730    };
731    use windows::Win32::UI::WindowsAndMessaging::{
732        CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, HMENU, MENUITEMINFOW,
733        MFS_CHECKED, MFS_DEFAULT, MFS_DISABLED, MFS_GRAYED, MFT_SEPARATOR, MIIM_BITMAP, MIIM_FTYPE,
734        MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SW_SHOWNORMAL, WM_INITMENUPOPUP,
735    };
736
737    /// The shell's commands are numbered from here, so an id read back out of the menu
738    /// is turned into a command offset by subtracting it.
739    const FIRST: u32 = 0x1000;
740    const LAST: u32 = 0x7FFF;
741    /// Deep enough for anything real; a guard against a malformed extension.
742    const MAX_DEPTH: u32 = 4;
743
744    /// A PIDL that frees itself.
745    struct Pidl(*mut ITEMIDLIST);
746
747    impl Drop for Pidl {
748        fn drop(&mut self) {
749            if !self.0.is_null() {
750                // SAFETY: allocated by `SHParseDisplayName`, freed exactly once.
751                unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
752            }
753        }
754    }
755
756    fn pidl_of(path: &Path) -> Option<Pidl> {
757        let wide = crate::shell::wide(path);
758        let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
759        // SAFETY: `wide` is null-terminated and outlives the call.
760        let ok =
761            unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut raw, 0, None).is_ok() };
762        (ok && !raw.is_null()).then_some(Pidl(raw))
763    }
764
765    /// The `IContextMenu` for a selection, or for the folder itself when the selection is
766    /// empty — which is the menu you get by right-clicking the background, and where New
767    /// and Paste come from.
768    unsafe fn context_of(parent: &Path, items: &[PathBuf]) -> Option<IContextMenu> {
769        if items.is_empty() {
770            let pidl = pidl_of(parent)?;
771            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
772            let folder: IShellFolder = SHBindToParent(pidl.0, Some(&mut child)).ok()?;
773            if child.is_null() {
774                return None;
775            }
776            let children = [child as *const ITEMIDLIST];
777            return folder.GetUIObjectOf(HWND::default(), &children, None).ok();
778        }
779
780        // Every item has to be a child of one folder for one `IContextMenu`, which a
781        // selection in this program always is.
782        let pidls: Vec<Pidl> = items.iter().filter_map(|p| pidl_of(p)).collect();
783        let first = pidls.first()?;
784        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
785        let folder: IShellFolder = SHBindToParent(first.0, Some(&mut child)).ok()?;
786
787        let mut children: Vec<*const ITEMIDLIST> = Vec::with_capacity(pidls.len());
788        children.push(child as *const ITEMIDLIST);
789        for pidl in pidls.iter().skip(1) {
790            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
791            if SHBindToParent::<IShellFolder>(pidl.0, Some(&mut child)).is_ok() && !child.is_null() {
792                children.push(child as *const ITEMIDLIST);
793            }
794        }
795        folder.GetUIObjectOf(HWND::default(), &children, None).ok()
796    }
797
798    /// What to ask `QueryContextMenu` for. See [`Depth`] for the measurements behind this.
799    ///
800    /// `CMF_EXPLORE` on the full one asks for the menu Explorer shows rather than the shorter
801    /// one a file dialog gets.
802    fn flags(depth: Depth) -> u32 {
803        match depth {
804            Depth::Full => CMF_NORMAL | CMF_EXPLORE,
805            Depth::Fast => windows::Win32::UI::Shell::CMF_DEFAULTONLY,
806        }
807    }
808
809    /// One open menu's shell state, on the thread that made it.
810    ///
811    /// `IContextMenu` is apartment-threaded and the `HMENU` belongs to whoever created it,
812    /// so this never leaves the thread it was made on — which is
813    /// [`super::Builder`]'s, not the UI's.
814    pub struct Live {
815        context: IContextMenu,
816        hmenu: HMENU,
817        /// Every submenu found so far, under the id handed out with it.
818        submenus: std::collections::HashMap<u32, Sub>,
819        /// The next id. Only ever grows, so an id names one submenu for this menu's life.
820        next: u32,
821    }
822
823    /// What is needed to fill one submenu.
824    #[derive(Clone, Copy)]
825    struct Sub {
826        menu: HMENU,
827        /// The item's position inside its *parent* `HMENU`, which is what
828        /// `WM_INITMENUPOPUP` wants — and which is not the entry index, since separators are
829        /// coalesced and unusable items dropped on the way out.
830        position: u32,
831        /// How deep the parent was, for the guard against a malformed extension.
832        depth: u32,
833    }
834
835    impl Drop for Live {
836        fn drop(&mut self) {
837            // Destroys the submenus with it, which is why they are not tracked for freeing.
838            // SAFETY: created by `CreatePopupMenu` here, destroyed exactly once.
839            unsafe {
840                let _ = DestroyMenu(self.hmenu);
841            }
842        }
843    }
844
845    impl Live {
846        /// Ask the shell, and read the top level. Submenus are left for [`Live::fill`].
847        pub fn open(parent: &Path, items: &[PathBuf], depth: Depth) -> Option<(Self, Vec<Entry>)> {
848            // SAFETY: the menu is destroyed by `Drop` on every path out, and the
849            // interfaces are reference counted.
850            unsafe {
851                let context = context_of(parent, items)?;
852                let hmenu = CreatePopupMenu().ok()?;
853                let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags(depth));
854                let mut live = Self {
855                    context,
856                    hmenu,
857                    submenus: std::collections::HashMap::new(),
858                    next: 0,
859                };
860                let entries = live.read(hmenu, 0);
861                Some((live, entries))
862            }
863        }
864
865        /// The contents of the submenu with this id, asking the extension to fill it first.
866        pub fn fill(&mut self, id: u32) -> Vec<Entry> {
867            let Some(sub) = self.submenus.get(&id).copied() else {
868                return Vec::new();
869            };
870            if sub.depth >= MAX_DEPTH {
871                // Deep enough for anything real; a guard against a malformed extension.
872                return Vec::new();
873            }
874            // SAFETY: both handles came out of this menu and are alive as long as it is.
875            unsafe {
876                // An extension fills its submenu when the menu is about to pop up. This
877                // menu never pops up, so it is told to anyway — otherwise Send To and New
878                // come back empty.
879                if let Ok(two) = self.context.cast::<IContextMenu2>() {
880                    let _ = two.HandleMenuMsg(
881                        WM_INITMENUPOPUP,
882                        WPARAM(sub.menu.0 as usize),
883                        LPARAM(sub.position as isize),
884                    );
885                }
886                self.read(sub.menu, sub.depth + 1)
887            }
888        }
889
890        /// Read one `HMENU` the shell has filled into something drawable.
891        unsafe fn read(&mut self, hmenu: HMENU, depth: u32) -> Vec<Entry> {
892            let count = GetMenuItemCount(Some(hmenu));
893            if count <= 0 {
894                return Vec::new();
895            }
896            let mut entries: Vec<Entry> = Vec::with_capacity(count as usize);
897
898            for position in 0..count {
899                let mut info = MENUITEMINFOW {
900                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
901                    fMask: MIIM_FTYPE
902                        | MIIM_STATE
903                        | MIIM_ID
904                        | MIIM_SUBMENU
905                        | MIIM_STRING
906                        | MIIM_BITMAP,
907                    ..Default::default()
908                };
909                // Two calls: the first to learn the length, the second to get the text. A
910                // fixed buffer would truncate a long "Open with <application>".
911                if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
912                    continue;
913                }
914                let mut text = vec![0u16; info.cch as usize + 1];
915                info.dwTypeData = PWSTR(text.as_mut_ptr());
916                info.cch = text.len() as u32;
917                if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
918                    continue;
919                }
920
921                if info.fType.0 & MFT_SEPARATOR.0 != 0 {
922                    // Two separators running together, or one at either end, are what a
923                    // menu assembled from several extensions looks like before anyone
924                    // tidies it.
925                    if !matches!(entries.last(), None | Some(Entry { kind: Kind::Separator, .. })) {
926                        entries.push(Entry::separator());
927                    }
928                    continue;
929                }
930
931                let raw = String::from_utf16_lossy(&text[..info.cch as usize]);
932                let (label, shortcut) = split_label(&raw);
933                if label.is_empty() {
934                    continue;
935                }
936
937                let enabled = info.fState.0 & (MFS_DISABLED.0 | MFS_GRAYED.0) == 0;
938                let checked = info.fState.0 & MFS_CHECKED.0 != 0;
939                let default = info.fState.0 & MFS_DEFAULT.0 != 0;
940                let icon = menu_bitmap(info.hbmpItem);
941
942                let kind = if !info.hSubMenu.is_invalid() && depth < MAX_DEPTH {
943                    // Noted rather than read. Whether there is anything in it is not known
944                    // until the extension is asked, and asking is the expensive part.
945                    let id = self.next;
946                    self.next += 1;
947                    self.submenus.insert(
948                        id,
949                        Sub {
950                            menu: info.hSubMenu,
951                            position: position as u32,
952                            depth,
953                        },
954                    );
955                    Kind::unfilled(id)
956                } else if info.wID >= FIRST && info.wID <= LAST {
957                    let offset = info.wID - FIRST;
958                    Kind::Command(Command::Shell {
959                        verb: canonical_verb(&self.context, offset as usize),
960                        id: offset,
961                    })
962                } else {
963                    // An id outside the range this program handed out is not ours to
964                    // invoke.
965                    continue;
966                };
967
968                entries.push(Entry {
969                    label,
970                    shortcut,
971                    kind,
972                    enabled,
973                    checked,
974                    default,
975                    icon,
976                });
977            }
978
979            while matches!(entries.last(), Some(Entry { kind: Kind::Separator, .. })) {
980                entries.pop();
981            }
982            entries
983        }
984    }
985
986    /// A menu label as the shell writes it, split into what to draw.
987    ///
988    /// `&` marks the keyboard accelerator and `&&` is a literal ampersand; a tab
989    /// separates the label from its shortcut text.
990    pub(super) fn split_label(raw: &str) -> (String, String) {
991        let (left, right) = match raw.split_once('\t') {
992            Some((left, right)) => (left, right.trim().to_owned()),
993            None => (raw, String::new()),
994        };
995        let mut label = String::with_capacity(left.len());
996        let mut chars = left.chars().peekable();
997        while let Some(c) = chars.next() {
998            if c == '&' {
999                if chars.peek() == Some(&'&') {
1000                    chars.next();
1001                    label.push('&');
1002                }
1003                continue;
1004            }
1005            label.push(c);
1006        }
1007        (label.trim().to_owned(), right)
1008    }
1009
1010    /// The canonical verb for a command, if it has one.
1011    ///
1012    /// Stable across processes and threads, which is what lets the command be invoked
1013    /// later against a freshly obtained `IContextMenu`.
1014    unsafe fn canonical_verb(context: &IContextMenu, offset: usize) -> Option<String> {
1015        let mut buffer = [0u8; 260];
1016        // `GCS_VERBW` writes wide characters into the same buffer, so it is sized for
1017        // them and read back as such.
1018        context
1019            .GetCommandString(
1020                offset,
1021                GCS_VERBW,
1022                None,
1023                PSTR(buffer.as_mut_ptr()),
1024                (buffer.len() / 2) as u32,
1025            )
1026            .ok()?;
1027        let wide: Vec<u16> = buffer
1028            .chunks_exact(2)
1029            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
1030            .take_while(|unit| *unit != 0)
1031            .collect();
1032        let verb = String::from_utf16_lossy(&wide);
1033        (!verb.is_empty()).then_some(verb)
1034    }
1035
1036    /// A menu item's bitmap as RGBA, when it is a real one.
1037    ///
1038    /// `hbmpItem` doubles as a slot for a dozen `HBMMENU_*` magic values — small
1039    /// integers, not handles — which is what the bound below is filtering out.
1040    unsafe fn menu_bitmap(
1041        bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
1042    ) -> Option<egui::ColorImage> {
1043        const LAST_MAGIC: isize = 16;
1044        if bitmap.is_invalid() || (bitmap.0 as isize) <= LAST_MAGIC {
1045            return None;
1046        }
1047        crate::shell::icons::bitmap_of(bitmap)
1048    }
1049
1050    /// What each `CMF_` flag combination costs, and what it leaves out.
1051    ///
1052    /// The expensive work happens *inside* `QueryContextMenu`, before any entry exists, so
1053    /// there is nothing to filter afterwards. The only lever is asking for less. This is what
1054    /// asking for less actually buys.
1055    #[cfg(test)]
1056    pub(super) fn probe_flags(parent: &Path, items: &[PathBuf]) {
1057        use std::time::Instant;
1058        use windows::Win32::UI::Shell::{CMF_DEFAULTONLY, CMF_DONOTPICKDEFAULT, CMF_NOVERBS};
1059
1060        for (label, flags) in [
1061            ("NORMAL|EXPLORE (what it uses)", CMF_NORMAL | CMF_EXPLORE),
1062            ("OPTIMIZEFORINVOKE", CMF_OPTIMIZEFORINVOKE),
1063            (
1064                "NORMAL|EXPLORE|DONOTPICKDEFAULT",
1065                CMF_NORMAL | CMF_EXPLORE | CMF_DONOTPICKDEFAULT,
1066            ),
1067            ("DEFAULTONLY", CMF_DEFAULTONLY),
1068            ("NOVERBS", CMF_NOVERBS),
1069        ] {
1070            // SAFETY: the menu is destroyed before the next round, and the interfaces are
1071            // reference counted.
1072            unsafe {
1073                let Some(context) = context_of(parent, items) else {
1074                    return;
1075                };
1076                let Ok(hmenu) = CreatePopupMenu() else { return };
1077                let at = Instant::now();
1078                let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags);
1079                let took = at.elapsed().as_secs_f32() * 1e3;
1080                let mut live = Live {
1081                    context,
1082                    hmenu,
1083                    submenus: std::collections::HashMap::new(),
1084                    next: 0,
1085                };
1086                let entries = live.read(hmenu, 0);
1087                eprintln!(
1088                    "  {label:<32} {took:>9.1} ms  {} entries",
1089                    entries.len()
1090                );
1091                eprintln!(
1092                    "      {}",
1093                    entries
1094                        .iter()
1095                        .filter(|e| !e.label.is_empty())
1096                        .map(|e| e.label.as_str())
1097                        .collect::<Vec<_>>()
1098                        .join(" | ")
1099                );
1100            }
1101        }
1102    }
1103
1104    /// Time every shell call one menu costs, call by call.
1105    ///
1106    /// Not a test of anything — a measurement, and the only way to find out which of these is
1107    /// the slow one when the file is on a share. See `probe_menu_costs`.
1108    #[cfg(test)]
1109    pub(super) fn probe(parent: &Path, items: &[PathBuf]) {
1110        use std::time::Instant;
1111        let ms = |at: Instant| at.elapsed().as_secs_f32() * 1e3;
1112
1113        unsafe {
1114            let at = Instant::now();
1115            let pidl = pidl_of(items.first().unwrap_or(&parent.to_owned()));
1116            eprintln!("  SHParseDisplayName      {:>9.1} ms", ms(at));
1117            drop(pidl);
1118
1119            let at = Instant::now();
1120            let Some(context) = context_of(parent, items) else {
1121                eprintln!("  no IContextMenu");
1122                return;
1123            };
1124            eprintln!("  bind + GetUIObjectOf    {:>9.1} ms", ms(at));
1125
1126            let at = Instant::now();
1127            let Ok(hmenu) = CreatePopupMenu() else { return };
1128            let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, CMF_NORMAL | CMF_EXPLORE);
1129            eprintln!("  QueryContextMenu        {:>9.1} ms", ms(at));
1130
1131            // The read, split into the three things it does per item.
1132            let count = GetMenuItemCount(Some(hmenu));
1133            let mut reading = 0.0f32;
1134            let mut bitmaps = 0.0f32;
1135            let mut verbs: Vec<(String, f32, Option<String>)> = Vec::new();
1136            for position in 0..count {
1137                let at = Instant::now();
1138                let mut info = MENUITEMINFOW {
1139                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
1140                    fMask: MIIM_FTYPE
1141                        | MIIM_STATE
1142                        | MIIM_ID
1143                        | MIIM_SUBMENU
1144                        | MIIM_STRING
1145                        | MIIM_BITMAP,
1146                    ..Default::default()
1147                };
1148                if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
1149                    continue;
1150                }
1151                let mut text = vec![0u16; info.cch as usize + 1];
1152                info.dwTypeData = PWSTR(text.as_mut_ptr());
1153                info.cch = text.len() as u32;
1154                if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
1155                    continue;
1156                }
1157                let raw = String::from_utf16_lossy(&text[..info.cch as usize]);
1158                reading += ms(at);
1159
1160                let at = Instant::now();
1161                let _ = menu_bitmap(info.hbmpItem);
1162                bitmaps += ms(at);
1163
1164                if info.hSubMenu.is_invalid() && info.wID >= FIRST && info.wID <= LAST {
1165                    let at = Instant::now();
1166                    let verb = canonical_verb(&context, (info.wID - FIRST) as usize);
1167                    verbs.push((split_label(&raw).0, ms(at), verb));
1168                }
1169            }
1170            eprintln!("  GetMenuItemInfoW x{count:<3}   {reading:>9.1} ms");
1171            eprintln!("  item bitmaps            {bitmaps:>9.1} ms");
1172            let total: f32 = verbs.iter().map(|(_, ms, _)| ms).sum();
1173            eprintln!("  GetCommandString x{:<3}   {total:>9.1} ms", verbs.len());
1174            verbs.sort_by(|a, b| b.1.total_cmp(&a.1));
1175            for (label, took, verb) in verbs.iter().take(8) {
1176                eprintln!("      {took:>9.1} ms  {label:<32} -> {verb:?}");
1177            }
1178            let _ = DestroyMenu(hmenu);
1179        }
1180    }
1181
1182    /// Run a shell command, by verb where there is one.
1183    pub fn invoke(
1184        parent: &Path,
1185        items: &[PathBuf],
1186        verb: Option<&str>,
1187        id: u32,
1188        owner: crate::shell::Owner,
1189    ) {
1190        // SAFETY: the menu is destroyed before returning, and every string outlives the
1191        // call that reads it.
1192        unsafe {
1193            let Some(context) = context_of(parent, items) else {
1194                return;
1195            };
1196            let Ok(hmenu) = CreatePopupMenu() else {
1197                return;
1198            };
1199            // A verb needs no numbering, so the shell is told not to build a menu it will
1200            // never show: `CMF_OPTIMIZEFORINVOKE` is what lets an extension skip the
1201            // registry and disk work that `QueryContextMenu` otherwise costs, and that
1202            // work was measured at half a second on a file. Without a verb the flag cannot
1203            // be used — the command is a *position* in the menu, so the menu has to be
1204            // built the same way it was when the position was read, or the wrong thing
1205            // runs.
1206            let flags = match verb {
1207                Some(_) => CMF_OPTIMIZEFORINVOKE,
1208                None => CMF_NORMAL | CMF_EXPLORE,
1209            };
1210            let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags);
1211
1212            let verb_bytes: Option<Vec<u8>> = verb.map(|verb| {
1213                let mut bytes = verb.as_bytes().to_vec();
1214                bytes.push(0);
1215                bytes
1216            });
1217            let verb_wide: Option<Vec<u16>> = verb.map(|verb| {
1218                verb.encode_utf16().chain(std::iter::once(0)).collect()
1219            });
1220
1221            let mut info = CMINVOKECOMMANDINFOEX {
1222                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
1223                fMask: 0,
1224                hwnd: if owner.0 != 0 {
1225                    owner.hwnd()
1226                } else {
1227                    HWND::default()
1228                },
1229                // A verb is a string; an id is a small integer pretending to be one,
1230                // which is how `IContextMenu` has taken numeric commands since it was
1231                // introduced.
1232                lpVerb: match &verb_bytes {
1233                    Some(bytes) => PCSTR(bytes.as_ptr()),
1234                    None => PCSTR(id as usize as *const u8),
1235                },
1236                lpVerbW: match &verb_wide {
1237                    Some(wide) => PCWSTR(wide.as_ptr()),
1238                    None => PCWSTR(id as usize as *const u16),
1239                },
1240                nShow: SW_SHOWNORMAL.0,
1241                ..Default::default()
1242            };
1243            let _ = context.InvokeCommand(&mut info as *mut _ as *const _);
1244            let _ = DestroyMenu(hmenu);
1245        }
1246    }
1247}
1248
1249#[cfg(test)]
1250mod tests {
1251    use super::*;
1252
1253    /// The *folder's* menu — the one an empty-space right click asks for, which has no
1254    /// items in it and takes a different route through the shell than a selection does.
1255    #[test]
1256    #[cfg(windows)]
1257    fn a_folder_with_nothing_selected_still_has_the_shell_s_menu() {
1258        let _serialised = crate::shell::serialised();
1259        crate::shell::init();
1260
1261        let mut dir = std::env::temp_dir();
1262        dir.push(format!("yafe-folder-menu-{}", std::process::id()));
1263        std::fs::create_dir_all(&dir).expect("temp dir");
1264
1265        let entries = build(&dir, &[]);
1266        assert!(
1267            entries.len() > 3,
1268            "the folder's own menu came back with {} entries -- cut, copy, properties and              whatever is installed should all be in it",
1269            entries.len()
1270        );
1271        let _ = std::fs::remove_dir_all(&dir);
1272    }
1273
1274    /// A temp folder with a file and a subfolder in it, for the tests that need something
1275    /// real to right-click.
1276    #[cfg(windows)]
1277    fn scratch(name: &str) -> (PathBuf, PathBuf, PathBuf) {
1278        let mut dir = std::env::temp_dir();
1279        dir.push(format!("yafe-{name}-{}", std::process::id()));
1280        std::fs::create_dir_all(&dir).expect("temp dir");
1281        let file = dir.join("one.txt");
1282        std::fs::write(&file, b"x").expect("write");
1283        let sub = dir.join("folder");
1284        std::fs::create_dir_all(&sub).expect("subdir");
1285        (dir, file, sub)
1286    }
1287
1288    /// The lazy half of the design: opening the menu must *not* fill the submenus, and
1289    /// filling one afterwards must give what the eager path would have.
1290    ///
1291    /// This is the saving that matters on a file -- Open With alone was 41-104 ms of the
1292    /// build -- so a change that quietly went back to prefilling would show up here as a
1293    /// submenu that already had children.
1294    #[test]
1295    #[cfg(windows)]
1296    fn submenus_stay_empty_until_they_are_asked_for() {
1297        let _serialised = crate::shell::serialised();
1298        crate::shell::init();
1299        let (dir, file, _) = scratch("lazy");
1300
1301        let (mut live, entries) =
1302            super::win::Live::open(&dir, std::slice::from_ref(&file), Depth::Full)
1303                .expect("the shell's menu");
1304
1305        let submenus: Vec<(u32, String)> = entries
1306            .iter()
1307            .filter_map(|e| e.kind.unasked().map(|id| (id, e.label.clone())))
1308            .collect();
1309        assert!(
1310            !submenus.is_empty(),
1311            "a text file on any Windows has at least Open With or Send To: {:?}",
1312            entries.iter().map(|e| &e.label).collect::<Vec<_>>()
1313        );
1314        for entry in &entries {
1315            if let Kind::Submenu { children, source } = &entry.kind {
1316                assert!(
1317                    children.is_empty() && source.is_some(),
1318                    "`{}` came back already filled -- opening the menu paid for a submenu \
1319                     nobody had opened",
1320                    entry.label
1321                );
1322            }
1323        }
1324
1325        // And asking works: at least one of them has something in it.
1326        let filled: Vec<(String, usize)> = submenus
1327            .iter()
1328            .map(|(id, label)| (label.clone(), live.fill(*id).len()))
1329            .collect();
1330        assert!(
1331            filled.iter().any(|(_, count)| *count > 0),
1332            "every submenu came back empty when asked, so the fill never reached the \
1333             extensions: {filled:?}"
1334        );
1335
1336        let _ = std::fs::remove_dir_all(&dir);
1337    }
1338
1339    /// The asynchronous half: the builder answers by channel, and a submenu asked for
1340    /// after the fact comes back against the same token.
1341    #[test]
1342    #[cfg(windows)]
1343    fn the_builder_answers_by_channel() {
1344        let _serialised = crate::shell::serialised();
1345        crate::shell::init();
1346        let (dir, file, _) = scratch("builder");
1347
1348        let ctx = egui::Context::default();
1349        let mut builder = Builder::new(&ctx);
1350
1351        // Wait for one answer. Generous, because the whole point is that this is slow.
1352        fn next(builder: &mut Builder) -> Said {
1353            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
1354            loop {
1355                if let Some(said) = builder.poll() {
1356                    return said;
1357                }
1358                assert!(
1359                    std::time::Instant::now() < deadline,
1360                    "the builder thread never answered"
1361                );
1362                std::thread::sleep(std::time::Duration::from_millis(5));
1363            }
1364        }
1365
1366        let token = builder.build(&dir, std::slice::from_ref(&file), Depth::Full);
1367        let entries = match next(&mut builder) {
1368            Said::Built { token: got, entries, .. } => {
1369                assert_eq!(got, token, "answered for a menu nobody asked for");
1370                entries
1371            }
1372            _ => panic!("the first answer to a build should be the menu"),
1373        };
1374        assert!(
1375            entries.len() > 3,
1376            "the builder came back with only {} entries",
1377            entries.len()
1378        );
1379
1380        // The submenu is still on the builder's thread, and can be filled from here.
1381        let (id, label) = entries
1382            .iter()
1383            .find_map(|e| e.kind.unasked().map(|id| (id, e.label.clone())))
1384            .expect("a submenu");
1385        builder.fill(token, id);
1386        match next(&mut builder) {
1387            Said::Filled { token: got, id: got_id, children } => {
1388                assert_eq!(got, token);
1389                assert_eq!(got_id, id);
1390                assert!(!children.is_empty(), "`{label}` filled to nothing");
1391            }
1392            _ => panic!("expected the submenu"),
1393        }
1394
1395        // Closing frees the shell's side; a fill against a closed token is still answered,
1396        // and answered emptily, rather than reaching a menu that is gone.
1397        builder.close(token);
1398        builder.fill(token, id);
1399        match next(&mut builder) {
1400            Said::Filled { children, .. } => assert!(children.is_empty()),
1401            _ => panic!("expected the submenu"),
1402        }
1403
1404        let _ = std::fs::remove_dir_all(&dir);
1405    }
1406
1407    /// Where the time in a context menu actually goes.
1408    ///
1409    /// Not an assertion -- a measurement, and the one that decided the design of this
1410    /// file. The numbers it printed are in the note at the top. What to look at: the whole
1411    /// menu against the part the user waits for, which is now only `Live::open` reading the
1412    /// top level, and the submenu fill that a hover pays for instead.
1413    #[test]
1414    #[ignore = "measures the shell; run explicitly, single-threaded, with --nocapture"]
1415    #[cfg(windows)]
1416    fn what_the_shell_menu_takes_to_build() {
1417        let _serialised = crate::shell::serialised();
1418        crate::shell::init();
1419        let (dir, file, sub) = scratch("menu-cost");
1420
1421        for (what, parent, items) in [
1422            ("a text file", dir.clone(), vec![file.clone()]),
1423            ("a folder", dir.clone(), vec![sub.clone()]),
1424            ("empty space", dir.clone(), Vec::new()),
1425            (
1426                r"a folder in C:\",
1427                PathBuf::from(r"C:\"),
1428                vec![PathBuf::from(r"C:\Windows")],
1429            ),
1430        ] {
1431            for round in 1..=3 {
1432                let whole = std::time::Instant::now();
1433                let all = build(&parent, &items);
1434                let whole = whole.elapsed();
1435
1436                let root = std::time::Instant::now();
1437                let Some((mut live, entries)) = super::win::Live::open(&parent, &items, Depth::Full) else {
1438                    continue;
1439                };
1440                let root = root.elapsed();
1441
1442                // Every submenu, one at a time, the way a hover pays for it.
1443                let mut fills = Vec::new();
1444                for entry in &entries {
1445                    if let Some(id) = entry.kind.unasked() {
1446                        let at = std::time::Instant::now();
1447                        let children = live.fill(id);
1448                        fills.push((
1449                            entry.label.clone(),
1450                            children.len(),
1451                            at.elapsed().as_secs_f32() * 1e3,
1452                        ));
1453                    }
1454                }
1455
1456                eprintln!(
1457                    "{what} #{round}: whole menu {:>7.1}ms ({} entries)   top level only \
1458                     {:>7.1}ms ({} entries)",
1459                    whole.as_secs_f32() * 1e3,
1460                    all.len(),
1461                    root.as_secs_f32() * 1e3,
1462                    entries.len()
1463                );
1464                for (label, count, ms) in fills {
1465                    eprintln!("    fill `{label}` {ms:>7.1}ms ({count} children)");
1466                }
1467            }
1468        }
1469
1470        let _ = std::fs::remove_dir_all(&dir);
1471    }
1472
1473    /// The reduced menu has to still be a menu — the shell's own verbs, all of them working.
1474    ///
1475    /// [`Depth::Fast`] exists because on an executable on a share the full query takes
1476    /// twenty-four seconds and this one takes half of one. What it buys is worth nothing if what
1477    /// comes back cannot cut, copy, delete, rename or open. Checked against a local file, where
1478    /// both are fast, because what is being tested is the *content* of the reduced menu; the
1479    /// timings are in the note on [`Depth`].
1480    #[test]
1481    #[cfg(windows)]
1482    fn the_reduced_menu_is_still_a_menu() {
1483        let _serialised = crate::shell::serialised();
1484        crate::shell::init();
1485        let (dir, file, _) = scratch("reduced");
1486
1487        let verbs = |depth: Depth| -> Vec<String> {
1488            let (_live, entries) = super::win::Live::open(&dir, std::slice::from_ref(&file), depth)
1489                .expect("the shell's menu");
1490            entries
1491                .iter()
1492                .filter_map(|e| match &e.kind {
1493                    Kind::Command(Command::Shell { verb: Some(verb), .. }) => {
1494                        Some(verb.to_lowercase())
1495                    }
1496                    _ => None,
1497                })
1498                .collect()
1499        };
1500        let full = verbs(Depth::Full);
1501        let fast = verbs(Depth::Fast);
1502
1503        // Everything anybody actually does to a file. `open` is deliberately in here: it is the
1504        // default verb, and a menu whose default is missing is not a context menu.
1505        for wanted in ["open", "cut", "copy", "delete", "rename", "properties"] {
1506            assert!(
1507                fast.iter().any(|v| v == wanted),
1508                "the reduced menu has no `{wanted}` in it, so it is not usable: {fast:?}"
1509            );
1510        }
1511        // And it really is reduced, or there would be no point to any of this.
1512        assert!(
1513            fast.len() < full.len(),
1514            "the reduced menu came back with as much as the full one ({} against {}), so the \
1515             flags did nothing: {fast:?}",
1516            fast.len(),
1517            full.len()
1518        );
1519        eprintln!("full {} verbs, reduced {} verbs", full.len(), fast.len());
1520
1521        let _ = std::fs::remove_dir_all(&dir);
1522    }
1523
1524    /// A menu the shell is taking for ever over must not be able to hold up the next one.
1525    ///
1526    /// This is the whole reason [`Builder`] spawns a worker per menu. Right-clicking a 14 MB
1527    /// executable on a mapped share puts `QueryContextMenu` inside one extension for
1528    /// twenty-four seconds — measured; see the note on [`Builder`] — and with one thread and a
1529    /// queue, every menu asked for in that time waited behind it. A right click on a local file
1530    /// did nothing at all, which is what it looks like from the outside: the context menu has
1531    /// stopped working.
1532    ///
1533    /// The stall stands in for the share. What is being tested is not the shell's speed but the
1534    /// scheduling: that the second answer arrives while the first build is demonstrably still
1535    /// running, and that the first one's answer never turns up.
1536    #[test]
1537    #[cfg(windows)]
1538    fn a_slow_menu_does_not_hold_up_the_next_one() {
1539        use std::sync::atomic::Ordering;
1540        use std::time::{Duration, Instant};
1541
1542        let _serialised = crate::shell::serialised();
1543        crate::shell::init();
1544        let (dir, file, _) = scratch("cancel");
1545
1546        let ctx = egui::Context::default();
1547        let mut builder = Builder::new(&ctx);
1548
1549        /// Long enough that a real menu built inside it cannot be the stall finishing early,
1550        /// short enough that the abandoned worker is gone before the suite is.
1551        const STALL: u64 = 8_000;
1552        STALLED.store(0, Ordering::SeqCst);
1553        STALL_MS.store(STALL, Ordering::SeqCst);
1554        let slow = builder.build(&dir, std::slice::from_ref(&file), Depth::Full);
1555
1556        // Wait until the worker is really inside it, so what follows is a menu asked for
1557        // during a slow build rather than after one.
1558        let deadline = Instant::now() + Duration::from_secs(5);
1559        while STALLED.load(Ordering::SeqCst) == 0 {
1560            assert!(
1561                Instant::now() < deadline,
1562                "the worker never started the slow build"
1563            );
1564            std::thread::sleep(Duration::from_millis(5));
1565        }
1566        // Cleared only now: the stall is a property of the *first* build, and clearing it before
1567        // that build had picked it up would race it.
1568        STALL_MS.store(0, Ordering::SeqCst);
1569        assert!(builder.busy(), "a build is outstanding and the builder says it is not");
1570        assert!(
1571            builder.poll().is_none(),
1572            "the slow build answered in no time, so it was not slow and this proves nothing"
1573        );
1574
1575        // The second menu, asked for with the first still running.
1576        let at = Instant::now();
1577        let quick = builder.build(&dir, std::slice::from_ref(&file), Depth::Full);
1578        let entries = loop {
1579            if let Some(said) = builder.poll() {
1580                match said {
1581                    Said::Built { token, entries, .. } => {
1582                        assert_ne!(
1583                            token, slow,
1584                            "the abandoned build answered, and its answer was taken"
1585                        );
1586                        assert_eq!(token, quick);
1587                        break entries;
1588                    }
1589                    Said::Filled { .. } => {}
1590                }
1591            }
1592            assert!(
1593                at.elapsed() < Duration::from_millis(STALL / 2),
1594                "the second menu has been {:?} and has not arrived -- it is queued behind the \
1595                 first, which is the bug",
1596                at.elapsed()
1597            );
1598            std::thread::sleep(Duration::from_millis(5));
1599        };
1600        let took = at.elapsed();
1601
1602        assert!(
1603            entries.len() > 3,
1604            "the second menu came back with only {} entries, so it was not really built",
1605            entries.len()
1606        );
1607        assert!(
1608            !builder.busy(),
1609            "the second menu has been delivered and the builder still thinks it is waiting"
1610        );
1611        eprintln!("the second menu took {took:?} while the first had {STALL} ms left to run");
1612
1613        let _ = std::fs::remove_dir_all(&dir);
1614    }
1615
1616    /// Where the time goes on a file the shell has to reach across a network for.
1617    ///
1618    /// `YAFE_PROBE="H:\some\file.exe" cargo test probe_menu_costs -- --ignored --nocapture`
1619    #[test]
1620    #[ignore = "measures the shell against a path of your choosing"]
1621    #[cfg(windows)]
1622    fn probe_menu_costs() {
1623        let Some(path) = std::env::var_os("YAFE_PROBE") else {
1624            eprintln!("set YAFE_PROBE to a file or folder");
1625            return;
1626        };
1627        let _serialised = crate::shell::serialised();
1628        crate::shell::init();
1629        for path in path.to_string_lossy().split(';').filter(|p| !p.is_empty()) {
1630            let path = PathBuf::from(path);
1631            let parent = path.parent().unwrap_or(&path).to_owned();
1632
1633            // The decision in `App::menu_depth` is made on the UI thread before anything slow is
1634            // allowed to happen, so what it costs is part of the claim.
1635            let at = std::time::Instant::now();
1636            let remote = crate::shell::over_network(&parent);
1637            let cold = at.elapsed();
1638            let at = std::time::Instant::now();
1639            let _ = crate::shell::over_network(&parent);
1640            eprintln!(
1641                "  over_network -> {remote}: {:.1} µs uncached, {:.1} µs cached",
1642                cold.as_secs_f64() * 1e6,
1643                at.elapsed().as_secs_f64() * 1e6
1644            );
1645            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
1646            eprintln!("\n--- {} ({} bytes)", path.display(), size);
1647            for _ in 1..=2 {
1648                let at = std::time::Instant::now();
1649                let opened = super::win::Live::open(&parent, std::slice::from_ref(&path), Depth::Full);
1650                let took = at.elapsed().as_secs_f32() * 1e3;
1651                match opened {
1652                    Some((live, entries)) => {
1653                        eprintln!("  Live::open {took:>9.1} ms -> {} entries", entries.len());
1654                        drop(live);
1655                    }
1656                    None => eprintln!("  no menu at all ({took:.1} ms)"),
1657                }
1658            }
1659            super::win::probe(&parent, std::slice::from_ref(&path));
1660            super::win::probe_flags(&parent, std::slice::from_ref(&path));
1661        }
1662    }
1663
1664    /// Invoking a shell command has to actually do it, and the result has to be usable.
1665    ///
1666    /// The menu's own Cut, Copy, Paste, Delete and Rename are the *shell's* entries, so they go
1667    /// through [`invoke`] by canonical verb, on [`crate::shell::Modal`]. Copy is the one to test
1668    /// with, because whether it worked is a fact about the clipboard rather than an opinion.
1669    ///
1670    /// It did not work, and the way it failed is the point. `InvokeCommand` put the file on the
1671    /// clipboard perfectly well -- read from the invoking thread it was right there -- and
1672    /// `IsClipboardFormatAvailable` from anywhere else said the clipboard held no files at all.
1673    /// Clipboard data belongs to the apartment that put it there, and reading it from elsewhere
1674    /// is a call back into that apartment. The modal thread was parked in `recv()` and answered
1675    /// nothing, so a Copy from the context menu was a copy nobody could paste. Hence the wait in
1676    /// `Modal`'s loop; see [`crate::shell::answering_calls`].
1677    ///
1678    /// Goes through the real `Modal` rather than a thread of its own, because a thread of its
1679    /// own is what made this look like it worked: read the clipboard on the thread that wrote it
1680    /// and everything is fine.
1681    #[test]
1682    #[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
1683    #[cfg(windows)]
1684    fn a_shell_verb_from_the_menu_actually_runs() {
1685        let _serialised = crate::shell::serialised();
1686        crate::shell::init();
1687        crate::shell::clipboard::settle_for_tests();
1688
1689        let (dir, file, _) = scratch("verb");
1690
1691        let entries = build(&dir, std::slice::from_ref(&file));
1692        let copy = entries
1693            .iter()
1694            .find_map(|e| match &e.kind {
1695                Kind::Command(command @ Command::Shell { verb: Some(verb), .. })
1696                    if verb.eq_ignore_ascii_case("copy") =>
1697                {
1698                    Some(command.clone())
1699                }
1700                _ => None,
1701            })
1702            .expect("every file's menu has a Copy with a canonical verb");
1703
1704        let ctx = egui::Context::default();
1705        let mut modal = crate::shell::Modal::new(&ctx);
1706        assert!(modal.send(crate::shell::Request::Invoke {
1707            parent: dir.clone(),
1708            items: vec![file.clone()],
1709            command: copy,
1710            owner: crate::shell::Owner::default(),
1711        }));
1712
1713        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
1714        while modal.poll().is_none() {
1715            assert!(
1716                std::time::Instant::now() < deadline,
1717                "the modal thread never came back"
1718            );
1719            std::thread::sleep(std::time::Duration::from_millis(10));
1720        }
1721
1722        assert!(
1723            crate::shell::clipboard::has_files(),
1724            "the shell's Copy ran and the clipboard says it holds no files -- the thread that \
1725             owns them is not answering for them"
1726        );
1727        let on_clipboard = crate::shell::clipboard::get()
1728            .expect("and it has to read back, since that is what a paste does");
1729        assert_eq!(on_clipboard.items.len(), 1, "{:?}", on_clipboard.items);
1730        assert!(
1731            on_clipboard.items[0]
1732                .to_string_lossy()
1733                .to_lowercase()
1734                .ends_with("one.txt"),
1735            "{:?}",
1736            on_clipboard.items
1737        );
1738
1739        crate::shell::clipboard::clear();
1740        let _ = std::fs::remove_dir_all(&dir);
1741    }
1742
1743    #[test]
1744    fn the_right_drag_entries_are_the_only_ones_of_our_own() {
1745        // Everything else this program used to put above the shell's menu is gone; these three
1746        // are here because Windows has no answer for "a right-button drag just landed".
1747        assert_eq!(Own::CopyHere.label(), "Copy here");
1748        assert_eq!(Own::MoveHere.label(), "Move here");
1749        assert_eq!(Own::Cancel.label(), "Cancel");
1750        let entry = Entry::own(Own::CopyHere);
1751        assert!(entry.enabled);
1752        assert!(!entry.checked && !entry.default);
1753        assert!(
1754            entry.shortcut.is_empty(),
1755            "a drop answer is not on a shortcut"
1756        );
1757    }
1758
1759    #[test]
1760    #[cfg(windows)]
1761    fn shell_labels_lose_their_ampersands_and_keep_their_shortcuts() {
1762        use super::win::split_label;
1763        assert_eq!(split_label("&Open"), ("Open".to_owned(), String::new()));
1764        assert_eq!(
1765            split_label("Cu&t\tCtrl+X"),
1766            ("Cut".to_owned(), "Ctrl+X".to_owned())
1767        );
1768        // `&&` is a literal ampersand, which "Scan && Repair" depends on.
1769        assert_eq!(
1770            split_label("Scan && Repair"),
1771            ("Scan & Repair".to_owned(), String::new())
1772        );
1773        assert_eq!(split_label(""), (String::new(), String::new()));
1774    }
1775
1776    /// The chain that actually breaks: paths, the parent folder, the shell's
1777    /// `IContextMenu`, a populated `HMENU`, and reading it back into entries.
1778    #[test]
1779    #[cfg(windows)]
1780    fn the_shell_fills_a_menu_that_reads_back() {
1781        let _serialised = crate::shell::serialised();
1782        crate::shell::init();
1783
1784        let mut dir = std::env::temp_dir();
1785        dir.push(format!("yafe-menu-{}", std::process::id()));
1786        std::fs::create_dir_all(&dir).expect("temp dir");
1787        let file = dir.join("one.txt");
1788        std::fs::write(&file, b"x").expect("write");
1789
1790        let entries = build(&dir, std::slice::from_ref(&file));
1791        assert!(
1792            entries.len() > 3,
1793            "the shell offered only {} entries -- Open, Cut, Copy, Delete, Rename and \
1794             Properties should all be there at a minimum",
1795            entries.len()
1796        );
1797
1798        // Every entry has to be drawable and doable.
1799        for entry in &entries {
1800            match &entry.kind {
1801                Kind::Separator => {}
1802                Kind::Submenu { children, .. } => {
1803                    assert!(!children.is_empty(), "{}", entry.label)
1804                }
1805                Kind::Command(_) => assert!(!entry.label.is_empty()),
1806            }
1807            assert!(
1808                !entry.label.contains('&') || entry.label.matches('&').count() == 1,
1809                "`{}` still has an accelerator marker in it",
1810                entry.label
1811            );
1812            assert!(!entry.label.contains('\t'), "`{}`", entry.label);
1813        }
1814
1815        // The commands worth having should be recognisable by verb.
1816        let verbs: Vec<String> = entries
1817            .iter()
1818            .filter_map(|e| match &e.kind {
1819                Kind::Command(Command::Shell { verb, .. }) => verb.clone(),
1820                _ => None,
1821            })
1822            .collect();
1823        assert!(
1824            verbs.iter().any(|v| v.eq_ignore_ascii_case("properties")),
1825            "no Properties verb among {verbs:?}"
1826        );
1827        assert!(
1828            verbs.iter().any(|v| v.eq_ignore_ascii_case("copy")),
1829            "no Copy verb among {verbs:?}"
1830        );
1831
1832        // And the folder's own menu, which is where New and Paste live.
1833        let background = build(&dir, &[]);
1834        assert!(
1835            background.len() > 3,
1836            "the background menu had only {} entries",
1837            background.len()
1838        );
1839
1840        let _ = std::fs::remove_dir_all(&dir);
1841    }
1842
1843    #[test]
1844    #[cfg(windows)]
1845    fn submenus_are_populated_rather_than_left_empty() {
1846        let _serialised = crate::shell::serialised();
1847        crate::shell::init();
1848        let mut dir = std::env::temp_dir();
1849        dir.push(format!("yafe-submenu-{}", std::process::id()));
1850        std::fs::create_dir_all(&dir).expect("temp dir");
1851        let file = dir.join("one.txt");
1852        std::fs::write(&file, b"x").expect("write");
1853
1854        let entries = build(&dir, std::slice::from_ref(&file));
1855        let submenus: Vec<&Entry> = entries
1856            .iter()
1857            .filter(|e| matches!(e.kind, Kind::Submenu { .. }))
1858            .collect();
1859        // A plain text file on any Windows has at least "Open with" or "Send to".
1860        assert!(
1861            !submenus.is_empty(),
1862            "no submenu came back at all, which means `WM_INITMENUPOPUP` is not reaching \
1863             the extensions: {:?}",
1864            entries.iter().map(|e| &e.label).collect::<Vec<_>>()
1865        );
1866        for submenu in submenus {
1867            if let Kind::Submenu { children, .. } = &submenu.kind {
1868                assert!(
1869                    !children.is_empty(),
1870                    "`{}` came back empty -- the submenu was never asked to fill itself",
1871                    submenu.label
1872                );
1873            }
1874        }
1875
1876        let _ = std::fs::remove_dir_all(&dir);
1877    }
1878}
