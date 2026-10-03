//! Reading a directory, as fast as the platform will allow.
//!
//! # What is actually expensive
//!
//! All the numbers below are from [`tests::scan_speed`], over 60,000 files in a
//! warm directory on this machine. Run it yourself with
//! `cargo test --release -- --ignored --nocapture scan_speed`; the shape matters
//! more than the absolute figures, and the shape is not what you would guess.
//!
//! | doing the same work | per entry | vs this scanner |
//! | --- | --- | --- |
//! | this module | 657 ns | — |
//! | `std::fs::read_dir` | 690 ns | 1.05× |
//! | `read_dir` + a `stat` per entry | 133 µs | **202× slower** |
//! | `SHGetFileInfo` for one type name | 1.1 ms | **1700× slower** |
//!
//! So the syscall is **not** where the win is. `FindFirstFileExW` at
//! [`FindExInfoBasic`] with [`FIND_FIRST_EX_LARGE_FETCH`] is used here because it
//! is a few percent quicker and because it hands the UTF-16 name straight into the
//! arena with no `OsString` in between — but `std::fs::read_dir` is within 5% of it,
//! and anyone claiming a four-times speedup from the choice of enumeration call has
//! not measured one.
//!
//! What *is* the win is the last two rows, and both are things this program refuses
//! to do:
//!
//! - **Never ask about an entry twice.** The find data already carries the name, the
//!   size, the times and the attributes. The moment code reaches for `Path::is_dir`
//!   or `fs::metadata` on top of that it has turned one sequential read into an
//!   `open`/`query`/`close` per file — 202 times the cost, and the reason most
//!   file managers crawl on a large folder. Nothing here stats an entry it has
//!   already enumerated.
//! - **Never ask the shell what a file is.** `SHGetFileInfo` with `SHGFI_TYPENAME`
//!   is a `HKEY_CLASSES_ROOT` walk per file, and it measures at over a millisecond
//!   each here — 67 *seconds* for this folder. [`super::fmt::type_label`] is a
//!   binary search over a static table at 45 ns, which is the same answer 25,000
//!   times sooner.

use super::dir::{Dir, DirBuilder, FLAG_DIR, FLAG_HIDDEN, FLAG_LINK, FLAG_READONLY};
// Only the Windows enumeration has an attribute word to read it out of; nothing portable
// reports it, so the portable scanner below never sets it.
#[cfg(windows)]
use super::dir::FLAG_SYSTEM;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(windows)]
#[path = "../../windows/scan.rs"]
mod win;
#[cfg(windows)]
use win::scan_real;
#[cfg(windows)]
pub use win::silence_device_dialogs;

/// Read `path` into a [`Dir`]. Never fails: an unreadable directory comes back as
/// an empty one carrying the reason.
///
/// Two paths are not directories at all and are answered without touching the filesystem API:
///
/// - **An empty path** is the synthetic "This PC" listing of volumes.
/// - **A bare `\\server`** is the machine's list of shares. A server is not a directory —
///   `FindFirstFileW` against `\\fileserver\*` fails with `ERROR_BAD_PATHNAME` (161), which is
///   exactly what a bare server path used to come up as — so it is asked of the machine instead.
///   See [`super::drives::server_dir`]: that call goes to the network and can take twenty-two
///   seconds to fail, which is affordable here and nowhere else, because this function only ever
///   runs on a [`crate::loader`] worker.
pub fn scan(path: &Path) -> Dir {
    let started = Instant::now();
    if path.as_os_str().is_empty() {
        return super::drives::this_pc(started);
    }
    if let Some(server) = super::drives::unc_server(path) {
        return super::drives::server_dir(&server, path, started);
    }
    scan_real(path, started)
}

/// The handful of failures a file manager actually meets, in words.
///
/// Here rather than only in the platform half because a failure is not only a directory read's:
/// [`super::drives::server_dir`] has a Win32 code to put into words too, and one vocabulary for
/// both is what stops a server that is not there from being worded differently from a share that
/// is not there.
#[cfg(windows)]
pub(crate) use win::error_text;

/// Nothing portable hands back an error *code*, so there is nothing to translate.
#[cfg(not(windows))]
pub(crate) fn error_text(code: u32) -> String {
    format!("Could not read this folder (error {code})")
}

