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

use super::dir::{Dir, DirBuilder, FLAG_DIR, FLAG_HIDDEN, FLAG_LINK, FLAG_READONLY, FLAG_SYSTEM};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Read `path` into a [`Dir`]. Never fails: an unreadable directory comes back as
/// an empty one carrying the reason.
///
/// An empty path is the synthetic "This PC" listing of drives.
pub fn scan(path: &Path) -> Dir {
    let started = Instant::now();
    if path.as_os_str().is_empty() {
        return super::drives::this_pc(started);
    }
    scan_real(path, started)
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn scan_real(path: &Path, started: Instant) -> Dir {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
        FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
    };

    let pattern = search_pattern(path);
    let mut data = WIN32_FIND_DATAW::default();

    let handle = unsafe {
        FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            (&mut data) as *mut WIN32_FIND_DATAW as *mut c_void,
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let code = unsafe { GetLastError() };
        // An empty directory reports "no more files" from the *first* call rather
        // than coming back with zero entries, so it lands here and is not an error.
        const ERROR_FILE_NOT_FOUND: u32 = 2;
        const ERROR_NO_MORE_FILES: u32 = 18;
        if matches!(code, ERROR_FILE_NOT_FOUND | ERROR_NO_MORE_FILES) {
            return DirBuilder::new(path).finish(elapsed_micros(started));
        }
        return Dir::failed(path, error_text(code));
    }

    let mut builder = DirBuilder::new(path);
    loop {
        let name = trimmed_name(&data.cFileName);
        if !is_dot_entry(name) {
            builder.push_wide(
                name,
                (data.nFileSizeHigh as u64) << 32 | data.nFileSizeLow as u64,
                filetime(&data.ftLastWriteTime),
                flags_of(data.dwFileAttributes),
            );
        }
        if unsafe { FindNextFileW(handle, &mut data) } == 0 {
            break;
        }
    }
    unsafe { FindClose(handle) };

    builder.finish(elapsed_micros(started))
}

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
fn hands() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 8))
        .unwrap_or(2)
}

