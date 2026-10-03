//! Directory reads, off the UI thread, with a cache in front of them.
//!
//! Three things together are what make navigation feel like it has no latency:
//!
//! 1. **A synchronous cache probe.** [`Loader::cached`] answers from memory before
//!    a request is ever made, so Back, Forward, and clicking into a folder you
//!    were just in are not asynchronous at all — the listing is on screen in the
//!    same frame as the click, with no spinner and no flash of empty.
//! 2. **Workers, for the rest.** A cold read of a network share can take a
//!    second; it does not get to hold up a repaint. Results arrive by channel and
//!    wake the UI with [`egui::Context::request_repaint`].
//! 3. **Prefetch.** Moving the selection onto a folder queues it at low priority,
//!    so by the time Enter is pressed the answer is usually already cached.
//!
//! Stale results are dropped by token rather than cancelled: a scan is cheap
//! enough that racing to stop one costs more than letting it finish, and a tab
//! that has moved on simply ignores an answer to a question it no longer asks.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as Atomic};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use crate::fs::drives::{self, Drive};
use crate::fs::{scan, Dir};

/// A finished scan, tagged with the request it answers.
pub struct Loaded {
    pub token: u64,
    pub dir: Arc<Dir>,
}

/// How badly a request is wanted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Priority {
    /// Someone is looking at a blank pane waiting for this.
    Navigation,
    /// A guess that it will be wanted soon.
    Prefetch,
}

struct Request {
    token: u64,
    path: PathBuf,
    priority: Priority,
}

/// The work queue: a deque so navigation can jump the prefetches already in it.
struct Queue {
    pending: Mutex<VecDeque<Request>>,
    /// Paths already queued or being scanned, so a prefetch storm — one per
    /// arrow-key press — does not enqueue the same folder forty times.
    claimed: Mutex<HashSet<PathBuf>>,
    wake: Condvar,
    shutdown: AtomicBool,
}

/// Recently read directories, newest use last.
struct Cache {
    dirs: HashMap<PathBuf, Arc<Dir>>,
    /// Least-recently-used first.
    order: VecDeque<PathBuf>,
    /// Total entries held, which is the number that actually costs memory — a
    /// cache of 60 small folders and one of 60 huge ones are not the same thing.
    entries: usize,
    /// When each was last read, for [`Cache::sweep`].
    used: HashMap<PathBuf, std::time::Instant>,
    /// When the sweep last ran.
    swept: std::time::Instant,
}

/// What the cache may hold, in entries — the number that actually costs memory, since a
/// cache of 60 small folders and one of 60 huge ones are not the same thing.
///
/// An entry costs its 32-byte record plus its name, which measured **140 bytes** over
/// `C:\Windows\WinSxS` (27,636 entries, 3.8 MB) — a folder whose names are long. So 150,000
/// is about 21 MB at that rate and 7 MB at the ~50 bytes an ordinary folder costs.
///
/// It was 1,200,000, which is 160 MB of the same folder — most of a memory budget spent on
/// listings nobody is going to look at again. The point of the cache is that Back, Up and
/// stepping back into the folder you just left are instant, and that is a working set of a
/// dozen folders.
const CACHE_ENTRY_BUDGET: usize = 150_000;
/// Even if they are all tiny, stop somewhere.
///
/// 96 held 1.6 MB of *empty* arenas before [`Dir`] learned to shrink to what it holds; it now
/// costs what it contains, and 32 folders is still deeper than anyone walks back.
const CACHE_DIR_BUDGET: usize = 32;

/// How long a listing nothing has asked for is kept.
///
/// So that a session which browsed a hundred folders and then settled on one gives the
/// memory back rather than holding its high-water mark until the window closes. A tab still
/// showing a folder keeps its own listing alive through an `Arc` regardless of this, which is
/// exactly right: what is on screen is not stale.
const CACHE_STALE: std::time::Duration = std::time::Duration::from_secs(120);
/// How often the sweep is worth running.
const CACHE_SWEEP: std::time::Duration = std::time::Duration::from_secs(10);

impl Cache {
    fn get(&mut self, path: &Path) -> Option<Arc<Dir>> {
        let dir = self.dirs.get(path)?.clone();
        // Touch: move to the back of the eviction order.
        if let Some(at) = self.order.iter().position(|p| p == path) {
            let owned = self.order.remove(at).expect("index came from this deque");
            self.order.push_back(owned);
        }
        self.used.insert(path.to_path_buf(), std::time::Instant::now());
        Some(dir)
    }

