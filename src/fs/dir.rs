//! The directory model.
//!
//! A directory is two allocations: one string holding every name end to end, and
//! one `Vec<Entry>` of 32-byte records pointing into it. That shape is the reason
//! this explorer can hold a 200,000-entry folder without the scan showing up as a
//! pause:
//!
//! - **One allocation for the names.** A `Vec<PathBuf>` would be one heap block
//!   per entry, and the allocator, not the disk, would be the bottleneck.
//! - **A fixed-size record.** 32 bytes means eight entries per cache line, so a
//!   sort or a filter pass streams instead of chasing pointers.
//! - **Raw values, not strings.** Sizes stay `u64` and times stay `FILETIME`, so
//!   sorting compares integers and formatting happens only for the ~40 rows that
//!   are actually on screen.
//!
//! A `Dir` is immutable once built, which is what lets it be an `Arc` shared by
//! every tab looking at the same folder while each keeps its own sort order.

use std::path::{Path, PathBuf};

/// This entry is a directory.
pub const FLAG_DIR: u16 = 1 << 0;
/// `FILE_ATTRIBUTE_HIDDEN`.
pub const FLAG_HIDDEN: u16 = 1 << 1;
/// `FILE_ATTRIBUTE_SYSTEM`.
pub const FLAG_SYSTEM: u16 = 1 << 2;
/// A reparse point: symlink, junction, mount point. **Not** a cloud placeholder, which is the file
/// itself rather than a pointer elsewhere — see [`FLAG_PLACEHOLDER`].
pub const FLAG_LINK: u16 = 1 << 3;
/// `FILE_ATTRIBUTE_READONLY`.
pub const FLAG_READONLY: u16 = 1 << 4;
/// The size is not knowable without decompressing the whole entry.
///
/// Only ever set by [`crate::archive`], and only for the single-stream formats — a lone `.bz2`,
/// `.zst` or `.lzma`, whose container records the compressed length and nothing about what comes
/// out of it. (A `.gz` is not one of them: its footer carries the uncompressed size, so it gets a
/// real one.)
///
/// The alternative was `0`, which the details view would render as `0 B` — a definite claim, and a
/// false one, about a file that may be a gigabyte. So the cell goes **blank** instead, exactly as
/// it already does for a directory, and for the same reason: this row has no size to show rather
/// than a size of nothing. See [`Entry::is_unsized`].
pub const FLAG_UNSIZED: u16 = 1 << 5;
/// A cloud-files placeholder: the reparse tag is `IO_REPARSE_TAG_CLOUD_*`, which is what OneDrive
/// (and any other provider built on the Cloud Files API) leaves on every file and folder it syncs.
///
/// Read off the find data's `dwReserved0`, which carries the tag for a reparse point — so knowing
/// that a folder is synced costs nothing the scan was not already given. See [`Sync`].
pub const FLAG_PLACEHOLDER: u16 = 1 << 6;
/// The content is in the cloud and not on this disk: `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`,
/// `RECALL_ON_OPEN` or `OFFLINE`. Explorer's blue cloud.
pub const FLAG_ONLINE: u16 = 1 << 7;
/// Kept on this device whatever happens to free space: `FILE_ATTRIBUTE_PINNED` without `UNPINNED`.
/// Explorer's solid green tick, "Always keep on this device".
pub const FLAG_PINNED: u16 = 1 << 8;

/// Where a synced file's content is, as Explorer's Status column says it.
///
/// Two sources, and they answer different rows:
///
/// - **A file answers from its attributes**, in [`Entry::sync`] — the three flags above, all read
///   from the scan that already happened. Free, and right on the frame the listing lands.
/// - **A folder cannot.** OneDrive leaves every folder `0x410` whatever is under it: the green tick
///   on a folder is the provider's own summary of its contents, and only the provider knows it. So a
///   folder's state is asked of the shell's property store — `System.StorageProviderState`, the
///   property Explorer's column is — off the UI thread. See [`crate::shell::cloud`].
///
/// The shell's answer, when it arrives, wins for a file too: it is the only source that knows about
/// an upload still pending or a file the provider could not sync.
///
/// **Declared in the order the column sorts in**: what is furthest from this disk first, then the
/// states with something wrong, so a click on the header gathers those together.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Sync {
    /// Available when online: nothing on this disk but the name.
    Online,
    /// On this disk, and the provider may free it again.
    Local,
    /// Always kept on this device.
    Pinned,
    /// On its way up or down.
    Syncing,
    /// The provider has something to say about it, short of failing.
    Warning,
    /// The provider could not sync it.
    Error,
}