/// Whether [`scan_deep`] opens an entry with these flags.
///
/// A directory, unless it is a reparse point. Named and taken apart from the walk so the rule
/// that keeps the walk finite can be tested without a link in the filesystem — making one
/// needs either a privilege this program does not ask for or a shell-out, and the rule is too
/// important to leave to a test that gets skipped on the machines that have neither.
#[inline]
fn descends(flags: u16) -> bool {
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

/// `\\?\C:\some\dir\*`, ready for `FindFirstFileExW`.
///
/// The `\\?\` prefix lifts the 260-character limit and skips the Win32 path
/// parser, which is a per-call cost this makes on every directory. It also turns
/// off `.`/`..` collapsing — fine here, because every path in this program comes
/// from a canonicalised navigation, never from user text that was not resolved
/// first.
#[cfg(windows)]
fn search_pattern(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    let raw: Vec<u16> = path.as_os_str().encode_wide().collect();
    let mut out: Vec<u16> = Vec::with_capacity(raw.len() + 8);

    const SEP: u16 = b'\\' as u16;
    let starts_with = |prefix: &str| {
        raw.len() >= prefix.len()
            && raw
                .iter()
                .zip(prefix.encode_utf16())
                .take(prefix.len())
                .all(|(a, b)| *a == b || (*a == '/' as u16 && b == SEP))
    };

    if starts_with(r"\\?\") || starts_with(r"\\.\") {
        out.extend_from_slice(&raw);
    } else if starts_with(r"\\") {
        // `\\server\share` -> `\\?\UNC\server\share`.
        out.extend(r"\\?\UNC".encode_utf16());
        out.extend_from_slice(&raw[1..]);
    } else if raw.len() >= 2 && raw[1] == b':' as u16 {
        out.extend(r"\\?\".encode_utf16());
        out.extend_from_slice(&raw);
    } else {
        // Relative, or something exotic. Hand it to the normal parser.
        out.extend_from_slice(&raw);
    }

    // The extended prefix takes backslashes only.
    for unit in &mut out {
        if *unit == '/' as u16 {
            *unit = SEP;
        }
    }
    if out.last() != Some(&SEP) {
        out.push(SEP);
    }
    out.push(b'*' as u16);
    out.push(0);
    out
}

/// The name out of a `cFileName`, without the padding after the terminator.
#[cfg(windows)]
#[inline]
fn trimmed_name(buf: &[u16; 260]) -> &[u16] {
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    &buf[..len]
}

/// `.` and `..`, which every directory reports and nobody wants to see.
#[cfg(windows)]
#[inline]
fn is_dot_entry(name: &[u16]) -> bool {
    const DOT: u16 = b'.' as u16;
    match name.len() {
        1 => name[0] == DOT,
        2 => name[0] == DOT && name[1] == DOT,
        _ => false,
    }
}

#[cfg(windows)]
#[inline]
fn filetime(ft: &windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

#[cfg(windows)]
#[inline]
fn flags_of(attrs: u32) -> u16 {
    use windows_sys::Win32::Storage::FileSystem as fs;
    let mut flags = 0;
    if attrs & fs::FILE_ATTRIBUTE_DIRECTORY != 0 {
        flags |= FLAG_DIR;
    }
    if attrs & fs::FILE_ATTRIBUTE_HIDDEN != 0 {
        flags |= FLAG_HIDDEN;
    }
    if attrs & fs::FILE_ATTRIBUTE_SYSTEM != 0 {
        flags |= FLAG_SYSTEM;
    }
    if attrs & fs::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        flags |= FLAG_LINK;
    }
    if attrs & fs::FILE_ATTRIBUTE_READONLY != 0 {
        flags |= FLAG_READONLY;
    }
    flags
}

/// The handful of failures a file manager actually meets, in words.
///
/// `FormatMessageW` would give the localised text for all of them, at the cost of
/// another feature of `windows-sys` and a `LocalFree` on every path; these five
/// cover everything a user can act on.
#[cfg(windows)]
fn error_text(code: u32) -> String {
    match code {
        2 | 3 => "This folder no longer exists".to_owned(),
        5 => "Access denied".to_owned(),
        15 => "The drive is not available".to_owned(),
        21 => "The device is not ready".to_owned(),
        53 | 67 => "The network path was not found".to_owned(),
        1223 => "Cancelled".to_owned(),
        other => format!("Could not read this folder (error {other})"),
    }
}

/// Ask Windows not to put up an "insert a disk" dialog behind our back.
///
/// Any call that touches an empty removable drive — the drive list does, on every
/// refresh — pops a modal from inside the syscall unless this is set. It is
/// per-thread, so the loader's workers each call it once.
#[cfg(windows)]
pub fn silence_device_dialogs() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SetThreadErrorMode, SEM_FAILCRITICALERRORS,
    };
    unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, std::ptr::null_mut()) };
}

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
        let _ = FLAG_SYSTEM;
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
mod tests {
    use super::*;

    /// Read a directory this program's own source lives in, which is guaranteed to
    /// exist and to have both files and subdirectories in it.
    fn here() -> Dir {
        scan(Path::new(env!("CARGO_MANIFEST_DIR")))
    }