/// Whether a failure is one that **signing in would fix**.
///
/// Both halves of the question, because either alone is wrong:
///
/// - **The code has to be about who is asking.** `ERROR_ACCESS_DENIED` is the one that matters and
///   the one that is ambiguous: a server refusing an unauthenticated session returns it, and so
///   does a folder whose ACL genuinely excludes you. The rest of the list is unambiguous.
/// - **The path has to be a UNC path.** There is nobody to sign in to on `C:`, so offering it
///   there would be a credential dialog raised over a file this account is simply not allowed to
///   read — a prompt that cannot succeed, in front of the one message that explained why.
///
/// Codes left out on purpose: `ERROR_ACCOUNT_DISABLED`, `ERROR_ACCOUNT_LOCKED_OUT` and
/// `ERROR_PASSWORD_EXPIRED` name an account that cannot be used at all, and
/// `ERROR_SESSION_CREDENTIAL_CONFLICT` (1219) is the one where Windows itself refuses a second
/// identity for a server already connected under another — a prompt for any of them fails again
/// with the same answer, which is worse than the sentence they came with.
pub(crate) fn wants_credentials(code: u32, path: &Path) -> bool {
    /// `ERROR_ACCESS_DENIED`, and the four that say so outright:
    /// `ERROR_INVALID_PASSWORD`, `ERROR_NOT_AUTHENTICATED`, `ERROR_LOGON_FAILURE`,
    /// `ERROR_BAD_USERNAME`.
    const ASKING: [u32; 5] = [5, 86, 1244, 1326, 2202];
    ASKING.contains(&code) && path.to_string_lossy().starts_with("\\\\")
}

// ---------------------------------------------------------------------------
// The deep walk
// ---------------------------------------------------------------------------

/// Read `root` and everything under it into one flat [`Dir`], names relative to
/// `root`, stopping after `budget` entries.
///
/// What the flatten button in the path bar shows. The listing is an ordinary `Dir` in
/// every respect the rest of the program can see: [`Dir::target`] still joins a name
/// onto the folder, so `sub\deep\file.txt` resolves to the file it names, and the sort,
/// the filter, the selection, the icons and every file operation work on it unchanged.
/// The one thing that is different is the names, and [`Dir::leaf`] is there for the two
/// places that need the file's own name back — renaming, and revealing a row.
///
/// # Three rules the walk has to keep
///
/// - **A reparse point is not descended into.** `C:\Users\All Users` is a junction to
///   `C:\ProgramData`, `C:\Documents and Settings` is a junction to `C:\Users`, and a
///   walk that follows either of those on a system drive never finishes. The link is
///   still *listed* — it is content of the folder — it is simply not opened. That is
///   what every backup tool and `robocopy` do by default, for the same reason.
/// - **It stops somewhere.** A flatten of `C:\` is a request to enumerate a million
///   files, and this is a button next to a text field. `budget` caps the rows and
///   `patience` caps the wall clock, whichever comes first; the listing says it was cut
///   off either way. Because every directory the walk descends into was itself pushed
///   as an entry first, the row cap bounds the number of directories opened too.
/// - **Breadth first.** A depth-first walk spends its budget down the leftmost branch
///   and a cut-off listing shows one deep path and nothing else; breadth first spends
///   it evenly, so a truncated answer is still a fair picture of the tree. It also
///   costs one queue entry per directory rather than a recursion whose depth is the
///   tree's, and it is what makes the directories at each level a *batch* that can be
///   read in parallel.
///
/// # Where the time goes
///
/// A directory read is a syscall waiting on the file system, so the walk is not CPU-bound and a
/// thread that is waiting is a thread another directory could have been read on. The batch of
/// directories at each level is read on up to [`hands`] of them at once — see [`enumerate`],
/// which keeps the answers in batch order so the walk is still breadth-first.
///
/// Measured warm by [`tests::flatten_speed`], where one thread is the serial walk this replaced:
///
/// | tree | entries | 1 thread | 4 | **8** | 16 |
/// | --- | --- | --- | --- | --- | --- |
/// | this crate's `src` | 35 | 0.2 ms | 0.2 | **0.2** | 0.2 |
/// | its `target` | 26,917 | 47.9 ms | 25.5 | **22.8** | 24.9 |
/// | `C:\Program Files` | 188,729 | 1,945 ms | 672 | **561** | 537 |
///
/// So the case that was worth doing something about — a tree big enough to wait for — comes back
/// **3.5× sooner**, and the case that was already instant is untouched: a batch no bigger than
/// the thread count is read inline, because spawning threads to read three directories costs
/// more than reading them. Sixteen threads is a wash against eight, which is why [`hands`] stops
/// there.
///
/// Errors in the middle are skipped rather than reported: a tree of ten thousand
/// folders where one is denied is not a failed read, and the denied folder simply
/// contributes nothing. A root that cannot be read at all still comes back as
/// [`Dir::failed`], which is what the caller shows.
pub fn scan_deep(root: &Path, budget: usize, patience: std::time::Duration) -> Dir {
    let started = Instant::now();
    // "This PC" is not a folder and has no tree: its rows are volumes, each of which is
    // a place to flatten of its own. Flattening it would mean walking every drive in the
    // machine, which is not what anybody means by the button.
    if root.as_os_str().is_empty() {
        return super::drives::this_pc(started);
    }
    // A machine is the same kind of thing one level down: its rows are shares, each a tree of its
    // own, and flattening it would mean walking every share on the server. So the button gives
    // back the shares, exactly as it gives back the volumes for This PC.
    if let Some(server) = super::drives::unc_server(root) {
        return super::drives::server_dir(&server, root, started);
    }

    walk(root, budget, patience, hands())
}

