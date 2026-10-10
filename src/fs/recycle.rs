//! The Recycle Bin, as a listing like any other.
//!
//! The bin is not a directory. It is a namespace extension stitched together out of one hidden
//! folder per volume — `X:\$Recycle.Bin\<SID>`, one per user — and each deleted item in it is
//! **two files**: `$RKOFDIE.txt`, which is the item itself under a mangled name, and
//! `$IKOFDIE.txt` beside it, a few hundred bytes saying where it came from, how big it was and
//! when it went. Listing the folder with `FindFirstFile` shows the mangled names and none of that;
//! that was why this program used to hand the bin to Explorer rather than show it.
//!
//! So this reads what Explorer reads. [`listing`] enumerates each volume's folder once, pairs every
//! `$I…` with its `$R…`, and reads the `$I…` — which is the one round trip per item the design
//! otherwise refuses, and here it is the whole of the information: the original path is in that
//! file and nowhere else. The shell's own enumeration would be the same read with COM and two
//! property lookups per item on top.
//!
//! # What a row is
//!
//! A bin listing is an ordinary [`Dir`] with [`Dir::recycled`] set, and three of its fields mean
//! something slightly different there:
//!
//! | | a folder | the Recycle Bin |
//! | --- | --- | --- |
//! | [`Dir::name`] | the name | the **original path** — so [`Dir::leaf`] is the name it had and [`Dir::within`] is its Original Location, shown dimmed beside it exactly as a flattened listing shows a row's folder |
//! | [`Dir::target`] | the file | the `$R…` file the bin holds it as, which is a real file: the preview, Open and the icon all work on it |
//! | [`crate::fs::dir::Entry::modified`] | last written | **when it was deleted**, which is the column the bin is sorted by in practice |
//!
//! # What cannot be done to a row, and what is done instead
//!
//! A `$R…` file is real, so every file operation in this program *would* work on one — and every
//! one of them would be wrong. Moving it out leaves its `$I…` behind; renaming it breaks the pair;
//! deleting it to the bin recycles a recycled file. The bin's own verbs are the only correct way to
//! act on its items: `undelete` puts one back, and `delete` on the *bin's* item removes both halves.
//! So those go through the shell's menu for the bin's items — see `crate::shell::ops::bin` — and
//! [`is_held`] is the guard everything else asks.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use super::dir::{Dir, DirBuilder};

#[cfg(windows)]
#[path = "../windows/recycle.rs"]
mod win;
#[cfg(windows)]
use win::bins;

/// The path a pane is at when it is showing the Recycle Bin.
///
/// The shell's own name for it, so the one string means the bin to this program, to
/// `SHParseDisplayName` — which is what gives the context menu the bin's own verbs, including
/// **Empty Recycle Bin** — and to `explorer.exe`. Nothing on a disk can be called this: a colon is
/// not allowed in a file name, and `shell` is not a drive letter.
pub const LOCATION: &str = "shell:RecycleBinFolder";

/// The shell's CLSID for the bin, which `::{…}` paths and `shell:::{…}` spell it with.
const CLSID: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";

/// What every refusal to act on an item in the bin says: the one way out of it is Restore.
///
/// One sentence for all of them — a copy, a cut, a drag, a rename, a job — because they are one
/// rule, and a rule worded four ways reads as four rules. See [`is_held`].
pub const RESTORE_FIRST: &str =
    "That is in the Recycle Bin. Restore it first — right-click it, then Restore.";

/// [`LOCATION`] as a path.
pub fn location() -> PathBuf {
    PathBuf::from(LOCATION)
}

/// Whether a pane at this path is showing the Recycle Bin.
///
/// Only the one spelling, in any case, and only ever asked of paths that have been through
/// [`canonical`] — which every path a tab points at has. A string comparison, so it is free to ask
/// per frame.
pub fn is_bin(path: &Path) -> bool {
    path.as_os_str().eq_ignore_ascii_case(LOCATION)
}

