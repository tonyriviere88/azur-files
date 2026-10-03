//! Keywords on any file: a sidecar store, keyed by the file's NTFS identity rather than its path.
//!
//! # Why not the property Explorer shows
//!
//! Explorer's *Tags* column is `System.Keywords`, and it is not a property of the file system: it is
//! written *into the file* by the property handler for its type — XMP inside a JPEG, `core.xml`
//! inside a `.docx`. A `.txt`, a `.zip`, a folder or a source file has no handler and so no tags at
//! all, and reading the property is a COM call per file that opens and parses it. So this program
//! keeps its own, and they are its own: nothing else on the machine reads them.
//!
//! # Why the file ID, and what that costs
//!
//! A path is the wrong key: a rename or a move inside the volume is exactly the thing a tag should
//! survive. The NTFS file ID — the MFT record number and its sequence number, 128 bits on ReFS — is
//! stable across both, and it arrives with the directory read for nothing: see
//! [`crate::fs::scan`], which enumerates with `FileIdExtdDirectoryInfo` for exactly this. Keyed with
//! the volume's serial, because two volumes hand out the same numbers.
//!
//! What it does **not** survive, and nothing short of writing into the file would:
//!
//! - a copy, or a move to another volume — the destination is a new record;
//! - a save that writes a new file and renames it over the old one, which is how plenty of editors
//!   save — the name is the same and the record is not.
//!
//! The sequence number is what makes a *reused* record safe: a deleted file's slot handed to a new
//! file comes back with a different ID, so it does not inherit anything. An entry whose file has
//! gone is simply never looked up again; nothing sweeps them, and a line each is what they cost.
//!
//! **Only NTFS and ReFS.** FAT and exFAT report an "ID" that is where the entry sits in its
//! directory, which moves; a listing on one comes back with no IDs, and the column is not drawn.
//!
//! # The file
//!
//! `keywords.tsv` beside `config.ini`, one line per tagged file:
//!
//! ```text
//! 3a1f00c2d4e5b6a7:0000000000000000000200000001a2b3 → work; 2026 → D:\Projects\report.pdf
//! ```
//!
//! The key, the keywords, and the path the file had when it was last tagged, separated by tabs —
//! the arrows above. The path is never
//! read back as a key — it is there so the file can be read by a person, and so a tag whose file
//! was copied elsewhere can at least be found by hand. Unparseable lines are skipped, never fatal,
//! as in `config.ini`.
//!
//! Loaded once, on first use, and written whole on every change: a change is one person typing into
//! one cell, and the file is a line per tagged file. **Never from a test** — for the reason
//! [`crate::config::Config::save`] gives, and a test does not read the user's file either.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock, RwLockReadGuard};

/// Which file, independent of where it is: the volume's serial and the file's ID on it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileKey {
    pub volume: u64,
    pub id: u128,
}

impl FileKey {
    fn to_text(self) -> String {
        format!("{:016x}:{:032x}", self.volume, self.id)
    }

    fn parse(text: &str) -> Option<Self> {
        let (volume, id) = text.split_once(':')?;
        Some(Self {
            volume: u64::from_str_radix(volume, 16).ok()?,
            id: u128::from_str_radix(id, 16).ok()?,
        })
    }
}

/// One tagged file.
#[derive(Clone, Debug, PartialEq)]
struct Tagged {
    keywords: String,
    /// Where it was when it was last tagged. For a person reading the file; see the module header.
    path: String,
}

/// Every file that has keywords.
#[derive(Default, Debug)]
pub struct Store {
    tags: HashMap<FileKey, Tagged>,
}

impl Store {
    /// The keywords of a file, if it has any.
    #[inline]
    pub fn get(&self, key: FileKey) -> Option<&str> {
        self.tags.get(&key).map(|tagged| tagged.keywords.as_str())
    }

