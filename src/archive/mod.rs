//! Opening an archive as though it were a folder.
//!
//! `D:\downloads\pkg.tar.gz\src\main.rs` is a path this program navigates to, breadcrumbs, sorts,
//! filters, previews and copies out of. There is no such file on disk, and almost nothing in this
//! codebase knows that.
//!
//! # One idea, and it is the path
//!
//! An archive's interior is addressed by **extending the archive file's own path**, which is what
//! Explorer does for a `.zip` and what makes the rest of this feature nearly free:
//!
//! | already works, unchanged | because |
//! | --- | --- |
//! | tabs, history, Back, Forward | a tab holds a [`std::path::PathBuf`] and never asks what it means |
//! | Up | [`crate::fs::parent_of`] is [`std::path::Path::parent`], and the parent of `pkg.zip\src` is `pkg.zip` |
//! | the breadcrumb | it splits a path into prefixes; the archive is one more segment |
//! | the listing cache | [`crate::loader::Loader`] is keyed by path, so a folder inside an archive caches like any other |
//! | sort, filter, selection, rename-in-place | they see a [`crate::fs::Dir`], and this module makes one |
//!
//! So the whole of the navigation side is [`split`] — which decides where the archive ends and its
//! interior begins, by extension and **without touching the disk** — plus one arm in
//! [`crate::fs::scan::scan`], beside the two synthetic listings that were already there ("This PC"
//! and a bare `\\server`).
//!
//! # Read twice, list many times
//!
//! The formats divide sharply on what listing one folder costs:
//!
//! - A `.zip` and a `.7z` have a **directory** at a known offset. Reading it is two seeks.
//! - A `.tar` has **no index at all** — the names are interleaved with the data, so finding them
//!   means walking the whole file, header to header.
//! - A `.tar.gz` is that walk *through a decompressor*, so listing one subfolder of a 200 MB source
//!   tarball means inflating up to 200 MB.
//!
//! A listing per folder would pay that per click. So the unit of work here is the **whole archive,
//! read once** into an [`Index`] and cached ([`CACHE`]); every folder inside it is then answered
//! from memory by [`build`], which is string slicing. Navigating a tarball costs what its first
//! folder cost, and nothing after that.
//!
//! **Nothing prefetches an archive**, and that falls out rather than being enforced: the prefetch
//! in [`crate::ui::filelist`] fires when the selection lands on a *directory*, and an archive is a
//! file until something opens it. Which is the right answer anyway — a prefetch is a guess, and
//! inflating 200 MB of tarball because an arrow key went past it is not a guess worth making. The
//! first click into an archive pays for it, and says so in the status line's scan time.
//!
//! # What is not here
//!
//! **Nothing writes.** No paste in, no delete, no rename, no new folder — [`crate::app`] disables
//! all of it for a path this module claims, and [`crate::shell`] is never handed one. That is
//! partly scope and partly the format: a `.zip` can have an entry replaced, but changing one file
//! in a `.tar.gz` means decompressing and rebuilding the entire archive, and a rewrite that fails
//! half way has destroyed something the user still wanted. Reading is the whole of it.
//!
//! Everything that needs *bytes* — opening a file, the preview panel, `Ctrl+C`, a drag out — goes
//! through [`extract`], which writes to a temp directory and hands the real path on. That is how
//! one hook makes the preview panel, the video player and the shell's own "open with" work inside
//! an archive without any of them learning a thing.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use crate::fs::dir::{Dir, DirBuilder, FLAG_DIR, FLAG_LINK, FLAG_READONLY, FLAG_UNSIZED};

pub mod extract;
mod read;

#[cfg(test)]
mod tests;

pub use extract::extracted;

/// How many entries one archive may contribute before the read gives up.
///
/// A listing that is missing rows must say so — see [`Dir::truncated`], which the status line
/// reads — and the number is here for the same reason [`crate::fs::scan::FLATTEN_BUDGET`] is:
/// there is no natural end to a hostile input. A zip whose central directory claims four billion
/// entries costs 4 billion × the size of an [`Item`] before anything else notices, and a "zip
/// bomb" of that shape is a 22 KB file.
pub const BUDGET: usize = 250_000;

