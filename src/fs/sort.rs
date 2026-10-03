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

/// The one word in the filter box that is not a word in a name.
///
/// **`@git` filters by what git says rather than by what the row is called**, which is the one
/// question about a folder that the name cannot answer and that people ask constantly: what have I
/// touched here. It is a *word* rather than a button because it composes — `@git rs` is both tests —
/// and because the filter box is already where you go to narrow a listing.
///
/// `@` for the same reason every other tool uses a sigil for this: it cannot be the start of a name
/// anybody is typing a fragment of, so nothing is taken away from the ordinary case. [`Query`] never
/// sees it — [`split_special`] takes it off first — so the design system's syntax is untouched and
/// this stays an application's idea about an application's data.
///
/// The word names **who is being asked**, not what the answer happens to be. It was `@changes`, which
/// read as a promise about the rows — and the set is wider than that word: staged, untracked and
/// conflicted are all in it, and so is a folder with any of them under it. `@git` says the only thing
/// that is true of all of them, and it is the word somebody reaching for this would guess first.
pub const CHANGED: &str = "@git";

/// Take [`CHANGED`] off a filter line, and say whether it was there.
///
/// Case-insensitively, and from anywhere in the line: it is a switch rather than a prefix, and a
/// switch that only works when it is typed first is a switch you have to remember the order of.
pub fn split_special(filter: &str) -> (bool, String) {
    let mut asked = false;
    let mut rest = String::with_capacity(filter.len());
    for word in filter.split_whitespace() {
        if word.eq_ignore_ascii_case(CHANGED) {
            asked = true;
            continue;
        }
        if !rest.is_empty() {
            rest.push(' ');
        }
        rest.push_str(word);
    }
    (asked, rest)
}

/// Build the display order for a directory.
///
/// `order` is reused between calls so a re-sort allocates nothing.
///
/// `keep` is the other half of the filter, for the part of it that is not about names: `Some` only when
/// the line asked a question a name cannot answer, and then a row has to pass both. See [`CHANGED`].
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
mod tests {
    use super::*;
    use crate::fs::dir::{DirBuilder, FLAG_DIR};