    /// This crate's own `src`, which has files at the top, three subdirectories under it and
    /// nothing enormous anywhere — unlike the crate root, which has `target` in it.
    fn sources() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// What a flatten costs, and what reading in parallel buys.
    ///
    /// ```text
    /// cargo test --release -- --ignored --nocapture flatten_speed
    /// ```
    ///
    /// Over this crate's own `target` directory by default, which is the largest warm tree on
    /// hand; `YAFE_FLATTEN_ROOT` points it somewhere else. One worker is the serial walk this
    /// replaced, so the comparison is against the real alternative rather than against nothing.
    /// Two passes, and the second is the one to read: the first warms the file system's cache,
    /// and a flatten of a folder somebody is looking at is a warm read.
    #[test]
    #[ignore = "walks a large tree; run explicitly"]
    fn flatten_speed() {
        let root = std::env::var("YAFE_FLATTEN_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
        if !root.is_dir() {
            println!("no {}; skipping", root.display());
            return;
        }
        println!("flattening {}", root.display());
        let patience = std::time::Duration::from_secs(600);
        for pass in 1..=2 {
            for hands in [1usize, 2, 4, 8, 16] {
                let started = Instant::now();
                let dir = walk(&root, FLATTEN_BUDGET, patience, hands);
                let elapsed = started.elapsed();
                let per = elapsed.as_nanos() as f64 / dir.len().max(1) as f64;
                println!(
                    "pass {pass}  {hands:>2} thread(s): {:>7} entries in {:>8.1} ms  ({per:>6.0} ns/entry){}",
                    dir.len(),
                    elapsed.as_secs_f64() * 1000.0,
                    if dir.truncated { "  [truncated]" } else { "" }
                );
            }
        }
    }

    /// However many threads read it, the answer is the same listing.
    ///
    /// The order is part of that: it is what makes a truncated walk a fair picture of the tree,
    /// and it is the thing a parallel read is most likely to lose.
    #[test]
    fn reading_in_parallel_does_not_change_the_answer() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let names = |hands: usize| -> Vec<String> {
            let dir = walk(&root, FLATTEN_BUDGET, FLATTEN_PATIENCE, hands);
            (0..dir.len()).map(|i| dir.name(i).to_owned()).collect()
        };
        let serial = names(1);
        assert!(serial.len() > 20, "the fixture tree is too small to mean much");
        for hands in [2, 4, 8] {
            assert_eq!(
                names(hands),
                serial,
                "{hands} threads produced a different listing"
            );
        }
        // And the budget still cuts in the same place, which is the property the order is for.
        let cut = |hands: usize| -> Vec<String> {
            let dir = walk(&root, 12, FLATTEN_PATIENCE, hands);
            assert!(dir.truncated);
            (0..dir.len()).map(|i| dir.name(i).to_owned()).collect()
        };
        assert_eq!(cut(8), cut(1), "a truncated walk kept different rows");
    }

    #[test]
    fn a_flattened_listing_holds_paths_relative_to_the_folder_it_walked() {
        let dir = scan_deep(&sources(), FLATTEN_BUDGET, FLATTEN_PATIENCE);
        assert!(dir.error.is_none(), "{:?}", dir.error);
        assert!(!dir.truncated, "this crate's own sources fit in any budget");
        let named = |want: &str| (0..dir.len()).find(|&i| dir.name(i) == want);

        // A child of the root is a bare name, exactly as a shallow scan has it.
        assert!(named("main.rs").is_some(), "the root's own files are in it");
        // A folder is content too: listed, as well as walked into.
        let ui = named("ui").expect("`src\\ui` is a row of its own");
        assert!(dir.entries[ui].is_dir());

        // And a grandchild carries the folders in front of it, which is what the row shows and
        // what the sort and the filter see.
        let deep = named("ui\\filelist.rs").expect("the walk reached into `ui`");
        assert_eq!(dir.leaf(deep), "filelist.rs", "the file's own name");
        assert_eq!(dir.ext(deep), "rs", "the extension is the last component's");
        assert_eq!(
            dir.target(deep),
            sources().join("ui").join("filelist.rs"),
            "a row has to lead to the file it names"
        );
        // Every count is the tree's, not the folder's.
        assert!(
            dir.file_count > 20,
            "only {} files: the walk did not go down",
            dir.file_count
        );
    }

    #[test]
    fn a_reparse_point_is_listed_but_never_descended_into() {
        // The rule that keeps a flatten of `C:\` finite. `C:\Users\All Users` is a junction to
        // `C:\ProgramData` and `C:\Documents and Settings` is one to `C:\Users`, so a walk that
        // follows either never finishes — it does not even loop visibly, it just keeps finding
        // more tree. Every backup tool refuses the same thing for the same reason.
        assert!(descends(FLAG_DIR), "a plain folder is walked into");
        assert!(
            !descends(FLAG_DIR | FLAG_LINK),
            "a junction must not be followed"
        );
        assert!(!descends(0), "a file is not a folder");
        assert!(!descends(FLAG_HIDDEN), "nor is a hidden one");
        // A hidden *folder* is still a folder: `.git` is content of the tree, and hiding a row
        // is the display's business — `show_hidden` — not the walk's.
        assert!(descends(FLAG_DIR | FLAG_HIDDEN));
    }

    #[test]
    fn a_flatten_stops_at_its_budget_and_admits_it() {
        let dir = scan_deep(&sources(), 3, FLATTEN_PATIENCE);
        assert_eq!(dir.len(), 3, "the budget is a hard stop");
        assert!(dir.truncated, "a listing missing rows has to say so");
    }