/// What kind of archive a name says it is.
///
/// Extension only, and deliberately: [`split`] is on the path of a keystroke in the path bar and of
/// every breadcrumb repaint, where a file read is not affordable. The consequence is that a
/// `.zip` which is really a `.rar` is found out by [`read`] rather than here, and comes back as a
/// listing that says it could not be read — which is the right place for that answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// `.zip` and the several containers that are one.
    Zip,
    SevenZ,
    /// A `.tar`, plain, and the four compressed wrappings of one. The wrapping is what
    /// [`read::index`] puts in front of the tar walk; the entries are identical.
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    /// One compressed stream and no container at all: a `.gz` is *a file*, not a folder of them.
    /// Listed as a single row named after the archive with its suffix removed, which is the same
    /// answer 7-Zip gives and is what makes `access.log.gz` browsable.
    Gz,
    Bz2,
    Xz,
    Zst,
    Lzma,
}

impl Format {
    /// Whether the entries come out of a tar walk, whatever is wrapped around it.
    pub fn is_tar(self) -> bool {
        matches!(
            self,
            Self::Tar | Self::TarGz | Self::TarBz2 | Self::TarXz | Self::TarZst
        )
    }

}

/// The recognised suffixes, **longest first**, which is what makes `.tar.gz` win over `.gz`.
///
/// # What is in the list, and the rule that decided it
///
/// A format is here when *being an archive is what it is*. A `.docx`, a `.xlsx`, an `.odt` and an
/// `.epub` are all zip files, and every one of them is left out: double-clicking a document must
/// open the document, and a file manager that answered a Word file with a listing of
/// `word/document.xml` would be broken rather than clever. A `.jar`, a `.whl`, a `.nupkg` and a
/// `.vsix` go the other way — they are *packages*, people open them to see what is inside, and no
/// application is going to be upset about it.
///
/// `.rar`, `.iso`, `.cab` and `.dmg` are absent because nothing in this program can read them: see
/// the dependency block in `Cargo.toml`, which turned down the only RAR implementation there is on
/// licence grounds. They keep the `Kind::Archive` icon [`crate::fs::fmt`] gives them and open with
/// whatever the machine has, exactly as before.
const SUFFIXES: &[(&str, Format)] = &[
    (".tar.bz2", Format::TarBz2),
    (".tar.zst", Format::TarZst),
    (".tar.gz", Format::TarGz),
    (".tar.xz", Format::TarXz),
    (".nupkg", Format::Zip),
    (".tbz2", Format::TarBz2),
    (".lzma", Format::Lzma),
    (".tzst", Format::TarZst),
    (".vsix", Format::Zip),
    (".aar", Format::Zip),
    (".apk", Format::Zip),
    (".cbz", Format::Zip),
    (".ear", Format::Zip),
    (".ipa", Format::Zip),
    (".jar", Format::Zip),
    (".tar", Format::Tar),
    (".tbz", Format::TarBz2),
    (".tgz", Format::TarGz),
    (".txz", Format::TarXz),
    (".war", Format::Zip),
    (".whl", Format::Zip),
    (".xpi", Format::Zip),
    (".zip", Format::Zip),
    (".bz2", Format::Bz2),
    (".zst", Format::Zst),
    (".7z", Format::SevenZ),
    (".gz", Format::Gz),
    (".xz", Format::Xz),
];

/// Which format a file name claims to be, or `None` for anything this module does not open.
///
/// ASCII-lowercased rather than Unicode-lowercased, because every suffix in [`SUFFIXES`] is ASCII
/// and `to_lowercase` on a whole file name allocates and walks a case table to reach the same
/// answer.
pub fn format_of(name: &str) -> Option<Format> {
    // Only the tail can match, so only the tail is folded — and the longest suffix is 8 bytes.
    let tail = name.as_bytes();
    let from = tail.len().saturating_sub(16);
    let tail = name.get(from..).unwrap_or(name).to_ascii_lowercase();
    SUFFIXES
        .iter()
        // `>` and not `>=`: a file *called* `.zip` is a dotfile named "zip", not an archive.
        .find(|(suffix, _)| tail.len() > suffix.len() && tail.ends_with(suffix))
        .map(|(_, format)| *format)
}

/// A path that points at an archive, or into one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Inside {
    /// The archive file itself, which is a real path on a real disk.
    pub file: PathBuf,
    /// Where inside it, `/`-separated with no leading or trailing slash. Empty for the archive's
    /// own root — which is what a path naming just the archive means.
    pub within: String,
    pub format: Format,
}

impl Inside {
    /// Whether the path was the archive itself rather than something in it.
    pub fn is_root(&self) -> bool {
        self.within.is_empty()
    }
}