/// Where a path goes, if it is one of the other ways of naming the Recycle Bin.
///
/// Every spelling of the bin ends up at [`LOCATION`], because the tab, the history, the bookmarks
/// and the saved settings compare paths, and two spellings of one place are two places to all of
/// them. Which spellings:
///
/// - the shell's names: `shell:RecycleBinFolder` in any case, `::{645FF040-…}`, `shell:::{645FF040-…}`;
/// - **the folders themselves**: `D:\$Recycle.Bin` and `D:\$Recycle.Bin\<SID>`. Those are real
///   directories and would list, as mangled `$R…` names with nothing to say what they were. So
///   they are the bin — which is also what makes Up out of a deleted folder you have opened land
///   back in the bin rather than in the raw folder it is held in.
///
/// Anything deeper is left alone: `D:\$Recycle.Bin\<SID>\$RKOFDIE` is a deleted *folder*, which
/// is a real directory with its contents under their real names, and browsing it is the way to
/// see what is in one before putting it back.
///
/// No disk is asked anything, so this is safe on every navigation.
pub fn canonical(path: PathBuf) -> PathBuf {
    if is_alias(&path) || holds_items(&path) {
        return location();
    }
    path
}

/// One of the shell's names for the bin.
fn is_alias(path: &Path) -> bool {
    let text = path.as_os_str();
    let Some(text) = text.to_str() else { return false };
    let text = text.trim_end_matches(['\\', '/']);
    text.eq_ignore_ascii_case(LOCATION)
        || text.eq_ignore_ascii_case(CLSID)
        || text
            .get(..6)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("shell:"))
            && text[6..].eq_ignore_ascii_case(CLSID)
}

/// `X:\$Recycle.Bin`, or one user's folder directly inside it.
fn holds_items(path: &Path) -> bool {
    let mut parts = path.components();
    let (Some(Component::Prefix(_)), Some(Component::RootDir), Some(Component::Normal(bin))) =
        (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if !bin.eq_ignore_ascii_case("$Recycle.Bin") {
        return false;
    }
    matches!(
        (parts.next(), parts.next()),
        (None, _) | (Some(Component::Normal(_)), None)
    )
}

/// Whether this path is one of the items the bin holds — a `$R…` file or folder directly inside
/// a user's `$Recycle.Bin\<SID>`.
///
/// **The guard over items**, and what stops every ordinary file operation reaching one: see the
/// module header for why each of them would be wrong. Asked of what a row *targets*, so it is a
/// few string comparisons and never a question of the disk.
///
/// Only the item itself. A file inside a deleted folder — `…\<SID>\$RKOFDIE\notes.txt` — is an
/// ordinary file on an ordinary path, and copying it out is how you get one file back without
/// restoring the whole folder.
pub fn is_held(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    // Compared as bytes: `name[..2]` panics when byte 2 falls inside a character, as in `▽x`.
    if !(name.len() > 2 && name.as_bytes()[..2].eq_ignore_ascii_case(b"$R")) {
        return false;
    }
    let Some(user) = path.parent() else { return false };
    holds_items(user) && user.file_name().is_some_and(|sid| !sid.eq_ignore_ascii_case("$Recycle.Bin"))
}

/// What a `$I…` file says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    /// The size of the item when it was deleted — for a folder, everything in it, which is a
    /// figure the folder's own directory entry cannot give.
    pub size: u64,
    /// When it was deleted, as a `FILETIME`.
    pub deleted: u64,
    /// Where it was.
    pub original: PathBuf,
}

