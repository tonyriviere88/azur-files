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
        // **Detached, not joined**, and that is the whole of this function's cost.
        //
        // A worker parked in a slow call on a dead network share cannot be interrupted: the flag
        // above is read between requests, and a thread 200 ms into a 21-second `NetShareEnum` will
        // not look at it again until the redirector gives up. Joining meant the process outlived its
        // own window by that timeout — measured at **22.4 seconds** from clicking close to the
        // process going away, against 1.5 s once nothing waits. That is the freeze this comment
        // used to describe while the code did the opposite of it.
        //
        // Dropping the handles is safe because a worker borrows nothing from here: the queue and the
        // cache are `Arc`s it holds its own clones of, `send` on a dead receiver is an error it
        // already handles, and the scan reads rather than writes, so a thread the process exit cuts
        // off mid-syscall leaves nothing half-done. Same reason [`Volumes::probe`] has always
        // detached its volume probes.
        self.workers.clear();
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
///
/// Two lists, because the panel shows them as two groups and nothing else about them differs:
/// [`crate::fs::drives::list_shares`] is the volumes with no drive letter, which are as instant
/// to list and as slow to describe as the lettered ones. Both go through the same probe and the
/// same channel.
pub struct Volumes {
    drives: Vec<Drive>,
    /// The network locations with no letter of their own. See [`Volumes::shares`].
    shares: Vec<Drive>,
    /// The machines those connections are to. See [`Volumes::servers`].
    servers: Vec<std::path::PathBuf>,
    /// Machines that are there but not connected to, from either of the two things that can find
    /// one: a browse ([`Volumes::discover`]) and a probe of a dropped connection
    /// ([`Volumes::confirm`]). One list, because the row they earn is the same row — see
    /// [`Volumes::found`].
    found: Vec<std::path::PathBuf>,
    /// Whether a browse is in flight, so the button can say so. **A probe is not a browse** and
    /// deliberately does not touch this: a confirmation landing while the button is spinning would
    /// pop it back up with the browse still running.
    finding: bool,
    discovered: Receiver<Vec<std::path::PathBuf>>,
    discoveries: Sender<Vec<std::path::PathBuf>>,
    /// Machines whose dropped connection turned out to still answer, one per probe thread. Its own
    /// channel rather than a second sender on `discoveries`, for the reason on `finding` above.
    confirmed: Receiver<std::path::PathBuf>,
    confirmations: Sender<std::path::PathBuf>,
    arrivals: Receiver<Drive>,
    /// Kept so the channel stays open while probes are still running, and so a
    /// refresh can replace it.
    sender: Sender<Drive>,
}

impl Volumes {
    /// List the letters and the network locations now, and start describing them.
    pub fn new(ctx: &egui::Context) -> Self {
        let (sender, arrivals) = channel();
        let (discoveries, discovered) = channel();
        let (confirmations, confirmed) = channel();
        let volumes = Self {
            drives: drives::list_letters(),
            shares: drives::list_shares(),
            servers: drives::list_servers(),
            // Empty on this frame, and nothing here presses the browse button: that one reaches the
            // network to ask about other people's machines, and opening a window does not ask for
            // that. What [`Volumes::confirm`] fills it with is the opposite question — machines this
            // program had its own connections to, asked whether they still answer.
            found: Vec::new(),
            finding: false,
            discovered,
            discoveries,
            confirmed,
            confirmations,
            arrivals,
            sender,
        };
        volumes.probe(ctx);
        volumes.confirm(ctx);
        volumes
    }

