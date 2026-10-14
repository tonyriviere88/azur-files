//! How many names each file of a folder has: the Hard links column.
//!
//! A hard link is a second directory entry for the same file record. Neither name is the original,
//! they share the data and the times, and NTFS keeps the count on the record. **The directory read
//! does not carry it** — neither `WIN32_FIND_DATAW` nor `FILE_ID_EXTD_DIR_INFO` has a field for it —
//! so this is the one column whose every cell is a question per file, which is exactly what
//! [`crate::fs`] promises never to ask during a scan. Hence a service, the shape of
//! [`crate::shell::cloud`]: asked once when a listing lands, answered on a worker in batches,
//! addressed to the [`crate::pane::Tab::view`] that asked, and dropped if that view has gone.
//!
//! # What it costs, and where it is not asked at all
//!
//! One `NtQueryInformationByName(FileStatInformation)` per file, relative to one handle on the
//! folder: no handle per file, no path parsed per file. The folder's own read is one syscall per
//! few hundred entries, so this is still the dearer half — 17 µs a file, measured by
//! `hardlinks_cost`, which is a second for 60,000 — and that is why it is **only asked on a local
//! NTFS or ReFS volume** ([`crate::fs::Dir::volume`] and not [`crate::shell::over_network`]). Over
//! SMB each file would be a round trip to the server, for a column that is empty in nearly every
//! folder there is.
//!
//! **Files only.** A folder cannot be hard linked, and a reparse point is listed and never followed,
//! so its own count would be the link's rather than the file's — and nobody makes a second name for
//! a symlink.
//!
//! # The column appears when it has something to say
//!
//! A folder with no file of more than one name shows no column, so the answers carry only the files
//! with two names or more. The first such answer is what opens the column; see
//! [`crate::app::App::collect_hard_links`].
//!
//! # Every name, for the tooltip
//!
//! `FindFirstFileNameW` lists them. Asked on the UI thread, for the one row under the pointer and
//! only for a row this already counted at two or more — so only ever on a local volume, where it is a
//! read of one file record. Kept in a single slot keyed by view and row, so hovering costs one call
//! per row and nothing is kept per file. See [`names_of`].

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use crate::fs::Dir;
use crate::service::Service;

#[cfg(windows)]
#[path = "windows/hardlinks.rs"]
mod win;

/// Off Windows there are no counts to ask for: every folder opens nothing, and the column never shows.
#[cfg(not(windows))]
mod win {
    use std::path::Path;
    pub(super) struct Counter;
    impl Counter {
        pub(super) fn open(_: &Path) -> Option<Self> {
            None
        }
        pub(super) fn links(&self, _: &str) -> Option<u32> {
            None
        }
    }
    pub(super) fn names(_: &Path) -> Vec<String> {
        Vec::new()
    }
}

/// How many files go into one answer. Larger than the cloud's batch, because a count is a fraction
/// of what a property read costs and every answer is a pass over the panes.
const BATCH: usize = 512;

/// The most files of one folder that are asked about. A folder bigger than this is a flattened tree
/// or `WinSxS`, and its column fills in for the first this-many and says nothing about the rest.
const LIMIT: usize = 100_000;

/// Whether a listing is one this asks about at all: a real folder on a local volume that keeps file
/// IDs, which is NTFS and ReFS — the only file systems with hard links that this window can read.
pub fn wanted(dir: &Dir, path: &Path) -> bool {
    dir.volume.is_some()
        && !dir.recycled
        && dir.file_count > 0
        && !crate::fs::is_synthetic(path)
        && !crate::archive::is_virtual_location(path)
        && !crate::shell::over_network(path)
}

/// One folder's worth of asking.
struct Request {
    view: u64,
    dir: PathBuf,
    /// Entry index and name — relative to `dir`, so a flattened listing's `sub\file.txt` works too.
    rows: Vec<(u32, String)>,
}

/// Some of the answers for one view: entry index and count, only for counts of two or more.
pub struct Answer {
    pub view: u64,
    pub counts: Vec<(u32, u32)>,
    /// The last answer for this view. A listing sorted by the column re-sorts on it rather than on
    /// every batch.
    pub done: bool,
}

/// The counting service. One thread, started the first time a listing on NTFS lands.
pub struct HardLinks(Service<Request, Answer>);