impl Sync {
    /// Explorer's own words for the state, for the tooltip.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Online => "Available when online",
            Self::Local => "Available on this device",
            Self::Pinned => "Always available on this device",
            Self::Syncing => "Sync pending",
            Self::Warning => "Needs attention",
            Self::Error => "Sync error",
        }
    }
}

/// One name in a [`Dir`], plus everything the details view and the sort need.
///
/// Laid out largest field first so the record is exactly 32 bytes with no
/// interior padding, and the name is a slice of the directory's one string
/// rather than an allocation of its own.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// Bytes, from the find data. Meaningless for a directory, which is why the
    /// details view leaves that cell blank rather than printing `0 B`.
    pub size: u64,
    /// Raw `FILETIME`: 100-nanosecond ticks since 1601-01-01 UTC. Kept raw so a
    /// sort by date is an integer compare and no timezone maths happens for a row
    /// nobody is looking at.
    pub modified: u64,
    /// Byte offset of the name in [`Dir::names`].
    name_off: u32,
    /// Byte length of the name. A path component is at most 255 UTF-16 units, so
    /// this cannot overflow.
    name_len: u16,
    /// Byte offset *within the name* of the extension, past the dot. `0` means
    /// there is none — an offset of 0 is impossible for a real extension, since
    /// a leading dot makes a dotfile, not an extension.
    ext_off: u16,
    /// The `FLAG_*` set.
    pub flags: u16,
}

impl Entry {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.flags & FLAG_DIR != 0
    }

    #[inline]
    pub fn is_hidden(&self) -> bool {
        self.flags & (FLAG_HIDDEN | FLAG_SYSTEM) != 0
    }

    #[inline]
    pub fn is_link(&self) -> bool {
        self.flags & FLAG_LINK != 0
    }

    /// Whether [`Entry::size`] means nothing for this row. See [`FLAG_UNSIZED`].
    #[inline]
    pub fn is_unsized(&self) -> bool {
        self.flags & FLAG_UNSIZED != 0
    }

    /// Where a synced entry's content is, as far as its attributes say. See [`Sync`].
    ///
    /// `None` for anything that is not a placeholder, and for a folder that is neither pinned nor
    /// waiting to be populated — its attributes say nothing about it, and the shell has to be asked.
    #[inline]
    pub fn sync(&self) -> Option<Sync> {
        if self.flags & FLAG_PINNED != 0 {
            Some(Sync::Pinned)
        } else if self.flags & FLAG_ONLINE != 0 {
            Some(Sync::Online)
        } else if self.flags & FLAG_PLACEHOLDER != 0 && !self.is_dir() {
            Some(Sync::Local)
        } else {
            None
        }
    }
}

/// A scanned directory: immutable, shared, and the unit the cache stores.
pub struct Dir {
    /// The folder this is a listing of. Empty means the synthetic "This PC".
    pub path: PathBuf,
    /// Every name, end to end, unseparated. Only [`Dir::name`] indexes it.
    names: String,
    pub entries: Vec<Entry>,
    /// Where an entry leads, for synthetic listings whose rows are not children
    /// of `path` — the drives under "This PC". Empty for a real directory, where
    /// [`Dir::target`] joins the name onto the path instead.
    targets: Vec<PathBuf>,
    pub dir_count: u32,
    pub file_count: u32,
    /// Summed size of the files (not the directories, whose `size` is noise).
    pub total_size: u64,
    /// Why the listing is empty, if it is empty because something went wrong.
    pub error: Option<String>,
    /// The read failed for want of credentials, and signing in could fix it.
    ///
    /// Kept as well as [`Dir::error`] because the words are for the user and this is for the
    /// program: "Access denied" on `C:\Windows\System32\config` is final, and the same words on
    /// `\\server\share` mean a server that has not been told who is asking. Only the code the
    /// syscall returned can tell those apart, and by the time it is a sentence it is gone — so the
    /// question is answered where the code still exists. See [`crate::fs::scan::wants_credentials`].
    pub credentials: bool,
    /// The read stopped at a limit rather than at the end of what is there.
    ///
    /// Only ever true for a flattened listing — [`crate::fs::scan::scan_deep`] — which
    /// is the one read in this program that has no natural end. The status line says so,
    /// because a listing that is missing rows and does not admit it is the one kind of
    /// wrong answer a file manager must not give.
    pub truncated: bool,
    /// Some entry is a cloud-files placeholder, so this is a folder a sync provider looks after and
    /// the details view shows a Status column. Decided while the entries were pushed, so asking costs
    /// nothing per frame. See [`Sync`].
    pub synced: bool,
    /// This is the Recycle Bin: each name is the path an item was deleted from, each target the
    /// `$R…` file the bin holds it as, and `modified` is when it was deleted. See
    /// [`crate::fs::recycle`], which sets it, for what that changes about a row.
    ///
    /// A field rather than a question of [`Dir::path`] because the sort, the header and the icon of
    /// every row on screen ask it, and a bool is what that should cost.
    pub recycled: bool,
    /// How long the scan took. Shown in the status bar, which is the only honest
    /// way to claim the word "fast".
    pub scan_micros: u64,
}