    /// What one keystroke in the filter box costs on a listing big enough to notice.
    ///
    /// ```text
    /// cargo test --release -- --ignored --nocapture filter_speed
    /// ```
    ///
    /// This is the measurement behind [`crate::pane::FILTER_DELAY`]: a filter is re-applied from
    /// scratch on every change, so if one pass is slow then *typing* is slow, and no amount of
    /// making the pass faster fixes a listing where every keystroke costs a pass. The tree is
    /// whatever `YAFE_FLATTEN_ROOT` names, flattened — which is the biggest listing this program
    /// can produce and the case the delay exists for.
    ///
    /// **Both flatten modes**, because the delay covers both and the tree's pass does strictly more
    /// work: it groups every row by the folder it is in before it sorts anything. If that were the
    /// difference between a pause and a stall it would be the tree mode's own problem and not the
    /// delay's, which is the thing this is here to say either way.
    #[test]
    #[ignore = "walks a large tree; run explicitly"]
    fn filter_speed() {
        let root = std::env::var("YAFE_FLATTEN_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target")
            });
        if !root.is_dir() {
            println!("no {}; skipping", root.display());
            return;
        }
        let dir = crate::fs::scan::scan_deep(
            &root,
            crate::fs::scan::FLATTEN_BUDGET,
            std::time::Duration::from_secs(600),
        );
        println!("{} entries from {}", dir.len(), root.display());

        let mut order = Vec::new();
        // Regrouped, which is how a tree is shown by default — so the figures below are the ones a
        // keystroke actually costs.
        let mut shape = Vec::new();
        // The single words are what typing `exe` costs, one keystroke at a time. The last
        // three are the syntax: a word that keeps everything, two words that each cost a
        // pass over every path, and an exclusion — which is the worst case, because a term
        // that fails is a term that has searched the whole path first.
        for filter in [
            "", "e", "ex", "exe", "microsoft", "^c:", "micro exe", "!exe",
        ] {
            // Twice, and the second is the one to read: the first pass warms the caches the
            // listing's own arena needs, which a real second keystroke would find warm too.
            let mut took = std::time::Duration::ZERO;
            let mut as_tree = std::time::Duration::ZERO;
            let mut tree_rows = 0;
            for _ in 0..2 {
                let started = std::time::Instant::now();
                build_order(&dir, &mut order, Column::Type, true, false, filter, None);
                took = started.elapsed();

                let started = std::time::Instant::now();
                build_tree_order(
                    &dir,
                    &mut order,
                    &mut shape,
                    Column::Type,
                    true,
                    false,
                    filter,
                    None,
                    &|_| false,
                    true,
                );
                as_tree = started.elapsed();
                tree_rows = order.len();
            }
            println!(
                "  filter {:>12}: list {:>7.2} ms · tree {:>7.2} ms ({tree_rows} rows)",
                format!("{filter:?}"),
                took.as_secs_f64() * 1000.0,
                as_tree.as_secs_f64() * 1000.0,
            );
        }
    }

    fn sorted(mut names: Vec<&str>) -> Vec<&str> {
        names.sort_by(|a, b| natural_cmp(a, b));
        names
    }

    /// A listing of the given names, folders marked by a trailing `/`.
    fn listing(names: &[&str]) -> Dir {
        let mut builder = DirBuilder::new(r"C:\x");
        for name in names {
            match name.strip_suffix('/') {
                Some(folder) => builder.push(folder, 0, 0, FLAG_DIR),
                None => builder.push(name, 1, 0, 0),
            }
        }
        builder.finish(0)
    }

    fn by(dir: &Dir, column: Column, ascending: bool) -> Vec<String> {
        let mut order = Vec::new();
        build_order(dir, &mut order, column, ascending, true, "", None);
        order
            .iter()
            .map(|&i| dir.name(i as usize).to_owned())
            .collect()
    }

    #[test]
    fn type_sorts_by_the_label_the_column_shows() {
        // Not by extension: `cpp` < `exe`, but "Application" < "C++ source". Sorting on
        // the extension put the column visibly out of order, which is what this is for.
        let dir = listing(&["b.cpp", "a.exe"]);
        assert_eq!(by(&dir, Column::Type, true), ["a.exe", "b.cpp"]);
        assert_eq!(by(&dir, Column::Type, false), ["b.cpp", "a.exe"]);
    }

    #[test]
    fn two_extensions_with_one_label_form_one_group() {
        // Both are "JPEG image", so they rank equal and the name tie-break interleaves
        // them — one group in the column rather than two that happen to be adjacent.
        let dir = listing(&["z.jpg", "a.jpeg", "m.jpg"]);
        assert_eq!(by(&dir, Column::Type, true), ["a.jpeg", "m.jpg", "z.jpg"]);
    }

    #[test]
    fn a_type_sort_still_orders_by_name_inside_a_type() {
        let dir = listing(&["b.txt", "a.txt", "c.exe"]);
        assert_eq!(by(&dir, Column::Type, true), ["c.exe", "a.txt", "b.txt"]);
    }

    #[test]
    fn folders_lead_every_column_and_ignore_its_key() {
        // A folder has no type of its own, and none of the labels may reorder it out of
        // the block at the top.
        let dir = listing(&["zebra/", "a.exe", "alpha/"]);
        assert_eq!(
            by(&dir, Column::Type, true),
            ["alpha", "zebra", "a.exe"],
            "folders first, in name order"
        );
        assert_eq!(
            by(&dir, Column::Type, false)[0],
            "zebra",
            "still folders first when reversed"
        );
    }

    #[test]
    fn digits_compare_as_numbers() {
        assert_eq!(
            sorted(vec!["file10.txt", "file2.txt", "file1.txt"]),
            vec!["file1.txt", "file2.txt", "file10.txt"]
        );
        assert_eq!(
            sorted(vec!["scan_100", "scan_9", "scan_20"]),
            vec!["scan_9", "scan_20", "scan_100"]
        );
    }

    #[test]
    fn case_is_ignored_but_still_breaks_ties() {
        assert_eq!(natural_cmp("Alpha", "alpha"), natural_cmp("Alpha", "alpha"));
        assert_eq!(natural_cmp("beta", "Alpha"), Ordering::Greater);
        // A pure case difference has to be *some* consistent order, not Equal, or
        // an unstable sort could swap two rows between frames.
        assert_ne!(natural_cmp("Alpha", "alpha"), Ordering::Equal);
    }

    #[test]
    fn leading_zeros_do_not_change_the_value() {
        assert_eq!(
            sorted(vec!["v007", "v10", "v7", "v0008"]),
            vec!["v7", "v007", "v0008", "v10"]
        );
    }

    #[test]
    fn prefixes_come_first() {
        assert_eq!(
            sorted(vec!["report.txt", "report", "reports"]),
            vec!["report", "report.txt", "reports"]
        );
    }

    #[test]
    fn ordering_is_a_total_order() {
        // An unstable sort with an inconsistent comparator can loop or panic, so
        // check antisymmetry across a set built to collide.
        let names = [
            "a", "A", "a1", "a01", "a2", "a10", "b", "", "1", "01", "2", "z9z", "z10z",
        ];
        for x in names {
            for y in names {
                assert_eq!(
                    natural_cmp(x, y).reverse(),
                    natural_cmp(y, x),
                    "`{x}` vs `{y}`"
                );
            }
        }
    }

    /// What survives `filter` in a listing of `C:\x`.
    fn kept(names: &[&str], filter: &str) -> Vec<String> {
        let dir = listing(names);
        let mut order = Vec::new();
        build_order(&dir, &mut order, Column::Name, true, true, filter, None);
        order
            .iter()
            .map(|&i| dir.name(i as usize).to_owned())
            .collect()
    }

    #[test]
    fn the_filter_is_a_case_insensitive_substring_of_the_name() {
        // The one-word case, which is what a filter box holds nearly all of the time.
        let names = ["README.md", "some-readme-file", "read.me"];
        assert_eq!(
            kept(&names, "  ReadMe "),
            ["README.md", "some-readme-file"],
            "either case, and not `read.me`, which has the letters but not the word"
        );
        assert_eq!(kept(&names, "").len(), 3, "no filter keeps everything");
        assert_eq!(kept(&names, "   ").len(), 3, "nor does whitespace");
    }

    #[test]
    fn the_filter_is_asked_about_the_whole_path() {
        // The folder is `C:\x`, so it is there to be matched, anchored and excluded — and
        // it is shared, so a word only it holds keeps every row.
        assert_eq!(kept(&["a.txt", "b.txt"], r"c:\x").len(), 2);
        assert_eq!(kept(&["a.txt", "b.txt"], "^C:").len(), 2);
        assert_eq!(kept(&["a.txt", "b.txt"], "!x").len(), 0);
        assert_eq!(kept(&["a.txt", "b.txt"], "^y").len(), 0);

        // And the folders in front of a flattened row's name are matched with it.
        assert_eq!(
            kept(&[r"deep\a.txt", "b.txt"], "deep"),
            [r"deep\a.txt"],
            "the row's own folders count"
        );
    }

    #[test]
    fn every_word_of_the_filter_narrows_in_any_order() {
        let names = ["hello world.txt", "world hello.txt", "hello to you.txt"];
        for filter in ["wor he", "he wor"] {
            assert_eq!(
                kept(&names, filter),
                ["hello world.txt", "world hello.txt"],
                "`{filter}`"
            );
        }
        // The two markers that make it a filter rather than a search. `.txt$` would hold
        // for all three, so the exclusion is what is being read here.
        assert_eq!(kept(&names, "!world .txt$"), ["hello to you.txt"]);
    }

    #[test]
    fn a_filter_of_nothing_but_markers_keeps_everything() {
        // Every prefix of a query is typed on the way to it, and a listing that empties on
        // the first keystroke of `^src` and fills again on the second is a flicker.
        for filter in ["^", "!", "$", "^$"] {
            assert_eq!(kept(&["a.txt", "b.txt"], filter).len(), 2, "`{filter}`");
        }
    }

    /// `@git` comes off the line before the name test ever sees it.
    ///
    /// The two things worth holding: it is **not** passed through to [`Query`], where it would be a
    /// literal nobody's file is called; and what is left of the line still filters, so the two
    /// compose. `Tab::rebuild_order` is where the git half is answered — this is only the parsing.
    #[test]
    fn the_git_word_is_taken_off_the_filter_and_the_rest_still_filters() {
        assert_eq!(split_special(""), (false, String::new()));
        assert_eq!(split_special("report"), (false, "report".to_owned()));
        assert_eq!(split_special(CHANGED), (true, String::new()));
        // Case, and either side of the rest of the line.
        assert_eq!(split_special("@GIT rs"), (true, "rs".to_owned()));
        assert_eq!(split_special("rs @git"), (true, "rs".to_owned()));
        assert_eq!(
            split_special("  @git   ^src  .rs$ "),
            (true, "^src .rs$".to_owned()),
            "the words that are left keep their markers and lose the extra air"
        );
        // A word that merely starts with it is a word, not the switch — and this one is a file
        // somebody filtering a repository will genuinely type.
        assert_eq!(
            split_special("@gitignore"),
            (false, "@gitignore".to_owned())
        );

        // And with it taken off, the rest still narrows by name.
        let names = ["one.rs", "two.txt"];
        let (asked, rest) = split_special("@git .rs$");
        assert!(asked);
        assert_eq!(kept(&names, &rest), ["one.rs"]);
    }

    /// The git half: a row has to pass both tests.
    #[test]
    fn the_git_half_of_a_filter_is_asked_of_every_row() {
        let mut builder = DirBuilder::new(r"C:\repo");
        for name in ["kept.rs", "gone.rs", "kept.txt"] {
            builder.push(name, 1, 0, 0);
        }
        let dir = builder.finish(0);
        let changed = |entry: usize| dir.name(entry).starts_with("kept");

        let mut order = Vec::new();
        build_order(
            &dir,
            &mut order,
            Column::Name,
            true,
            true,
            "",
            Some(&changed),
        );
        let names: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();
        assert_eq!(names, ["kept.rs", "kept.txt"], "the git test alone");

        build_order(
            &dir,
            &mut order,
            Column::Name,
            true,
            true,
            ".rs$",
            Some(&changed),
        );
        let names: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();
        assert_eq!(names, ["kept.rs"], "and both tests together");
    }

    // -----------------------------------------------------------------------
    // The tree the same rows make
    // -----------------------------------------------------------------------

    /// A flattened tree, in the order the walk produces one: a folder is always pushed before
    /// anything inside it, and the levels arrive one after another. The listing under test has to
    /// be built this way round or it is not testing what `scan_deep` hands over.
    fn tree_listing() -> Dir {
        listing(&[
            "docs/",
            "src/",
            "readme.md",
            r"docs\guide.md",
            r"src\ui/",
            r"src\main.rs",
            r"src\ui\list.rs",
            r"src\ui\theme.rs",
        ])
    }

    fn as_tree_by(dir: &Dir, column: Column, filter: &str, shut: &[&str]) -> Vec<String> {
        as_shown_by(dir, column, filter, shut, false)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect()
    }

    /// The tree as it is **shown**: every row's name, the depth it is drawn at, and how many folders
    /// are merged into it. Which is the whole of what [`TreeRow`] says, so the merge tests can assert
    /// the shape rather than infer it from the names.
    fn as_shown_by(
        dir: &Dir,
        column: Column,
        filter: &str,
        shut: &[&str],
        regroup: bool,
    ) -> Vec<(String, u32, u32)> {
        let (mut order, mut shape) = (Vec::new(), Vec::new());
        build_tree_order(
            dir,
            &mut order,
            &mut shape,
            column,
            true,
            true,
            filter,
            None,
            &|name| shut.contains(&name),
            regroup,
        );
        assert_eq!(order.len(), shape.len(), "the order and its shape came apart");
        order
            .iter()
            .zip(&shape)
            .map(|(&i, row)| (dir.name(i as usize).to_owned(), row.depth, row.merged))
            .collect()
    }

    /// The same, regrouped, which is how a tree is shown by default.
    fn regrouped(dir: &Dir, filter: &str, shut: &[&str]) -> Vec<(String, u32, u32)> {
        as_shown_by(dir, Column::Name, filter, shut, true)
    }

    fn as_tree(dir: &Dir, filter: &str, shut: &[&str]) -> Vec<String> {
        as_tree_by(dir, Column::Name, filter, shut)
    }

    /// **A folder, then what is inside it, then the next folder** — and folders before files at
    /// every level, which is the one rule the flat order already had.
    #[test]
    fn a_tree_is_pre_order_with_each_level_sorted_among_itself() {
        assert_eq!(
            as_tree(&tree_listing(), "", &[]),
            [
                "docs",
                r"docs\guide.md",
                "src",
                r"src\ui",
                r"src\ui\list.rs",
                r"src\ui\theme.rs",
                r"src\main.rs",
                "readme.md",
            ]
        );
    }

    /// The same rows the list mode shows, and only those: the two modes are two orders over one
    /// listing, so a row missing from one of them is a bug in that one.
    #[test]
    fn the_tree_and_the_list_hold_the_same_rows() {
        let dir = tree_listing();
        let mut flat = as_tree(&dir, "", &[]);
        let mut list = by(&dir, Column::Name, true);
        flat.sort();
        list.sort();
        assert_eq!(flat, list);
    }

    /// A shut folder takes its whole subtree out of the order — not merely off the screen.
    ///
    /// Which is what makes a collapsed tree cheap: the rows below it are not drawn, not hit-tested
    /// and not scrolled past, because as far as the listing is concerned they are not there.
    #[test]
    fn a_shut_folder_keeps_its_subtree_out_of_the_order() {
        let dir = tree_listing();
        assert_eq!(
            as_tree(&dir, "", &["src"]),
            ["docs", r"docs\guide.md", "src", "readme.md"],
            "`src` is still a row; nothing under it is"
        );
        // Shutting a folder in the middle keeps the folder and drops the two leaves under it.
        assert_eq!(
            as_tree(&dir, "", &[r"src\ui"]),
            [
                "docs",
                r"docs\guide.md",
                "src",
                r"src\ui",
                r"src\main.rs",
                "readme.md",
            ]
        );
    }

    /// **A filter keeps the folders that lead to a match**, or the match is a row with nothing
    /// above it to say where it came from.
    #[test]
    fn a_filtered_tree_keeps_the_folders_that_lead_to_a_match() {
        let dir = tree_listing();
        assert_eq!(
            as_tree(&dir, "theme", &[]),
            ["src", r"src\ui", r"src\ui\theme.rs"],
            "the two folders are not matches; they are the way to the one that is"
        );
        // And a match that is itself a folder brings what is inside it, because the filter matches
        // every row under it too — every one of their paths contains the folder's name.
        assert_eq!(
            as_tree(&dir, "docs", &[]),
            ["docs", r"docs\guide.md"]
        );
    }

    /// And it ignores what is shut, because a filter that answered and then hid the answer inside a
    /// folder you closed an hour ago would be worse than one that found nothing.
    #[test]
    fn a_filter_reaches_into_a_shut_folder() {
        let dir = tree_listing();
        assert_eq!(
            as_tree(&dir, "list.rs", &["src", r"src\ui"]),
            ["src", r"src\ui", r"src\ui\list.rs"],
        );
        // The set is not modified by that — clearing the filter puts the tree back as it was, which
        // is `Tab::collapsed` living on the tab and this taking a closure over it.
        assert_eq!(as_tree(&dir, "", &["src", r"src\ui"]).len(), 4);
    }

    /// A row that is out takes its subtree with it: you cannot see inside a folder you cannot see.
    ///
    /// The same statement for both tests that can exclude a row — `show_hidden` and the git
    /// question — because a tree has nothing to hang an orphan off. Browsing does the same: a
    /// hidden folder hides what is in it, and it is the folder that is marked hidden, not each file.
    #[test]
    fn a_hidden_folder_hides_what_is_under_it() {
        use crate::fs::dir::FLAG_HIDDEN;
        let mut builder = DirBuilder::new(r"C:\x");
        builder.push("shown", 0, 0, FLAG_DIR);
        builder.push("hidden", 0, 0, FLAG_DIR | FLAG_HIDDEN);
        builder.push(r"shown\a.txt", 1, 0, 0);
        // Not hidden itself — only the folder it is in is.
        builder.push(r"hidden\b.txt", 1, 0, 0);
        let dir = builder.finish(0);

        let (mut order, mut shape) = (Vec::new(), Vec::new());
        build_tree_order(
            &dir,
            &mut order,
            &mut shape,
            Column::Name,
            true,
            false,
            "",
            None,
            &|_| false,
            false,
        );
        let names: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();
        assert_eq!(names, ["shown", r"shown\a.txt"]);

        // With hidden files shown, both branches are there — which is what says the rule above is
        // about the folder being hidden and not about `b.txt`.
        build_tree_order(
            &dir,
            &mut order,
            &mut shape,
            Column::Name,
            true,
            true,
            "",
            None,
            &|_| false,
            false,
        );
        assert_eq!(order.len(), 4);
    }

    /// A sort by a column orders each folder's own children, and never moves a row out of the
    /// folder it is in — which is the difference between sorting a tree and flattening it.
    #[test]
    fn a_column_sort_orders_siblings_and_not_the_tree() {
        let mut builder = DirBuilder::new(r"C:\x");
        builder.push("a", 0, 0, FLAG_DIR);
        builder.push("z.txt", 9, 0, 0);
        builder.push(r"a\big.txt", 100, 0, 0);
        builder.push(r"a\small.txt", 1, 0, 0);
        let dir = builder.finish(0);

        let (mut order, mut shape) = (Vec::new(), Vec::new());
        build_tree_order(
            &dir,
            &mut order,
            &mut shape,
            Column::Size,
            false,
            true,
            "",
            None,
            &|_| false,
            false,
        );
        let names: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();
        assert_eq!(
            names,
            ["a", r"a\big.txt", r"a\small.txt", "z.txt"],
            "biggest first inside `a`, and `z.txt` is still not inside it"
        );
    }

    /// A folder chain fixture: one of every case the merge has to answer.
    ///
    /// Level by level, the way [`super::scan::scan_deep`] hands a listing over — `conf` with one
    /// *file* in it, `deep` with one empty folder, `lib` with two things in it, and `src` with a
    /// three-deep chain of only-children ending in a folder that holds two files.
    fn chain_listing() -> Dir {
        listing(&[
            "conf/",
            "deep/",
            "lib/",
            "src/",
            r"conf\app.ini",
            r"deep\deeper/",
            r"lib\a.txt",
            r"lib\b/",
            r"src\main/",
            r"lib\b\c.txt",
            r"src\main\java/",
            r"src\main\java\App.java",
            r"src\main\java\Other.java",
        ])
    }

    /// **A folder whose whole content is one folder is not a row of its own.**
    ///
    /// Four rules in one listing, and the depths are half of what is being asserted: the row that
    /// comes out is the *innermost* folder of the chain, drawn where the outermost one stood, with
    /// everything inside it one level in from there. A chain merges as many levels as the rule holds
    /// for — `src\main\java` is one row of three folders — and it stops at anything else in the
    /// folder: a file (`conf`), a second entry (`lib`), and nothing at all (`deep\deeper`, whose
    /// chain forms and then ends).
    #[test]
    fn a_folder_holding_nothing_but_one_folder_is_merged_into_its_row() {
        let shown = regrouped(&chain_listing(), "", &[]);
        assert_eq!(
            shown,
            [
                // One file in it is not one folder in it.
                ("conf".to_owned(), 0, 0),
                (r"conf\app.ini".to_owned(), 1, 0),
                // One folder, and it is empty: the chain is still a chain.
                (r"deep\deeper".to_owned(), 0, 1),
                // Two things in it, so both stay.
                ("lib".to_owned(), 0, 0),
                (r"lib\b".to_owned(), 1, 0),
                (r"lib\b\c.txt".to_owned(), 2, 0),
                (r"lib\a.txt".to_owned(), 1, 0),
                // Three folders, one row, drawn where `src` was — and its files are at depth 1
                // rather than at the 3 their paths would say.
                (r"src\main\java".to_owned(), 0, 2),
                (r"src\main\java\App.java".to_owned(), 1, 0),
                (r"src\main\java\Other.java".to_owned(), 1, 0),
            ]
        );
    }

    /// Turned off, the same listing is the whole ladder again.
    ///
    /// The other half of the option being an option — and the reason the figures matter: with nothing
    /// merged, every row's depth is its path's depth, which is what it was before any of this.
    #[test]
    fn without_regrouping_every_folder_of_a_chain_keeps_its_row() {
        let shown = as_shown_by(&chain_listing(), Column::Name, "", &[], false);
        let ladder: Vec<(String, u32, u32)> = shown
            .iter()
            .filter(|(name, _, _)| name.starts_with("src"))
            .cloned()
            .collect();
        assert_eq!(
            ladder,
            [
                ("src".to_owned(), 0, 0),
                (r"src\main".to_owned(), 1, 0),
                (r"src\main\java".to_owned(), 2, 0),
                (r"src\main\java\App.java".to_owned(), 3, 0),
                (r"src\main\java\Other.java".to_owned(), 3, 0),
            ]
        );
    }

    /// A chain cannot reach through a closed door, and the door is the merged row's own.
    ///
    /// Two different shuts, and they are the two halves of the gesture. Shutting the row the user can
    /// see — the innermost folder, which is what its twisty is keyed on — leaves the chain merged and
    /// takes away what is inside it, which is what shutting a row should do. Shutting a folder in the
    /// *middle* of a chain is the state a stale key leaves behind, and it breaks the chain there
    /// rather than merging through it: what is behind a closed door is not on show, so it cannot be
    /// drawn as part of the row in front of it.
    #[test]
    fn a_shut_folder_stops_a_chain_where_it_is() {
        let dir = chain_listing();

        // The row as shown, shut: still one row of three folders, and nothing under it.
        assert_eq!(
            regrouped(&dir, "", &[r"src\main\java"]),
            [
                ("conf".to_owned(), 0, 0),
                (r"conf\app.ini".to_owned(), 1, 0),
                (r"deep\deeper".to_owned(), 0, 1),
                ("lib".to_owned(), 0, 0),
                (r"lib\b".to_owned(), 1, 0),
                (r"lib\b\c.txt".to_owned(), 2, 0),
                (r"lib\a.txt".to_owned(), 1, 0),
                (r"src\main\java".to_owned(), 0, 2),
            ]
        );

        // And shut at the top of the chain: `src` is a row of its own again, with the door closed.
        let shown = regrouped(&dir, "", &["src"]);
        assert_eq!(shown.last(), Some(&("src".to_owned(), 0, 0)));
        assert_eq!(shown.len(), 8, "nothing under a shut folder is in the order");
    }

    /// Under a filter, a chain forms out of whatever the filter left.
    ///
    /// Which is the same rule read honestly — "only one folder in it" is a question about the rows
    /// that are *on show* — and it is what makes a filtered tree readable instead of a ladder of
    /// single matches. `c.txt` leaves `lib` holding one folder where it held two, so `lib > b`
    /// becomes one row that is not there without the filter.
    #[test]
    fn a_filter_makes_chains_of_the_rows_it_leaves() {
        let dir = chain_listing();
        assert_eq!(
            regrouped(&dir, "c.txt", &[]),
            [
                (r"lib\b".to_owned(), 0, 1),
                (r"lib\b\c.txt".to_owned(), 1, 0),
            ],
            "the filter left `lib` one folder, so it is a chain"
        );

        // And the folders leading to a match still merge as far as the match: a file at the end of
        // one is a file, so the chain stops at the folder holding it. `conf` is in this listing
        // because `app.ini` matches `App` too, which is the case worth having beside the other one —
        // the same filter leaves one folder a chain and the other a row of its own, and the
        // difference is only what is left inside them.
        assert_eq!(
            regrouped(&dir, "App", &[]),
            [
                ("conf".to_owned(), 0, 0),
                (r"conf\app.ini".to_owned(), 1, 0),
                (r"src\main\java".to_owned(), 0, 2),
                (r"src\main\java\App.java".to_owned(), 1, 0),
            ]
        );
    }

    /// The Type column, which is the one whose ordering costs a table the size of the listing.
    ///
    /// Worth its own test because the tree computes that table **once** and the list computes it
    /// per call: one per sibling group would be the listing's length squared. So this is both an
    /// ordering assertion and the only thing that would catch the table being indexed wrong.
    #[test]
    fn a_tree_sorted_by_type_ranks_every_level_from_one_table() {
        let dir = listing(&["a/", r"a\z.exe", r"a\b.cpp", "c.cpp", "b.exe"]);
        assert_eq!(
            as_tree_by(&dir, Column::Type, "", &[]),
            ["a", r"a\z.exe", r"a\b.cpp", "b.exe", "c.cpp"],
            "Application before C++ source, at both levels"
        );
    }

    #[test]
    fn the_filter_survives_a_listing_with_no_folder_of_its_own() {
        // "This PC": no path, rows carrying their own targets. The prefix is empty and the
        // name is the whole haystack, which is why no path is joined onto anything here.
        let mut builder = DirBuilder::new("");
        builder.push_link("Windows (C:)", r"C:\".into(), 0, FLAG_DIR);
        builder.push_link("Data (D:)", r"D:\".into(), 0, FLAG_DIR);
        let dir = builder.finish(0);

        let mut order = Vec::new();
        build_order(&dir, &mut order, Column::Name, true, true, "(c:)", None);
        assert_eq!(order.len(), 1);
        assert_eq!(dir.name(order[0] as usize), "Windows (C:)");
    }
}