/// The walk, with the number of enumerating threads spelled out.
///
/// Split from [`scan_deep`] for [`tests::flatten_speed`], which is what the figures in this
/// module's documentation come from: one worker is the old serial walk, and the comparison is
/// the only honest way to keep the claim about the others.
fn walk(root: &Path, budget: usize, patience: std::time::Duration, hands: usize) -> Dir {
    let started = Instant::now();
    let mut builder = DirBuilder::new(root);
    // Directories still to open, each with its path relative to the root. Popped from the front
    // in batches, so the walk stays breadth-first however many threads are reading.
    let mut pending: std::collections::VecDeque<(PathBuf, String)> =
        std::collections::VecDeque::from([(root.to_path_buf(), String::new())]);
    // Composed once per entry and reused, so a tree of 200,000 files is not 200,000
    // allocations for names that are copied into the arena immediately afterwards.
    let mut relative = String::with_capacity(260);
    let mut truncated = false;
    let mut failed: Option<Dir> = None;
    // **The folder itself is always read**, whatever the deadline says: a flatten that came back
    // with nothing at all would be a worse answer than the listing it replaced. So the deadline
    // arrives after the first batch, which is the root alone.
    let mut deadline: Option<Instant> = None;

    'walk: while !pending.is_empty() {
        // Once per batch rather than once per entry: a clock read is cheap, and a batch is the
        // unit the walk can stop between.
        if deadline.is_some_and(|until| Instant::now() >= until) {
            truncated = true;
            break;
        }
        let take = pending.len().min(BATCH);
        let batch: Vec<(PathBuf, String)> = pending.drain(..take).collect();
        let (listings, gave_up) = enumerate(&batch, hands, deadline);
        deadline = Some(started + patience);
        // A batch that ran out of time mid-way has left directories unread, and the outer check
        // above cannot be relied on to notice: the batch may have been the last one, and then
        // the walk would end looking complete. This is the only thing that must not happen.
        truncated |= gave_up;

        for (here, dir) in listings {
            // The *root* failing is the only failure worth reporting: it means there is nothing
            // to flatten. A directory somewhere in the middle that cannot be read contributes
            // nothing and is not an error — a tree of ten thousand folders where one is denied
            // is not a failed read.
            if dir.error.is_some() && dir.path == root {
                failed = Some(dir);
                break 'walk;
            }
            for i in 0..dir.len() {
                if builder.len() >= budget {
                    truncated = true;
                    break 'walk;
                }
                let entry = dir.entries[i];
                relative.clear();
                if !here.is_empty() {
                    relative.push_str(&here);
                    relative.push('\\');
                }
                relative.push_str(dir.name(i));
                builder.push(&relative, entry.size, entry.modified, entry.flags);
                if descends(entry.flags) {
                    pending.push_back((dir.target(i), relative.clone()));
                }
            }
        }
    }

    if let Some(failed) = failed {
        return failed;
    }
    let mut flat = builder.finish(elapsed_micros(started));
    flat.truncated = truncated;
    flat
}