    /// Give a file these keywords, or take them all away when there are none. The text is tidied
    /// first — see [`normalize`]. Returns whether anything changed.
    pub fn set(&mut self, key: FileKey, typed: &str, path: &Path) -> bool {
        let keywords = normalize(typed);
        if keywords.is_empty() {
            return self.tags.remove(&key).is_some();
        }
        let path = path.to_string_lossy().into_owned();
        let tagged = Tagged { keywords, path };
        if self.tags.get(&key) == Some(&tagged) {
            return false;
        }
        self.tags.insert(key, tagged);
        true
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.tags.len()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Read the file's contents. Anything that does not parse is skipped.
    pub(crate) fn parse(text: &str) -> Self {
        let mut store = Self::default();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut fields = line.splitn(3, '\t');
            let (Some(key), Some(keywords)) = (fields.next(), fields.next()) else {
                continue;
            };
            let Some(key) = FileKey::parse(key) else { continue };
            // Tidied on the way in too, so a hand-edited line reads the same as a typed one.
            let keywords = normalize(keywords);
            if keywords.is_empty() {
                continue;
            }
            let path = fields.next().unwrap_or_default().to_owned();
            store.tags.insert(key, Tagged { keywords, path });
        }
        store
    }

    /// What the file should hold. **Sorted by path**, so that a file that has not changed is written
    /// byte for byte the same — a `HashMap` would shuffle it on every save — and so that it reads
    /// folder by folder.
    pub(crate) fn to_text(&self) -> String {
        let mut rows: Vec<(&FileKey, &Tagged)> = self.tags.iter().collect();
        rows.sort_by(|a, b| {
            a.1.path
                .cmp(&b.1.path)
                .then_with(|| (a.0.volume, a.0.id).cmp(&(b.0.volume, b.0.id)))
        });
        let mut text = format!("# {} keywords\n", crate::brand::NAME);
        for (key, tagged) in rows {
            text.push_str(&key.to_text());
            text.push('\t');
            text.push_str(&tagged.keywords);
            text.push('\t');
            text.push_str(&tagged.path);
            text.push('\n');
        }
        text
    }
}

/// Keywords as they are kept: separated by `; `, each trimmed, none empty, none twice.
///
/// **Either separator is accepted.** Explorer's own field takes `;`, and a comma is what anybody
/// types who has not been told — splitting on both costs nothing but the ability to put a comma
/// *inside* a keyword, which is not a thing a tag needs. A repeat is dropped by case, keeping the
/// first spelling, so `Work; work` is one keyword.
///
/// Tabs and line breaks go too, because they are the store's own separators.
pub fn normalize(typed: &str) -> String {
    let mut out = String::with_capacity(typed.len());
    let mut seen: Vec<String> = Vec::new();
    for word in typed.split([';', ',']) {
        let word: String = word
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let word = word.split_whitespace().collect::<Vec<_>>().join(" ");
        if word.is_empty() {
            continue;
        }
        let folded = word.to_lowercase();
        if seen.contains(&folded) {
            continue;
        }
        seen.push(folded);
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(&word);
    }
    out
}

// ---------------------------------------------------------------------------
// The one store the window uses
// ---------------------------------------------------------------------------

/// The window's keywords, loaded the first time anything asks.
///
/// **Process-wide rather than a field of `App`**, because the two places that read it have no `App`
/// to reach: the sort, which a tab rebuilds from a dozen places — see
/// [`crate::pane::Tab::rebuild_order`] — and the row loop. A read lock per frame and per sort, and a
/// write lock only in `App::apply`, which is where a change is allowed to happen.
fn global() -> &'static RwLock<Store> {
    static STORE: OnceLock<RwLock<Store>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(load()))
}

/// The window's keywords, for reading.
pub fn read() -> RwLockReadGuard<'static, Store> {
    // A panic while the lock was held is a panic in `set`, which leaves the map whole either way.
    global().read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Give a file keywords, and write the store back out. Returns whether anything changed.
