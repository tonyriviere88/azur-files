//! Ordering a listing.
//!
//! Two things make this fast enough to re-run on every column click, even on a
//! folder of a few hundred thousand entries:
//!
//! - **Indices are sorted, not entries.** The order is a `Vec<u32>`, so a swap
//!   moves four bytes instead of thirty-two, and the `Dir` itself stays immutable
//!   and shareable between tabs.
//! - **Keys are already integers.** Size and date sort on the `u64` they were
//!   enumerated as. Only the name comparison touches text, and only as a
//!   tie-break for the other three.

use std::cmp::Ordering;

use azur_egui_theme::filter::Query;

use super::dir::Dir;

/// The four columns of the details view.
///
/// The discriminants are spelled out because the view indexes its width array by
/// them, and a column reordered here would otherwise silently resize the wrong one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub enum Column {
    Name = 0,
    Size = 1,
    Type = 2,
    Modified = 3,
}

impl Column {
    pub const ALL: [Column; 4] = [Self::Name, Self::Size, Self::Type, Self::Modified];

    /// Index into a per-column array.
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn header(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Type => "Type",
            Self::Modified => "Modified",
        }
    }

    /// Right-aligned, because the digits should line up.
    pub fn numeric(self) -> bool {
        matches!(self, Self::Size)
    }

    /// Which way a fresh click on this header should sort.
    ///
    /// Names want A→Z, but a click on Size or Modified almost always means "what
    /// is biggest" or "what did I just touch", so those start descending — which
    /// is Explorer's behaviour too.
    pub fn starts_ascending(self) -> bool {
        matches!(self, Self::Name | Self::Type)
    }
}

/// Which rows [`crate::pane::Lens::Images`] keeps: every picture, and the folders that lead to one.
///
/// **A bitmap over the listing rather than a test per row**, because the folders are not a question
/// about themselves. A file is a picture or it is not — `Kind::Image` in [`super::fmt`], the same
/// answer the Type column prints, so the lens cannot come to disagree with what the listing says a
/// row *is* — but a folder is kept for what is *under* it, which is only knowable by walking up from
/// each picture. Handed to [`build_order`] and [`build_tree_order`] as `keep`, where a `false` folder
/// takes its whole subtree with it.
///
/// `tree` is whether the listing on show is one: the ancestors are what makes a
/// [`crate::pane::FlatMode::Tree`] gallery reachable — a folder that is out cannot be descended into
/// — and in a flat list they would be rows saying nothing, since every picture already carries the
/// path down to it in its name.
///
/// The walk **up** from each picture, stopping at the first ancestor already marked, is
/// [`build_tree_order`]'s own for a name match: one step per row plus one per ancestor not yet
/// accounted for, rather than one per row per level.
pub fn image_rows(dir: &Dir, tree: bool) -> Vec<bool> {
    use std::collections::HashMap;

    let mut kept = vec![false; dir.len()];
    for (i, keep) in kept.iter_mut().enumerate() {
        *keep = !dir.entries[i].is_dir()
            && super::fmt::kind_of(dir.ext(i), false) == super::fmt::Kind::Image;
    }
    if !tree {
        return kept;
    }

    // Where each folder is, so a picture can be walked back up to the rows that lead to it.
    // Directories only — nothing is ever inside a file.
    let mut folder_at: HashMap<&str, u32> = HashMap::new();
    for i in 0..dir.len() {
        if dir.entries[i].is_dir() {
            folder_at.insert(dir.name(i), i as u32);
        }
    }
    for i in 0..dir.len() {
        if !kept[i] {
            continue;
        }
        let mut within = dir.within(i);
        while !within.is_empty() {
            let Some(&at) = folder_at.get(within) else {
                break;
            };
            if kept[at as usize] {
                break;
            }
            kept[at as usize] = true;
            within = dir.within(at as usize);
        }
    }
    kept
}

