//! Getting an entry's bytes, by putting them on a disk.
//!
//! # Why this is the whole of the "everything else" story
//!
//! A path inside an archive has no bytes behind it, and four separate parts of this program want
//! bytes: the preview panel, the video player, `Ctrl+C`, and handing a file to whatever opens it.
//! Teaching each of them to read from an archive would mean four new code paths through four
//! subsystems that currently take a [`Path`] and are correct.
//!
//! So none of them is told anything. [`extracted`] writes the entry to a temp directory and returns
//! the **real** path, and the preview panel previews a file, the shell opens a file, the clipboard
//! carries a file. It is also what Explorer does with a `.zip`, which is why double-clicking a
//! document inside one has always left a copy in `%TEMP%`.
//!
//! # What is guaranteed
//!
//! - **Nothing is written outside [`temp_root`].** Entry paths come through
//!   [`super::read::interior`], which cannot produce a `..` or a drive letter, and [`destination`]
//!   checks the joined result anyway. An archive is a list of paths chosen by a stranger.
//! - **Extraction is idempotent and cached.** A file already there at the right size is not written
//!   again, so previewing the same entry twice, or previewing it and then opening it, costs one
//!   extraction.
//! - **A rewritten archive extracts afresh**, because the directory is named after the archive's
//!   size and timestamp as well as its path — the same key [`super::CACHE`] uses.
//! - **What comes out is read-only.** Editing an extracted copy cannot change the archive, and a
//!   silent no-op is the worst possible way to find that out: an application that tries to save
//!   gets a proper refusal instead.
//! - **A cancelled extraction leaves nothing half-written.** See [`save`], and [`CANCELLED`] for
//!   what the caller is told.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use super::{read, Format, Index, Inside};
use crate::shell::ops::fast::{self, Steer, Transfer};

// ---- Saying how far it has got, and being told to stop ----------------

/// Every extraction in flight, each with the [`Transfer`] the window's transfer panel draws — the
/// same card in the same corner as a copy, with the same Pause and Cancel.
///
/// # Bytes decompressed, and not files finished
///
/// Which is the whole of the design, and it is decided by what the wait actually *is*. Pulling one
/// 3 KB file out of a solid `.7z` takes as long as reading every entry that shares its block — see
/// [`write_sevenz`] — so a count of files would sit at `0 of 1` for twenty seconds and then jump to
/// done. Bytes through the decompressor is the one number that moves at the rate the work is being
/// done, whatever the format and whatever was asked for. Files are counted as well, for the
/// headline, but the bar goes by bytes.
///
/// # The total is an upper bound
///
/// [`upper_bound`] works it out per format, and for a `.7z` it is the whole archive: the walk stops
/// as soon as the last wanted entry has been seen, and where in the block order that falls is not
/// knowable in advance. So the bar can finish early. That is the right way round — a wait that
/// ends sooner than it promised needs no apology, while one that stalls at 100% looks broken — and
/// it is why the panel never prints a total smaller than what has already come out.
///
/// A `0` total means no figure at all was available, which happens for the single-stream formats
/// that do not record their uncompressed size: `.bz2`, `.xz`, `.zst`. The panel then counts up
/// without a denominator, exactly as the Size column goes blank for the same entries — see
/// [`crate::fs::dir::FLAG_UNSIZED`].
///
/// # Process-global
///
/// For the same reason [`super::CACHE`] is, and then for one more: of the four places that extract,
/// one is the drag thread rendering `CF_HDROP` inside `DoDragDrop` — see [`crate::windows::dnd`] —
/// which has no reference to the window and could not be given one without threading a handle
/// through OLE. A global is reported from wherever the work happens to be.
static RUNNING: Mutex<Vec<Arc<Transfer>>> = Mutex::new(Vec::new());

thread_local! {
    /// The extraction on *this* thread, which its bytes are counted into.
    ///
    /// **What keeps two extractions from adding to one number.** The preview panel materialises on
    /// its own reading thread while the user is free to `Ctrl+C` something on the UI's; each has a
    /// transfer of its own, and so a card of its own that stops only it.
    static MINE: RefCell<Option<Arc<Transfer>>> = const { RefCell::new(None) };
}

/// What an extraction that was cancelled answers, in place of whatever the decompressor said on its
/// way out.
pub const CANCELLED: &str = "Extraction cancelled";

fn running() -> MutexGuard<'static, Vec<Arc<Transfer>>> {
    // What is behind it is a list of handles, and still worth drawing after a panic.
    RUNNING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What every extraction in flight is doing, for [`crate::ui::transfers`].
pub fn transfers() -> Vec<fast::Snapshot> {
    running().iter().map(|transfer| transfer.snapshot()).collect()
}

/// Pass a button on an extraction's card to it. An id that is not an extraction's is ignored, as
/// [`crate::shell::ops::Operations::steer`] ignores one that is not a copy's — the window hands
/// every press to both.
///
/// Cancel is the one button the card has: nothing is ever asked of an extraction, it is not
/// [`fast::Snapshot::pausable`], and it leaves the panel the moment it ends — a failure goes to the
/// status line, as it did before there was a panel.
pub fn steer(id: u64, steer: Steer) {
    if steer != Steer::Cancel {
        return;
    }
    if let Some(transfer) = running().iter().find(|transfer| transfer.id == id) {
        transfer.cancel();
    }
}