/// Read a `$I…` file's contents.
///
/// Two layouts, and the version number in front says which:
///
/// | | version 1 (Vista to 8.1) | version 2 (Windows 10 on) |
/// | --- | --- | --- |
/// | `0..8` | `1` | `2` |
/// | `8..16` | size | size |
/// | `16..24` | deletion `FILETIME` | deletion `FILETIME` |
/// | `24..` | the path, 260 UTF-16 units, NUL-padded | a `u32` count of units including the NUL, then the path |
///
/// Neither is documented; both are what every forensic tool reads, and version 2 exists because
/// version 1 could not hold a path over `MAX_PATH`. Anything shorter than it claims, or any other
/// version, is `None` — a file the bin itself would not show either.
pub fn parse_index(bytes: &[u8]) -> Option<Index> {
    let u64_at = |at: usize| -> Option<u64> {
        Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
    };
    let version = u64_at(0)?;
    let size = u64_at(8)?;
    let deleted = u64_at(16)?;
    let units: &[u8] = match version {
        1 => bytes.get(24..24 + 520)?,
        2 => {
            let count = u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?) as usize;
            bytes.get(28..28 + count.checked_mul(2)?)?
        }
        _ => return None,
    };
    let wide: Vec<u16> = units
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    if wide.is_empty() {
        return None;
    }
    Some(Index {
        size,
        deleted,
        original: PathBuf::from(String::from_utf16_lossy(&wide)),
    })
}

/// The folders the last [`listing`] read, for [`crate::watch`] to watch in the bin's place.
///
/// A process-wide answer rather than one carried on the [`Dir`], because the watcher is handed
/// paths and not listings — the same reason [`super::drives`] keeps what it has described. Only
/// folders that were actually read are in it: a volume with nothing deleted on it has no folder
/// for this user yet, and a card reader with no card has no folder at all, and a watch opened on
/// either would fail and be retried every time the set of watched folders changed.
static READ: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Where the bin's items are on disk, as far as the last listing found. See [`READ`].
pub fn folders() -> Vec<PathBuf> {
    READ.lock().map(|read| read.clone()).unwrap_or_default()
}

/// Everything in this user's Recycle Bin, on every local volume, as a [`Dir`].
///
/// Only ever called from [`super::scan::scan`], and so only ever on a loader worker: it reads one
/// small file per item. A bin of a few thousand items is a few thousand of those, which is
/// milliseconds warm and is the one question the bin cannot be asked any other way.
///
/// An item whose `$I…` cannot be read, or whose `$R…` has gone, is left out — the shell leaves it
/// out too. A volume whose folder cannot be read contributes nothing and is not an error: every
/// volume nothing has ever been deleted from is one of those.
pub fn listing(started: Instant) -> Dir {
    let mut read = Vec::new();
    // Every pair on every volume first, and then the reading: the `$I…` files are where the time
    // goes, and they are read together. See [`read_all`].
    let mut pairs: Vec<(PathBuf, u16, PathBuf)> = Vec::new();
    for folder in bins() {
        // The ordinary directory read, so an item's flags are the ones the folder it was deleted
        // from would have shown.
        let dir = super::scan::scan(&folder);
        if dir.error.is_some() {
            continue;
        }
        // Paired by what follows the `$R` or `$I`, which is the one thing the two names share.
        let mut held: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut indexes: Vec<(String, usize)> = Vec::new();
        for i in 0..dir.len() {
            let name = dir.name(i);
            // Checked: anything can be put in the bin's folder, and a name that starts with a
            // character wider than two bytes has no cut at byte 2.
            let Some((tag, key)) = name.split_at_checked(2).filter(|(_, key)| !key.is_empty())
            else {
                continue;
            };
            let key = key.to_ascii_lowercase();
            if tag.eq_ignore_ascii_case("$R") {
                held.insert(key, i);
            } else if tag.eq_ignore_ascii_case("$I") {
                indexes.push((key, i));
            }
        }
        for (key, index) in indexes {
            if let Some(item) = held.remove(&key) {
                pairs.push((dir.target(item), dir.entries[item].flags, dir.target(index)));
            }
        }
        read.push(folder);
    }
    if let Ok(mut known) = READ.lock() {
        *known = read;
    }

    let said = read_all(&pairs);
    let mut builder = DirBuilder::new(location());
    builder.reserve(pairs.len());
    for ((item, flags, _), said) in pairs.into_iter().zip(said) {
        let Some(said) = said else { continue };
        let name = said.original.to_string_lossy();
        // A name is a `u16` of bytes in the arena; a path longer than that is one no listing
        // could show anyway.
        if name.len() > u16::MAX as usize {
            continue;
        }
        builder.push_held(&name, item, said.size, said.deleted, flags);
    }
    let mut dir = builder.finish(started.elapsed().as_micros() as u64);
    dir.recycled = true;
    dir
}

