1//! Directory reads, off the UI thread, with a cache in front of them.
2//!
3//! Three things together are what make navigation feel like it has no latency:
4//!
5//! 1. **A synchronous cache probe.** [`Loader::cached`] answers from memory before
6//!    a request is ever made, so Back, Forward, and clicking into a folder you
7//!    were just in are not asynchronous at all — the listing is on screen in the
8//!    same frame as the click, with no spinner and no flash of empty.
9//! 2. **Workers, for the rest.** A cold read of a network share can take a
10//!    second; it does not get to hold up a repaint. Results arrive by channel and
11//!    wake the UI with [`egui::Context::request_repaint`].
12//! 3. **Prefetch.** Moving the selection onto a folder queues it at low priority,
13//!    so by the time Enter is pressed the answer is usually already cached.
14//!
15//! Stale results are dropped by token rather than cancelled: a scan is cheap
16//! enough that racing to stop one costs more than letting it finish, and a tab
17//! that has moved on simply ignores an answer to a question it no longer asks.
18
19use std::collections::{HashMap, HashSet, VecDeque};
20use std::path::{Path, PathBuf};
21use std::sync::atomic::{AtomicBool, Ordering as Atomic};
22use std::sync::mpsc::{channel, Receiver, Sender};
23use std::sync::{Arc, Condvar, Mutex};
24
25use crate::fs::drives::{self, Drive};
26use crate::fs::{scan, Dir};
27
28/// A finished scan, tagged with the request it answers.
29pub struct Loaded {
30    pub token: u64,
31    pub dir: Arc<Dir>,
32}
33
34/// How badly a request is wanted.
35#[derive(Clone, Copy, PartialEq, Eq, Debug)]
36enum Priority {
37    /// Someone is looking at a blank pane waiting for this.
38    Navigation,
39    /// A guess that it will be wanted soon.
40    Prefetch,
41}
42
43struct Request {
44    token: u64,
45    path: PathBuf,
46    priority: Priority,
47}
48
49/// The work queue: a deque so navigation can jump the prefetches already in it.
50struct Queue {
51    pending: Mutex<VecDeque<Request>>,
52    /// Paths already queued or being scanned, so a prefetch storm — one per
53    /// arrow-key press — does not enqueue the same folder forty times.
54    claimed: Mutex<HashSet<PathBuf>>,
55    wake: Condvar,
56    shutdown: AtomicBool,
57}
58
59/// Recently read directories, newest use last.
60struct Cache {
61    dirs: HashMap<PathBuf, Arc<Dir>>,
62    /// Least-recently-used first.
63    order: VecDeque<PathBuf>,
64    /// Total entries held, which is the number that actually costs memory — a
65    /// cache of 60 small folders and one of 60 huge ones are not the same thing.
66    entries: usize,
67    /// When each was last read, for [`Cache::sweep`].
68    used: HashMap<PathBuf, std::time::Instant>,
69    /// When the sweep last ran.
70    swept: std::time::Instant,
71}
72
73/// What the cache may hold, in entries — the number that actually costs memory, since a
74/// cache of 60 small folders and one of 60 huge ones are not the same thing.
75///
76/// An entry costs its 32-byte record plus its name, which measured **140 bytes** over
77/// `C:\Windows\WinSxS` (27,636 entries, 3.8 MB) — a folder whose names are long. So 150,000
78/// is about 21 MB at that rate and 7 MB at the ~50 bytes an ordinary folder costs.
79///
80/// It was 1,200,000, which is 160 MB of the same folder — most of a memory budget spent on
81/// listings nobody is going to look at again. The point of the cache is that Back, Up and
82/// stepping back into the folder you just left are instant, and that is a working set of a
83/// dozen folders.
84const CACHE_ENTRY_BUDGET: usize = 150_000;
85/// Even if they are all tiny, stop somewhere.
86///
87/// 96 held 1.6 MB of *empty* arenas before [`Dir`] learned to shrink to what it holds; it now
88/// costs what it contains, and 32 folders is still deeper than anyone walks back.
89const CACHE_DIR_BUDGET: usize = 32;
90
91/// How long a listing nothing has asked for is kept.
92///
93/// So that a session which browsed a hundred folders and then settled on one gives the
94/// memory back rather than holding its high-water mark until the window closes. A tab still
95/// showing a folder keeps its own listing alive through an `Arc` regardless of this, which is
96/// exactly right: what is on screen is not stale.
97const CACHE_STALE: std::time::Duration = std::time::Duration::from_secs(120);
98/// How often the sweep is worth running.
99const CACHE_SWEEP: std::time::Duration = std::time::Duration::from_secs(10);
100
101impl Cache {
102    fn get(&mut self, path: &Path) -> Option<Arc<Dir>> {
103        let dir = self.dirs.get(path)?.clone();
104        // Touch: move to the back of the eviction order.
105        if let Some(at) = self.order.iter().position(|p| p == path) {
106            let owned = self.order.remove(at).expect("index came from this deque");
107            self.order.push_back(owned);
108        }
109        self.used.insert(path.to_path_buf(), std::time::Instant::now());
110        Some(dir)
111    }
112
113    /// Drop anything nothing has read in [`CACHE_STALE`], at most every [`CACHE_SWEEP`].
114    fn sweep(&mut self) {
115        let now = std::time::Instant::now();
116        if now.duration_since(self.swept) < CACHE_SWEEP {
117            return;
118        }
119        self.swept = now;
120        let stale: Vec<PathBuf> = self
121            .used
122            .iter()
123            .filter(|(_, used)| now.duration_since(**used) >= CACHE_STALE)
124            .map(|(path, _)| path.clone())
125            .collect();
126        for path in stale {
127            self.forget(&path);
128        }
129    }
130
131    fn insert(&mut self, dir: Arc<Dir>) {
132        let path = dir.path.clone();
133        if let Some(old) = self.dirs.insert(path.clone(), dir.clone()) {
134            self.entries -= old.len();
135            if let Some(at) = self.order.iter().position(|p| *p == path) {
136                self.order.remove(at);
137            }
138        }
139        self.entries += dir.len();
140        self.used.insert(path.clone(), std::time::Instant::now());
141        self.order.push_back(path);
142
143        while (self.entries > CACHE_ENTRY_BUDGET || self.order.len() > CACHE_DIR_BUDGET)
144            && self.order.len() > 1
145        {
146            let Some(oldest) = self.order.pop_front() else {
147                break;
148            };
149            if let Some(evicted) = self.dirs.remove(&oldest) {
150                self.entries -= evicted.len();
151            }
152            self.used.remove(&oldest);
153        }
154    }
155
156    fn forget(&mut self, path: &Path) {
157        if let Some(old) = self.dirs.remove(path) {
158            self.entries -= old.len();
159        }
160        if let Some(at) = self.order.iter().position(|p| p == path) {
161            self.order.remove(at);
162        }
163        self.used.remove(path);
164    }
165}
166
167/// The scanning service. One per application.
168pub struct Loader {
169    queue: Arc<Queue>,
170    cache: Arc<Mutex<Cache>>,
171    results: Receiver<Loaded>,
172    /// The other end of `results`, so a one-off scan of its own can answer through the
173    /// same channel the workers do. See [`Loader::request_deep`].
174    answers: Sender<Loaded>,
175    /// Only ever used to wake the window when an answer lands.
176    ctx: egui::Context,
177    workers: Vec<std::thread::JoinHandle<()>>,
178    next_token: u64,
179}
180
181impl Loader {
182    /// Start the workers. The context is only used to wake the UI when a scan
183    /// lands, which is what lets egui stay idle in between.
184    pub fn new(ctx: &egui::Context) -> Self {
185        let queue = Arc::new(Queue {
186            pending: Mutex::new(VecDeque::new()),
187            claimed: Mutex::new(HashSet::new()),
188            wake: Condvar::new(),
189            shutdown: AtomicBool::new(false),
190        });
191        let cache = Arc::new(Mutex::new(Cache {
192            dirs: HashMap::new(),
193            order: VecDeque::new(),
194            entries: 0,
195            used: HashMap::new(),
196            swept: std::time::Instant::now(),
197        }));
198        let (tx, results) = channel();
199
200        // Directory reads are kernel- and disk-bound, not compute-bound, so more
201        // threads than this buys nothing and costs context switches. Four is
202        // enough to keep a slow network share from blocking three local folders.
203        let count = std::thread::available_parallelism()
204            .map(|n| n.get().clamp(2, 4))
205            .unwrap_or(2);
206
207        let workers = (0..count)
208            .map(|i| {
209                let queue = queue.clone();
210                let cache = cache.clone();
211                let tx = tx.clone();
212                let ctx = ctx.clone();
213                std::thread::Builder::new()
214                    .name(format!("scan-{i}"))
215                    .spawn(move || worker(queue, cache, tx, ctx))
216                    .expect("the OS refused a thread")
217            })
218            .collect();
219
220        Self {
221            queue,
222            cache,
223            results,
224            answers: tx,
225            ctx: ctx.clone(),
226            workers,
227            next_token: 1,
228        }
229    }
230
231    /// A listing already in memory, if there is one. Never touches the disk.
232    pub fn cached(&self, path: &Path) -> Option<Arc<Dir>> {
233        self.cache.lock().ok()?.get(path)
234    }
235
236    /// How much the cache is holding: folders, and entries across them.
237    ///
238    /// For `--trace`. "Memory grows as I browse" has two answers — the cache filling to its
239    /// budget, or something that never lets go — and they are told apart by watching this
240    /// against the process's own figure. A cache sitting at its cap while the process keeps
241    /// climbing is not the cache.
242    pub fn held(&self) -> (usize, usize) {
243        self.cache
244            .lock()
245            .map(|cache| (cache.order.len(), cache.entries))
246            .unwrap_or_default()
247    }
248
249    /// Ask for a listing. The returned token identifies the answer.
250    pub fn request(&mut self, path: &Path) -> u64 {
251        let token = self.next_token;
252        self.next_token += 1;
253        self.enqueue(Request {
254            token,
255            path: path.to_path_buf(),
256            priority: Priority::Navigation,
257        });
258        token
259    }
260
261    /// Ask for a folder and everything under it, flattened into one listing.
262    ///
263    /// Off the queue, on a thread of its own, and **not cached**. Three deliberate
264    /// differences from every other read in this program, and the same reason behind all
265    /// of them: a flatten is a query over a tree rather than a listing of a folder.
266    ///
267    /// - *Not queued*, because it is the one read here that can take seconds. There are
268    ///   two to four workers; two flattens in the pool would leave one worker for every
269    ///   other folder in the window, so a tree walk must not be allowed to occupy one.
270    /// - *Not cached*, because it is the largest listing the program can hold and the
271    ///   most quickly wrong — anything created anywhere under the root invalidates it —
272    ///   and because turning the button off and on again is a request to look afresh.
273    ///   The folder's own shallow listing stays cached, so turning it *off* is instant.
274    /// - *Detached*, like [`Volumes::probe`]: there is nothing useful to do with a walk
275    ///   that is still going when the window closes, and joining it would hold the
276    ///   process open for as long as the walk had left to run.
277    pub fn request_deep(&mut self, path: &Path) -> u64 {
278        let token = self.next_token;
279        self.next_token += 1;
280        let path = path.to_path_buf();
281        let answers = self.answers.clone();
282        let ctx = self.ctx.clone();
283        let spawned = std::thread::Builder::new()
284            .name("flatten".to_owned())
285            .spawn(move || {
286                scan::silence_device_dialogs();
287                let dir = Arc::new(scan::scan_deep(
288                    &path,
289                    scan::FLATTEN_BUDGET,
290                    scan::FLATTEN_PATIENCE,
291                ));
292                if answers.send(Loaded { token, dir }).is_ok() {
293                    ctx.request_repaint();
294                }
295            });
296        // A machine that will not give us a thread leaves the tab waiting for a token
297        // that will never arrive, which is what the *next* toggle or navigation clears.
298        // Nothing else in the window is affected, which is the best that can be done
299        // here without inventing a second failure path for one impossible case.
300        let _ = spawned;
301        token
302    }
303
304    /// Read a folder that has not been asked for yet, in case it is about to be.
305    ///
306    /// Does nothing if it is already cached or already queued, which is what makes
307    /// this safe to call from a selection change.
308    pub fn prefetch(&mut self, path: &Path) {
309        if self.cached(path).is_some() {
310            return;
311        }
312        let token = self.next_token;
313        self.next_token += 1;
314        self.enqueue(Request {
315            token,
316            path: path.to_path_buf(),
317            priority: Priority::Prefetch,
318        });
319    }
320
321    /// Drop a cached listing, so the next request re-reads it. For Refresh.
322    pub fn invalidate(&self, path: &Path) {
323        if let Ok(mut cache) = self.cache.lock() {
324            cache.forget(path);
325        }
326    }
327
328    /// Every scan that has finished since the last call.
329    pub fn drain(&self) -> impl Iterator<Item = Loaded> + '_ {
330        self.results.try_iter()
331    }
332
333    /// Let go of listings nothing has asked for in a while.
334    ///
335    /// Called once a frame and costs a lock and a clock read unless ten seconds have gone by.
336    pub fn sweep(&self) {
337        if let Ok(mut cache) = self.cache.lock() {
338            cache.sweep();
339        }
340    }
341
342    fn enqueue(&self, request: Request) {
343        {
344            let Ok(mut claimed) = self.queue.claimed.lock() else {
345                return;
346            };
347            // A navigation for a path already being prefetched still has to be
348            // enqueued — the prefetch's token is not the one the tab is waiting
349            // for, and its result would be discarded as stale.
350            let already = !claimed.insert(request.path.clone());
351            if already && request.priority == Priority::Prefetch {
352                return;
353            }
354        }
355        if let Ok(mut pending) = self.queue.pending.lock() {
356            match request.priority {
357                Priority::Navigation => pending.push_front(request),
358                Priority::Prefetch => {
359                    // Keep the backlog from growing without bound when the user
360                    // holds an arrow key down: the oldest guesses are the least
361                    // likely to still be right.
362                    while pending.len() > 32 {
363                        pending.pop_back();
364                    }
365                    pending.push_back(request);
366                }
367            }
368        }
369        self.queue.wake.notify_one();
370    }
371}
372
373impl Drop for Loader {
374    fn drop(&mut self) {
375        // The flag is set *while holding the queue lock*, which is what makes the
376        // handshake sound. A worker checks `shutdown` with the lock held and then
377        // calls `wait`, which releases it; setting the flag without the lock leaves a
378        // window where the notify lands before the worker is waiting for it, and the
379        // worker then sleeps for ever on a queue nothing will ever push to.
380        {
381            let _held = self.queue.pending.lock();
382            self.queue.shutdown.store(true, Atomic::Release);
383        }
384        self.queue.wake.notify_all();
385        for worker in self.workers.drain(..) {
386            // A worker parked in a slow `FindFirstFile` on a dead network share
387            // cannot be interrupted, and blocking the window's close on it would
388            // be worse than letting the process exit around it.
389            let _ = worker.join();
390        }
391    }
392}
393
394// ---------------------------------------------------------------------------
395// Volumes
396// ---------------------------------------------------------------------------
397
398/// The drive list, with the slow half done in the background.
399///
400/// [`crate::fs::drives::list_letters`] is instant, so the sidebar has rows on the
401/// first frame. [`crate::fs::drives::describe`] is not — a mapped share that is not
402/// currently reachable takes tens of seconds — so each volume is described on its own
403/// thread and merged in when it answers. One stalled share therefore delays one row
404/// rather than the window.
405pub struct Volumes {
406    drives: Vec<Drive>,
407    arrivals: Receiver<Drive>,
408    /// Kept so the channel stays open while probes are still running, and so a
409    /// refresh can replace it.
410    sender: Sender<Drive>,
411}
412
413impl Volumes {
414    /// List the letters now and start describing them.
415    pub fn new(ctx: &egui::Context) -> Self {
416        let (sender, arrivals) = channel();
417        let volumes = Self {
418            drives: drives::list_letters(),
419            arrivals,
420            sender,
421        };
422        volumes.probe(ctx);
423        volumes
424    }
425
426    /// Re-list and re-describe. For F5, and for the window regaining focus after a
427    /// drive may have been plugged in or ejected.
428    pub fn refresh(&mut self, ctx: &egui::Context) {
429        drives::forget_all();
430        let fresh = drives::list_letters();
431        // Keep whatever is already known for letters that are still there, so a
432        // refresh does not blank every bar for as long as the probes take.
433        self.drives = fresh
434            .into_iter()
435            .map(|drive| {
436                self.drives
437                    .iter()
438                    .find(|d| d.letter == drive.letter && d.described)
439                    .cloned()
440                    .unwrap_or(drive)
441            })
442            .collect();
443        // A fresh channel, so answers from the previous round cannot arrive after it.
444        let (sender, arrivals) = channel();
445        self.sender = sender;
446        self.arrivals = arrivals;
447        self.probe(ctx);
448    }
449
450    /// Merge in anything that has been described since the last frame.
451    pub fn poll(&mut self) {
452        while let Ok(described) = self.arrivals.try_recv() {
453            if let Some(existing) = self
454                .drives
455                .iter_mut()
456                .find(|d| d.letter == described.letter)
457            {
458                *existing = described;
459            }
460        }
461    }
462
463    pub fn all(&self) -> &[Drive] {
464        &self.drives
465    }
466
467    fn probe(&self, ctx: &egui::Context) {
468        for drive in &self.drives {
469            if drive.described {
470                continue;
471            }
472            let mut drive = drive.clone();
473            let sender = self.sender.clone();
474            let ctx = ctx.clone();
475            // Detached: there is nothing useful to do with a probe that is still
476            // waiting on SMB when the window closes, and joining it would hold the
477            // process open for the whole timeout.
478            let spawned = std::thread::Builder::new()
479                .name(format!("volume-{}", drive.letter))
480                .spawn(move || {
481                    scan::silence_device_dialogs();
482                    drives::describe(&mut drive);
483                    if sender.send(drive).is_ok() {
484                        ctx.request_repaint();
485                    }
486                });
487            // A machine that will not give us a thread still gets a usable sidebar,
488            // just without the labels.
489            let _ = spawned;
490        }
491    }
492}
493
494fn worker(
495    queue: Arc<Queue>,
496    cache: Arc<Mutex<Cache>>,
497    results: Sender<Loaded>,
498    ctx: egui::Context,
499) {
500    // Per-thread, and the reason an empty card reader does not open a modal.
501    scan::silence_device_dialogs();
502
503    loop {
504        let request = {
505            let Ok(mut pending) = queue.pending.lock() else {
506                return;
507            };
508            loop {
509                if queue.shutdown.load(Atomic::Acquire) {
510                    return;
511                }
512                if let Some(request) = pending.pop_front() {
513                    break request;
514                }
515                let Ok(next) = queue.wake.wait(pending) else {
516                    return;
517                };
518                pending = next;
519            }
520        };
521
522        // Someone may have read it between the request and now.
523        let dir = match cache.lock().ok().and_then(|mut c| c.get(&request.path)) {
524            Some(hit) => hit,
525            None => {
526                let dir = Arc::new(scan::scan(&request.path));
527                // A failed read is not cached: the drive may come back, the share
528                // may reconnect, and a retry should actually retry.
529                if dir.error.is_none() {
530                    if let Ok(mut cache) = cache.lock() {
531                        cache.insert(dir.clone());
532                    }
533                }
534                dir
535            }
536        };
537
538        if let Ok(mut claimed) = queue.claimed.lock() {
539            claimed.remove(&request.path);
540        }
541
542        if results
543            .send(Loaded {
544                token: request.token,
545                dir,
546            })
547            .is_err()
548        {
549            return; // The UI is gone.
550        }
551        ctx.request_repaint();
552    }
553}
