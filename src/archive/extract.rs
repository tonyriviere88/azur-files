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

use std::collections::HashMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{read, Format, Index, Inside};

// ---- Saying how far it has got --------------------------------------

/// How far the extraction in flight has got, for the status line to say so.
///
/// # Bytes decompressed, and not files finished
///
/// Which is the whole of the design, and it is decided by what the wait actually *is*. Pulling one
/// 3 KB file out of a solid `.7z` takes as long as reading every entry that shares its block — see
/// [`write_sevenz`] — so a count of files would sit at `0 of 1` for twenty seconds and then jump to
/// done. Bytes through the decompressor is the one number that moves at the rate the work is being
/// done, whatever the format and whatever was asked for.
///
/// # The total is an upper bound
///
/// [`upper_bound`] works it out per format, and for a `.7z` it is the whole archive: the walk stops
/// as soon as the last wanted entry has been seen, and where in the block order that falls is not
/// knowable in advance. So the readout can finish early. That is the right way round — a wait that
/// ends sooner than it promised needs no apology, while one that stalls at 100% looks broken — and
/// it is why [`Doing::write`] never prints a figure smaller than the one it has already reached.
///
/// A `0` total means no figure at all was available, which happens for the single-stream formats
/// that do not record their uncompressed size: `.bz2`, `.xz`, `.zst`. The sentence then counts up
/// without a denominator, exactly as the Size column goes blank for the same entries — see
/// [`crate::fs::dir::FLAG_UNSIZED`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Doing {
    /// Bytes out of the decompressor so far — entries wanted and entries merely read past alike.
    pub done: u64,
    /// What that is expected to reach, or `0` when nothing could be worked out.
    pub total: u64,
}

impl Doing {
    /// The sentence for the status line, written into a caller-owned buffer for the reason
    /// [`crate::fs::fmt`] gives.
    pub fn write(&self, out: &mut String) {
        out.push_str("Extracting ");
        crate::fs::fmt::size(self.done, out);
        if self.total > 0 {
            out.push_str(" of ");
            // Never less than what has already come out: `upper_bound` can be short — a zip entry
            // whose size is not in its header counts as zero — and *41.2 MB of 30.0 MB* would read
            // as a bug in the program rather than a gap in the archive.
            crate::fs::fmt::size(self.total.max(self.done), out);
        }
        out.push('…');
    }
}

/// The counter behind [`doing`].
struct Counter {
    /// Bytes out of the decompressor so far.
    done: AtomicU64,
    /// What [`upper_bound`] said that would reach, or `0` for an archive that does not say.
    total: AtomicU64,
    /// Whether an extraction holds it. Claimed with [`counting`] and released by its guard.
    held: AtomicBool,
}

/// One per process.
///
/// Process-global for the same reason [`super::CACHE`] is, and then for one more: of the four places
/// that extract, one is the drag thread rendering `CF_HDROP` inside `DoDragDrop` — see
/// [`crate::windows::dnd`] — which has no reference to the window and could not be given one without
/// threading a handle through OLE. A global is reported from wherever the work happens to be.
static COUNT: Counter = Counter {
    done: AtomicU64::new(0),
    total: AtomicU64::new(0),
    held: AtomicBool::new(false),
};

