//! Notice when a folder changes on disk, so a listing is never stale.
//!
//! Without this, a listing is only as fresh as the last thing this program did to it. Anything
//! anybody *else* did went unseen: a file dragged out to Explorer stayed on screen because
//! Explorer's move finishes after our drag does and there was nothing to wait on; a build
//! writing into the folder you were watching showed the folder as it had been; a file deleted
//! from a terminal left a row that opened nothing. Worse than looking wrong, it made the *next*
//! gesture fail — dragging a row that no longer names a file cannot start a drag, so the window
//! appeared to have stopped responding.
//!
//! # One thread, not one per folder
//!
//! Navigating changes the watched set constantly — every folder you open and leave — so a thread
//! per folder would mean a thread spawned and joined per click. Instead there is one thread
//! holding a directory handle and an event per watched folder, parked in
//! `WaitForMultipleObjects` over all of them plus one more event the UI thread signals when the
//! set changes. Nothing spins and nothing polls: the thread wakes when a folder changes or when
//! the set does.
//!
//! # What it does not do
//!
//! It does not read the notifications. `ReadDirectoryChangesW` will say which file changed and
//! how, and none of that is worth having here: the answer to any of it is to re-read the folder,
//! which is one scan either way. Not parsing the buffer also removes the two ways this API is
//! usually got wrong — the alignment of `FILE_NOTIFY_INFORMATION` and the overflow case where
//! the buffer was too small and the contents are gone but the fact of the change is not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// How long to let a burst of changes settle before re-reading, in seconds.
///
/// A single copy fires several notifications — the file appears, its size changes, its timestamp
/// changes — and one folder scan answers all of them. Long enough to collapse a burst, short
/// enough that a file appearing looks immediate.
const SETTLE: f64 = 0.15;

/// The most folders one thread can watch, since it waits on them all at once.
///
/// `MAXIMUM_WAIT_OBJECTS` is 64 and one of those is the wake event. Reaching this needs 63 tabs
/// open at once; the ones past it simply are not watched, which is the behaviour this had for
/// every folder before.
#[cfg(windows)]
const MAX_WATCHED: usize = 63;

#[derive(Default)]
struct Shared {
    /// The folders the UI wants watched, replaced wholesale by [`Watch::keep`].
    wanted: Vec<PathBuf>,
    /// Folders seen to change, waiting to be asked for.
    changed: Vec<PathBuf>,
    quit: bool,
}

/// Watches the folders on screen and reports the ones that change.
pub struct Watch {
    shared: Arc<Mutex<Shared>>,
    /// The last set handed to [`Watch::keep`], so an unchanged frame takes no lock.
    mine: Vec<PathBuf>,
    /// Changes waiting out their settle window, and when each is due.
    pending: HashMap<PathBuf, f64>,
    /// The event that wakes the thread, as an integer because a `HANDLE` is not `Send`.
    #[cfg(windows)]
    wake: isize,
    #[cfg(windows)]
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    pub fn new(ctx: &egui::Context) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        #[cfg(windows)]
        {
            let (wake, thread) = win::start(shared.clone(), ctx.clone());
            Self {
                shared,
                mine: Vec::new(),
                pending: HashMap::new(),
                wake,
                thread,
            }
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
            Self {
                shared,
                mine: Vec::new(),
                pending: HashMap::new(),
            }
        }
    }

    /// Watch exactly these folders and no others.
    ///
    /// Called every frame with whatever is on screen, so it has to be cheap when nothing has
    /// moved: the set is compared against a local copy first and the lock is only taken when it
    /// actually differs.
    pub fn keep(&mut self, folders: &[PathBuf]) {
        let mut wanted: Vec<PathBuf> = folders
            .iter()
            .filter(|path| !path.as_os_str().is_empty())
            .cloned()
            .collect();
        wanted.sort();
        wanted.dedup();
        if wanted == self.mine {
            return;
        }
        self.mine = wanted.clone();
        if let Ok(mut shared) = self.shared.lock() {
            shared.wanted = wanted;
        }
        #[cfg(windows)]
        win::signal(self.wake);
    }

    /// Folders that have changed and have settled, so are due a re-read.
    ///
    /// `now` is egui's own clock, which is the one the caller can wake itself against.
    pub fn changed(&mut self, now: f64) -> Vec<PathBuf> {
        if let Ok(mut shared) = self.shared.lock() {
            for path in shared.changed.drain(..) {
                // First notification of a burst starts the clock; the rest ride along with it.
                self.pending.entry(path).or_insert(now + SETTLE);
            }
        }
        if self.pending.is_empty() {
            return Vec::new();
        }
        let due: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, &at)| at <= now)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &due {
            self.pending.remove(path);
        }
        due
    }

    /// Whether anything is still waiting out its settle window, and so whether the caller has to
    /// come back for it.
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if let Ok(mut shared) = self.shared.lock() {
                shared.quit = true;
            }
            win::signal(self.wake);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            win::close(self.wake);
        }
    }
}

#[cfg(windows)]
#[path = "windows/watch.rs"]
mod win;

#[cfg(test)]
mod tests {
    use super::*;

    /// A change waits out its settle window, then comes back exactly once.
    ///
    /// The window is what keeps one copy from becoming five folder scans: writing a file fires
    /// several notifications — the name, then the size, then the timestamp — and one scan answers
    /// all of them.
    #[test]
    fn a_burst_of_changes_becomes_one_re_read() {
        let ctx = egui::Context::default();
        let mut watch = Watch::new(&ctx);
        let folder = PathBuf::from(r"C:\Temp");

        // Three notifications, as a single file being written would produce.
        if let Ok(mut shared) = watch.shared.lock() {
            shared.changed.push(folder.clone());
            shared.changed.push(folder.clone());
            shared.changed.push(folder.clone());
        }

        assert!(
            watch.changed(0.0).is_empty(),
            "nothing is due until the burst has settled"
        );
        assert!(watch.waiting(), "and the caller is told to come back");
        assert_eq!(
            watch.changed(SETTLE + 0.001),
            vec![folder],
            "one re-read for the burst, not three"
        );
        assert!(!watch.waiting());
        assert!(
            watch.changed(10.0).is_empty(),
            "and it does not come back again"
        );
    }

    /// The empty path is This PC, which is a list of volumes rather than a folder.
    #[test]
    fn nothing_watches_this_pc() {
        let ctx = egui::Context::default();
        let mut watch = Watch::new(&ctx);
        watch.keep(&[PathBuf::new(), PathBuf::from(r"C:\Temp")]);
        assert_eq!(watch.mine, vec![PathBuf::from(r"C:\Temp")]);
    }

    /// Two panes on one folder ask for it once.
    #[test]
    fn the_same_folder_twice_is_watched_once() {
        let ctx = egui::Context::default();
        let mut watch = Watch::new(&ctx);
        let one = PathBuf::from(r"C:\Temp");
        watch.keep(&[one.clone(), one.clone()]);
        assert_eq!(watch.mine, vec![one]);
    }
}