/// Build the display order for a directory.
///
/// `order` is reused between calls so a re-sort allocates nothing.
///
/// `keep` is the other half of the filter, for the part of it that is not about names: `Some` only
/// when a lens is on show, and then a row has to pass both. See [`crate::pane::Lens`].
#[allow(clippy::too_many_arguments)]
pub fn build_order(
    dir: &Dir,
    order: &mut Vec<u32>,
    column: Column,
    ascending: bool,
    show_hidden: bool,
    filter: &str,
    keep: Option<&dyn Fn(usize) -> bool>,
) {
    order.clear();
    order.reserve(dir.len());

    let query = Query::parse(filter);
    let filtering = !query.is_empty();
    let mut path = String::new();
    let base = if filtering { prefix(dir, &mut path) } else { 0 };
    for i in 0..dir.len() {
        let entry = &dir.entries[i];
        if !show_hidden && entry.is_hidden() {
            continue;
        }
        // The cheap test first: `keep` is a hash lookup per row and a name match is a walk over one.
        if let Some(keep) = keep {
            if !keep(i) {
                continue;
            }
        }
        if filtering {
            path.truncate(base);
            path.push_str(dir.name(i));
            if !query.matches(&path) {
                continue;
            }
        }
        order.push(i as u32);
    }

    sort_order(dir, order, column, ascending);
}

/// What the tree made of one row of the display order.
///
/// Parallel to the order rather than part of it, because the order is what everything else in the
/// program indexes rows by — a selection, a cursor, a band, a drop target — and none of that cares
/// how the tree is drawn. Only [`build_tree_order`] fills this in; a listing that is not a tree
/// leaves it empty and every reader treats that as "no tree, no chains, depth zero".
///
/// Both figures are the *displayed* tree's and not the file system's, and that is the whole point of
/// storing them: once a chain of folders is one row, `Dir::depth` is no longer how far in the row is
/// drawn, and the row's own name is no longer the last component of its path. Worked out once per
/// rebuild instead of per row per frame — and the two questions a row asks about its neighbours
/// ("has this folder anything in it", "which row is its parent") become a lookup rather than a scan
/// over a name.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// How far in the row is drawn: its depth in the tree **as shown**.
    ///
    /// `Dir::depth` minus every level merged into this row and into the rows above it. A folder
    /// chain merged into one row takes the place of the outermost folder in it, so its children are
    /// one level in from *that* and not from the path they happen to have.
    pub depth: u32,
    /// How many folders are merged into this row's name: `0` for `c`, `2` for `a > b > c`.
    pub merged: u32,
}