/// Put this thread's extraction on the panel, named after `archive` — or `None` when this thread
/// already has one, whose figures this call then adds to.
fn claim(archive: &Path) -> Option<Claim> {
    if MINE.with_borrow(Option::is_some) {
        return None;
    }
    let transfer = Arc::new(Transfer::extracting(fast::next_id(), archive.to_path_buf()));
    running().push(transfer.clone());
    MINE.set(Some(transfer.clone()));
    Some(Claim(transfer))
}

/// Takes an extraction off the panel however it ends, including through a `?` or a panic.
pub(crate) struct Claim(pub(crate) Arc<Transfer>);

impl Drop for Claim {
    fn drop(&mut self) {
        MINE.with_borrow_mut(|mine| {
            if mine.as_ref().is_some_and(|transfer| Arc::ptr_eq(transfer, &self.0)) {
                *mine = None;
            }
        });
        running().retain(|transfer| !Arc::ptr_eq(transfer, &self.0));
    }
}

/// Ask something of this thread's extraction, if it has one.
fn mine<R>(with: impl FnOnce(&Transfer) -> R) -> Option<R> {
    MINE.with_borrow(|mine| mine.as_deref().map(with))
}

/// An extraction that is not happening, stuck at a figure of its own and already past the panel's
/// delay — for the tests that are about what the *window* does with one.
///
/// The same seam, for the same reason, as `crate::shell::dnd::Drag::pretend`: what the panel says
/// while work is in flight cannot be asserted by starting real work and hoping to catch it, and a
/// test that polls for a race is a test that fails on somebody else's machine. Hold the returned
/// guard for as long as the extraction is meant to be running; its transfer is there to be asked
/// whether a button reached it. Not this thread's, so nothing extracted here counts into it.
#[cfg(test)]
pub(crate) fn pretend(archive: &Path, files: u64, done: u64, total: u64) -> Claim {
    let transfer = Arc::new(
        Transfer::extracting(fast::next_id(), archive.to_path_buf())
            .started_ago(std::time::Duration::from_secs(1)),
    );
    transfer.found(files, total);
    transfer.moved_bytes(done);
    transfer.counted();
    running().push(transfer.clone());
    Claim(transfer)
}

/// A reader that counts what comes out of it — and the place an extraction is stopped.
///
/// **Stopped between reads, by failing the next one.** A decompressor cannot be interrupted from
/// outside, but every one of them asks its reader for the next few kilobytes, and an error there
/// comes back up through the format's own code exactly as a truncated archive would: through each
/// `?`, out of [`save`], which removes what it had written.
///
/// **Wrapped once per format, at the place that format's cost really is** — which is not the same
/// place for all four, and getting it wrong would either double-count or miss the bulk of the wait:
///
/// | format | wrapped around | because |
/// | --- | --- | --- |
/// | zip | each entry | it seeks, so only what was asked for is ever decompressed |
/// | `.7z` | each entry, wanted or drained | a solid block is read straight through — the drain *is* the work |
/// | tar | the whole stream | the crate skips unwanted entries, but the gz layer under it still inflates them |
/// | single stream | the stream | there is only one of each |
struct Counted<R>(R);

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if mine(Transfer::cancelled) == Some(true) {
            // Not `Interrupted`, which `io::copy` takes as leave to ask again.
            return Err(io::Error::other(CANCELLED));
        }
        let read = self.0.read(buf)?;
        mine(|transfer| transfer.moved_bytes(read as u64));
        Ok(read)
    }
}

/// How many bytes an extraction should expect to pull out of the decompressor.
///
/// An upper bound, and per format — see [`RUNNING`] for why it is allowed to be one, and [`Counted`]
/// for the matching decision about where the bytes are counted. The two have to agree or the readout
/// either creeps or races.
fn upper_bound(format: Format, index: &Index, outstanding: &[&Wanted]) -> u64 {
    let asked_for = || {
        outstanding
            .iter()
            .filter(|item| !item.is_dir)
            .map(|item| item.size)
            .sum()
    };
    match format {
        // A seek per entry, so the only bytes decompressed are the ones wanted. Exact.
        Format::Zip => asked_for(),
        // Everything, because the block order is not the listed order and the walk cannot know which
        // entry is the last one it needs until it has met it.
        Format::SevenZ => index.items.iter().map(|item| item.size).sum(),
        // Read from the front and stopped at the last entry wanted — which for a tar *is* knowable,
        // its index being its position in the file.
        format if format.is_tar() => {
            let last = outstanding.iter().map(|item| item.item).max().unwrap_or(0);
            index
                .items
                .iter()
                .take(last + 1)
                .map(|item| item.size)
                .sum()
        }
        // One entry and one stream. `0` when the format does not record the size, which is what
        // the panel drops the denominator for.
        _ => asked_for(),
    }
}