pub fn set(key: FileKey, typed: &str, path: &Path) -> bool {
    let mut store = global().write().unwrap_or_else(|poisoned| poisoned.into_inner());
    let changed = store.set(key, typed, path);
    if changed {
        save(&store);
    }
    changed
}

fn file() -> Option<PathBuf> {
    crate::config::profile_file("keywords.tsv")
}

fn load() -> Store {
    // Not the user's file, from a test: a test sees an empty store and builds its own.
    if cfg!(test) {
        return Store::default();
    }
    file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| Store::parse(&text))
        .unwrap_or_default()
}

/// Write the store, the way [`crate::config::Config::save`] writes the settings: beside the file and
/// renamed over it, so a crash mid-write leaves the previous keywords rather than half of them, and
/// with the previous contents kept as `.bak`.
fn save(store: &Store) {
    if cfg!(test) {
        return;
    }
    let Some(path) = file() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = store.to_text();
    if let Ok(existing) = std::fs::read(&path) {
        if !existing.is_empty() && existing != text.as_bytes() {
            let _ = std::fs::write(path.with_extension("tsv.bak"), &existing);
        }
    }
    let staged = path.with_extension("tsv.new");
    if std::fs::write(&staged, &text).is_ok() {
        if std::fs::rename(&staged, &path).is_ok() {
            return;
        }
        let _ = std::fs::remove_file(&staged);
    }
    let _ = std::fs::write(path, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: FileKey = FileKey {
        volume: 0x3a1f_00c2_d4e5_b6a7,
        id: 0x0002_0000_0001_a2b3,
    };

    #[test]
    fn keywords_are_tidied_into_one_spelling() {
        assert_eq!(normalize("work;2026"), "work; 2026");
        assert_eq!(normalize("  work ,  2026 ; "), "work; 2026", "a comma is a separator too");
        assert_eq!(normalize("Work; work; WORK"), "Work", "a repeat is dropped by case");
        assert_eq!(normalize("big   deal"), "big deal", "runs of space inside a keyword close up");
        assert_eq!(normalize("a\tb\nc"), "a b c", "the store's own separators cannot get in");
        assert_eq!(normalize(" ; ;, "), "");
    }

    #[test]
    fn the_file_reads_back_what_it_wrote() {
        let mut store = Store::default();
        assert!(store.set(KEY, "work, 2026", Path::new(r"D:\Projects\report.pdf")));
        let other = FileKey { volume: 7, id: u128::MAX };
        assert!(store.set(other, "a", Path::new(r"C:\a.txt")));

        let text = store.to_text();
        assert!(
            text.contains("3a1f00c2d4e5b6a7:0000000000000000000200000001a2b3\twork; 2026\tD:\\Projects\\report.pdf\n"),
            "{text}"
        );
        let back = Store::parse(&text);
        assert_eq!(back.len(), 2);
        assert_eq!(back.get(KEY), Some("work; 2026"));
        assert_eq!(back.get(other), Some("a"));
        assert_eq!(back.to_text(), text, "a store that has not changed writes the same bytes");
    }

    #[test]
    fn a_line_that_does_not_parse_is_skipped() {
        let store = Store::parse(
            "# header\n\
             nonsense\n\
             zz:00\tbad key\tC:\\x\n\
             0000000000000001:00000000000000000000000000000002\t\tC:\\empty\n\
             0000000000000001:00000000000000000000000000000003\tkept\r\n",
        );
        assert_eq!(store.len(), 1, "{store:?}");
        assert_eq!(store.get(FileKey { volume: 1, id: 3 }), Some("kept"));
    }

    #[test]
    fn clearing_the_keywords_forgets_the_file() {
        let mut store = Store::default();
        store.set(KEY, "x", Path::new("C:\\a"));
        assert!(!store.set(KEY, "x", Path::new("C:\\a")), "the same keywords are not a change");
        assert!(store.set(KEY, " ; ", Path::new("C:\\a")));
        assert!(store.is_empty());
        assert!(!store.set(KEY, "", Path::new("C:\\a")), "nothing to forget");
    }
}