    /// Drop anything nothing has read in [`CACHE_STALE`], at most every [`CACHE_SWEEP`].
    fn sweep(&mut self) {
        let now = std::time::Instant::now();
        if now.duration_since(self.swept) < CACHE_SWEEP {
            return;
        }
        self.swept = now;
        let stale: Vec<PathBuf> = self
            .used
            .iter()
            .filter(|(_, used)| now.duration_since(**used) >= CACHE_STALE)
            .map(|(path, _)| path.clone())
            .collect();
        for path in stale {
            self.forget(&path);
        }
    }

    fn insert(&mut self, dir: Arc<Dir>) {
        let path = dir.path.clone();
        if let Some(old) = self.dirs.insert(path.clone(), dir.clone()) {
            self.entries -= old.len();
            if let Some(at) = self.order.iter().position(|p| *p == path) {
                self.order.remove(at);
            }
        }
        self.entries += dir.len();
        self.used.insert(path.clone(), std::time::Instant::now());
        self.order.push_back(path);

        while (self.entries > CACHE_ENTRY_BUDGET || self.order.len() > CACHE_DIR_BUDGET)
            && self.order.len() > 1
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.dirs.remove(&oldest) {
                self.entries -= evicted.len();
            }
            self.used.remove(&oldest);
        }
    }

    fn forget(&mut self, path: &Path) {
        if let Some(old) = self.dirs.remove(path) {
            self.entries -= old.len();
        }
        if let Some(at) = self.order.iter().position(|p| p == path) {
            self.order.remove(at);
        }
        self.used.remove(path);
    }
}

/// The scanning service. One per application.
pub struct Loader {
    queue: Arc<Queue>,
    cache: Arc<Mutex<Cache>>,
    results: Receiver<Loaded>,
    /// The other end of `results`, so a one-off scan of its own can answer through the
    /// same channel the workers do. See [`Loader::request_deep`].
    answers: Sender<Loaded>,
    /// Only ever used to wake the window when an answer lands.
    ctx: egui::Context,
    workers: Vec<std::thread::JoinHandle<()>>,
    next_token: u64,
}