/// Where the archive in this path ends and its interior begins — or `None` if there is no archive
/// in it.
///
/// **Touches nothing.** No `metadata`, no `is_file`, no allocation beyond the two the answer is
/// made of. That is a hard requirement rather than an optimisation: this is called from the
/// breadcrumb while it draws, from the path bar's completion on every keystroke, and from
/// [`crate::app`] while it decides whether a menu entry should be greyed — and a `metadata` call
/// against a mapped drive whose share has gone away blocks for the twenty-two seconds
/// [`crate::fs::drives`] measures.
///
/// The cost of that is one wrong answer: a **folder** genuinely named `stuff.zip` looks exactly
/// like an archive from here. The one place it matters is the one place that can afford to check —
/// [`listing`] confirms with a single stat, on a worker, before reading anything — and everywhere
/// else the mistake is invisible, because a real folder's rows never route through this module.
///
/// # The first archive wins, and that is what makes nesting work
///
/// For `a.zip\b.zip\readme` this answers *`a.zip`, interior `b.zip/readme`* rather than descending
/// twice. Reading a zip that is itself a compressed member of another zip is not something the
/// format supports — the inner one has to be inflated somewhere first — so the interior path names
/// a **file**, which is the truth. Nesting then falls out of [`extract`] instead: opening
/// `b.zip` writes it to a temp file, and that temp file is an archive on a disk like any other. See
/// [`crate::app::Explorer::perform`]'s `Open` arm.
pub fn split(path: &Path) -> Option<Inside> {
    let mut file = PathBuf::new();
    let mut parts = path.components();
    let mut format = None;

    for part in parts.by_ref() {
        file.push(part);
        // A prefix (`D:`, `\\server\share`), a root or a `..` cannot be an archive; only a name
        // can. Checking `Normal` also keeps `to_string_lossy` off the prefix of every UNC path.
        if let Component::Normal(name) = part {
            if let Some(found) = format_of(&name.to_string_lossy()) {
                format = Some(found);
                break;
            }
        }
    }

    let mut within = String::new();
    for part in parts {
        if let Component::Normal(name) = part {
            if !within.is_empty() {
                within.push('/');
            }
            within.push_str(&name.to_string_lossy());
        }
    }

    Some(Inside {
        file,
        within,
        format: format?,
    })
}

/// Whether this path names an archive this program can open as a folder.
///
/// The question [`crate::app`]'s `Open` arm asks of a double-clicked file. `false` for a path
/// *inside* an archive, which is a different question with a different answer — that one is a
/// file, and opening it means [`extract`].
pub fn browsable(path: &Path) -> bool {
    split(path).is_some_and(|inside| inside.is_root())
}

/// Whether this path is an archive or anything inside one — the test for "nothing here can be
/// written to".
///
/// Every guard in [`crate::app`] and [`crate::shell::ops`] is this function, so that a new one
/// cannot be added to the wrong side of the boundary. `true` for the archive *file* as well as its
/// contents, which is what the callers want: a pane showing `pkg.zip` is showing an archive's root.
///
/// # Why an extension is not enough, and a `metadata` call is not allowed
///
/// [`split`] answers by extension, so on its own it says `true` for a **real folder** somebody has
/// named `stuff.zip`. That is not a cosmetic slip. This function gates the delete, the rename, the
/// paste, the clipboard and the shell's context menu, so an extension-only answer turns such a
/// folder into one the user can no longer work in — every write refused, with a sentence about
/// archives over a perfectly ordinary directory.
///
/// The obvious second half is a `metadata` call to ask whether the archive component is a file, and
/// it is exactly the wrong thing here: these guards run **on the UI thread**, and a stat against a
/// mapped drive whose share has gone away blocks for the twenty-two seconds [`drives`] measures. A
/// window that stops repainting when you press Delete is a worse bug than the one being fixed.
///
/// So the second half is [`indexed`] — *has this archive actually been read* — which is a lock and a
/// scan of at most [`CACHE_ARCHIVES`] entries, and which is exact for the only case that can arise:
/// you cannot be looking inside an archive whose index was not read to draw the listing you are
/// looking at. A real folder was never indexed, because [`listing`] stats it once, on a worker, and
/// hands it to the ordinary scan. See [`tests::a_real_folder_named_like_an_archive_can_still_be_written_to`].
pub fn is_virtual(path: &Path) -> bool {
    inside_archive(path).is_some()
}