/// Build the display order for a flattened listing shown as **the tree it came from**.
///
/// The same listing as [`build_order`]'s and the same rows: [`super::scan::scan_deep`] walked a
/// folder into one `Dir` whose names are paths relative to it, and the two flatten modes are two
/// orders over that one listing. Nothing is re-read to switch between them — see
/// [`crate::pane::FlatMode`], which is where the choice lives.
///
/// What comes out is **pre-order**: a folder, then everything inside it, then the next folder.
/// Siblings are sorted among themselves by whichever column the header says, so a click on Size
/// orders each folder's own contents rather than shuffling the tree into a list. That is the
/// whole difference between the two modes, and it is why this is a separate pass rather than a
/// flag on the sort — a tree's order is not a permutation the comparator can produce.
///
/// # The three rules that are not the sort
///
/// - **A row that is out takes its subtree with it.** Hidden, or excluded by `keep`: you cannot
///   see inside a folder you cannot see, which is also what browsing one does — the folder is
///   what is hidden, not each file in it. So the walk simply does not descend.
/// - **A collapsed folder's children are not in the order at all**, which is what makes a
///   collapsed tree cheap: the rows below it are not drawn, not hit-tested and not scrolled
///   past, because as far as the listing is concerned they are not there.
/// - **A filter ignores the collapse state and keeps the folders that lead to a match.** Both
///   halves of that are the same decision. A match three levels down is unreachable without the
///   folders above it, so a tree that dropped them would be a list with holes; and having found
///   it, hiding it inside a folder the user closed an hour ago would be a filter that answered
///   and then covered the answer. Every search-in-a-tree does the same. The collapse state is
///   kept, not cleared, so clearing the filter puts the tree back as it was.
///
/// # And the one that is not the tree either: a chain of folders is one row
///
/// **A folder whose entire content is one folder is not worth a row of its own.** `src\main\java\com`
/// is four rows, four indents and four twisties saying nothing — there was never a choice to make at
/// any of them. So with `regroup` set they become one row called `src > main > java > com`, which is
/// what a chain of only-children actually *is*: one step.
///
/// The row that comes out is the **innermost** folder, and everything else follows from that. Its
/// Size, Type and Modified are that folder's, because that is the folder you would have arrived at;
/// opening the row goes there; its twisty and its collapse state are that folder's, so what shuts is
/// what has something in it; and the folders in front of it are shown in the Name column, dimmed,
/// because they are where the row *is* rather than what it is. Only its **indent** belongs to the
/// outermost of them — the chain stands in the tree exactly where the folder it starts with stood.
/// See [`TreeRow`], which is where the two figures that say all that come out.
///
/// A chain of any length collapses, since the rule applies again at every step. What stops one:
/// anything else in the folder (a file, a second folder), nothing at all in it, and a folder the user
/// has **shut** — a chain cannot reach through a closed door, and opening it merges the rest.
///
/// **It is asked of the rows that are on show**, which is the honest reading of "only one folder in
/// it": a folder holding one subfolder and one hidden file is a chain when hidden files are hidden
/// and two rows when they are shown, and under a filter a chain forms out of whatever the filter
/// left. That is the same rule search-in-a-tree follows everywhere, and it is what makes a filtered
/// tree readable instead of a ladder of single matches.
///
/// # What it costs, against the list
///
/// This pass does strictly more work than [`build_order`] — it groups every row by the folder it is
/// in before it sorts anything — and it is re-run on every keystroke in the filter box, so the
/// question is whether it is the tree's problem or [`crate::pane::FILTER_DELAY`]'s. Measured by
/// [`tests::filter_speed`] over a flattened `C:\Program Files`, 189,981 entries, release build:
///
/// | filter | rows it leaves | list | **tree** |
/// | --- | --- | --- | --- |
/// | `""` | 189,810 | 250 ms | **136 ms** |
/// | `"e"` | 189,810 | 287 ms | **172 ms** |
/// | `"ex"` | 65,461 | 124 ms | **130 ms** |
/// | `"exe"` | 4,103 | 28 ms | **82 ms** |
/// | `"!exe"` | 186,728 | 271 ms | **173 ms** |
///
/// **The tree is faster on every case that keeps the listing**, by nearly half, and the grouping is
/// what pays for it: sorting twenty thousand small sibling groups is `n log k` where one sort of
/// 190,000 rows is `n log n`, and that saving is larger than the hash pass costs. Where it loses is
/// a filter narrow enough that the list has almost nothing left to sort — the grouping is one pass
/// over every row whatever survives, so `exe` costs 82 ms against 28.
///
/// Which leaves the answer: the tree's *worst* case (173 ms) is better than the list's (287 ms), so
/// nothing here moves the delay this is measured against. The 82 ms is the one figure to keep in
/// mind, and it is a third of that delay.
#[allow(clippy::too_many_arguments)]
pub fn build_tree_order(
    dir: &Dir,
    order: &mut Vec<u32>,
    // Filled in step with `order`: one entry per row. See [`TreeRow`].
    shape: &mut Vec<TreeRow>,
    column: Column,
    ascending: bool,
    show_hidden: bool,
    filter: &str,
    keep: Option<&dyn Fn(usize) -> bool>,
    collapsed: &dyn Fn(&str) -> bool,
    regroup: bool,
) {
    use std::collections::HashMap;

    order.clear();
    order.reserve(dir.len());
    shape.clear();
    shape.reserve(dir.len());

    let query = Query::parse(filter);
    let filtering = !query.is_empty();

    // Which rows are in each folder, keyed by that folder's own relative path — which is exactly
    // what [`Dir::within`] answers, and it is a slice of the listing's arena rather than a string
    // built here. The keys line up because the walk composed both: a child's name is its parent's
    // name, a separator, and its own.
    let mut kids: HashMap<&str, Vec<u32>> = HashMap::new();
    // And where each folder *is*, so a match can be walked back up to the rows that lead to it.
    // Directories only — nothing is ever inside a file — which is a small fraction of a tree.
    let mut folder_at: HashMap<&str, u32> = HashMap::new();
    for i in 0..dir.len() {
        kids.entry(dir.within(i)).or_default().push(i as u32);
        if dir.entries[i].is_dir() {
            folder_at.insert(dir.name(i), i as u32);
        }
    }

    // Whether each row is on show under the filter: it matched, or something under it did.
    //
    // Walked **up** from each match rather than down from the root, and each walk stops at the
    // first ancestor already marked — so the whole pass costs one step per row plus one per
    // ancestor that was not already accounted for, rather than one per row per level.
    let mut wanted = vec![false; dir.len()];
    if filtering {
        // One buffer for the whole pass, for the reason [`prefix`] exists: the filter matches
        // against the whole path, and building one per row would be an allocation per row.
        let mut path = String::new();
        let base = prefix(dir, &mut path);
        for i in 0..dir.len() {
            path.truncate(base);
            path.push_str(dir.name(i));
            if !query.matches(&path) {
                continue;
            }
            wanted[i] = true;
            let mut within = dir.within(i);
            while !within.is_empty() {
                let Some(&at) = folder_at.get(within) else {
                    break;
                };
                if wanted[at as usize] {
                    break;
                }
                wanted[at as usize] = true;
                within = dir.within(at as usize);
            }
        }
    }

    // The Type column's ranks, over the whole listing and once. Per sibling group they would be
    // a `Vec` the length of the listing for every folder in it — see [`compare`], which takes
    // them for this reason.
    let ranks = if column == Column::Type {
        let all: Vec<u32> = (0..dir.len() as u32).collect();
        type_ranks(dir, &all)
    } else {
        Vec::new()
    };

    let blocked = |i: usize| {
        (!show_hidden && dir.entries[i].is_hidden()) || keep.is_some_and(|keep| !keep(i))
    };
    let sort_kids = |group: &mut [u32]| {
        group.sort_unstable_by(|&a, &b| compare(dir, &ranks, column, ascending, a, b));
    };
    // A folder's children, in display order, with everything that is out **taken off first**.
    //
    // Eagerly, where the rows used to be tested as they were popped off the stack. The merge has to
    // *count* them — "a folder with one folder in it" is a question about the rows that are on show,
    // not about the directory — and it is the same test on the same rows either way, moved earlier.
    // The sort is strictly cheaper for it: it no longer orders rows that are about to be dropped.
    //
    // `remove` rather than `get`: each group is walked once, and a map that empties as the walk goes
    // is one that cannot be made to loop.
    let mut take_kids = |folder: &str| -> Vec<u32> {
        let mut group = kids.remove(folder).unwrap_or_default();
        group.retain(|&k| !blocked(k as usize) && (!filtering || wanted[k as usize]));
        sort_kids(&mut group);
        group
    };

    // Pre-order on an explicit stack, each level holding its sorted children, how far through them
    // the walk is, and how many levels the merged chains above it have taken out of the indent —
    // which is what a recursive version would have held on the call stack. Explicit because the
    // depth is whatever somebody made it: a folder tree is user data, and a recursion over
    // user-supplied depth is a stack overflow waiting for the right directory.
    let mut stack: Vec<(Vec<u32>, usize, u32)> = vec![(take_kids(""), 0, 0)];
    while let Some(level) = stack.last_mut() {
        let Some(&index) = level.0.get(level.1) else {
            stack.pop();
            continue;
        };
        level.1 += 1;
        let above = level.2;

        // Down the chain of only-children, as far as it goes. What comes out is the row to push and
        // the children to walk next — and `merged` is how many folders ended up in its name.
        let mut index = index;
        let mut merged = 0;
        let children = loop {
            let i = index as usize;
            if !dir.entries[i].is_dir() {
                break Vec::new();
            }
            // A collapsed folder is a folder the walk stops at — but not while a filter is asking a
            // question, which is the third rule above. It is where a chain stops too: what is behind
            // a closed door is not on show, so it cannot be merged into the row in front of it.
            if !filtering && collapsed(dir.name(i)) {
                break Vec::new();
            }
            let group = take_kids(dir.name(i));
            match group.as_slice() {
                [only] if regroup && dir.entries[*only as usize].is_dir() => {
                    index = *only;
                    merged += 1;
                }
                _ => break group,
            }
        };

        let lift = above + merged;
        order.push(index);
        shape.push(TreeRow {
            depth: (dir.depth(index as usize) as u32).saturating_sub(lift),
            merged,
        });
        if !children.is_empty() {
            stack.push((children, 0, lift));
        }
    }
}