impl Dir {
    /// The name of entry `i`.
    #[inline]
    pub fn name(&self, i: usize) -> &str {
        let e = &self.entries[i];
        let start = e.name_off as usize;
        &self.names[start..start + e.name_len as usize]
    }

    /// The file's own name, without the folders in front of it.
    ///
    /// The same thing as [`Dir::name`] for an ordinary listing, where a name is one
    /// path component. In a **flattened** listing — [`crate::fs::scan::scan_deep`] —
    /// a name is the path relative to the folder being flattened, because that is
    /// what a row has to show and what sorting and filtering have to see. This is
    /// the part of it that is the file: what a rename edits, and what a row is
    /// revealed by.
    #[inline]
    pub fn leaf(&self, i: usize) -> &str {
        leaf_of(self.name(i))
    }

    /// The folders in front of entry `i`'s own name — where it is, relative to the
    /// folder being listed.
    ///
    /// `""` in an ordinary listing, where a name is one component and every row is in
    /// the folder on show. In a **flattened** one it is what the Name column shows after
    /// the name, dimmed: `filelist.rs` is `ui`, and `heads` is `logs\refs`.
    #[inline]
    pub fn within(&self, i: usize) -> &str {
        let name = self.name(i);
        name[..name.len() - self.leaf(i).len()].trim_end_matches(['\\', '/'])
    }

    /// How many folders entry `i` is below the one being listed.
    ///
    /// `0` in an ordinary listing, where every row is in the folder on show. In a **flattened**
    /// one it is what the tree view indents by — see [`crate::pane::FlatMode::Tree`] — and it is
    /// the separator count rather than a stored field because that is what the name already is:
    /// the walk composed each name out of its parent's, a separator and its own.
    #[inline]
    pub fn depth(&self, i: usize) -> usize {
        self.name(i)
            .bytes()
            .filter(|&byte| byte == b'\\' || byte == b'/')
            .count()
    }

    /// The extension of entry `i`, lowercase-insensitive as stored, without the
    /// dot. `""` when there is none.
    #[inline]
    pub fn ext(&self, i: usize) -> &str {
        let e = &self.entries[i];
        if e.ext_off == 0 {
            return "";
        }
        let start = e.name_off as usize + e.ext_off as usize;
        let end = e.name_off as usize + e.name_len as usize;
        &self.names[start..end]
    }

    /// Where opening entry `i` goes.
    pub fn target(&self, i: usize) -> PathBuf {
        if let Some(explicit) = self.targets.get(i) {
            return explicit.clone();
        }
        self.path.join(self.name(i))
    }

    /// The target of entry `i` when it is *not* a child of this folder.
    ///
    /// `Some` only for a synthetic listing — the drives under This PC — and that is exactly
    /// when a row's icon has to be asked about by path rather than by type: a volume's icon
    /// is its own, and joining `Windows (C:)` onto an empty path names nothing at all, which
    /// is why those rows drew the generic folder.
    ///
    /// **Not for the Recycle Bin**, whose rows do have targets elsewhere but are files like any
    /// other: their icon is their type's, and a bin of thousands asking the shell per path would be
    /// thousands of questions for the answer the extension already gives.
    pub fn explicit_target(&self, i: usize) -> Option<&Path> {
        if self.recycled {
            return None;
        }
        self.targets
            .get(i)
            .filter(|target| !target.as_os_str().is_empty())
            .map(PathBuf::as_path)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A directory that could not be read. Still a `Dir`, so the UI has one code
    /// path: a failed listing is an empty one that knows why.
    pub fn failed(path: impl Into<PathBuf>, error: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            names: String::new(),
            entries: Vec::new(),
            targets: Vec::new(),
            dir_count: 0,
            file_count: 0,
            total_size: 0,
            error: Some(error.into()),
            credentials: false,
            truncated: false,
            synced: false,
            recycled: false,
            scan_micros: 0,
        }
    }

    /// The same, marked as a failure a sign-in could fix. See [`Dir::credentials`].
    pub fn wanting_credentials(mut self, wanted: bool) -> Self {
        self.credentials = wanted;
        self
    }
}