/// The archive this path is really inside, confirmed the way [`is_virtual`] confirms it — and
/// carrying the [`Inside`] for a caller that needs to name the archive rather than only know there
/// is one.
///
/// What the status line's archive mark is drawn from, and what tells Reveal which real file to point
/// Explorer at.
pub fn inside_archive(path: &Path) -> Option<Inside> {
    split(path).filter(|inside| indexed(&inside.file))
}

/// Whether this archive's index is in [`CACHE`] — that is, whether it has been read as an archive.
///
/// The disk is not asked. See [`is_virtual`], which is the whole reason this exists.
fn indexed(file: &Path) -> bool {
    CACHE
        .lock()
        .map(|cache| cache.iter().any(|(key, _)| key.file == file))
        .unwrap_or(false)
}

/// One entry, as the archive states it.
#[derive(Clone, Debug)]
pub struct Item {
    /// The full interior path, `/`-separated, normalised by [`read::interior`]: no leading slash,
    /// no `.` or `..`, no drive letter, no backslash.
    pub path: String,
    pub size: u64,
    /// A raw `FILETIME`, converted from whatever the format stores — Unix seconds in a tar, a DOS
    /// wall-clock stamp in a zip, a `FILETIME` already in a `.7z`. `0` where the format has none,
    /// which draws as a dash.
    pub modified: u64,
    pub is_dir: bool,
    /// The size is not knowable without decompressing. See [`FLAG_UNSIZED`].
    pub unsized_: bool,
    /// Needs a password, which there is nowhere to type. Listed anyway — its name and size are
    /// real, and an entry silently missing from a listing is worse than one that will not open.
    pub encrypted: bool,
    /// A symlink or a hard link, which a tar records as an entry with no data. Drawn with the link
    /// glyph the listing already has for a junction, so that a row reading `0 B` is explained
    /// rather than looking like an empty file.
    pub link: bool,
    /// Which entry of the archive this is, in the order the format lists them — **not** an index
    /// into [`Index::items`], which drops the entries [`read::interior`] rejects.
    ///
    /// Only a zip's extraction uses it, that being the one format that can seek to an entry by
    /// number. A `.tar` is walked from the front and a `.7z` is decompressed block by block in an
    /// order that is not its file order, so both of those match by name instead — see
    /// [`extract::write_sevenz`], where that is the whole reason the field is not enough.
    pub at: u32,
}

/// Every entry in one archive, read once.
#[derive(Debug, Default)]
pub struct Index {
    pub items: Vec<Item>,
    /// Why it could not be read, in words for the pane. When this is set `items` is empty.
    pub error: Option<String>,
    /// The read stopped at [`BUDGET`] rather than at the end of the archive.
    pub truncated: bool,
    /// How long the read took, which is what the status line reports for the folder that paid it.
    pub micros: u64,
}

impl Index {
    fn failed(why: impl Into<String>) -> Self {
        Self {
            error: Some(why.into()),
            ..Self::default()
        }
    }
}

/// What identifies a cached [`Index`]: the archive, and enough of its metadata to notice it has
/// been replaced.
///
/// Size and timestamp rather than a hash, because a hash means reading the file — which is the
/// thing the cache exists to avoid. Anything that rewrites an archive changes at least one of the
/// two; a rewrite that preserves both to the 100-nanosecond tick is not a case this defends
/// against, and no file manager does.
#[derive(PartialEq, Eq, Debug)]
struct Key {
    file: PathBuf,
    size: u64,
    modified: u64,
}

/// The read archives, least recently used first.
///
/// A free-standing static rather than a field on [`crate::loader::Loader`], because
/// [`crate::fs::scan::scan`] is a free function called from four worker threads and threading a
/// cache handle down to it would mean a parameter on every scan in the program to serve one arm of
/// it. The lock is held for a clone of an `Arc` and never across a read — [`read::index`] runs
/// with it released, so two workers opening two archives do not queue behind each other. Two
/// workers opening *the same* archive both read it, and the second insert wins; that is a wasted
/// read on a race nobody can provoke by hand, and it is cheaper than holding a lock across a
/// 200 MB inflate.
static CACHE: LazyLock<Mutex<Held>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// What [`CACHE`] holds: the read archives, least recently used first.
///
/// A `Vec` and a linear scan rather than a map, because [`CACHE_ARCHIVES`] is six — a hash of a
/// [`PathBuf`] costs more than six comparisons — and because the eviction order is the vector's own
/// order, which a map would need a second structure to keep.
type Held = Vec<(Key, Arc<Index>)>;