impl HardLinks {
    pub fn new(ctx: &egui::Context) -> Self {
        // No apartment: a count is a file-system call, and nothing on this thread is COM.
        Self(Service::new(ctx, "hard-links", false, |request, wanted, tx, _| {
            count(request, wanted, tx)
        }))
    }

    /// Count the names of every file in a listing, for one view of it.
    pub fn request(&mut self, view: u64, dir: &Dir) {
        let rows = (0..dir.len())
            .filter(|&i| !dir.entries[i].is_dir() && !dir.entries[i].is_link())
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

/// Work through one request a batch at a time, and stop as soon as nobody is looking at the view.
///
/// Always ends with a `done` answer, even an empty one, unless the view has gone — the tab sorted by
/// this column is waiting for it.
fn count(request: &Request, wanted: &dyn Fn(u64) -> bool, tx: &Sender<Answer>) {
    if !wanted(request.view) {
        return;
    }
    let view = request.view;
    let finished = || {
        let _ = tx.send(Answer { view, counts: Vec::new(), done: true });
    };
    let Some(counter) = win::Counter::open(&request.dir) else {
        return finished();
    };
    let last = request.rows.len().div_ceil(BATCH);
    if last == 0 {
        return finished();
    }
    for (at, chunk) in request.rows.chunks(BATCH).enumerate() {
        if !wanted(view) {
            return;
        }
        let counts: Vec<(u32, u32)> = chunk
            .iter()
            .filter_map(|(index, name)| Some((*index, counter.links(name).filter(|&n| n > 1)?)))
            .collect();
        let done = at + 1 == last;
        // A batch of single names says nothing, unless it is the last and somebody is waiting.
        if counts.is_empty() && !done {
            continue;
        }
        if tx.send(Answer { view, counts, done }).is_err() {
            return;
        }
    }
}

/// The one row whose names were last listed, and what they were.
static NAMES: Mutex<Option<(u64, u32, Vec<String>)>> = Mutex::new(None);

/// Every path the file at `path` is known by, the given one included, as `C:\a\b.txt`.
///
/// For the tooltip of `row` in `view`, and remembered for that row alone: the tooltip asks every
/// frame it is up, and this answers from the slot until the pointer moves to another row. Empty when
/// the file system will not say.
pub fn names_of(view: u64, row: u32, path: &Path) -> Vec<String> {
    let mut slot = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((v, r, names)) = slot.as_ref() {
        if *v == view && *r == row {
            return names.clone();
        }
    }
    let names = win::names(path);
    *slot = Some((view, row, names.clone()));
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file with two names counts two from either, and lists both; a file with one is left out.
    #[test]
    #[cfg(windows)]
    fn a_second_name_is_counted_and_listed() {
        let dir = crate::sandbox::fresh("hardlinks-counted");
        let one = dir.join("one.txt");
        let two = dir.join("two.txt");
        let alone = dir.join("alone.txt");
        std::fs::write(&one, b"shared").unwrap();
        std::fs::write(&alone, b"single").unwrap();
        std::fs::hard_link(&one, &two).unwrap();

        let counter = win::Counter::open(&dir).expect("the sandbox opens");
        assert_eq!(counter.links("one.txt"), Some(2));
        assert_eq!(counter.links("two.txt"), Some(2));
        assert_eq!(counter.links("alone.txt"), Some(1));
        assert_eq!(counter.links("missing.txt"), None);

        let mut names = win::names(&one);
        names.sort();
        let mut want = vec![one.display().to_string(), two.display().to_string()];
        want.sort();
        // The volume root comes back from the file system, so compare without caring about case.
        let lower = |v: &[String]| v.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>();
        assert_eq!(lower(&names), lower(&want));
    }

    /// What a count costs per file, on a folder full of them. Not a gate; a number.
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn hardlinks_cost() {
        let dir = crate::sandbox::fresh("hardlinks-cost");
        let n = 5_000;
        for i in 0..n {
            std::fs::write(dir.join(format!("f{i}.txt")), b"x").unwrap();
        }
        let counter = win::Counter::open(&dir).unwrap();
        let started = std::time::Instant::now();
        let mut total = 0u64;
        for i in 0..n {
            total += counter.links(&format!("f{i}.txt")).unwrap_or(0) as u64;
        }
        let each = started.elapsed().as_secs_f64() * 1e6 / n as f64;
        println!("{n} files: {each:.1} µs each ({total} names)");
    }
}