    /// Re-list and re-describe. For F5, and for the window regaining focus after a
    /// drive may have been plugged in or ejected — or, for the network half, after a share
    /// has been connected or dropped in another window, which is the same kind of event.
    pub fn refresh(&mut self, ctx: &egui::Context) {
        drives::forget_all();
        // Keep whatever is already known for volumes that are still there, so a
        // refresh does not blank every bar for as long as the probes take.
        let keep = |had: &[Drive], fresh: Vec<Drive>| -> Vec<Drive> {
            fresh
                .into_iter()
                .map(|drive| {
                    had.iter()
                        .find(|d| d.path == drive.path && d.described)
                        .cloned()
                        .unwrap_or(drive)
                })
                .collect()
        };
        self.drives = keep(&self.drives, drives::list_letters());
        self.shares = keep(&self.shares, drives::list_shares());
        // A machine has no description to lose, so this is rebuilt outright rather than merged.
        self.servers = drives::list_servers();
        // A fresh channel, so answers from the previous round cannot arrive after it.
        let (sender, arrivals) = channel();
        self.sender = sender;
        self.arrivals = arrivals;
        self.probe(ctx);
        // **The confirmation channel is not replaced**, unlike the one above. A description is
        // about a volume that is still in the list and a late one would land on the wrong round; a
        // confirmation says "this machine answered a moment ago", which is exactly the claim its row
        // makes and is no less true for arriving after an F5. See [`Volumes::found`].
        self.confirm(ctx);
    }

    /// Merge in anything that has been described since the last frame.
    pub fn poll(&mut self) {
        // A browse that has answered. **Merged, not replaced.**
        //
        // Replacing was the first design and it was wrong in the way that matters: a browse is a
        // snapshot of what happened to be announcing itself in the seconds it ran, and WSD and SSDP
        // are chatty rather than reliable — the same machine is found by one press and missed by the
        // next. Replacing meant a second press could empty the list, so a machine found once and
        // then not found again vanished from the panel while still being perfectly reachable.
        //
        // So a machine found in this session stays for the session. Nothing about it is a claim that
        // it is *there* — that is what its ink says, and what clicking it settles.
        while let Ok(found) = self.discovered.try_recv() {
            for machine in found {
                self.remember_found(machine);
            }
            self.finding = false;
        }
        // A dropped connection whose machine still answers. Merged the same way and into the same
        // list, because it earns the same row — and **without touching `finding`**, which belongs to
        // the browse button alone.
        while let Ok(machine) = self.confirmed.try_recv() {
            self.remember_found(machine);
        }
        while let Ok(described) = self.arrivals.try_recv() {
            // By path, which is the key both lists share — every network location has an empty
            // letter, so a letter-keyed lookup would put the first share's answer on all of them.
            if let Some(existing) = self
                .drives
                .iter_mut()
                .chain(self.shares.iter_mut())
                .find(|d| d.path == described.path)
            {
                *existing = described;
            }
        }
    }

    /// Add a machine to the found list unless it is already there. See [`drives::holds`] for why the
    /// comparison is the one it is.
    fn remember_found(&mut self, machine: std::path::PathBuf) {
        if !drives::holds(&self.found, &machine) {
            self.found.push(machine);
        }
    }

    pub fn all(&self) -> &[Drive] {
        &self.drives
    }

    /// The network locations with no drive letter — what the Network group in the sidebar shows.
    ///
    /// Separate from [`Volumes::all`] rather than appended to it because they are a separate
    /// group in the panel, and because a row with no letter among rows named after theirs would
    /// have to explain itself.
    pub fn shares(&self) -> &[Drive] {
        &self.shares
    }

    /// The machines to browse — `\\fileserver` and the like.
    ///
    /// Not `Drive`s, and deliberately: a machine has no capacity, no label and nothing to
    /// describe, so a row for one is a name and an icon. What it *does* have is shares, and those
    /// are a listing rather than sidebar rows — see [`crate::fs::drives::server_dir`], which is
    /// what asking a machine costs and why nothing here does it.
    pub fn servers(&self) -> &[std::path::PathBuf] {
        &self.servers
    }