/// Sort an existing order in place, leaving the filter alone.
pub fn sort_order(dir: &Dir, order: &mut [u32], column: Column, ascending: bool) {
    // Type sorts on the label the column shows, which is a lookup per row rather than a
    // field — so the lookups happen once, up front, and become an integer per entry.
    // Empty for every other column, and never indexed by one. See [`type_ranks`].
    let ranks = if column == Column::Type {
        type_ranks(dir, order)
    } else {
        Vec::new()
    };
    // `sort_unstable_by` because the comparator is a total order down to the name,
    // so stability would only cost time.
    order.sort_unstable_by(|&a, &b| compare(dir, &ranks, column, ascending, a, b));
}

/// Which of two entries comes first, by the column on show.
///
/// Split out of [`sort_order`] because [`build_tree_order`] sorts each folder's children as a
/// group of its own and needs the same answer — the two orders differ in *what* is compared
/// against what, never in how.
///
/// `ranks` is [`type_ranks`]' answer, and empty for every column but Type. It is passed in
/// rather than built here for the reason the tree needs it: one `Vec` the size of the listing
/// per sibling group would be the listing's length squared.
fn compare(
    dir: &Dir,
    ranks: &[u32],
    column: Column,
    ascending: bool,
    a: u32,
    b: u32,
) -> Ordering {
    let (ea, eb) = (&dir.entries[a as usize], &dir.entries[b as usize]);
    // Directories first regardless of column or direction: a folder is a place and
    // a file is a thing, and mixing them by size makes a listing you have to read
    // twice. Explorer, File Pilot and every other shell do the same.
    match eb.is_dir().cmp(&ea.is_dir()) {
        Ordering::Equal => {}
        folders_first => return folders_first,
    }

    let primary = match column {
        Column::Name => Ordering::Equal,
        // A directory has no meaningful size, so within the folder block the
        // Size column may as well fall through to the name.
        Column::Size if ea.is_dir() => Ordering::Equal,
        Column::Size => ea.size.cmp(&eb.size),
        Column::Type => ranks[a as usize].cmp(&ranks[b as usize]),
        Column::Modified => ea.modified.cmp(&eb.modified),
    };
    let primary = if ascending { primary } else { primary.reverse() };

    match primary {
        Ordering::Equal => {
            let names = natural_cmp(dir.name(a as usize), dir.name(b as usize));
            // The tie-break follows the same direction, so reversing the sort
            // reverses the whole listing rather than scrambling each group.
            if ascending {
                names
            } else {
                names.reverse()
            }
        }
        other => other,
    }
}