/// Where extracted copies live: one directory per process, under the system temp directory.
///
/// **Per process**, keyed by pid, because two windows of this program are two processes and a
/// shared directory would mean one deleting the other's open file on exit. That also makes
/// [`cleanup`] safe: it removes a tree only this process put anything in.
pub fn temp_root() -> PathBuf {
    // **Never `%TEMP%` from a test.** This function is the one place in this module that names a
    // directory to write in, and [`crate::sandbox`] is the rule about where a test may write —
    // written after a test run deleted this repository's working tree. `%TEMP%` is on that
    // module's list of places a test may not touch by name, so the redirection belongs here rather
    // than in each test remembering to ask.
    #[cfg(test)]
    {
        crate::sandbox::root().join("archive-extracts")
    }
    #[cfg(not(test))]
    {
        std::env::temp_dir()
            .join("azur-files")
            .join(format!("archives-{}", std::process::id()))
    }
}

/// Remove everything this process extracted.
///
/// Called from `main` on the way out. Best-effort by nature: a file still open in the application
/// the user opened it with cannot be deleted on Windows, and the next run gets a new pid and a new
/// directory rather than tripping over it. Nothing here retries or complains — a leftover temp file
/// is not worth a dialog, and `%TEMP%` is swept by the system.
pub fn cleanup() {
    let _ = std::fs::remove_dir_all(temp_root());
}

/// The real path for a path inside an archive, extracting it if it is not already there.
///
/// Handles all three things a caller might be pointing at:
///
/// - **The archive itself** (`D:\dl\pkg.zip`) — already a real file, returned unchanged. Nothing is
///   read. This is what makes the function safe to call on any path at all.
/// - **A file inside it** — extracted, and its own path returned.
/// - **A folder inside it** — extracted with everything under it, and the folder's path returned.
///   Which is what a `Ctrl+C` or a drag of a folder inside a zip has to mean.
///
/// `Err` carries words for the user. Every failure here is worth reporting rather than swallowing:
/// unlike a listing, this only ever runs because somebody asked for this file.
///
/// # Never call this on the UI thread
///
/// It decompresses, which for a solid `.7z` means unpacking every entry ahead of the one wanted.
/// Both callers run it on a worker — [`crate::preview`] on its reading thread, and
/// [`crate::app::Explorer`] on a thread of its own whose answer comes back as an action.
pub fn extracted(path: &Path) -> Result<PathBuf, String> {
    let mut answers = all(std::slice::from_ref(&path.to_path_buf()))?;
    Ok(answers.pop().expect("one path in, one path out"))
}

/// The same, for several paths at once — **and this is the one to call for a selection.**
///
/// # One walk per archive, not one per file
///
/// [`extracted`] on each of 300 selected files would start 300 separate reads of the archive, and
/// for a **solid** `.7z` each of those reads decompresses the block from its beginning: the cost is
/// the selection size times the block size, which on a 144 MB archive is not slow but hung. Here the
/// wanted entries are gathered across the whole selection first, so the archive is walked **once**
/// however many files were picked.
///
/// Answers are returned in the order asked, so a caller can pair them with what it asked for — which
/// `CF_HDROP` needs, its list being positional.
///
/// A path that is not in an archive is its own answer, and the archive file itself is its own
/// answer, so this is safe to call on a mixed selection or on one that turns out to contain nothing
/// virtual at all.
pub fn all(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut answers: Vec<Option<PathBuf>> = vec![None; paths.len()];
    // Each archive, with the indices of the paths that live in it. A `Vec` and a linear scan: a
    // selection spans one archive in every real case, and two in the contrived one.
    let mut groups: Vec<(Inside, Vec<usize>)> = Vec::new();

    for (at, path) in paths.iter().enumerate() {
        match super::split(path) {
            // Not in an archive, or *is* the archive: a real path on a real disk either way.
            None => answers[at] = Some(path.clone()),
            Some(inside) if inside.is_root() => answers[at] = Some(inside.file.clone()),
            Some(inside) => match groups.iter_mut().find(|(held, _)| held.file == inside.file) {
                Some((_, members)) => members.push(at),
                None => groups.push((inside, vec![at])),
            },
        }
    }

    // On the panel for the length of the call, and off it on every way out including a `?` — see
    // [`Claim`]. Claimed before anything is read, so the card can appear at the start of the wait
    // rather than partway through it; the grouping above only split strings. Nothing to claim for a
    // selection with nothing virtual in it.
    let _claim = groups.first().and_then(|(inside, _)| claim(&inside.file));
    match write_groups(paths, &groups, &mut answers) {
        Ok(()) => Ok(answers
            .into_iter()
            .map(|answer| answer.expect("every path is answered by one of the arms above"))
            .collect()),
        // Whatever the decompressor said as the read under it failed, the reason is the button.
        Err(_) if mine(Transfer::cancelled) == Some(true) => Err(CANCELLED.to_owned()),
        Err(why) => Err(why),
    }
}