/// Accumulates a [`Dir`] one entry at a time.
///
/// The scanner pushes into this in find-order and never looks back, so both
/// vectors grow forwards only and the name string is written once.
pub struct DirBuilder {
    path: PathBuf,
    names: String,
    entries: Vec<Entry>,
    targets: Vec<PathBuf>,
    dir_count: u32,
    file_count: u32,
    total_size: u64,
    synced: bool,
}

impl DirBuilder {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            // A folder of a few hundred files is the common case and fits in one
            // grow from here; a folder of 200k reaches its final size in ~12
            // doublings, which is not where the time goes.
            names: String::with_capacity(8 * 1024),
            entries: Vec::with_capacity(256),
            targets: Vec::new(),
            dir_count: 0,
            file_count: 0,
            total_size: 0,
            synced: false,
        }
    }

    /// Reserve for a known entry count, when the caller has one.
    pub fn reserve(&mut self, entries: usize) {
        self.entries.reserve(entries);
        self.names.reserve(entries * 16);
    }

    pub fn push(&mut self, name: &str, size: u64, modified: u64, flags: u16) {
        let name_off = self.names.len() as u32;
        self.names.push_str(name);
        self.finish_entry(name_off, size, modified, flags);
    }

    /// How many entries are in the builder so far, for a caller working to a budget.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Push a name that arrived as UTF-16, transcoding straight into the arena.
    ///
    /// The obvious `String::from_utf16_lossy` would be one heap allocation per
    /// entry, thrown away immediately after being copied in — which for a folder
    /// of 200,000 files is 200,000 malloc/free pairs, and more time than the
    /// directory read itself. This writes the bytes where they are going.
    #[cfg(windows)]
    pub fn push_wide(&mut self, wide: &[u16], size: u64, modified: u64, flags: u16) {
        let name_off = self.names.len() as u32;
        {
            // SAFETY: every byte pushed below is valid UTF-8. The fast path
            // pushes single units already known to be < 0x80, and the tail goes
            // through `char::encode_utf8`. The borrow ends with this block, so
            // the string is a `String` again before anything else reads it.
            let bytes = unsafe { self.names.as_mut_vec() };
            bytes.reserve(wide.len());
            // Nearly every file name on a Windows volume is ASCII, and the ones
            // that are not are usually ASCII up to the first accent.
            let mut i = 0;
            while i < wide.len() && wide[i] < 0x80 {
                bytes.push(wide[i] as u8);
                i += 1;
            }
            if i < wide.len() {
                let mut buf = [0u8; 4];
                for c in char::decode_utf16(wide[i..].iter().copied()) {
                    let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
                    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
        }
        self.finish_entry(name_off, size, modified, flags);
    }

    /// Close the record for a name already appended at `name_off`.
    fn finish_entry(&mut self, name_off: u32, size: u64, modified: u64, flags: u16) {
        let name = &self.names[name_off as usize..];
        let name_len = name.len() as u16;

        let is_dir = flags & FLAG_DIR != 0;
        // A directory's dots are part of its name, not an extension: `v1.2` is a
        // folder, not a file of type `2`.
        let ext_off = if is_dir {
            0
        } else {
            // The extension is the last dot of the last *component*. A name here is
            // usually one component and then the two are the same thing — but a
            // flattened listing stores relative paths (see [`Dir::leaf`]), and
            // `v1.2\README` has a dot in it that belongs to the folder.
            let leaf = leaf_of(name);
            // `rfind` rather than `split`: `archive.tar.gz` is a `gz`, which is
            // what the shell thinks too.
            match leaf.rfind('.') {
                // A leading dot makes `.gitignore` a name, not an extension.
                Some(0) | None => 0,
                Some(dot) => (name.len() - leaf.len() + dot + 1) as u16,
            }
        };

        self.synced |= flags & (FLAG_PLACEHOLDER | FLAG_ONLINE | FLAG_PINNED) != 0;
        if is_dir {
            self.dir_count += 1;
        } else {
            self.file_count += 1;
            self.total_size += size;
        }

        self.entries.push(Entry {
            size,
            modified,
            name_off,
            name_len,
            ext_off,
            flags,
        });
    }

    /// Push an entry that leads somewhere other than `path/name` — a drive row
    /// under "This PC".
    pub fn push_link(&mut self, name: &str, target: PathBuf, size: u64, flags: u16) {
        // `targets` is indexed in lockstep with `entries`, so a builder that
        // mixes plain and linked entries has to pad it.
        while self.targets.len() < self.entries.len() {
            self.targets.push(PathBuf::new());
        }
        self.push(name, size, 0, flags);
        self.targets.push(target);
    }

    /// Push an item of the Recycle Bin: named by the path it was deleted from, leading to the file
    /// the bin holds it as, and dated when it went. See [`crate::fs::recycle`].
    pub fn push_held(&mut self, original: &str, held: PathBuf, size: u64, deleted: u64, flags: u16) {
        while self.targets.len() < self.entries.len() {
            self.targets.push(PathBuf::new());
        }
        self.push(original, size, deleted, flags);
        self.targets.push(held);
    }

    /// Hand the arenas over as a finished listing, sized to what they hold.
    ///
    /// The reservations in [`DirBuilder::new`] and [`DirBuilder::reserve`] are exactly right
    /// for *building* — a folder of a few hundred files fills without a single reallocation —
    /// and exactly wrong for *holding*, because the listing then goes into a cache that keeps
    /// scores of them. A 2-entry folder was keeping the 8KB of names and 256 records it was
    /// given to start with: measured, 96 cached folders came to 1.6MB while holding about 300
    /// entries between them. Growth by doubling means a large folder can be carrying an arena
    /// up to twice the size of its contents, too.
    ///
    /// The cost is one reallocation and copy per folder read, on the worker thread that did
    /// the reading, against a syscall per entry that has already happened.
    pub fn finish(mut self, scan_micros: u64) -> Dir {
        self.names.shrink_to_fit();
        self.entries.shrink_to_fit();
        self.targets.shrink_to_fit();
        Dir {
            path: self.path,
            names: self.names,
            entries: self.entries,
            targets: self.targets,
            dir_count: self.dir_count,
            file_count: self.file_count,
            total_size: self.total_size,
            error: None,
            credentials: false,
            truncated: false,
            synced: self.synced,
            recycled: false,
            scan_micros,
        }
    }
}

/// The last component of a stored name. Free for the ordinary case, which has no
/// separator in it at all.
#[inline]
fn leaf_of(name: &str) -> &str {
    match name.rfind(['\\', '/']) {
        Some(at) => &name[at + 1..],
        None => name,
    }
}

/// The display name of a folder, for a tab title or a breadcrumb segment.
///
/// A drive root has no file name of its own, so `C:\` would come back empty from
/// [`Path::file_name`]; this gives `C:` instead. An empty path is "This PC", and
/// [`crate::fs::recycle::LOCATION`] is the Recycle Bin.
pub fn display_name(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        return "This PC".to_owned();
    }
    if crate::fs::recycle::is_bin(path) {
        return "Recycle Bin".to_owned();
    }
    if let Some(name) = path.file_name() {
        return name.to_string_lossy().into_owned();
    }
    // A root: `C:\`, or `\\server\share`.
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        text.into_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name in a flattened listing is a relative path, and both the extension and the
    /// leaf are properties of its *last component*.
    ///
    /// `v1.2\README` is the case that says why: a folder is allowed dots, so an extension
    /// read off the whole name would make a file with none a `2\README`, and the Type
    /// column, the type icon and the sort would all follow it there.
    #[test]
    fn an_extension_belongs_to_the_last_component_of_a_name() {
        let mut builder = DirBuilder::new("D:\\root");
        for (name, flags) in [
            ("plain.txt", 0),
            ("archive.tar.gz", 0),
            (".gitignore", 0),
            ("sub\\deep.rs", 0),
            ("v1.2\\README", 0),
            ("v1.2", FLAG_DIR),
            ("sub\\nested", FLAG_DIR),
        ] {
            builder.push(name, 0, 0, flags);
        }
        let dir = builder.finish(0);
        let ext_of = |want: &str| {
            let i = (0..dir.len())
                .find(|&i| dir.name(i) == want)
                .expect("pushed above");
            (dir.ext(i), dir.leaf(i))
        };

        assert_eq!(ext_of("plain.txt"), ("txt", "plain.txt"));
        assert_eq!(ext_of("archive.tar.gz"), ("gz", "archive.tar.gz"));
        assert_eq!(ext_of(".gitignore"), ("", ".gitignore"));
        assert_eq!(ext_of("sub\\deep.rs"), ("rs", "deep.rs"));
        assert_eq!(
            ext_of("v1.2\\README"),
            ("", "README"),
            "the dot belongs to the folder, not to the file"
        );
        // A directory's dots are part of its name either way.
        assert_eq!(ext_of("v1.2"), ("", "v1.2"));
        assert_eq!(ext_of("sub\\nested"), ("", "nested"));
    }
}