thread_local! {
    /// Whether *this* thread is the one whose bytes are being counted.
    ///
    /// **What keeps two extractions from adding to one number.** The preview panel materialises on
    /// its own reading thread while the user is free to `Ctrl+C` something on the UI's; the second to
    /// arrive claims nothing and reports nothing, rather than both interleaving into a figure that
    /// belongs to neither. Silence for one of them is the right answer: a status line can only say
    /// one thing anyway.
    static MINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Claim the counter, or `None` if another extraction already holds it.
fn counting() -> Option<Counting> {
    COUNT
        .held
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
        .then(|| {
            COUNT.done.store(0, Ordering::Relaxed);
            COUNT.total.store(0, Ordering::Relaxed);
            MINE.set(true);
            Counting
        })
}

/// Releases the claim however the extraction ends, including through a `?` or a panic.
struct Counting;

impl Drop for Counting {
    fn drop(&mut self) {
        MINE.set(false);
        COUNT.held.store(false, Ordering::Release);
    }
}

/// Add to what the extraction on this thread is expected to reach.
///
/// Added rather than set, so a selection spanning two archives ends up with the sum of both.
fn expect_more(bytes: u64) {
    if MINE.get() {
        COUNT.total.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Count bytes that have come out of a decompressor on this thread.
fn tally(bytes: u64) {
    if MINE.get() {
        COUNT.done.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// How far the extraction in flight has got, or `None` when nothing is being extracted.
///
/// Read from the frame loop every frame while it answers `Some`. The two figures are loaded
/// separately and so can be a few kilobytes out of step with each other, which is worth nothing to
/// synchronise: they are on their way to a status line, and the next frame is 16 ms away.
pub fn doing() -> Option<Doing> {
    COUNT.held.load(Ordering::Acquire).then(|| Doing {
        done: COUNT.done.load(Ordering::Relaxed),
        total: COUNT.total.load(Ordering::Relaxed),
    })
}

/// An extraction that is not happening, stuck at a figure of its own — for the tests that are about
/// what the *window* does with one.
///
/// The same seam, for the same reason, as `crate::shell::dnd::Drag::pretend`: what a status line says
/// while work is in flight cannot be asserted by starting real work and hoping to catch it, and a
/// test that polls for a race is a test that fails on somebody else's machine. Hold the returned
/// guard for as long as the extraction is meant to be running.
#[cfg(test)]
pub fn pretend(done: u64, total: u64) -> impl Drop {
    let held = counting().expect("nothing else is counting");
    expect_more(total);
    tally(done);
    held
}

/// A reader that counts what comes out of it.
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
        let read = self.0.read(buf)?;
        tally(read as u64);
        Ok(read)
    }
}

/// How many bytes an extraction should expect to pull out of the decompressor.
///
/// An upper bound, and per format — see [`Doing`] for why it is allowed to be one, and [`Counted`]
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
        // [`Doing::write`] drops the denominator for.
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
            .join("azur-file-explorer")
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
    // Held for the length of the call, and released on every way out of it including a `?` — see
    // [`Counting`]. Claimed before anything is read so that the sentence appears at the start of the
    // wait rather than partway through it, and `None` when another extraction is already being
    // reported, which costs this one only its readout.
    let _counting = counting();
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

    for (inside, members) in &groups {
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
        if !outstanding.is_empty() {
            // What this archive is about to cost, before it costs it. Only what is *outstanding* is
            // counted, so a second `Ctrl+C` of files already on disk promises nothing and is over
            // before a frame could have said otherwise.
            expect_more(upper_bound(inside.format, &index, &outstanding));
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

    Ok(answers
        .into_iter()
        .map(|answer| answer.expect("every path is answered by one of the arms above"))
        .collect())
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
        .map_err(|_| "This archive is no longer there".to_owned())?;

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
            return Err(
                "This entry is encrypted, and there is nowhere in this program to type a password"
                    .to_owned(),
            );
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
    let handle = File::open(&inside.file).map_err(|why| why.to_string())?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(handle))
        .map_err(|why| format!("This zip file could not be read ({why})"))?;

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

    let handle = File::open(&inside.file).map_err(|why| why.to_string())?;
    let mut archive = sevenz_rust2::ArchiveReader::new(
        std::io::BufReader::new(handle),
        sevenz_rust2::Password::empty(),
    )
    .map_err(|why| format!("This 7z archive could not be read ({why})"))?;

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
        if left == 0 {
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

    let stream = read::stream(&inside.file, inside.format).map_err(|why| why.to_string())?;
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
        let raw = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let Some(name) = read::interior(&raw) else {
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
    let stream = read::stream(&inside.file, inside.format).map_err(|why| why.to_string())?;
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
fn save(dest: &Path, from: &mut dyn Read) -> Result<(), String> {
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
    let written = io::copy(&mut from.take(MOST), &mut file)
        .map_err(|why| format!("This entry could not be decompressed ({why})"))?;
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
        return "This entry is encrypted, and there is nowhere in this program to type a password"
            .to_owned();
    }
    format!("This entry could not be decompressed ({why})")
}

/// The counter, driven through real extractions.
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
        use std::io::Write as _;

        let root = crate::sandbox::fresh(&format!("progress-{name}"));
        let pkg = root.join("pkg.zip");
        let file = File::create(&pkg).expect("sandbox");
        let mut writer = zip::ZipWriter::new(file);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let mut bytes = 0u64;
        for (entry, size) in entries {
            writer.start_file(*entry, stored).expect("zip");
            writer.write_all(&vec![b'x'; *size]).expect("zip");
            bytes += *size as u64;
        }
        writer.finish().expect("zip");
        (pkg, bytes)
    }

    /// Claim the counter for this test, waiting for any other test's extraction to have finished with
    /// it.
    ///
    /// **The claim is what makes these deterministic**, and it works because of the thread-local:
    /// `all` finds the counter already held and reports nothing of its own, while `expect` and
    /// `tally` — gated on the *thread* rather than on the claim — go on writing into this one. So the
    /// figures can be read back after the extraction has returned, with no polling and no second
    /// thread. Another test extracting at the same moment cannot disturb them, being another thread.
    fn counter() -> Counting {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(held) = counting() {
                return held;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "another extraction has held the counter for thirty seconds"
            );
            std::thread::yield_now();
        }
    }

    /// Nothing to say when nothing is being extracted — and, which is the half worth a test, nothing
    /// to say *afterwards* either. A readout left behind by a finished extraction would sit on the
    /// status line claiming work that is over.
    #[test]
    fn the_counter_is_released_when_the_extraction_ends() {
        let (pkg, _) = a_zip("released", &[("a.txt", 100)]);
        {
            let held = counter();
            assert!(doing().is_some(), "held, so there is something to say");
            drop(held);
        }
        assert!(doing().is_none(), "released, so there is not");

        extracted(&pkg.join("a.txt")).expect("extraction");
        assert!(
            doing().is_none(),
            "an extraction that has returned must leave nothing behind on the status line"
        );

        // And the failing path, which leaves through a `?` rather than off the end.
        assert!(extracted(&pkg.join("nope.txt")).is_err());
        assert!(
            doing().is_none(),
            "including when it fails — the guard is what releases it, not the happy path"
        );
    }

    /// A zip seeks to each entry, so the bytes counted are exactly the bytes asked for.
    #[test]
    fn a_zip_counts_the_entries_it_was_asked_for() {
        let (pkg, _) = a_zip("zip", &[("a.txt", 4_000), ("b.txt", 6_000), ("c.txt", 9_000)]);
        let held = counter();

        all(&[pkg.join("a.txt"), pkg.join("b.txt")]).expect("extraction");
        let doing = doing().expect("held for the length of this test");

        assert_eq!(
            doing.total, 10_000,
            "the two entries asked for, and not the third: a zip never decompresses what it was \
             not asked for"
        );
        assert_eq!(
            doing.done, 10_000,
            "and it reached the figure it promised, exactly"
        );
        drop(held);
    }

    /// **A solid `.7z` is the case the whole metric was chosen for**: pulling one small entry out of
    /// one costs every entry that shares its block, so the bytes counted have to include the ones
    /// nobody asked for. See [`write_sevenz`], where they are drained, and [`Doing`].
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

        let held = counter();
        all(&[pkg.join("part-2.bin")]).expect("extraction");
        let doing = doing().expect("held for the length of this test");

        let everything: u64 = sizes.iter().map(|size| *size as u64).sum();
        assert_eq!(
            doing.total, everything,
            "the whole archive, because which entry the block order puts last is not knowable \
             before the walk"
        );
        assert!(
            doing.done > sizes[2] as u64,
            "300 bytes were wanted and {} were read: a count of what was asked for would have sat \
             still for the whole wait",
            doing.done
        );
        drop(held);
    }

    /// The sentence itself, both ways round — the second being the single-stream formats that do not
    /// record their size, where a denominator would have to be invented.
    #[test]
    fn the_sentence_counts_up_and_says_what_it_is_counting_to() {
        let mut out = String::new();
        Doing {
            done: 43_200_512,
            total: 151_000_000,
        }
        .write(&mut out);
        assert_eq!(out, "Extracting 41.2 MB of 144 MB…");

        out.clear();
        Doing {
            done: 9_000,
            total: 0,
        }
        .write(&mut out);
        assert_eq!(
            out, "Extracting 8.79 KB…",
            "no denominator invented for an archive that does not record one"
        );

        // An underestimated total is never printed as one: see [`Doing::write`].
        out.clear();
        Doing {
            done: 5_000,
            total: 1_000,
        }
        .write(&mut out);
        assert_eq!(out, "Extracting 4.88 KB of 4.88 KB…");
    }
}