/// A rank per entry for the Type column, so that ordering by rank orders by the label
/// the column actually shows.
///
/// Sorting on the extension instead — which is what this did first — produces an order
/// nobody can verify by looking at it: `.cpp` before `.exe` puts "C++ source" *above*
/// "Application", and a folder where one extension dominates (a source tree, a photo
/// directory) comes out looking exactly like a sort by name, because every row ties and
/// the tie-break is the name. That is the bug this fixes.
///
/// Comparing the labels directly would mean resolving one per comparison — `n log n`
/// lookups. So each *distinct* extension is resolved once, the labels are ranked among
/// themselves, and the comparator is left comparing two `u32`s. A folder of 60,000 files
/// has perhaps thirty distinct types, so the added work is thirty lookups and one sort
/// of thirty strings.
///
/// The ranking is *dense* — equal labels get the same rank — which is what puts `.jpg`
/// beside `.jpeg`: both say "JPEG image", so both rank equal and the name tie-break
/// interleaves them into one group instead of two adjacent ones.
fn type_ranks(dir: &Dir, order: &[u32]) -> Vec<u32> {
    let mut ranks = vec![0u32; dir.len()];
    // Extension -> which label it resolved to. Keyed by the extension exactly as
    // stored, so `.TXT` and `.txt` are two keys — they then resolve to the same label
    // and the dense ranking gives them the same rank anyway.
    let mut slot_of: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let mut labels: Vec<String> = Vec::new();
    let mut label = String::new();

    for &index in order {
        let i = index as usize;
        // Folders sort ahead of every file whatever the column, so their rank is never
        // compared against anything and does not need resolving.
        if dir.entries[i].is_dir() {
            continue;
        }
        let ext = dir.ext(i);
        ranks[i] = match slot_of.get(ext) {
            Some(&slot) => slot,
            None => {
                label.clear();
                super::fmt::type_label(ext, false, &mut label);
                labels.push(label.clone());
                let slot = (labels.len() - 1) as u32;
                slot_of.insert(ext, slot);
                slot
            }
        };
    }

    // Rank the distinct labels among themselves, then rewrite each entry's slot as its
    // label's rank.
    let mut by_label: Vec<u32> = (0..labels.len() as u32).collect();
    by_label.sort_unstable_by(|&a, &b| natural_cmp(&labels[a as usize], &labels[b as usize]));
    let mut rank_of = vec![0u32; labels.len()];
    let mut rank = 0u32;
    for (position, &slot) in by_label.iter().enumerate() {
        if position > 0 && labels[slot as usize] != labels[by_label[position - 1] as usize] {
            rank += 1;
        }
        rank_of[slot as usize] = rank;
    }
    for &index in order {
        let i = index as usize;
        if !dir.entries[i].is_dir() {
            ranks[i] = rank_of[ranks[i] as usize];
        }
    }
    ranks
}

