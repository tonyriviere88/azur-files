//! Requests sent to a worker and not yet answered, counted two ways for two different reasons.
//!
//! - [`InFlight`] is counted only where something waits on it. What waits is a capture run: `--shot`
//!   has to hold off until the cloud and hard-link columns are filled in, or it photographs blank
//!   cells. Nothing in a shipped window reads the number — a cell that has no answer yet draws as one
//!   and the answer repaints it — so there `InFlight` is empty and every method on it is nothing at
//!   all, and the call sites read the same in either build.
//! - [`Capped`] is counted in every build, because the shipped window does read it: it is the bound on
//!   a worker's queue, so a folder of ten thousand rows asking at once cannot queue ten thousand
//!   questions. A request turned away is simply asked again on a later frame.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A count shared between the asker and its worker thread; clone it into the worker.
#[derive(Clone, Default)]
pub struct InFlight(#[cfg(any(test, feature = "devtools"))] Arc<AtomicUsize>);

impl InFlight {
    /// One more request on its way.
    pub fn sent(&self) {
        #[cfg(any(test, feature = "devtools"))]
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// One request answered, or never delivered.
    pub fn done(&self) {
        #[cfg(any(test, feature = "devtools"))]
        self.0.fetch_sub(1, Ordering::Relaxed);
    }

    /// Whether anything is still out.
    #[cfg(any(test, feature = "devtools"))]
    pub fn any(&self) -> bool {
        self.0.load(Ordering::Relaxed) > 0
    }
}

/// Requests outstanding on a worker, and the most there may be; clone it into the worker.
#[derive(Clone)]
pub struct Capped(Arc<AtomicUsize>, usize);

impl Capped {
    pub fn new(cap: usize) -> Self {
        Self(Arc::new(AtomicUsize::new(0)), cap)
    }

    /// Take a place in the queue, or answer `false` when it is full.
    pub fn try_take(&self) -> bool {
        if self.count() >= self.1 {
            return false;
        }
        self.0.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Give a place back: the request was answered, or never sent.
    pub fn give_back(&self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }

    /// How many places are taken.
    pub fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}