/// How many archives are kept. Walking back up out of a tarball and into another one is the
/// working set, and it is small.
const CACHE_ARCHIVES: usize = 6;
/// And how many entries across them, which is what actually costs memory — an [`Item`] is its
/// string plus about 48 bytes, so a Linux kernel tarball at ~90,000 entries is a few megabytes.
const CACHE_ITEMS: usize = 400_000;

/// The index for an archive, from the cache or by reading it.
///
/// The flag is **whether it was read just now**, and it exists so that the read's cost is charged
/// once. [`Dir::scan_micros`] is what the status line reports, and adding the archive's read time to
/// every folder listed out of it would claim the first click's cost again on each of the next ones —
/// a tarball that took 900 ms to inflate reporting 900 ms for every subfolder, for ever, which is
/// exactly the wrong lesson to teach about a cache that is working.
fn index(file: &Path, format: Format) -> (Arc<Index>, bool) {
    let Some(key) = key_for(file) else {
        return (
            Arc::new(Index::failed("This archive is no longer there")),
            true,
        );
    };

    if let Some(hit) = CACHE.lock().ok().and_then(|mut cache| {
        let at = cache.iter().position(|(cached, _)| *cached == key)?;
        // Touch: move to the back, so the eviction below takes the least recently used.
        let entry = cache.remove(at);
        let index = entry.1.clone();
        cache.push(entry);
        Some(index)
    }) {
        return (hit, false);
    }

    let index = Arc::new(read::index(file, format));

    // A failed read is not cached, for the reason [`crate::loader`] does not cache one either: the
    // file may still be being written, and a retry should actually retry.
    if index.error.is_none() {
        if let Ok(mut cache) = CACHE.lock() {
            cache.retain(|(cached, _)| *cached != key);
            cache.push((key, index.clone()));
            let mut held: usize = cache.iter().map(|(_, index)| index.items.len()).sum();
            while cache.len() > CACHE_ARCHIVES || (held > CACHE_ITEMS && cache.len() > 1) {
                let (_, dropped) = cache.remove(0);
                held -= dropped.items.len();
            }
        }
    }
    (index, true)
}

/// Empty the cache. For the tests, which share one process and therefore one [`CACHE`]: a test
/// asserting that an archive was read once cannot do so against a cache the test before it left
/// entries in.
#[cfg(test)]
fn forget_all() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.clear();
    }
}

/// The archive's identity, or `None` if it is not a readable file.
///
/// The one stat this module makes, and it does three jobs: it is the cache key, it is the check
/// that a *folder* named `stuff.zip` is not treated as an archive — see [`split`] — and it is how
/// an archive deleted since it was last listed is noticed.
fn key_for(file: &Path) -> Option<Key> {
    let found = std::fs::metadata(file).ok()?;
    if !found.is_file() {
        return None;
    }
    Some(Key {
        file: file.to_path_buf(),
        size: found.len(),
        modified: crate::fs::time::filetime_of(&found),
    })
}

/// Drop the cached index for any archive on this path, so the next listing re-reads it.
///
/// For Refresh, and for a pane arriving somewhere after the archive may have been rewritten.
/// Takes any path, archive or not, and takes the *containing* archive — pressing F5 inside
/// `pkg.zip\src` has to invalidate `pkg.zip`, because that is the only thing there is to re-read.
pub fn forget(path: &Path) {
    let Some(inside) = split(path) else {
        return;
    };
    if let Ok(mut cache) = CACHE.lock() {
        cache.retain(|(key, _)| key.file != inside.file);
    }
}

/// Everything the cache is holding: archives, and entries across them. For `--trace`, and the same
/// question [`crate::loader::Loader::held`] answers about folders.
pub fn held() -> (usize, usize) {
    CACHE
        .lock()
        .map(|cache| {
            (
                cache.len(),
                cache.iter().map(|(_, index)| index.items.len()).sum(),
            )
        })
        .unwrap_or_default()
}

/// The listing for a path inside an archive, or `None` if it is not one.
///
/// The whole of this module's contribution to [`crate::fs::scan::scan`]. `None` means "not mine" —
/// either there is no archive in the path, or what looked like one is a folder that happens to be
/// named like an archive, in which case the ordinary scan is exactly right.
pub fn listing(path: &Path, started: Instant) -> Option<Dir> {
    let inside = split(path)?;
    // Confirms the archive is a file before anything is read, which is what makes the extension
    // guess in [`split`] safe. A folder called `stuff.zip` falls through to the real scan here.
    key_for(&inside.file)?;

    let (index, freshly_read) = index(&inside.file, inside.format);
    if let Some(error) = &index.error {
        return Some(Dir::failed(path, error.clone()));
    }
    Some(build(path, &inside, &index, freshly_read, started))
}