/// Read a batch of directories, in batch order, on up to `hands` threads.
///
/// **This is where a flatten's time goes**, and why it is worth reading in parallel: a directory
/// read is a syscall waiting on the file system, so a thread that is waiting is a thread another
/// directory could have been read on. The measured figures are in [`scan_deep`]'s documentation.
///
/// Ordered, because the order is what makes the walk breadth-first, and breadth-first is what
/// makes a *truncated* listing a fair picture of the tree. Each worker returns what it read
/// tagged with where it came from in the batch, and the results are put back in order — a sort
/// of a few hundred integers per batch against a syscall each.
///
/// **A batch no bigger than the number of threads goes inline**, which is the first batch of
/// every walk and the whole of one over a shallow folder. Spawning a thread costs about 70µs
/// here and a warm directory read costs about 100µs, so a batch that gives each thread one
/// directory spends more on the threads than it saves; the rule asks for two each before it is
/// worth it. Measured, on this crate's own `src`: 0.2 ms inline against 0.4 ms with threads for
/// its second batch of three directories.
///
/// `until` is the walk's deadline, checked between directories on every thread: a batch of a
/// thousand folders on a share that has gone away would otherwise take a thousand timeouts to
/// get through, whatever the caller's patience said. `None` for the first batch, which is the
/// folder being flattened and is read whatever the deadline says. The second half of the answer
/// says whether it stopped for that reason, because the caller cannot tell from a short result
/// alone — and a walk that quietly came back short is the one thing this must not do.
fn enumerate(
    batch: &[(PathBuf, String)],
    hands: usize,
    until: Option<Instant>,
) -> (Vec<(String, Dir)>, bool) {
    let out_of_time = || until.is_some_and(|until| Instant::now() >= until);
    if batch.len() <= hands || hands <= 1 {
        let mut out = Vec::with_capacity(batch.len());
        for (path, here) in batch {
            if out_of_time() {
                return (out, true);
            }
            out.push((here.clone(), scan_real(path, Instant::now())));
        }
        return (out, false);
    }

    // The next directory in the batch nobody has taken. A shared counter rather than a slice
    // each, because directories differ in size by orders of magnitude: handing a worker a fixed
    // sixth of the batch means five workers waiting on whoever got `node_modules`.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let parts: Vec<Vec<(usize, String, Dir)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..hands.min(batch.len()))
            .map(|_| {
                let next = &next;
                scope.spawn(move || {
                    // Per-thread, and these threads are new: without it, a batch that reaches an
                    // empty card reader raises "Please insert a disk" from inside the syscall.
                    silence_device_dialogs();
                    let mut mine = Vec::new();
                    loop {
                        let at = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((path, here)) = batch.get(at) else {
                            return mine;
                        };
                        if out_of_time() {
                            return mine;
                        }
                        mine.push((at, here.clone(), scan_real(path, Instant::now())));
                    }
                })
            })
            .collect();
        // A worker that panicked contributes nothing rather than taking the window with it.
        workers.into_iter().filter_map(|w| w.join().ok()).collect()
    });

    let mut all: Vec<(usize, String, Dir)> = parts.into_iter().flatten().collect();
    all.sort_unstable_by_key(|(at, ..)| *at);
    let gave_up = all.len() < batch.len();
    (
        all.into_iter().map(|(_, here, dir)| (here, dir)).collect(),
        gave_up,
    )
}

/// How many directories are read before the walk stops to build them.
///
/// The batch is what bounds two things at once: how many listings are held in memory before they
/// are copied into the one arena and dropped, and how far past the budget the walk can read
/// before it notices. A thousand ordinary folders is a couple of megabytes in flight, and it is
/// large enough that the threads are spawned about ten times over a tree of ten thousand folders
/// rather than forty.
const BATCH: usize = 1024;