/// Every pair's `$I…` file, read and parsed, in the order the pairs are in.
///
/// **This is the whole cost of the bin**, and it is not CPU: each one is an open, a read of a few
/// hundred bytes and a close, about 110 µs apiece measured on this machine — half a second for a
/// bin of 4,387, read one after another. A thread that is waiting on the file system is a thread
/// another file could be read on, which is the argument [`super::scan::scan_deep`] makes for its
/// directories, and this reads on as many threads as that does.
///
/// Measured by [`tests::the_real_bin`], over those 4,387 items:
///
/// | threads | warm |
/// | --- | --- |
/// | 1 | 479 ms |
/// | [`super::scan::hands`], 8 here | **145 ms** |
///
/// 3.3×, which is the shape [`super::scan::scan_deep`] measured for the same reason.
///
/// A bin of a handful is read inline, since starting threads for it would cost more than it saves.
fn read_all(pairs: &[(PathBuf, u16, PathBuf)]) -> Vec<Option<Index>> {
    let read_one = |index: &Path| std::fs::read(index).ok().and_then(|bytes| parse_index(&bytes));
    let hands = super::scan::hands();
    if pairs.len() < 64 || hands <= 1 {
        return pairs.iter().map(|(_, _, index)| read_one(index)).collect();
    }
    let chunk = pairs.len().div_ceil(hands);
    std::thread::scope(|scope| {
        let workers: Vec<_> = pairs
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    // Per-thread, as every other reading thread in this program sets it: a
                    // removable volume that has gone must not raise "insert a disk".
                    super::scan::silence_device_dialogs();
                    part.iter()
                        .map(|(_, _, index)| read_one(index))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        // A worker that panicked contributes nothing rather than taking the listing with it — its
        // share of the items is left out, as an unreadable `$I…` would be.
        workers
            .into_iter()
            .zip(pairs.chunks(chunk))
            .flat_map(|(worker, part)| worker.join().unwrap_or_else(|_| vec![None; part.len()]))
            .collect()
    })
}

