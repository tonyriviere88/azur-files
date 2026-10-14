//! The shape every per-view background service here shares: a question asked when a listing lands,
//! answered on one worker thread, addressed to the [`crate::pane::Tab::view`] that asked, and
//! dropped once that view has gone.
//!
//! Two halves, because not every service wants both:
//!
//! - [`LiveViews`] is the set of views on screen, which is what lets a worker skip a question about a
//!   folder nobody is looking at any more. The icon and thumbnail services keep one of these and run
//!   their own workers, since what they ask and how they queue it is their own.
//! - [`Service`] is the rest of it — the channel each way, the worker started on first use, the count
//!   a capture waits on — for the services that are one question per listing and nothing else:
//!   [`crate::shell::cloud`] and [`crate::hardlinks`]. Each keeps its own choice of rows and its own
//!   way of asking; this is only the plumbing they used to write out twice.

use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// The views on screen, shared between the asker and its worker; clone it into the worker.
///
/// A poisoned lock answers every view as wanted: a question asked for nothing is cheaper than a
/// service that has stopped answering.
#[derive(Clone, Default)]
pub struct LiveViews(Arc<Mutex<HashSet<u64>>>);

impl LiveViews {
    /// Which views are on screen. Called every frame, so an unchanged set is left as it is.
    pub fn set(&self, views: &[u64]) {
        if let Ok(mut live) = self.0.lock() {
            if live.len() == views.len() && views.iter().all(|view| live.contains(view)) {
                return;
            }
            live.clear();
            live.extend(views.iter().copied());
        }
    }

    /// One more view on screen: the one that is asking, whatever [`Self::set`] last heard.
    pub fn add(&self, view: u64) {
        if let Ok(mut live) = self.0.lock() {
            live.insert(view);
        }
    }

    /// Whether a question for `view` is still worth answering.
    pub fn has(&self, view: u64) -> bool {
        self.0.lock().map(|live| live.contains(&view)).unwrap_or(true)
    }
}

/// What a service's worker does with one request: work through it, sending answers on the channel,
/// and stop as soon as the view it is for is no longer wanted.
pub type Work<Q, A> = fn(&Q, &dyn Fn(u64) -> bool, &Sender<A>, &egui::Context);

/// One question per listing, answered on one thread, started the first time it is needed — a session
/// that never asks never has the thread.
pub struct Service<Q, A> {
    ctx: egui::Context,
    /// The worker thread's name.
    name: &'static str,
    /// Whether the worker joins an apartment before its first request. Only where the work needs
    /// COM: a thread that never calls it has no business holding one.
    com: bool,
    work: Work<Q, A>,
    jobs: Option<Sender<Q>>,
    tx: Sender<A>,
    answers: Receiver<A>,
    /// The views on screen. A request for any other is skipped, and one being worked through stops.
    live: LiveViews,
    /// Requests sent and not yet finished with, for [`Service::pending`].
    queued: crate::inflight::InFlight,
}

impl<Q: Send + 'static, A: Send + 'static> Service<Q, A> {
    pub fn new(ctx: &egui::Context, name: &'static str, com: bool, work: Work<Q, A>) -> Self {
        let (tx, answers) = channel();
        Self {
            ctx: ctx.clone(),
            name,
            com,
            work,
            jobs: None,
            tx,
            answers,
            live: LiveViews::default(),
            queued: Default::default(),
        }
    }

    /// Send one request on behalf of `view`.
    pub fn request(&mut self, view: u64, request: Q) {
        // A view that is asking is on screen, whatever `only` last heard — the frame that lands a
        // listing asks before it reports what is showing.
        self.live.add(view);
        self.queued.sent();
        if self.worker().send(request).is_err() {
            self.queued.done();
        }
    }

    /// Which views are on screen.
    pub fn only(&self, views: &[u64]) {
        self.live.set(views);
    }

    /// Every answer that has arrived since the last call.
    pub fn drain(&self) -> impl Iterator<Item = A> + '_ {
        self.answers.try_iter()
    }

    /// Whether a capture should keep waiting: something asked and not yet finished with.
    #[cfg(any(test, feature = "devtools"))]
    pub fn pending(&self) -> bool {
        self.queued.any()
    }

    fn worker(&mut self) -> &Sender<Q> {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let live = self.live.clone();
        let queued = self.queued.clone();
        let (com, work) = (self.com, self.work);
        let name = self.name;
        self.jobs.get_or_insert_with(|| {
            let (send, receive) = channel::<Q>();
            let _ = std::thread::Builder::new()
                .name(name.to_owned())
                .spawn(move || {
                    if com {
                        crate::shell::init();
                    }
                    crate::fs::scan::silence_device_dialogs();
                    let wanted = |view: u64| live.has(view);
                    while let Ok(request) = receive.recv() {
                        work(&request, &wanted, &tx, &ctx);
                        queued.done();
                        // The last batch may have been empty, and a capture waiting on `pending`
                        // needs a frame to see that it is done.
                        ctx.request_repaint();
                    }
                });
            send
        })
    }
}