/// The half of [`all`] that reads: each archive in turn, its wanted entries written to disk, and the
/// folders among `answers` made.
fn write_groups(
    paths: &[PathBuf],
    groups: &[(Inside, Vec<usize>)],
    answers: &mut [Option<PathBuf>],
) -> Result<(), String> {
    for (n, (inside, members)) in groups.iter().enumerate() {
        // The same index the listing was built from, so an extraction asks for entries the pane is
        // actually showing. The read flag is the status line's business and not this function's.
        let (index, _freshly_read) = super::index(&inside.file, inside.format);
        if let Some(error) = &index.error {
            return Err(error.clone());
        }
        let root = destination(inside)?;

        let mut wanted: Vec<Wanted> = Vec::new();
        // Which answers are folders, so the directory can be made even when the archive stored no
        // entry for it.
        let mut folders: Vec<usize> = Vec::new();

        for &at in members {
            let mine = super::split(&paths[at]).expect("grouped by having split once already");
            let mut theirs = wanted_from(&mine, &index, &root)?;
            if theirs.len() > 1 || theirs.first().is_some_and(|item| item.is_dir) {
                folders.push(at);
            }
            answers[at] = Some(root.join(mine.within.replace('/', "\\")));
            wanted.append(&mut theirs);
        }

        // **Deduplicated**, because two selected paths can name the same entry: a folder and a file
        // inside it is an ordinary selection, and extracting that entry twice in one walk would write
        // the same bytes to the same place and, for a solid block, read it out of position the second
        // time.
        wanted.sort_by(|a, b| a.dest.cmp(&b.dest));
        wanted.dedup_by(|a, b| a.dest == b.dest);

        // Everything already on disk at the right size. Covers the second preview of the same
        // picture, and previewing a file and then opening it.
        let outstanding: Vec<&Wanted> = wanted.iter().filter(|item| !item.satisfied()).collect();
        // What this archive is about to cost, before it costs it. Only what is *outstanding* is
        // counted, so a second `Ctrl+C` of files already on disk promises nothing and is over
        // before a frame could have said otherwise. Added to rather than set, so a selection
        // spanning two archives ends up with the sum of both.
        let files = outstanding.iter().filter(|item| !item.is_dir).count() as u64;
        let bytes = upper_bound(inside.format, &index, &outstanding);
        mine(|transfer| transfer.found(files, bytes));
        // Every archive's share is known once the last one's is, and from then on the headline's
        // count is the job's rather than what has been found so far.
        if n + 1 == groups.len() {
            mine(Transfer::counted);
        }
        if !outstanding.is_empty() {
            write_out(inside, &index, &outstanding)?;
        }
        for at in folders {
            // A folder that existed in the archive only as a prefix of its children has no entry of
            // its own, so nothing above created it. Harmless if it is already there.
            if let Some(mine) = &answers[at] {
                let _ = std::fs::create_dir_all(mine);
            }
        }
    }
    Ok(())
}

/// One entry to be written, and where.
struct Wanted {
    /// Index into [`Index::items`].
    item: usize,
    dest: PathBuf,
    size: u64,
    /// The size is not known in advance, so [`Wanted::satisfied`] cannot check it.
    unknown: bool,
    is_dir: bool,
}

impl Wanted {
    /// Whether this is already on disk and the right size, so writing it again would be work for
    /// nothing.
    ///
    /// Size and not content: a hash would mean reading what was just about to be read anyway. A
    /// half-written file from an extraction that was interrupted has the wrong size and is
    /// rewritten, which is the case this check is really for.
    fn satisfied(&self) -> bool {
        if self.is_dir {
            return self.dest.is_dir();
        }
        match std::fs::metadata(&self.dest) {
            Ok(found) => found.is_file() && (self.unknown || found.len() == self.size),
            Err(_) => false,
        }
    }
}