    /// Read the connection table again, because something has just changed it.
    ///
    /// Called when a sign-in succeeds and when a listing on a machine comes back — the two moments a
    /// connection appears that the panel has no other way to hear about, since the list is otherwise
    /// only rebuilt by [`Volumes::refresh`] on F5 and on regaining focus. Costs what
    /// [`crate::fs::drives::list_servers`] costs, which is a local table and no I/O.
    ///
    /// **The list is what is connected now, and nothing more.** An earlier version kept every
    /// machine that had ever answered for the rest of the session, to survive Windows pruning an
    /// idle deviceless connection out from under a machine that still worked. That is the wrong
    /// trade: it also kept the row for a machine that had genuinely gone away, in the full ink that
    /// says *this is connected*. A row that disappears when the connection does is the honest one,
    /// and a machine that is merely *there* has the browse button and its muted rows to say so.
    pub fn relist(&mut self) {
        self.servers = drives::list_servers();
    }

    /// Machines that are there, which this one is not connected to.
    ///
    /// Kept apart from [`Volumes::servers`] because the two are not equally certain: a server in
    /// that list is one this machine has a live connection to, and one in this list has only been
    /// *seen*. The panel says that difference in the ink rather than with a badge, and clicking a
    /// row is what settles it either way.
    ///
    /// Two things put a machine here, and they are not equally strong:
    ///
    /// | source | evidence | when |
    /// | --- | --- | --- |
    /// | [`Volumes::confirm`] | a TCP handshake on the SMB port, to a machine this program had a connection to | startup and F5 |
    /// | [`Volumes::discover`] | an SSDP or WSD announcement, and nothing more | the browse button |
    ///
    /// One list all the same, because the *row* is the same: a machine that is there and is not
    /// connected. Grading the ink twice over would be asking the panel to explain a distinction
    /// that clicking either row resolves in the same way.
    ///
    /// **A machine found in this session stays for the session**, whichever found it — see
    /// [`Volumes::poll`] for why replacing rather than merging was wrong.
    pub fn found(&self) -> &[std::path::PathBuf] {
        &self.found
    }

    /// Whether a browse is still going.
    pub fn finding(&self) -> bool {
        self.finding
    }

    /// Put machines in the found list without going to the network.
    ///
    /// For the tests that are about the *rows* — that a found machine is drawn a step down the ink
    /// ladder and that clicking one opens it. The browse itself takes 14 seconds and asks the
    /// network about other people's computers, neither of which belongs in a test run.
    #[cfg(test)]
    pub(crate) fn pretend_found(&mut self, machines: Vec<std::path::PathBuf>) {
        self.found = machines;
    }

    /// Put a machine on the wire [`Volumes::confirm`] reports down, so a test can drive the merge
    /// without a machine to probe.
    ///
    /// **Through the real channel and not by writing `found`**, which is what [`pretend_found`] does
    /// and would skip the very step this is for. Needed because the state it stands in for — a
    /// connection Windows says is down whose machine is nevertheless answering — is one this machine
    /// may simply not be in, and usually is not.
    #[cfg(test)]
    pub(crate) fn pretend_confirmed(&mut self, machine: std::path::PathBuf) {
        let _ = self.confirmations.send(machine);
    }

    /// Pretend a browse is in flight, so a test can show that a confirmation landing does not put
    /// the button back up. Starting one for real would take 14 seconds and reach the network.
    #[cfg(test)]
    pub(crate) fn pretend_finding(&mut self) {
        self.finding = true;
    }

    /// Go and look for machines on the network. **Only ever from the button.**
    ///
    /// Not from startup, not from focus, not from F5 — see [`Volumes::refresh`], which deliberately
    /// leaves what was found alone. This is the one thing in this struct that goes to the network
    /// rather than reading a local table, it takes as long as the network takes, and it answers
    /// about other people's computers: all three are reasons for it to happen when it is asked for
    /// and not otherwise.
    ///
    /// A second press while one is still running is ignored rather than queued.
    pub fn discover(&mut self, ctx: &egui::Context) {
        if self.finding {
            return;
        }
        self.finding = true;
        let discoveries = self.discoveries.clone();
        let ctx = ctx.clone();
        // Detached, like a volume probe: a browse still going when the window closes has nothing
        // useful left to say.
        let spawned = std::thread::Builder::new()
            .name("network-browse".to_owned())
            .spawn(move || {
                let found = drives::discover::machines();
                if discoveries.send(found).is_ok() {
                    ctx.request_repaint();
                }
            });
        // A machine that will not give us a thread simply finds nothing, and the button comes back
        // up rather than staying pressed for ever.
        if spawned.is_err() {
            self.finding = false;
        }
    }