/// One file or folder a drag is offering, named the way the drop target will recreate it.
#[derive(Clone, Debug)]
pub struct Dragged {
    /// The name the target creates, relative to wherever the drop lands: `mod.rs` for a file picked
    /// up directly, `ui\mod.rs` for one inside a picked-up folder. `\`-separated, which is what
    /// `FILEDESCRIPTORW` means by a name with a path in it.
    pub name: String,
    pub size: u64,
    pub modified: u64,
    pub is_dir: bool,
    /// The virtual path, for [`extracted`] to be pointed at when the target asks for the bytes.
    pub path: PathBuf,
}

/// What a drag out of an archive offers, flattened: every file under everything selected, with the
/// relative name the target should give it.
///
/// # This function is the reason a drag out of an archive can start instantly
///
/// It decompresses **nothing**. Every field of every [`Dragged`] comes out of the [`Index`] that the
/// listing already read, which is exactly the information `CFSTR_FILEDESCRIPTORW` asks for — names,
/// sizes, dates, and which of them are folders. So the data object handed to OLE is complete before
/// the pointer has moved, and the bytes are fetched one entry at a time, at the drop, by
/// [`crate::windows::dnd`]. See that module for the other half.
///
/// A folder is expanded here rather than left for the target, because a target recreating a tree
/// needs the relative paths spelled out — that is how `FILEDESCRIPTORW` conveys a directory at all.
/// A folder with no entry of its own in the archive (the common case: see [`build`]) gets a
/// synthetic one, so that dragging an empty folder still creates an empty folder.
pub fn manifest(paths: &[PathBuf]) -> Result<Vec<Dragged>, String> {
    let mut out: Vec<Dragged> = Vec::new();

    for path in paths {
        let Some(inside) = split(path) else {
            continue;
        };
        // The archive itself is a real file on a real disk, so it is not this function's business —
        // a selection containing one is dragged by the shell's own data object.
        if inside.is_root() {
            continue;
        }
        let (index, _) = index(&inside.file, inside.format);
        if let Some(error) = &index.error {
            return Err(error.clone());
        }

        let within = &inside.within;
        // What the dragged thing is called where it lands: its own last component.
        let leaf = within.rsplit('/').next().unwrap_or(within).to_owned();
        let before = out.len();
        // Names already emitted for this archive, so a folder implied by twenty files is one
        // descriptor rather than twenty.
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for item in &index.items {
            if item.path == *within {
                if seen.insert(leaf.clone()) {
                    out.push(Dragged {
                        name: leaf.clone(),
                        size: item.size,
                        modified: item.modified,
                        is_dir: item.is_dir,
                        path: path.clone(),
                    });
                }
                continue;
            }
            // Below it. The separator test is the same one [`build`] makes, and for the same
            // reason: `srcfoo/x` is not part of `src`.
            if !item.path.starts_with(within.as_str())
                || item.path.as_bytes().get(within.len()) != Some(&b'/')
            {
                continue;
            }
            let rest = &item.path[within.len() + 1..];

            // **Every folder on the way down gets a descriptor of its own**, even when the archive
            // stores no entry for it — which is the usual case, most writers recording only files.
            //
            // A target given `src\ui\mod.rs` will generally create `src\ui` on the way, so this
            // looks redundant and is not: a folder that is *empty* is named by no file below it and
            // would simply not arrive. Being explicit costs one descriptor per folder and is what
            // makes a dragged tree land as the tree it was.
            let mut at = 0;
            while let Some(cut) = rest[at..].find('/') {
                at += cut;
                let folder = format!("{leaf}\\{}", rest[..at].replace('/', "\\"));
                let inner = &item.path[..within.len() + 1 + at];
                at += 1;
                if seen.insert(folder.clone()) {
                    out.push(Dragged {
                        name: folder,
                        size: 0,
                        modified: item.modified,
                        is_dir: true,
                        path: inside.file.join(inner.replace('/', "\\")),
                    });
                }
            }

            let name = format!("{leaf}\\{}", rest.replace('/', "\\"));
            if seen.insert(name.clone()) {
                out.push(Dragged {
                    name,
                    size: item.size,
                    modified: item.modified,
                    is_dir: item.is_dir,
                    path: inside.file.join(item.path.replace('/', "\\")),
                });
            }
        }

        if out.len() == before {
            return Err("This entry is no longer in the archive".to_owned());
        }
        // The dragged folder itself, when the archive only implies it. Inserted ahead of its
        // children so a target that creates things in order makes the directory before filling it.
        if !seen.contains(&leaf) {
            out.insert(
                before,
                Dragged {
                    name: leaf,
                    size: 0,
                    modified: out[before].modified,
                    is_dir: true,
                    path: path.clone(),
                },
            );
        }
    }

    if out.is_empty() {
        return Err("Nothing here can be dragged out".to_owned());
    }
    Ok(out)
}