/// How many threads read directories at once.
///
/// The same shape as the loader's worker count and for the same reason: this is bound by the file
/// system rather than by the CPU, so more threads than a handful buys nothing and costs context
/// switches. Eight rather than the loader's four because a flatten is one burst of thousands of
/// reads rather than a steady trickle of one.
///
/// **Shared with [`crate::sizes`]**, which is the other unbounded walk in this program and arrived at
/// the same figure by its own measurement — see its module header. One function rather than two
/// identical ones, so the clamp cannot drift in one place while both docs go on claiming they agree.
pub(crate) fn hands() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 8))
        .unwrap_or(2)
}

/// Whether a walk opens an entry with these flags.
///
/// A directory, unless it is a reparse point. Named and taken apart from the walk so the rule
/// that keeps the walk finite can be tested without a link in the filesystem — making one
/// needs either a privilege this program does not ask for or a shell-out, and the rule is too
/// important to leave to a test that gets skipped on the machines that have neither.
///
/// **Both unbounded walks in this program ask it**: [`scan_deep`] here and [`crate::sizes`]' own.
/// That is the point of it being one named, tested function — a second walk that re-expressed the
/// rule would be a second chance to get `C:\Users\All Users` wrong, outside the one place a test
/// looks.
#[inline]
pub(crate) fn descends(flags: u16) -> bool {
    flags & FLAG_DIR != 0 && flags & FLAG_LINK == 0
}

/// How many entries a flattened listing may hold.
///
/// Two things decide it. A row costs its 32-byte record plus its name, and a *relative
/// path* is a longer name than a bare one — call it 150 bytes a row, so this is about
/// 30 MB, which is the most a view nobody asked to keep should be allowed to cost.
/// And the listing has to stay usable: the details view draws only the rows on screen,
/// so 200,000 scrolls at the refresh rate, but the sort behind it is one pass over all
/// of them on every column click.
///
/// It is a stopping point rather than a judgement about what is reasonable to flatten.
/// The status line says when a listing hit it, so the answer is never quietly short.
pub const FLATTEN_BUDGET: usize = 200_000;

/// How long a flatten may take before it hands back what it has.
///
/// The row budget alone is not a bound on *time*: a tree of a hundred nearly-empty
/// directories on a sleeping network share costs one round trip each and no rows at all,
/// and the entry cap would never be reached. Ten seconds is long enough for any local
/// tree worth flattening — this repository is 40ms — and short enough that a mistake
/// made on `\\server\archive` is something you wait out rather than something that has
/// hung.
pub const FLATTEN_PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(not(windows))]
pub fn silence_device_dialogs() {}

// ---------------------------------------------------------------------------
// Everything else
// ---------------------------------------------------------------------------

/// The portable path. Correct, and slower — `read_dir` here has to `stat` for the
/// file type on some platforms, which is exactly what the Windows path avoids.
#[cfg(not(windows))]
fn scan_real(path: &Path, started: Instant) -> Dir {
    let iter = match std::fs::read_dir(path) {
        Ok(iter) => iter,
        Err(e) => return Dir::failed(path, e.to_string()),
    };
    let mut builder = DirBuilder::new(path);
    for entry in iter.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let mut flags = 0;
        if meta.is_dir() {
            flags |= FLAG_DIR;
        }
        if meta.file_type().is_symlink() {
            flags |= FLAG_LINK;
        }
        if name.starts_with('.') {
            flags |= FLAG_HIDDEN;
        }
        if meta.permissions().readonly() {
            flags |= FLAG_READONLY;
        }
        builder.push(name, meta.len(), unix_to_filetime(&meta), flags);
    }
    builder.finish(elapsed_micros(started))
}

/// Modification time as a `FILETIME`, so both platforms sort and format the same
/// integer.
#[cfg(not(windows))]
fn unix_to_filetime(meta: &std::fs::Metadata) -> u64 {
    let Ok(time) = meta.modified() else { return 0 };
    let Ok(since) = time.duration_since(std::time::UNIX_EPOCH) else {
        return 0;
    };
    super::time::UNIX_EPOCH_FILETIME + since.as_secs() * 10_000_000 + since.subsec_nanos() as u64 / 100
}

fn elapsed_micros(started: Instant) -> u64 {
    started.elapsed().as_micros() as u64
}

#[cfg(test)]
mod tests;