/// This user's folder on every volume that can have one — and on anything but Windows, none.
#[cfg(not(windows))]
fn bins() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version 2 `$I…` file, laid out the way Windows 10 writes one.
    fn index_v2(size: u64, deleted: u64, path: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u64.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&deleted.to_le_bytes());
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        bytes.extend_from_slice(&(wide.len() as u32).to_le_bytes());
        for unit in wide {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn both_index_layouts_read() {
        let path = r"C:\Users\somebody\Documents\Été\report.docx";
        assert_eq!(
            parse_index(&index_v2(1234, 0x01DA_0000_0000_0000, path)),
            Some(Index {
                size: 1234,
                deleted: 0x01DA_0000_0000_0000,
                original: PathBuf::from(path),
            })
        );

        let mut v1 = Vec::new();
        v1.extend_from_slice(&1u64.to_le_bytes());
        v1.extend_from_slice(&7u64.to_le_bytes());
        v1.extend_from_slice(&9u64.to_le_bytes());
        let mut wide: Vec<u16> = r"D:\old.txt".encode_utf16().collect();
        wide.resize(260, 0);
        for unit in wide {
            v1.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(
            parse_index(&v1),
            Some(Index {
                size: 7,
                deleted: 9,
                original: PathBuf::from(r"D:\old.txt"),
            })
        );
    }

    /// Short, unknown or empty is nothing, rather than a row with a made-up name.
    #[test]
    fn a_damaged_index_is_nothing() {
        let good = index_v2(1, 2, r"C:\a.txt");
        assert_eq!(parse_index(&good[..good.len() - 4]), None, "cut short");
        let mut unknown = good.clone();
        unknown[0] = 3;
        assert_eq!(parse_index(&unknown), None, "a version nobody has seen");
        assert_eq!(parse_index(&index_v2(1, 2, "")), None, "no path at all");
        assert_eq!(parse_index(&[]), None);
    }

    /// Every spelling of the bin is the one place, and nothing that merely resembles one is.
    #[test]
    fn every_name_for_the_bin_is_one_place() {
        for alias in [
            "shell:RecycleBinFolder",
            "SHELL:recyclebinfolder",
            "shell:RecycleBinFolder\\",
            "::{645FF040-5081-101B-9F08-00AA002F954E}",
            "shell:::{645ff040-5081-101b-9f08-00aa002f954e}",
            r"D:\$Recycle.Bin",
            r"c:\$RECYCLE.BIN\",
            r"D:\$Recycle.Bin\S-1-5-21-1-2-3-1001",
        ] {
            assert_eq!(canonical(PathBuf::from(alias)), location(), "{alias}");
        }
        for elsewhere in [
            r"D:\$Recycle.Bin\S-1-5-21-1-2-3-1001\$RKOFDIE",
            r"D:\Recycle.Bin",
            r"D:\stuff\$Recycle.Bin",
            r"D:\",
            "",
        ] {
            assert_eq!(canonical(PathBuf::from(elsewhere)), PathBuf::from(elsewhere), "{elsewhere}");
        }
        assert!(is_bin(&location()));
        assert!(!is_bin(Path::new("")));
    }

    /// What reading this machine's bin costs, and that the shell agrees about what is in it.
    ///
    /// Ignored because it reads the user's real bin — only reads: nothing is opened for writing and
    /// no verb is asked for. Run with `--ignored --nocapture the_real_bin`.
    #[test]
    #[ignore = "reads the user's real Recycle Bin; run explicitly"]
    fn the_real_bin() {
        for pass in ["cold", "warm"] {
            let started = Instant::now();
            let dir = listing(started);
            println!(
                "{pass}: {} items in {:.1} ms, from {:?}",
                dir.len(),
                started.elapsed().as_secs_f64() * 1000.0,
                folders()
            );
        }
    }

    /// An item is the `$R…` directly in a user's folder, and only that.
    #[test]
    fn only_the_bin_s_own_items_are_held() {
        let user = r"D:\$Recycle.Bin\S-1-5-21-1-2-3-1001";
        assert!(is_held(&Path::new(user).join("$RKOFDIE.txt")));
        assert!(is_held(&Path::new(user).join("$rkofdie")));
        assert!(!is_held(&Path::new(user).join("$IKOFDIE.txt")), "the index is not the item");
        assert!(
            !is_held(&Path::new(user).join(r"$RKOFDIE\inside.txt")),
            "a file inside a deleted folder is an ordinary file"
        );
        assert!(!is_held(Path::new(r"D:\$Recycle.Bin\$RKOFDIE.txt")), "not in a user's folder");
        assert!(!is_held(Path::new(r"D:\work\$Report.txt")));
    }

    /// A name whose first character is three bytes long. Slicing it at byte 2 panicked, and every
    /// Delete and Ctrl+C asks this first, so selecting the file and pressing either took the
    /// window down.
    #[test]
    fn a_name_starting_with_a_wide_character_is_not_held() {
        assert!(!is_held(Path::new(r"D:\work\▽┊Button up Shirt.pmp")));
        assert!(!is_held(Path::new(r"D:\work\📁x")));
    }
}