/// The **flattened** listing of a folder inside an archive: everything under it, at every depth, in
/// one listing. The flatten button, and the tree view built on it.
///
/// `None` for the same two reasons [`listing`] gives it: no archive in the path, or a real folder
/// that merely looks like one.
///
/// # This is the one read in the program that a flatten makes *cheaper*
///
/// [`crate::fs::scan::scan_deep`] walks a real tree with four threads, a budget and a patience,
/// because a directory tree has no natural end and each level costs a syscall. An archive has
/// already been read — the [`Index`] *is* the flattened form, every entry with its full interior
/// path — so this is the same string slicing [`build`] does with the depth test removed. No walk, no
/// threads, no truncation of its own: what [`Index::truncated`] already says is the only limit there
/// is.
pub fn flattened(path: &Path, started: Instant) -> Option<Dir> {
    let inside = split(path)?;
    key_for(&inside.file)?;

    let (index, freshly_read) = index(&inside.file, inside.format);
    if let Some(error) = &index.error {
        return Some(Dir::failed(path, error.clone()));
    }

    let within = &inside.within;
    let from = if within.is_empty() {
        0
    } else {
        within.len() + 1
    };

    // Every folder on the way down, so the tree view has something to indent under and something to
    // collapse — the real walk emits them because it opens them, and a listing without them would
    // draw `src\ui\mod.rs` with no `src` above it.
    let mut seen: HashMap<String, bool> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut rows: HashMap<String, (u64, u64, u16)> = HashMap::new();

    for item in &index.items {
        if !within.is_empty()
            && (!item.path.starts_with(within.as_str())
                || item.path.as_bytes().get(within.len()) != Some(&b'/'))
        {
            continue;
        }
        let rest = &item.path[from..];
        if rest.is_empty() {
            continue;
        }

        // Each ancestor, then the entry itself. `\` rather than `/`, which is what a real flattened
        // listing stores and what [`Dir::within`] and [`Dir::depth`] read.
        let mut at = 0;
        while let Some(cut) = rest[at..].find('/') {
            at += cut;
            let folder = rest[..at].replace('/', "\\");
            at += 1;
            if seen.insert(folder.clone(), true).is_none() {
                order.push(folder.clone());
                rows.insert(folder, (0, item.modified, FLAG_DIR | FLAG_READONLY));
            } else if let Some(held) = rows.get_mut(&folder) {
                held.1 = held.1.max(item.modified);
            }
        }

        let name = rest.replace('/', "\\");
        let flags = FLAG_READONLY
            | if item.is_dir { FLAG_DIR } else { 0 }
            | if item.unsized_ { FLAG_UNSIZED } else { 0 }
            | if item.link { FLAG_LINK } else { 0 };
        match seen.insert(name.clone(), true) {
            // Already there as an inferred folder, and the archive's own record of it wins.
            Some(_) => {
                if let Some(held) = rows.get_mut(&name) {
                    if flags & FLAG_DIR == 0 {
                        *held = (item.size, item.modified, flags);
                    }
                }
            }
            None => {
                order.push(name.clone());
                rows.insert(name, (item.size, item.modified, flags));
            }
        }
    }

    let mut builder = DirBuilder::new(path);
    builder.reserve(order.len());
    for name in &order {
        if let Some(&(size, modified, flags)) = rows.get(name) {
            builder.push(name, size, modified, flags);
        }
    }

    let mut dir = builder.finish(started.elapsed().as_micros() as u64);
    if freshly_read {
        dir.scan_micros += index.micros;
    }
    dir.truncated = index.truncated;
    Some(dir)
}

