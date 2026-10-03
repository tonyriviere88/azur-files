//! What the sync provider says about the entries of a synced folder: the Status column's folders.
//!
//! [`crate::fs::dir::Sync`] has the split. A file's state is in its attributes and costs nothing; a
//! folder's is the provider's summary of what is under it, which OneDrive publishes through the
//! shell's property store as `System.StorageProviderState` — the property Explorer's own Status
//! column shows — and nowhere else. Its attributes are `0x410` whether everything under it is on the
//! disk or none of it is.
//!
//! # Asked once per view, off the UI thread
//!
//! The same shape as [`crate::git`]: the question is asked when a synced listing lands, the answers
//! are addressed to that [`crate::pane::Tab::view`], and a view that has gone finds nothing to land
//! on. Nothing is keyed by path and nothing outlives the listing it describes.
//!
//! **Every entry, not only the folders.** The attributes are a file's answer on the frame the listing
//! lands, but only the provider knows about an upload still pending or a file it could not sync, so
//! the files are asked too — after the folders, which are the rows that have nothing to show until
//! this answers. A property read goes to the provider's own handler, which is why none of it is done
//! on the UI thread, and why the answers are sent in small batches: a big folder fills in from the
//! top rather than all at once at the end.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::fs::dir::Sync;
use crate::fs::Dir;

#[cfg(windows)]
#[path = "../windows/cloud.rs"]
mod win;

/// How many answers go back in one message. Small enough that the rows on screen fill in within a
/// frame or two of each other, large enough that a folder of thousands is not thousands of repaints.
const BATCH: usize = 64;

/// The most entries of one folder that are asked about. A folder bigger than this still has every
/// file's answer from its attributes; only the provider's extra word on the rest goes unasked.
const LIMIT: usize = 20_000;

/// One folder's worth of asking.
struct Request {
    view: u64,
    dir: PathBuf,
    /// Entry index and name, folders first.
    rows: Vec<(u32, String)>,
}

/// Some of the answers for one view: entry index and state. Rows the provider has nothing to say
/// about are left out.
pub struct Answer {
    pub view: u64,
    pub states: Vec<(u32, Sync)>,
}

/// Asks the sync provider about synced folders, one thread, started the first time it is needed —
/// a session that never opens a synced folder never has it.
pub struct Cloud {
    ctx: egui::Context,
    jobs: Option<Sender<Request>>,
    tx: Sender<Answer>,
    answers: Receiver<Answer>,
    /// The views on screen. A request for any other is skipped, and one being worked through stops.
    live: Arc<Mutex<Vec<u64>>>,
    /// Requests sent and not yet finished with, for [`Cloud::pending`].
    queued: Arc<AtomicUsize>,
}

impl Cloud {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, answers) = channel();
        Self {
            ctx: ctx.clone(),
            jobs: None,
            tx,
            answers,
            live: Arc::new(Mutex::new(Vec::new())),
            queued: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Ask about the entries of a listing, for one view of it: the folders first, since they are the
    /// rows with nothing to show until this answers.
    pub fn request(&mut self, view: u64, dir: &Dir) {
        // A view that is asking is on screen, whatever `only` last heard — the frame that lands a
        // listing asks before it reports what is showing.
        if let Ok(mut live) = self.live.lock() {
            if !live.contains(&view) {
                live.push(view);
            }
        }
        let (folders, files): (Vec<usize>, Vec<usize>) =
            (0..dir.len()).partition(|&i| dir.entries[i].is_dir());
        let rows = folders
            .into_iter()
            .chain(files)
            .take(LIMIT)
            .map(|i| (i as u32, dir.name(i).to_owned()))
            .collect();
        let request = Request {
            view,
            dir: dir.path.clone(),
            rows,
        };
        self.queued.fetch_add(1, Ordering::Relaxed);
        if self.worker().send(request).is_err() {
            self.queued.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Which views are on screen.
    pub fn only(&self, views: &[u64]) {
        if let Ok(mut live) = self.live.lock() {
            if live.as_slice() != views {
                live.clear();
                live.extend_from_slice(views);
            }
        }
    }

    /// Every answer that has arrived since the last call.
    pub fn drain(&self) -> impl Iterator<Item = Answer> + '_ {
        self.answers.try_iter()
    }

    /// Whether a capture should keep waiting: something asked and not yet finished with.
    pub fn pending(&self) -> bool {
        self.queued.load(Ordering::Relaxed) > 0
    }

    fn worker(&mut self) -> &Sender<Request> {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let live = self.live.clone();
        let queued = self.queued.clone();
        self.jobs.get_or_insert_with(|| {
            let (send, receive) = channel::<Request>();
            let _ = std::thread::Builder::new()
                .name("shell-cloud".to_owned())
                .spawn(move || {
                    crate::shell::init();
                    crate::fs::scan::silence_device_dialogs();
                    let wanted = |view: u64| live.lock().map(|l| l.contains(&view)).unwrap_or(true);
                    while let Ok(request) = receive.recv() {
                        // Caught for the reason `shell::thumbs` gives: the property read runs the
                        // provider's handler, which is somebody else's code on this thread.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            ask(&request, &wanted, &tx, &ctx)
                        }));
                        queued.fetch_sub(1, Ordering::Relaxed);
                        // The last batch may have been empty, and a capture waiting on `pending`
                        // needs a frame to see that it is done.
                        ctx.request_repaint();
                    }
                });
            send
        })
    }
}

/// Work through one request, sending the answers a batch at a time, and stop as soon as nobody is
/// looking at the view any more.
#[cfg(windows)]
fn ask(request: &Request, wanted: &dyn Fn(u64) -> bool, tx: &Sender<Answer>, ctx: &egui::Context) {
    if !wanted(request.view) {
        return;
    }
    let Some(parent) = win::Parent::open(&request.dir) else {
        return;
    };
    for chunk in request.rows.chunks(BATCH) {
        if !wanted(request.view) {
            return;
        }
        let states: Vec<(u32, Sync)> = chunk
            .iter()
            .filter_map(|(index, name)| parent.state(name).map(|state| (*index, state)))
            .collect();
        if states.is_empty() {
            continue;
        }
        if tx.send(Answer { view: request.view, states }).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Off Windows there is no sync provider to ask, and every row keeps what its attributes said.
#[cfg(not(windows))]
fn ask(_: &Request, _: &dyn Fn(u64) -> bool, _: &Sender<Answer>, _: &egui::Context) {}