impl Loader {
    /// Start the workers. The context is only used to wake the UI when a scan
    /// lands, which is what lets egui stay idle in between.
    pub fn new(ctx: &egui::Context) -> Self {
        let queue = Arc::new(Queue {
            pending: Mutex::new(VecDeque::new()),
            claimed: Mutex::new(HashSet::new()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let cache = Arc::new(Mutex::new(Cache {
            dirs: HashMap::new(),
            order: VecDeque::new(),
            entries: 0,
            used: HashMap::new(),
            swept: std::time::Instant::now(),
        }));
        let (tx, results) = channel();

        // Directory reads are kernel- and disk-bound, not compute-bound, so more
        // threads than this buys nothing and costs context switches. Four is
        // enough to keep a slow network share from blocking three local folders.
        let count = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 4))
            .unwrap_or(2);

        let workers = (0..count)
            .map(|i| {
                let queue = queue.clone();
                let cache = cache.clone();
                let tx = tx.clone();
                let ctx = ctx.clone();
                std::thread::Builder::new()
                    .name(format!("scan-{i}"))
                    .spawn(move || worker(queue, cache, tx, ctx))
                    .expect("the OS refused a thread")
            })
            .collect();

        Self {
            queue,
            cache,
            results,
            answers: tx,
            ctx: ctx.clone(),
            workers,
            next_token: 1,
        }
    }

    /// A listing already in memory, if there is one. Never touches the disk.
    pub fn cached(&self, path: &Path) -> Option<Arc<Dir>> {
        self.cache.lock().ok()?.get(path)
    }

    /// How much the cache is holding: folders, and entries across them.
    ///
    /// For `--trace`. "Memory grows as I browse" has two answers — the cache filling to its
    /// budget, or something that never lets go — and they are told apart by watching this
    /// against the process's own figure. A cache sitting at its cap while the process keeps
    /// climbing is not the cache.
    pub fn held(&self) -> (usize, usize) {
        self.cache
            .lock()
            .map(|cache| (cache.order.len(), cache.entries))
            .unwrap_or_default()
    }

    /// Ask for a listing. The returned token identifies the answer.
    pub fn request(&mut self, path: &Path) -> u64 {
        let token = self.next_token;
        self.next_token += 1;
        self.enqueue(Request {
            token,
            path: path.to_path_buf(),
            priority: Priority::Navigation,
        });
        token
    }

    /// Ask for a folder and everything under it, flattened into one listing.
    ///
    /// Off the queue, on a thread of its own, and **not cached**. Three deliberate
    /// differences from every other read in this program, and the same reason behind all
    /// of them: a flatten is a query over a tree rather than a listing of a folder.
    ///
    /// - *Not queued*, because it is the one read here that can take seconds. There are
    ///   two to four workers; two flattens in the pool would leave one worker for every
    ///   other folder in the window, so a tree walk must not be allowed to occupy one.
    /// - *Not cached*, because it is the largest listing the program can hold and the
    ///   most quickly wrong — anything created anywhere under the root invalidates it —
    ///   and because turning the button off and on again is a request to look afresh.
    ///   The folder's own shallow listing stays cached, so turning it *off* is instant.
    /// - *Detached*, like [`Volumes::probe`]: there is nothing useful to do with a walk
    ///   that is still going when the window closes, and joining it would hold the
    ///   process open for as long as the walk had left to run.
    pub fn request_deep(&mut self, path: &Path) -> u64 {
        let token = self.next_token;
        self.next_token += 1;
        let path = path.to_path_buf();
        let answers = self.answers.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("flatten".to_owned())
            .spawn(move || {
                scan::silence_device_dialogs();
                let dir = Arc::new(scan::scan_deep(
                    &path,
                    scan::FLATTEN_BUDGET,
                    scan::FLATTEN_PATIENCE,
                ));
                if answers.send(Loaded { token, dir }).is_ok() {
                    ctx.request_repaint();
                }
            });
        // A machine that will not give us a thread leaves the tab waiting for a token
        // that will never arrive, which is what the *next* toggle or navigation clears.
        // Nothing else in the window is affected, which is the best that can be done
        // here without inventing a second failure path for one impossible case.
        let _ = spawned;
        token
    }

    /// Read a folder that has not been asked for yet, in case it is about to be.
    ///
    /// Does nothing if it is already cached or already queued, which is what makes
    /// this safe to call from a selection change.
    pub fn prefetch(&mut self, path: &Path) {
        if self.cached(path).is_some() {
            return;
        }
        let token = self.next_token;
        self.next_token += 1;
        self.enqueue(Request {
            token,
            path: path.to_path_buf(),
            priority: Priority::Prefetch,
        });
    }

    /// Drop a cached listing, so the next request re-reads it. For Refresh.
    pub fn invalidate(&self, path: &Path) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.forget(path);
        }
    }

    /// Every scan that has finished since the last call.
    pub fn drain(&self) -> impl Iterator<Item = Loaded> + '_ {
        self.results.try_iter()
    }

    /// Let go of listings nothing has asked for in a while.
    ///
    /// Called once a frame and costs a lock and a clock read unless ten seconds have gone by.
    pub fn sweep(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.sweep();
        }
    }

    fn enqueue(&self, request: Request) {
        {
            let Ok(mut claimed) = self.queue.claimed.lock() else {
                return;
            };
            // A navigation for a path already being prefetched still has to be
            // enqueued — the prefetch's token is not the one the tab is waiting
            // for, and its result would be discarded as stale.
            let already = !claimed.insert(request.path.clone());
            if already && request.priority == Priority::Prefetch {
                return;
            }
        }
        if let Ok(mut pending) = self.queue.pending.lock() {
            match request.priority {
                Priority::Navigation => pending.push_front(request),
                Priority::Prefetch => {
                    // Keep the backlog from growing without bound when the user
                    // holds an arrow key down: the oldest guesses are the least
                    // likely to still be right.
                    while pending.len() > 32 {
                        pending.pop_back();
                    }
                    pending.push_back(request);
                }
            }
        }
        self.queue.wake.notify_one();
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // The flag is set *while holding the queue lock*, which is what makes the
        // handshake sound. A worker checks `shutdown` with the lock held and then
        // calls `wait`, which releases it; setting the flag without the lock leaves a
        // window where the notify lands before the worker is waiting for it, and the
        // worker then sleeps for ever on a queue nothing will ever push to.
        {
            let _held = self.queue.pending.lock();
            self.queue.shutdown.store(true, Atomic::Release);
        }
        self.queue.wake.notify_all();
        for worker in self.workers.drain(..) {
            // A worker parked in a slow `FindFirstFile` on a dead network share
            // cannot be interrupted, and blocking the window's close on it would
            // be worse than letting the process exit around it.
            let _ = worker.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Volumes
// ---------------------------------------------------------------------------

/// The drive list, with the slow half done in the background.
///
/// [`crate::fs::drives::list_letters`] is instant, so the sidebar has rows on the
/// first frame. [`crate::fs::drives::describe`] is not — a mapped share that is not
/// currently reachable takes tens of seconds — so each volume is described on its own
/// thread and merged in when it answers. One stalled share therefore delays one row
/// rather than the window.
pub struct Volumes {
    drives: Vec<Drive>,
    arrivals: Receiver<Drive>,
    /// Kept so the channel stays open while probes are still running, and so a
    /// refresh can replace it.
    sender: Sender<Drive>,
}

impl Volumes {
    /// List the letters now and start describing them.
    pub fn new(ctx: &egui::Context) -> Self {
        let (sender, arrivals) = channel();
        let volumes = Self {
            drives: drives::list_letters(),
            arrivals,
            sender,
        };
        volumes.probe(ctx);
        volumes
    }

    /// Re-list and re-describe. For F5, and for the window regaining focus after a
    /// drive may have been plugged in or ejected.
    pub fn refresh(&mut self, ctx: &egui::Context) {
        drives::forget_all();
        let fresh = drives::list_letters();
        // Keep whatever is already known for letters that are still there, so a
        // refresh does not blank every bar for as long as the probes take.
        self.drives = fresh
            .into_iter()
            .map(|drive| {
                self.drives
                    .iter()
                    .find(|d| d.letter == drive.letter && d.described)
                    .cloned()
                    .unwrap_or(drive)
            })
            .collect();
        // A fresh channel, so answers from the previous round cannot arrive after it.
        let (sender, arrivals) = channel();
        self.sender = sender;
        self.arrivals = arrivals;
        self.probe(ctx);
    }

    /// Merge in anything that has been described since the last frame.
    pub fn poll(&mut self) {
        while let Ok(described) = self.arrivals.try_recv() {
            if let Some(existing) = self
                .drives
                .iter_mut()
                .find(|d| d.letter == described.letter)
            {
                *existing = described;
            }
        }
    }

    pub fn all(&self) -> &[Drive] {
        &self.drives
    }

    fn probe(&self, ctx: &egui::Context) {
        for drive in &self.drives {
            if drive.described {
                continue;
            }
            let mut drive = drive.clone();
            let sender = self.sender.clone();
            let ctx = ctx.clone();
            // Detached: there is nothing useful to do with a probe that is still
            // waiting on SMB when the window closes, and joining it would hold the
            // process open for the whole timeout.
            let spawned = std::thread::Builder::new()
                .name(format!("volume-{}", drive.letter))
                .spawn(move || {
                    scan::silence_device_dialogs();
                    drives::describe(&mut drive);
                    if sender.send(drive).is_ok() {
                        ctx.request_repaint();
                    }
                });
            // A machine that will not give us a thread still gets a usable sidebar,
            // just without the labels.
            let _ = spawned;
        }
    }
}

fn worker(
    queue: Arc<Queue>,
    cache: Arc<Mutex<Cache>>,
    results: Sender<Loaded>,
    ctx: egui::Context,
) {
    // Per-thread, and the reason an empty card reader does not open a modal.
    scan::silence_device_dialogs();

    loop {
        let request = {
            let Ok(mut pending) = queue.pending.lock() else {
                return;
            };
            loop {
                if queue.shutdown.load(Atomic::Acquire) {
                    return;
                }
                if let Some(request) = pending.pop_front() {
                    break request;
                }
                let Ok(next) = queue.wake.wait(pending) else {
                    return;
                };
                pending = next;
            }
        };

        // Someone may have read it between the request and now.
        let dir = match cache.lock().ok().and_then(|mut c| c.get(&request.path)) {
            Some(hit) => hit,
            None => {
                let dir = Arc::new(scan::scan(&request.path));
                // A failed read is not cached: the drive may come back, the share
                // may reconnect, and a retry should actually retry.
                if dir.error.is_none() {
                    if let Ok(mut cache) = cache.lock() {
                        cache.insert(dir.clone());
                    }
                }
                dir
            }
        };

        if let Ok(mut claimed) = queue.claimed.lock() {
            claimed.remove(&request.path);
        }

        if results
            .send(Loaded {
                token: request.token,
                dir,
            })
            .is_err()
        {
            return; // The UI is gone.
        }
        ctx.request_repaint();
    }
}