// ---------------------------------------------------------------------------
// Natural ordering
// ---------------------------------------------------------------------------

/// Compare two names the way a person reads them: case-insensitively, and with
/// runs of digits compared as numbers so `file2` comes before `file10`.
///
/// Bytes rather than chars, with an ASCII fast path. Beyond ASCII this falls back
/// to comparing UTF-8 bytes, which is code-point order — so accented names sort
/// consistently but not by the locale's collation rules. Doing that properly needs
/// ICU, and the sort would no longer be free.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);

    while i < a.len() && j < b.len() {
        let (ca, cb) = (a[i], b[j]);

        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            // Skip leading zeros so `007` and `7` compare equal in value, and let
            // the shorter spelling win only if everything else ties.
            let (za, ia) = skip_zeros(a, i);
            let (zb, jb) = skip_zeros(b, j);
            let (ea, eb) = (digits_end(a, ia), digits_end(b, jb));
            let (la, lb) = (ea - ia, eb - jb);

            // More digits (after zeros) is a bigger number.
            match la.cmp(&lb) {
                Ordering::Equal => match a[ia..ea].cmp(&b[jb..eb]) {
                    Ordering::Equal => {}
                    unequal => return unequal,
                },
                unequal => return unequal,
            }
            // Equal in value: remember the zero padding as a last resort, so
            // `01` and `1` still have a stable order.
            if za != zb {
                return za.cmp(&zb);
            }
            i = ea;
            j = eb;
            continue;
        }

        let (la, lb) = (lower(ca), lower(cb));
        if la != lb {
            return la.cmp(&lb);
        }
        i += 1;
        j += 1;
    }

    // One is a prefix of the other, or they differ only in case.
    match (a.len() - i).cmp(&(b.len() - j)) {
        Ordering::Equal => a.cmp(b),
        unequal => unequal,
    }
}

#[inline]
fn lower(byte: u8) -> u8 {
    // Only ASCII: a byte in a UTF-8 continuation sequence is >= 0x80 and must be
    // left alone, or two different characters could fold onto each other.
    if byte.is_ascii_uppercase() {
        byte + 32
    } else {
        byte
    }
}

#[inline]
fn skip_zeros(s: &[u8], mut i: usize) -> (usize, usize) {
    let start = i;
    while i + 1 < s.len() && s[i] == b'0' && s[i + 1].is_ascii_digit() {
        i += 1;
    }
    (i - start, i)
}

#[inline]
fn digits_end(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
    }
    i
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

// What the filter box means is `azur_egui_theme::filter`'s to say: every word narrows, in
// any order, `!` excludes, `^` and `$` hold an end. It lives there rather than here because
// a filter field is a component of the design system and its behaviour is part of the
// component — the same reason a hover grey is not decided per window.
//
// What is decided here is **what the words are matched against: the whole path.** Not the
// name on the row, which was the rule while there was only ever one word. Two reasons:
//
// - A **flattened** listing (`super::scan::scan_deep`) is mostly a question about folders —
//   `!node_modules`, `!\target\`, `^src` — and the name is only the part after them. That
//   much already worked, because a flattened row's name *is* its path relative to the folder
//   being listed.
// - The folder on show is part of what you are looking at, so it should be part of what you
//   can say about it.
//
// The cost is one surprise worth knowing about: **every row shares the folder's own path**,
// so a fragment that appears in it keeps everything. Typing `s` in `C:\Sources` narrows
// nothing. A word or two in is where that stops mattering, and the filter waits for the
// typing to stop anyway (`crate::pane::FILTER_DELAY`) — so it is mostly a state you never
// see.

/// Write the path every row in `dir` shares into `path`, and answer how much of it that is.
///
/// One buffer for the whole pass, truncated back to this length per row: a listing of
/// 190,000 entries is 190,000 paths per keystroke, and [`Dir::target`] would allocate every
/// one of them. Empty for a synthetic listing — "This PC", whose rows carry their own
/// targets — where the row's name (`Windows (C:)`) is already the whole of what it says.
fn prefix(dir: &Dir, path: &mut String) -> usize {
    path.push_str(&dir.path.to_string_lossy());
    if !path.is_empty() && !path.ends_with(['\\', '/']) {
        path.push('\\');
    }
    path.len()
}

#[cfg(test)]
mod tests;