/// Turn the flat list of interior paths into the listing of one folder.
///
/// # Directories that are not in the archive
///
/// A tar of `src/ui/mod.rs` need contain no entry for `src` and none for `src/ui`, and a zip
/// written by a streaming writer usually contains neither. So the folders are **inferred**: any
/// entry with a separator left in it after the prefix is removed contributes the name up to that
/// separator, deduplicated. An archive that *does* store its directories takes the same path and
/// dedupes against them.
///
/// An inferred folder has no timestamp of its own, so it takes **the newest of everything under
/// it**, which is both a defensible answer and a useful one — it is what a Modified sort over a
/// tarball's top level wants. Zero, the alternative, draws as a dash on every folder in every
/// archive.
///
/// # The prefix match is case-sensitive
///
/// Archive paths are bytes and `src` is not `SRC` inside one, so this compares exactly, and two
/// entries differing only in case stay two rows. Every path this program builds for itself comes
/// from [`Dir::target`] joining a name this function emitted, so it always matches; the only way
/// to ask for a case the archive does not have is to type it by hand, which answers empty.
fn build(
    path: &Path,
    inside: &Inside,
    index: &Index,
    freshly_read: bool,
    started: Instant,
) -> Dir {
    let within = &inside.within;
    // Where an entry's own name starts, past the folder being listed and its separator.
    let from = if within.is_empty() {
        0
    } else {
        within.len() + 1
    };

    /// A row being accumulated. Kept as values rather than pushed straight into the builder
    /// because a folder's date is the newest of its children and is not known until the last of
    /// them has been seen.
    struct Row {
        size: u64,
        modified: u64,
        flags: u16,
    }

    let mut at: HashMap<&str, usize> = HashMap::new();
    // First-seen order, which for every archive worth reading is the order things were added.
    // The pane sorts it anyway; this only decides what an unsorted listing looks like.
    let mut names: Vec<&str> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();

    for item in &index.items {
        if !within.is_empty() {
            // The folder itself is not one of its own rows, and a sibling whose name merely starts
            // with the same letters — `srcfoo/x` under `src` — is not either: the byte after the
            // prefix has to be the separator.
            if !item.path.starts_with(within.as_str())
                || item.path.as_bytes().get(within.len()) != Some(&b'/')
            {
                continue;
            }
        }
        let rest = &item.path[from..];
        if rest.is_empty() {
            continue;
        }

        // A separator left in it means this entry is deeper than the folder on show, and what it
        // contributes here is the folder it is in.
        let (name, mut row) = match rest.find('/') {
            Some(cut) => (
                &rest[..cut],
                Row {
                    size: 0,
                    modified: item.modified,
                    flags: FLAG_DIR | FLAG_READONLY,
                },
            ),
            None => (
                rest,
                Row {
                    size: item.size,
                    modified: item.modified,
                    // Read-only throughout: nothing in an archive can be written, and the flag is
                    // what the details view and the rename guard already read.
                    flags: FLAG_READONLY
                        | if item.is_dir { FLAG_DIR } else { 0 }
                        | if item.unsized_ { FLAG_UNSIZED } else { 0 }
                        | if item.link { FLAG_LINK } else { 0 },
                },
            ),
        };

        match at.get(name) {
            // Seen already: an archive that stores `src/` as well as `src/main.rs`, or the second
            // of many files in the same subfolder. A folder keeps the newest date under it and its
            // own flags; a real entry arriving after an inferred folder replaces its size and
            // flags, because the archive's own record of a name beats one deduced from a child.
            Some(&index) => {
                let held = &mut rows[index];
                held.modified = held.modified.max(row.modified);
                if row.flags & FLAG_DIR == 0 {
                    held.size = row.size;
                    held.flags = row.flags;
                }
            }
            None => {
                at.insert(name, rows.len());
                names.push(name);
                // A folder's own stored size is noise, exactly as it is on a real volume.
                if row.flags & FLAG_DIR != 0 {
                    row.size = 0;
                }
                rows.push(row);
            }
        }
    }

    let mut builder = DirBuilder::new(path);
    builder.reserve(names.len());
    for (name, row) in names.iter().zip(&rows) {
        builder.push(name, row.size, row.modified, row.flags);
    }

    let mut dir = builder.finish(started.elapsed().as_micros() as u64);
    // The archive's own read is what the time went on, and it is charged to the folder that
    // triggered it rather than hidden: the first listing of a tarball is slow and the status line
    // should say so. **Once**, though — a later folder of the same archive reports only its own
    // slicing, which is microseconds, and which is the honest number for what that listing did.
    if freshly_read {
        dir.scan_micros += index.micros;
    }
    dir.truncated = index.truncated;
    dir
}