/// The directory this archive's contents are extracted into.
///
/// Named after the archive's file name — so a path in a title bar or an "open with" dialog is
/// recognisable — plus a hash of its full path, size and timestamp. The hash is what keeps two
/// different `src.zip`s apart and what makes a rewritten archive extract afresh instead of serving
/// yesterday's bytes.
fn destination(inside: &Inside) -> Result<PathBuf, String> {
    let found = std::fs::metadata(&inside.file)
        .map_err(|_| super::GONE.to_owned())?;

    // `DefaultHasher` is fixed-seed and so gives the same answer in every process, which is what
    // makes an extraction survive being asked for again by a different one. It is not a
    // cryptographic hash and does not need to be: a collision means two archives sharing a
    // directory, and the per-entry size check would still rewrite what did not match.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    inside.file.hash(&mut hasher);
    found.len().hash(&mut hasher);
    crate::fs::time::filetime_of(&found).hash(&mut hasher);

    let stem = inside
        .file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        // Every character Windows will not accept in a name, so an archive reached over a path with
        // something odd in it still gets a directory.
        .map(|name| name.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_"))
        .unwrap_or_else(|| "archive".to_owned());

    Ok(temp_root().join(format!("{stem}-{:016x}", hasher.finish())))
}

/// Which entries a path names: one file, or a folder and everything under it.
fn wanted_from(inside: &Inside, index: &Index, root: &Path) -> Result<Vec<Wanted>, String> {
    let within = inside.within.as_str();
    let mut wanted = Vec::new();

    for (at, item) in index.items.iter().enumerate() {
        // The entry itself, or anything below it. The separator test is what keeps `srcfoo/x` from
        // being taken as part of `src` — the same test [`super::build`] makes.
        let mine = item.path == within
            || (item.path.starts_with(within)
                && item.path.as_bytes().get(within.len()) == Some(&b'/'));
        if !mine {
            continue;
        }
        if item.encrypted {
            return Err(super::ENCRYPTED.to_owned());
        }
        wanted.push(Wanted {
            item: at,
            dest: place(root, &item.path)?,
            size: item.size,
            unknown: item.unsized_,
            is_dir: item.is_dir,
        });
    }

    if wanted.is_empty() {
        return Err("This entry is no longer in the archive".to_owned());
    }
    Ok(wanted)
}

/// Where one entry goes on disk, checked to be inside `root`.
///
/// The check is redundant — [`super::read::interior`] has already made a `..` impossible — and it
/// stays anyway. This is the function that turns a stranger's string into a filesystem write, and
/// one `assert`-shaped guard at the point of the write costs nothing against the cost of being
/// wrong about the sanitiser once.
fn place(root: &Path, interior: &str) -> Result<PathBuf, String> {
    let dest = root.join(interior.replace('/', "\\"));
    if !dest.starts_with(root) {
        return Err("This archive contains an entry with an unusable name".to_owned());
    }
    Ok(dest)
}

/// Write the wanted entries, in one pass over the archive.
///
/// One pass matters most for the formats where it is not optional: a `.tar.gz` can only be read
/// forwards, and a solid `.7z` block has to be decompressed from its start whatever is wanted out
/// of it. Extracting a folder of 200 files by seeking to each in turn would decompress the
/// containing block 200 times.
fn write_out(inside: &Inside, index: &Index, wanted: &[&Wanted]) -> Result<(), String> {
    match inside.format {
        Format::Zip => write_zip(inside, index, wanted),
        Format::SevenZ => write_sevenz(inside, index, wanted),
        _ if inside.format.is_tar() => write_tar(inside, index, wanted),
        _ => write_stream(inside, wanted),
    }
}

/// A zip is the one format that can seek to an entry, so this asks for each by its index.
fn write_zip(inside: &Inside, index: &Index, wanted: &[&Wanted]) -> Result<(), String> {
    let mut archive = read::open_zip(&inside.file)?;

    for item in wanted {
        if item.is_dir {
            std::fs::create_dir_all(&item.dest).map_err(|why| why.to_string())?;
            continue;
        }
        let at = index.items[item.item].at;
        let entry = archive
            .by_index(at as usize)
            .map_err(|why| unsupported(&why.to_string()))?;
        save(&item.dest, &mut Counted(entry))?;
    }
    Ok(())
}

/// A `.7z` is read block by block and **not** in the order its entries are listed, so the wanted
/// set is matched by name rather than by position — see [`super::read::index`], which is why
/// [`super::Item::at`] is a zip's index and nobody else's.
fn write_sevenz(inside: &Inside, index: &Index, wanted: &[&Wanted]) -> Result<(), String> {
    let by_name = to_extract(index, wanted)?;
    if by_name.is_empty() {
        return Ok(());
    }

    let handle = File::open(&inside.file).map_err(|why| read::reason(&why))?;
    let mut archive = sevenz_rust2::ArchiveReader::new(
        std::io::BufReader::new(handle),
        sevenz_rust2::Password::empty(),
    )
    .map_err(super::unreadable_7z)?;

    // Collected rather than returned through the closure, whose error type is the crate's and
    // cannot carry one of these.
    let mut failed: Option<String> = None;
    let mut left = by_name.len();
    let walked = archive.for_each_entries(|entry, reader| {
        // Everything asked for has been seen. Stop, rather than decompress the rest of a block to
        // reach an end nobody is waiting for — the same early exit [`write_tar`] makes, and safe for
        // the same reason: alignment only matters while there is still something to find.
        //
        // First, so that the drain below is not paid for one entry of every block that remains. Note
        // that `false` here ends the *current block* and not the walk — the crate discards this
        // answer between blocks — which is what still lets an empty file be written by its own pass
        // at the end.
        //
        // And for the same reason once something has failed — a cancel is a failed read — since
        // the next block would otherwise be opened, and walked, for an answer that is already `Err`.
        if left == 0 || failed.is_some() {
            return Ok(false);
        }
        // The stored name normalised the same way the listing normalised it, which is what makes
        // this lookup meet the other half.
        let name = read::interior(&entry.name);
        let wanted = name.as_deref().and_then(|name| by_name.get(name));
        // Counted whichever arm takes it, because for a solid block the drain below is not overhead
        // on the way to the work — it *is* the work. See [`Counted`].
        let reader = &mut Counted(reader);

        match wanted {
            Some(item) => {
                if let Err(why) = save(&item.dest, reader) {
                    failed = Some(why);
                    return Ok(false);
                }
                left -= 1;
            }
            // **An entry nobody asked for still has to be read to its end**, and getting this wrong
            // is what a 144 MB SDK archive taught: `info.yml` and `version` came back
            // `ChecksumVerificationFailed` while their neighbours extracted perfectly.
            //
            // Every entry of a **solid** block is a window onto one shared decompressed stream — the
            // crate hands out a `BoundedReader` over it, sized to that entry — and it does not skip
            // what a caller leaves behind. Returning without reading therefore does not skip the
            // entry; it leaves the stream short by exactly that entry's length, so the *next*
            // reader starts mid-file, and the CRC that notices is the only reason this was a clean
            // failure rather than 296 bytes of the wrong file.
            //
            // So the bytes are read and thrown away. That is not a waste that can be avoided: it is
            // what "solid" means, and it is why 7-Zip is also slow to pull one file out of the end
            // of a big `.7z`. What *can* be avoided is reading past the last entry wanted — see
            // below.
            None => {
                if let Err(why) = io::copy(reader, &mut io::sink()) {
                    failed = Some(format!("This archive could not be read ({why})"));
                    return Ok(false);
                }
            }
        }
        Ok(true)
    });

    if let Some(why) = failed {
        return Err(why);
    }
    walked.map_err(|why| unsupported(&why.to_string()))?;
    Ok(())
}

/// The entries to look out for as they go past, keyed by the name they are stored under — and the
/// folders among them made on the spot.
///
/// Shared by the two formats that can only be read forwards: a `.7z`, whose entries do not arrive in
/// the order they are listed, and a tar, which has no index to seek in. A folder needs no pass over
/// the archive at all, so it is created here and left out of the map — which is also what lets a
/// caller skip the walk entirely when what remains is empty.
fn to_extract<'a>(
    index: &'a Index,
    wanted: &[&'a Wanted],
) -> Result<HashMap<&'a str, &'a Wanted>, String> {
    let mut by_name = HashMap::new();
    for item in wanted {
        if item.is_dir {
            std::fs::create_dir_all(&item.dest).map_err(|why| why.to_string())?;
            continue;
        }
        by_name.insert(index.items[item.item].path.as_str(), *item);
    }
    Ok(by_name)
}

/// A tar is walked from the front, matching by name as the entries go past.
fn write_tar(inside: &Inside, index: &Index, wanted: &[&Wanted]) -> Result<(), String> {
    let by_name = to_extract(index, wanted)?;
    if by_name.is_empty() {
        return Ok(());
    }

    let stream = read::stream(&inside.file, inside.format).map_err(|why| read::reason(&why))?;
    // Around the whole stream and not around each entry: the crate skips what was not asked for
    // without handing it to anybody, and under a `.tar.gz` every skipped byte was still inflated to
    // be skipped. See [`Counted`].
    let mut archive = tar::Archive::new(Counted(stream));
    let entries = archive
        .entries()
        .map_err(|why| format!("This archive could not be read ({why})"))?;

    let mut left = by_name.len();
    for entry in entries {
        let mut entry = entry.map_err(|why| format!("This archive stops early ({why})"))?;
        let Some(name) = read::tar_name(&entry) else {
            continue;
        };
        if let Some(item) = by_name.get(name.as_str()) {
            save(&item.dest, &mut entry)?;
            left -= 1;
            // Stop as soon as everything asked for has been seen, rather than inflating the rest of
            // a tarball to reach an end nobody is waiting for.
            if left == 0 {
                break;
            }
        }
    }
    Ok(())
}

/// A single-stream archive holds one file, so there is nothing to match: decompress it.
fn write_stream(inside: &Inside, wanted: &[&Wanted]) -> Result<(), String> {
    let Some(item) = wanted.first() else {
        return Ok(());
    };
    let stream = read::stream(&inside.file, inside.format).map_err(|why| read::reason(&why))?;
    save(&item.dest, &mut Counted(stream))
}

/// How much one entry may expand to.
///
/// The same reasoning as [`super::read`]'s cap and a different number, because the two are asked
/// different questions. A listing is a guess that the user wants this archive at all; an extraction
/// is a file they have double-clicked, and a 4 GiB disk image inside a `.7z` is a real thing to
/// double-click. What this stops is the other case: a zip whose central directory claims 1 KB and
/// whose data expands until the disk is full. Nothing declares the truth about its own size, so the
/// only defence is a limit on what is written.
const MOST: u64 = 16 << 30;

/// Write one entry's bytes, then make the copy read-only.
///
/// **A write that does not finish is removed**, a cancel included. [`Wanted::satisfied`] goes by
/// size, and for an entry whose size the archive does not record it cannot: a stub left behind there
/// would be served as the whole file from then on.
fn save(dest: &Path, from: &mut dyn Read) -> Result<(), String> {
    if let Some(name) = dest.file_name() {
        mine(|transfer| transfer.now_on(&name.to_string_lossy()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|why| {
            format!("Could not make a temporary folder for this file ({why})")
        })?;
    }
    // An earlier extraction left it read-only, and a read-only file cannot be opened for writing.
    // Only reached when the size check rejected what is there, so this is the retry path.
    clear_readonly(dest);

    let mut file = File::create(dest)
        .map_err(|why| format!("Could not write this file to a temporary folder ({why})"))?;
    let written = match io::copy(&mut from.take(MOST), &mut file) {
        Ok(written) => written,
        Err(why) => {
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(format!("This entry could not be decompressed ({why})"));
        }
    };
    // `take` stops silently at its limit, so the only way to notice is to have asked for one byte
    // more than the cap and got it.
    if written >= MOST {
        drop(file);
        let _ = std::fs::remove_file(dest);
        return Err("This entry is too large to extract".to_owned());
    }
    drop(file);

    // Read-only, so that editing the copy cannot look like editing the archive.
    if let Ok(found) = std::fs::metadata(dest) {
        let mut how = found.permissions();
        how.set_readonly(true);
        let _ = std::fs::set_permissions(dest, how);
    }
    mine(Transfer::finished_one);
    Ok(())
}

fn clear_readonly(path: &Path) {
    if let Ok(found) = std::fs::metadata(path) {
        let mut how = found.permissions();
        if how.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            how.set_readonly(false);
            let _ = std::fs::set_permissions(path, how);
        }
    }
}

/// Put a decompressor's complaint in words that say what to do about it.
///
/// The methods this program cannot decompress are a deliberate short list — see the `zip` and
/// `sevenz-rust2` blocks in `Cargo.toml`, where each was traded against a duplicated LZMA decoder
/// or a swapped-out inflate backend — and an entry that uses one has to say so plainly. "Unsupported
/// compression method 14" is a sentence that sends somebody to a search engine; naming the archiver
/// that will open it is the useful half.
fn unsupported(why: &str) -> String {
    let lower = why.to_ascii_lowercase();
    if lower.contains("unsupported") || lower.contains("not supported") {
        return format!(
            "This entry uses a compression method this program does not decompress ({why}). \
             The archive will open in 7-Zip."
        );
    }
    if lower.contains("password") || lower.contains("encrypt") {
        return super::ENCRYPTED.to_owned();
    }
    format!("This entry could not be decompressed ({why})")
}

/// The panel's transfer, driven through real extractions.
///
/// Here rather than in [`super::tests`] because what these assert on is private to this module — the
/// claim, the thread-local that scopes it, and where [`Counted`] is wrapped. The module owning its
/// own mechanics is what [`crate::windows::dnd`]'s virtual-file half does for the same reason.
#[cfg(test)]
mod tests {
    use super::*;

    /// A zip of stored (uncompressed) entries in the sandbox, so a byte count is arithmetic anybody
    /// can check rather than a compression ratio.
    fn a_zip(name: &str, entries: &[(&str, usize)]) -> (PathBuf, u64) {
        let root = crate::sandbox::fresh(&format!("progress-{name}"));
        let pkg = root.join("pkg.zip");
        let bodies: Vec<String> = entries.iter().map(|&(_, size)| "x".repeat(size)).collect();
        let named: Vec<(&str, &str)> =
            entries.iter().zip(&bodies).map(|(&(entry, _), body)| (entry, body.as_str())).collect();
        crate::archive::tests::zip_at(&pkg, &named);
        (pkg, entries.iter().map(|&(_, size)| size as u64).sum())
    }

    /// Put an extraction on this thread before the one under test starts.
    ///
    /// **The claim is what makes these deterministic**, and it works because of the thread-local:
    /// `all` finds this thread already has a transfer and claims none of its own, while the counting
    /// — gated on the *thread* — goes on writing into this one. So the figures can be read back after
    /// the extraction has returned, with no polling and no second thread. Another test extracting at
    /// the same moment cannot disturb them, being another thread.
    fn counter(archive: &Path) -> Claim {
        claim(archive).expect("nothing else on this test's thread is extracting")
    }

    /// Whether an extraction out of `archive` is still on the panel. By archive, because the panel
    /// is the whole process's and other tests extract at the same time.
    fn shown(archive: &Path) -> bool {
        transfers().iter().any(|transfer| transfer.into == archive)
    }

    /// Nothing on the panel when nothing is being extracted — and, which is the half worth a test,
    /// nothing *afterwards* either. A card left behind by a finished extraction would sit in the
    /// corner claiming work that is over, with a Cancel that cancels nothing.
    #[test]
    fn the_panel_is_cleared_when_the_extraction_ends() {
        let (pkg, _) = a_zip("released", &[("a.txt", 100)]);
        {
            let held = counter(&pkg);
            assert!(shown(&pkg), "claimed, so there is something to show");
            drop(held);
        }
        assert!(!shown(&pkg), "released, so there is not");

        extracted(&pkg.join("a.txt")).expect("extraction");
        assert!(
            !shown(&pkg),
            "an extraction that has returned must leave nothing behind on the panel"
        );
        assert!(MINE.with_borrow(Option::is_none), "nor on its thread");

        // And the failing path, which leaves through a `?` rather than off the end.
        assert!(extracted(&pkg.join("nope.txt")).is_err());
        assert!(
            !shown(&pkg),
            "including when it fails — the guard is what releases it, not the happy path"
        );
    }

    /// A zip seeks to each entry, so the bytes counted are exactly the bytes asked for.
    #[test]
    fn a_zip_counts_the_entries_it_was_asked_for() {
        let (pkg, _) = a_zip("zip", &[("a.txt", 4_000), ("b.txt", 6_000), ("c.txt", 9_000)]);
        let held = counter(&pkg);

        all(&[pkg.join("a.txt"), pkg.join("b.txt")]).expect("extraction");
        let doing = held.0.snapshot();

        assert_eq!(
            doing.bytes_found, 10_000,
            "the two entries asked for, and not the third: a zip never decompresses what it was \
             not asked for"
        );
        assert_eq!(
            doing.bytes_done, 10_000,
            "and it reached the figure it promised, exactly"
        );
        assert_eq!((doing.files_found, doing.files_done), (2, 2), "two files, both written");
        assert!(doing.counted, "one archive, so its count is the job's");
        drop(held);
    }

    /// **A solid `.7z` is the case the whole metric was chosen for**: pulling one small entry out of
    /// one costs every entry that shares its block, so the bytes counted have to include the ones
    /// nobody asked for. See [`write_sevenz`], where they are drained, and [`RUNNING`].
    #[test]
    fn a_solid_archive_counts_what_it_had_to_read_past() {
        let root = crate::sandbox::fresh("progress-sevenz");
        let pkg = root.join("pkg.7z");
        // Three entries of very different sizes, and the *last* one is what gets asked for — so a
        // walk that stopped early would give the game away.
        let sizes = [40_000usize, 50_000, 300];
        {
            let file = File::create(&pkg).expect("sandbox");
            let mut writer = sevenz_rust2::ArchiveWriter::new(file).expect("7z");
            for (at, size) in sizes.iter().enumerate() {
                writer
                    .push_archive_entry(
                        sevenz_rust2::ArchiveEntry::new_file(&format!("part-{at}.bin")),
                        Some(&vec![b'x'; *size][..]),
                    )
                    .expect("7z");
            }
            writer.finish().expect("7z");
        }

        let held = counter(&pkg);
        all(&[pkg.join("part-2.bin")]).expect("extraction");
        let doing = held.0.snapshot();

        let everything: u64 = sizes.iter().map(|size| *size as u64).sum();
        assert_eq!(
            doing.bytes_found, everything,
            "the whole archive, because which entry the block order puts last is not knowable \
             before the walk"
        );
        assert!(
            doing.bytes_done > sizes[2] as u64,
            "300 bytes were wanted and {} were read: a count of what was asked for would have sat \
             still for the whole wait",
            doing.bytes_done
        );
        assert_eq!(doing.files_found, 1, "and one file, which is what the headline counts");
        drop(held);
    }

    /// **Cancel stops it, says so, and leaves nothing half-written** — the last being what decides
    /// whether a cancel can be trusted, since the next extraction of the same entry believes what it
    /// finds on disk at the right size.
    ///
    /// Cancelled before it starts rather than during, which is the same code path — the read under
    /// the decompressor fails — without a race to catch it mid-file.
    #[test]
    fn a_cancelled_extraction_stops_and_leaves_nothing_behind() {
        let (pkg, _) = a_zip("cancelled", &[("a.txt", 100_000)]);
        let held = counter(&pkg);
        held.0.cancel();

        assert_eq!(
            all(&[pkg.join("a.txt")]),
            Err(CANCELLED.to_owned()),
            "the button is the reason, whatever the decompressor said as its read failed"
        );
        let inside = super::super::split(&pkg.join("a.txt")).expect("inside the zip");
        let dest = destination(&inside).expect("the archive is there").join("a.txt");
        assert!(!dest.exists(), "a half-written {} was left behind", dest.display());
        assert_eq!(held.0.snapshot().files_done, 0);
        drop(held);

        // And it was the cancel, not the archive: the same entry comes out whole once asked again.
        let real = extracted(&pkg.join("a.txt")).expect("extraction");
        assert_eq!(std::fs::metadata(real).expect("extracted").len(), 100_000);
    }

    /// The panel's buttons reach the extraction they are drawn for, and nothing else's.
    #[test]
    fn the_panel_buttons_reach_the_extraction() {
        let (pkg, _) = a_zip("steered", &[("a.txt", 10)]);
        let pretend = pretend(&pkg, 1, 0, 10);
        let id = pretend.0.id;

        steer(id + 1_000_000, Steer::Cancel);
        assert!(!pretend.0.cancelled(), "another id's Cancel stopped this one");

        // Pause holds whichever thread is extracting, the drag thread rendering a drop among them —
        // see [`fast::Snapshot::pausable`] — so an extraction does not take one.
        steer(id, Steer::Pause(true));
        assert!(!pretend.0.snapshot().paused, "an extraction was paused");
        assert!(!pretend.0.snapshot().pausable(), "and the card would offer to");

        steer(id, Steer::Cancel);
        assert!(pretend.0.cancelled());
    }
}