    #[test]
    fn a_flatten_out_of_patience_hands_back_what_it_has() {
        // No time at all: the folder itself is still read — a flatten that came back with
        // nothing would be a worse answer than the listing it replaced — and nothing under it
        // is opened.
        let dir = scan_deep(&sources(), FLATTEN_BUDGET, std::time::Duration::ZERO);
        assert!(dir.truncated);
        assert!(!dir.is_empty(), "the root's own children are the floor");
        assert!(
            (0..dir.len()).all(|i| !dir.name(i).contains('\\')),
            "it descended anyway, with no time to do it in"
        );
    }

    #[test]
    fn a_listing_has_no_dot_entries() {
        let dir = here();
        assert!(dir.error.is_none(), "{:?}", dir.error);
        for i in 0..dir.len() {
            let name = dir.name(i);
            assert!(name != "." && name != "..", "`{name}` should be filtered out");
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn sizes_and_kinds_come_from_the_enumeration() {
        let dir = here();
        let named = |want: &str| (0..dir.len()).find(|&i| dir.name(i) == want);

        let cargo = named("Cargo.toml").expect("this crate has a manifest");
        assert!(!dir.entries[cargo].is_dir());
        assert!(dir.entries[cargo].size > 0, "the size came from the find data");
        assert!(
            dir.entries[cargo].modified > super::super::time::UNIX_EPOCH_FILETIME,
            "and so did the timestamp"
        );
        assert_eq!(dir.ext(cargo), "toml");

        let src = named("src").expect("this crate has sources");
        assert!(dir.entries[src].is_dir());
        assert_eq!(dir.ext(src), "", "a directory's dots are part of its name");
    }

    #[test]
    fn a_missing_directory_reports_why() {
        let dir = scan(Path::new(r"Q:\no\such\place\at\all"));
        assert!(dir.is_empty());
        assert!(dir.error.is_some(), "a failed read has to say so");
    }

    #[test]
    fn an_empty_directory_is_not_an_error() {
        let path = crate::sandbox::dir("empty");
        std::fs::create_dir_all(&path).expect("temp dir");

        let dir = scan(&path);
        assert!(dir.is_empty());
        assert!(
            dir.error.is_none(),
            "an empty folder reports `no more files` from the *first* call, which is \
             not a failure: {:?}",
            dir.error
        );

        crate::sandbox::remove_dir(&path);
    }

    /// The claim this whole module exists to make, checked rather than asserted.
    ///
    /// Ignored by default because it writes 60,000 files. Run it deliberately:
    ///
    /// ```text
    /// cargo test --release -- --ignored --nocapture scan_speed
    /// ```
    #[test]
    #[ignore = "creates 60k files; run explicitly"]
    fn scan_speed() {
        const COUNT: usize = 60_000;

        let root = crate::sandbox::dir("bench");
        std::fs::create_dir_all(&root).expect("temp dir");

        // Names of mixed length and extension, so the transcode and the extension
        // split are both exercised rather than measured on one shape.
        let exts = ["rs", "txt", "png", "e57", "", "tar.gz"];
        for i in 0..COUNT {
            let ext = exts[i % exts.len()];
            let name = if ext.is_empty() {
                format!("entry_{i:06}")
            } else {
                format!("some_moderately_long_name_{i:06}.{ext}")
            };
            let _ = std::fs::write(root.join(name), b"x");
        }

        // Warm: the first read pays for the directory's metadata coming into cache,
        // and what is being measured is the steady state a user actually sees.
        let _ = scan(&root);

        let mut best = u64::MAX;
        for _ in 0..5 {
            let dir = scan(&root);
            assert_eq!(dir.len(), COUNT, "{:?}", dir.error);
            best = best.min(dir.scan_micros);
        }
        let per_entry_ns = best as f64 * 1000.0 / COUNT as f64;
        println!(
            "scan of {COUNT} entries: {:.1} ms  ({per_entry_ns:.0} ns/entry)",
            best as f64 / 1000.0
        );

        // The same folder through the standard library, for the comparison the module
        // documentation makes. Same shape of work: every name, every size, every
        // timestamp, every attribute — into the same arena.
        let mut std_best = u128::MAX;
        for _ in 0..5 {
            let started = Instant::now();
            let mut names = String::new();
            let mut count = 0usize;
            let mut bytes = 0u64;
            for entry in std::fs::read_dir(&root).expect("read_dir").flatten() {
                let name = entry.file_name();
                names.push_str(&name.to_string_lossy());
                let meta = entry.metadata().expect("metadata");
                bytes += meta.len();
                count += 1;
            }
            std::hint::black_box((&names, bytes));
            assert_eq!(count, COUNT);
            std_best = std_best.min(started.elapsed().as_micros());
        }
        println!(
            "std::fs::read_dir, same work: {:.1} ms  ({:.0} ns/entry, {:.2}x)",
            std_best as f64 / 1000.0,
            std_best as f64 * 1000.0 / COUNT as f64,
            std_best as f64 / best as f64
        );

        // The mistake that actually costs: asking the filesystem about each entry
        // *again*, by path, after the enumeration has already answered. This is what
        // `Path::is_dir` in a loop compiles down to, and it is the single easiest way
        // to turn a fast listing into a slow one.
        {
            let started = Instant::now();
            let mut dirs = 0usize;
            for entry in std::fs::read_dir(&root).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs += 1;
                }
                let _ = std::fs::metadata(&path).map(|m| m.len());
            }
            std::hint::black_box(dirs);
            let restat = started.elapsed().as_micros();
            println!(
                "read_dir + a stat per entry:  {:.1} ms  ({:.0} ns/entry, {:.1}x slower)",
                restat as f64 / 1000.0,
                restat as f64 * 1000.0 / COUNT as f64,
                restat as f64 / best as f64
            );
        }

        // And the other per-entry cost this program refuses to pay: asking the shell
        // what a file *is*, which is what fills Explorer's Type column.
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_TYPENAME};