    /// Ask each machine whose connection has dropped whether it still answers.
    ///
    /// **The row this restores is one that used to vanish.** A connection Windows reports as down is
    /// filtered out before the panel ever sees it — it has to be, or a laptop that slept would show a
    /// Network group full of rows in the full ink that says *connected* — and the machine went with
    /// it. But a dropped connection is weak evidence about a *machine*: what dropped may have been
    /// the VPN, the sleep, or Windows pruning an idle deviceless connection out from under a server
    /// that never moved. So the machine is asked, and if it answers it comes back as a found row.
    ///
    /// **On the startup path, and that is not the contradiction it looks like** next to
    /// [`Volumes::discover`], which is forbidden there. A browse asks *who is out there*, costs
    /// 14.3 seconds because a multicast group has no way to say "that was everyone", and answers
    /// about other people's computers. This asks whether a **named** machine out of this machine's
    /// own connection table still answers: closed, so it costs one round trip — 3 ms warm, 53 ms
    /// cold — and it is nobody else's business but this program's. See
    /// [`crate::fs::drives::reachable`] for the measurements and for why a socket to 445 cannot trip
    /// the 22-second reconnect a volume query would.
    ///
    /// One detached thread each, like [`Volumes::probe`] beside it and for the same two reasons: a
    /// machine that has genuinely gone away holds its own thread for a second and nothing else, and a
    /// probe still waiting when the window closes has nothing left to say. Only the machines that
    /// answer ever report, so a dead one costs a thread and no row.
    fn confirm(&self, ctx: &egui::Context) {
        for machine in drives::down_servers() {
            // The name out of the path, since that is what a socket needs. `down_servers` only ever
            // yields `\\host`, so this cannot fail — but reading it back rather than carrying the
            // string keeps `\\host` the one form the found list holds.
            let Some(host) = drives::unc_server(&machine) else {
                continue;
            };
            let confirmations = self.confirmations.clone();
            let ctx = ctx.clone();
            // Named after the machine, which is the only place this name is ever read.
            let _ = std::thread::Builder::new()
                .name(format!("reach-{host}"))
                .spawn(move || {
                    // **Nothing is sent for a machine that did not answer.** An absent row is
                    // already the right rendering of "not there", so there is no verdict to report
                    // and no state for one to go stale in.
                    if drives::reachable(&host) && confirmations.send(machine).is_ok() {
                        ctx.request_repaint();
                    }
                });
            // A machine that will not give us a thread simply gets no row, which is exactly what it
            // had before this stage existed. There is nothing to undo and nobody to tell.
        }
    }

    fn probe(&self, ctx: &egui::Context) {
        for drive in self.drives.iter().chain(self.shares.iter()) {
            if drive.described {
                continue;
            }
            let mut drive = drive.clone();
            let sender = self.sender.clone();
            let ctx = ctx.clone();
            // Named after the volume rather than its letter, since a network location has not
            // got one — and a thread called `volume-` with nothing after it says nothing in a
            // debugger, which is the only place this name is ever read.
            let name = if drive.letter.is_empty() {
                format!("volume-{}", drive.path.display())
            } else {
                format!("volume-{}", drive.letter)
            };
            // Detached: there is nothing useful to do with a probe that is still
            // waiting on SMB when the window closes, and joining it would hold the
            // process open for the whole timeout.
            let spawned = std::thread::Builder::new()
                .name(name)
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
