//! The worker queue that [`crate::loader`], [`crate::sizes`] and [`crate::git`] each hand their
//! work to, and the threads that drain it.
//!
//! What is shared is only the plumbing: a deque behind a lock, a condition variable to sleep on,
//! and a flag that tells the workers to stop. What each service does to the deque — navigation
//! jumping prefetches, subdirectories going on the front, stale questions dropping off the back of
//! a short backlog — stays with the service, handed in as a closure to [`WorkQueue::push_one`] and
//! its siblings.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering as Atomic};
use std::sync::{Condvar, Mutex};

/// Work waiting for a pool of threads, and the flag that sends them home.
pub(crate) struct WorkQueue<T> {
    pending: Mutex<VecDeque<T>>,
    wake: Condvar,
    /// [`Atomic::SeqCst`] on both sides. It is read and written only with `pending` held, so the
    /// lock already orders it and the ordering here is never the thing being relied on.
    shutdown: AtomicBool,
}

impl<T> WorkQueue<T> {
    pub(crate) fn new() -> Self {
        Self {
            pending: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }

    /// The next piece of work, waiting for one if there is none.
    ///
    /// `None` once [`Self::close`] has been called, or if the lock was poisoned — either way the
    /// worker asking has nothing left to do but return.
    pub(crate) fn next(&self) -> Option<T> {
        let mut pending = self.pending.lock().ok()?;
        loop {
            if self.shutdown.load(Atomic::SeqCst) {
                return None;
            }
            if let Some(work) = pending.pop_front() {
                return Some(work);
            }
            pending = self.wake.wait(pending).ok()?;
        }
    }

    /// Change the queue without waking anyone, for work that is only being taken away.
    pub(crate) fn edit(&self, change: impl FnOnce(&mut VecDeque<T>)) {
        if let Ok(mut pending) = self.pending.lock() {
            change(&mut pending);
        }
    }

    /// Add work, and wake one worker to take it.
    pub(crate) fn push_one(&self, change: impl FnOnce(&mut VecDeque<T>)) {
        self.edit(change);
        self.wake.notify_one();
    }

    /// Add work, and wake every worker — for more than one piece at a time.
    pub(crate) fn push_all(&self, change: impl FnOnce(&mut VecDeque<T>)) {
        self.edit(change);
        self.wake.notify_all();
    }

    /// Tell every worker to stop, at its next look at the queue.
    pub(crate) fn close(&self) {
        // The flag is set *while holding the queue lock*, which is what makes the handshake sound.
        // A worker checks `shutdown` with the lock held and then calls `wait`, which releases it;
        // setting the flag without the lock leaves a window where the notify lands before the
        // worker is waiting for it, and the worker then sleeps for ever on a queue nothing will
        // ever push to.
        {
            let _held = self.pending.lock();
            self.shutdown.store(true, Atomic::SeqCst);
        }
        self.wake.notify_all();
    }
}

/// Start `count` threads named `name-0`, `name-1`…, each running what `start` hands back.
///
/// `start` is called once per thread on the calling one, which is where each worker takes its own
/// clones of whatever it shares.
pub(crate) fn spawn<F>(
    name: &str,
    count: usize,
    mut start: impl FnMut() -> F,
) -> Vec<std::thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    (0..count)
        .map(|i| {
            std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(start())
                .expect("the OS refused a thread")
        })
        .collect()
}