            // A thousand is plenty to get a per-file cost, and sixty thousand of these
            // would make the test unbearable — which is rather the point.
            const SAMPLE: usize = 1_000;
            let names: Vec<Vec<u16>> = (0..SAMPLE)
                .map(|i| {
                    let ext = exts[i % exts.len()];
                    let name = if ext.is_empty() {
                        format!("entry_{i:06}")
                    } else {
                        format!("some_moderately_long_name_{i:06}.{ext}")
                    };
                    root.join(name)
                        .as_os_str()
                        .encode_wide()
                        .chain(std::iter::once(0))
                        .collect()
                })
                .collect();

            let started = Instant::now();
            for wide in &names {
                let mut info = SHFILEINFOW::default();
                unsafe {
                    SHGetFileInfoW(
                        wide.as_ptr(),
                        0,
                        &mut info,
                        std::mem::size_of::<SHFILEINFOW>() as u32,
                        SHGFI_TYPENAME,
                    )
                };
                std::hint::black_box(info.szTypeName[0]);
            }
            let shell_ns = started.elapsed().as_nanos() as f64 / SAMPLE as f64;
            println!(
                "SHGetFileInfo type name:      {:.0} ns/entry  \
                 (= {:.0} ms for {COUNT} entries, {:.0}x the whole scan)",
                shell_ns,
                shell_ns * COUNT as f64 / 1_000_000.0,
                shell_ns * COUNT as f64 / 1000.0 / best as f64
            );

            // The table this program uses instead, over the same sample.
            let started = Instant::now();
            let mut label = String::new();
            for i in 0..SAMPLE {
                label.clear();
                super::super::fmt::type_label(exts[i % exts.len()], false, &mut label);
                std::hint::black_box(label.len());
            }
            println!(
                "the static table instead:     {:.0} ns/entry",
                started.elapsed().as_nanos() as f64 / SAMPLE as f64
            );
        }

        // Sorting is the other half of what happens before a listing appears.
        let mut order = Vec::new();
        let dir = scan(&root);
        let started = Instant::now();
        super::super::sort::build_order(
            &dir,
            &mut order,
            super::super::Column::Name,
            true,
            false,
            "",
            None,
        );
        let sort_us = started.elapsed().as_micros();
        println!("natural sort of {COUNT} entries: {:.1} ms", sort_us as f64 / 1000.0);
        assert_eq!(order.len(), COUNT);

        // Not a tight bound — a loaded machine or a slow volume can be several times
        // this. It is here to catch a regression of the kind that matters: a `stat`
        // per entry, or an allocation per name, which would be ten times over.
        assert!(
            per_entry_ns < 3_000.0,
            "{per_entry_ns:.0} ns per entry — something has started doing per-file work"
        );

        crate::sandbox::remove(&root);
    }
}
