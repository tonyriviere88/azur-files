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
use std::sync::mpsc::Sender;

use crate::fs::dir::Sync;
use crate::fs::Dir;
use crate::service::Service;

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
pub struct Cloud(Service<Request, Answer>);

impl Cloud {
    pub fn new(ctx: &egui::Context) -> Self {
        // An apartment, because the property store is COM.
        Self(Service::new(ctx, "shell-cloud", true, |request, wanted, tx, ctx| {
            // Caught for the reason `shell::thumbs` gives: the property read runs the provider's
            // handler, which is somebody else's code on this thread.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ask(request, wanted, tx, ctx)
            }));
        }))
    }

    /// Ask about the entries of a listing, for one view of it: the folders first, since they are the
    /// rows with nothing to show until this answers.
    pub fn request(&mut self, view: u64, dir: &Dir) {
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
        self.0.request(view, request);
    }

    /// Which views are on screen.
    pub fn only(&self, views: &[u64]) {
        self.0.only(views);
    }

    /// Every answer that has arrived since the last call.
    pub fn drain(&self) -> impl Iterator<Item = Answer> + '_ {
        self.0.drain()
    }

    /// Whether a capture should keep waiting: something asked and not yet finished with.
    #[cfg(any(test, feature = "devtools"))]
    pub fn pending(&self) -> bool {
        self.0.pending()
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
