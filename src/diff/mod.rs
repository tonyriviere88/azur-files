//! Folder diff: two trees side by side, and which rows of each the other one does not have.
//!
//! A diff tab is two flattened listings — [`crate::fs::scan::scan_deep`]'s, whose names are paths
//! relative to the folder walked — and this module is the question asked of the pair. **It never
//! touches the disk**: both walks already gave every name, kind and timestamp, so comparing them is
//! a sort and a merge over two `Dir`s that are already in memory. See the invariant in `fs/` about
//! enumerating once.
//!
//! What a row can be, from its own side's point of view, is a [`Mark`]:
//!
//! - **Same** — a file the other side has too, with the same type, size and modification time, or
//!   a folder both have with nothing different anywhere under it.
//! - **Only** — nothing of that name on the other side. Everything under a folder that is only here
//!   is only here too, which the merge finds on its own: none of those paths exist over there either.
//! - **Differs** — the name is on both sides and something about it is not: which of the type, the
//!   size and the modification time, cell by cell, is its [`Cells`].
//! - **Holds** — a folder both sides have, with a difference somewhere under it: the way down to
//!   what changed.
//!
//! **What is highlighted is exactly what differs, and nothing else.** A file on both sides with a
//! different size keeps its name in the secondary ink like every identical row, and it is the Size
//! cell that is picked out — see [`crate::ui::filelist`]. The name is highlighted only when it is
//! the name that has no counterpart.
//!
//! # What counts as the same
//!
//! **The name, case-insensitively; whether it is a folder; and for a file, the size and the
//! modification time.** Not the content — reading every byte of two trees is a different feature,
//! and the rest is what the walk already has. The time is allowed [`TOLERANCE`] of slack: FAT keeps
//! two-second timestamps, so a tree copied to a USB stick and back would otherwise differ in every
//! file.
//!
//! # Showing only what differs
//!
//! A diff tab shows everything by default, and can be narrowed to what differs — see [`Show`]. Both
//! halves always show the same way: the setting is the diff's, not a side's. A folder stays in either
//! narrowed view while something it leads to does, because in a tree a folder filtered out is a
//! folder you cannot open to reach what is inside it — which is what [`Lens`](crate::pane::Lens)
//! does for the same reason.
//!
//! A hidden entry is left out of the comparison unless hidden files are being shown — a folder
//! marked as holding differences, with none of them on screen, would be a claim nobody can check.
//!
//! # Off the UI thread
//!
//! A tree can be two hundred thousand rows, and sorting two of those is tens of milliseconds, so a
//! comparison is made by [`Differ`] on a thread of its own and lands a frame or two after the second
//! listing does. Until then the rows draw as they would anywhere else. A [`Comparison`] holds the two
//! listings it was made from, and a row asks it for a mark only through [`Side::mark`], which checks
//! the listing is still the one on show — so an answer about a folder that has since been left, or
//! re-read, is never drawn against the wrong rows.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::fs::Dir;
use crate::pane::PaneId;

/// How far apart two modification times can be and still be the same time: two seconds, in the
/// 100-nanosecond ticks a `FILETIME` counts. FAT's resolution, which is the coarsest a copy of a
/// tree is likely to have passed through.
pub const TOLERANCE: u64 = 2 * 10_000_000;

/// What one row is, measured against the other side. See the module header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    Same,
    Only,
    Differs(Cells),
    Holds,
}

/// Which of a row's cells differ from the same name's on the other side.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Cells {
    /// A folder on one side and a file on the other — the Type column.
    pub kind: bool,
    pub size: bool,
    pub modified: bool,
}

impl Mark {
    /// Whether this row is itself a difference — not merely the way down to one.
    pub fn differs(self) -> bool {
        matches!(self, Self::Only | Self::Differs(_))
    }

    /// The cells that differ, for a row whose name is on both sides.
    pub fn cells(self) -> Cells {
        match self {
            Self::Differs(cells) => cells,
            _ => Cells::default(),
        }
    }
}

/// Which rows a diff tab shows. The tri-state button at the right of each half's path bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Show {
    /// Every row, identical or not. What a diff opens as.
    #[default]
    All,
    /// Only the rows that differ in any way — a name on one side only, or a type, size or
    /// modification time that does not match — and the folders on the way down to them.
    Changes,
    /// Only the names on one side and not the other, and the folders on the way down to them. A file
    /// both sides have is left out however different its copies are.
    Names,
}

impl Show {
    /// The three, in the order the button steps through them.
    pub const ALL: [Self; 3] = [Self::All, Self::Changes, Self::Names];

    /// What a click on the button goes to.
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Changes,
            Self::Changes => Self::Names,
            Self::Names => Self::All,
        }
    }

    /// What the menu entry says.
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "Show all files",
            Self::Changes => "Show only changes",
            Self::Names => "Show only added or missing names",
        }
    }

    /// For the settings file and `--diff=<word>`: a word rather than a number, because the file is
    /// meant to be fixed by hand. See [`crate::pane::FlatMode::as_str`], which is the same choice.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Changes => "changes",
            Self::Names => "names",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|show| show.as_str().eq_ignore_ascii_case(text))
    }

    /// What the listing says when this leaves nothing to show.
    pub fn nothing_found(self) -> &'static str {
        match self {
            Self::All => "This folder is empty",
            Self::Changes => "Nothing differs",
            Self::Names => "Every name is on both sides",
        }
    }
}

/// How many rows of one side are each kind of difference.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Counts {
    pub only: usize,
    pub differ: usize,
    pub same: usize,
}

/// Two listings and how hidden files were treated: what a comparison is *of*, and so what tells an
/// answer still worth having from one about listings since replaced.
///
/// The listings themselves rather than their addresses, so an address cannot be freed and reused by
/// a new listing that would then look already compared. `show_hidden` is part of it because `Ctrl+H`
/// changes what is compared without re-reading either folder.
#[derive(Clone)]
pub struct Pair {
    pub left: Arc<Dir>,
    pub right: Arc<Dir>,
    pub show_hidden: bool,
}

impl Pair {
    pub fn is(&self, left: &Arc<Dir>, right: &Arc<Dir>, show_hidden: bool) -> bool {
        Arc::ptr_eq(&self.left, left) && Arc::ptr_eq(&self.right, right) && self.show_hidden == show_hidden
    }
}

/// One side of a comparison: every entry of its listing, measured against the other.
pub struct Half {
    /// One per entry, by entry index — the same index the listing's order, selection and icons are
    /// kept by.
    marks: Vec<Mark>,
    /// Per entry, whether a name only on this side is somewhere under it — the folders [`Show::Names`]
    /// keeps. [`Mark::Holds`] is the same question about any difference.
    names_below: Vec<bool>,
    /// The entries compared, in [`key_cmp`] order: what the merge walked, and what a name is looked up
    /// in afterwards — [`Lookup`], and [`Side::entry_named`].
    sorted: Vec<u32>,
    /// What the status line says in place of the counts: this side's differences, in words. Made here,
    /// on the worker, so the frame only ever borrows it.
    pub summary: String,
}

impl Half {
    /// Whether entry `entry` is a row `show` keeps.
    pub fn keeps(&self, show: Show, entry: usize) -> bool {
        let mark = self.marks.get(entry).copied().unwrap_or(Mark::Same);
        match show {
            Show::All => true,
            Show::Changes => mark != Mark::Same,
            Show::Names => mark == Mark::Only || self.names_below.get(entry).copied().unwrap_or(false),
        }
    }

    /// Where `name` is, or would be, in [`Half::sorted`].
    fn search(&self, dir: &Dir, name: &str) -> Result<usize, usize> {
        self.sorted
            .binary_search_by(|&probe| key_cmp(dir.name(probe as usize), name))
    }
}

/// Both sides of one comparison, and the two listings it was made from.
pub struct Comparison {
    pub of: Pair,
    pub left: Half,
    pub right: Half,
}

impl Comparison {
    /// One side, with the listing it describes.
    pub fn half(&self, left: bool) -> (&Arc<Dir>, &Half) {
        if left {
            (&self.of.left, &self.left)
        } else {
            (&self.of.right, &self.right)
        }
    }
}

/// Compare two flattened listings. See the module header for what it decides.
pub fn compare(pair: Pair) -> Comparison {
    let (left, right) = (&pair.left, &pair.right);
    let considered = |dir: &Dir| -> Vec<u32> {
        let mut rows: Vec<u32> = (0..dir.len() as u32)
            .filter(|&i| pair.show_hidden || !dir.entries[i as usize].is_hidden())
            .collect();
        rows.sort_unstable_by(|&a, &b| key_cmp(dir.name(a as usize), dir.name(b as usize)));
        rows
    };
    let (l, r) = (considered(left), considered(right));
    let mut left_marks = vec![Mark::Same; left.len()];
    let mut right_marks = vec![Mark::Same; right.len()];

    // ---- The merge -------------------------------------------------------
    // The folders both sides have, paired, for the last step below.
    let mut folders: Vec<(usize, usize)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < l.len() || j < r.len() {
        let order = match (l.get(i), r.get(j)) {
            (Some(&a), Some(&b)) => key_cmp(left.name(a as usize), right.name(b as usize)),
            (Some(_), None) => Ordering::Less,
            _ => Ordering::Greater,
        };
        match order {
            Ordering::Less => {
                left_marks[l[i] as usize] = Mark::Only;
                i += 1;
            }
            Ordering::Greater => {
                right_marks[r[j] as usize] = Mark::Only;
                j += 1;
            }
            Ordering::Equal => {
                let (a, b) = (l[i] as usize, r[j] as usize);
                let (ea, eb) = (&left.entries[a], &right.entries[b]);
                let cells = match (ea.is_dir(), eb.is_dir()) {
                    // Folders are compared by what is under them, which the walk up below decides.
                    (true, true) => {
                        folders.push((a, b));
                        Cells::default()
                    }
                    (false, false) => Cells {
                        kind: false,
                        size: ea.size != eb.size,
                        modified: ea.modified.abs_diff(eb.modified) > TOLERANCE,
                    },
                    // A folder here and a file there: the name matches and the type does not. What
                    // is under the folder has nothing to match, and the merge says so row by row.
                    _ => Cells {
                        kind: true,
                        ..Cells::default()
                    },
                };
                let mark = if cells == Cells::default() {
                    Mark::Same
                } else {
                    Mark::Differs(cells)
                };
                left_marks[a] = mark;
                right_marks[b] = mark;
                i += 1;
                j += 1;
            }
        }
    }

    // ---- The way down to each difference ---------------------------------
    let mut left_names_below = names_below(left, &l, &left_marks);
    let mut right_names_below = names_below(right, &r, &right_marks);
    holds(left, &l, &mut left_marks);
    holds(right, &r, &mut right_marks);
    // **A folder holds a difference if either side's copy does.** A file only on the left is also a
    // file *missing* on the right, and the right's folder is the way down to where it is missing —
    // so both halves of the tree lead to it.
    for (a, b) in folders {
        if left_marks[a] == Mark::Holds || right_marks[b] == Mark::Holds {
            left_marks[a] = Mark::Holds;
            right_marks[b] = Mark::Holds;
        }
        if left_names_below[a] || right_names_below[b] {
            left_names_below[a] = true;
            right_names_below[b] = true;
        }
    }

    let half = |sorted: Vec<u32>, marks: Vec<Mark>, names_below: Vec<bool>| {
        let summary = summary(count(&sorted, &marks));
        Half {
            marks,
            names_below,
            sorted,
            summary,
        }
    };
    let left = half(l, left_marks, left_names_below);
    let right = half(r, right_marks, right_names_below);
    Comparison {
        of: pair,
        left,
        right,
    }
}

/// Every folder above entry `row`, nearest first, for as long as each is one of the entries compared.
///
/// `sorted` is the side's compared entries in [`key_cmp`] order, which is what lets a parent be found
/// by binary search rather than by a map built for the purpose. A parent that is not among them is
/// hidden and left out, and nothing above it is reachable on screen through it, so the walk ends there.
fn parents<'a>(dir: &'a Dir, sorted: &'a [u32], row: usize) -> impl Iterator<Item = usize> + 'a {
    let mut at = row;
    std::iter::from_fn(move || {
        let parent = dir.within(at);
        if parent.is_empty() {
            return None;
        }
        let found = sorted
            .binary_search_by(|&probe| key_cmp(dir.name(probe as usize), parent))
            .ok()?;
        at = sorted[found] as usize;
        Some(at)
    })
}

/// Mark every folder above a difference as holding one. The walk up stops at the first folder already
/// marked, because everything above that one was marked by whoever got there first.
fn holds(dir: &Dir, sorted: &[u32], marks: &mut [Mark]) {
    for &row in sorted {
        if !marks[row as usize].differs() {
            continue;
        }
        for parent in parents(dir, sorted, row as usize) {
            if marks[parent] != Mark::Same {
                break;
            }
            marks[parent] = Mark::Holds;
        }
    }
}

/// Per entry, whether a name that is only on this side is somewhere under it. [`holds`] for
/// [`Show::Names`]: the same walk up, over a narrower question. A folder only here is kept for being
/// one, so the walk stops there as it does at one already marked.
fn names_below(dir: &Dir, sorted: &[u32], marks: &[Mark]) -> Vec<bool> {
    let mut below = vec![false; marks.len()];
    for &row in sorted {
        if marks[row as usize] != Mark::Only {
            continue;
        }
        for parent in parents(dir, sorted, row as usize) {
            if marks[parent] == Mark::Only || below[parent] {
                break;
            }
            below[parent] = true;
        }
    }
    below
}

fn count(sorted: &[u32], marks: &[Mark]) -> Counts {
    let mut counts = Counts::default();
    for &row in sorted {
        match marks[row as usize] {
            Mark::Only => counts.only += 1,
            Mark::Differs(_) => counts.differ += 1,
            Mark::Same => counts.same += 1,
            Mark::Holds => {}
        }
    }
    counts
}

/// The order two relative paths are matched in: ASCII case folded, and either slash the same.
///
/// Byte-wise beyond ASCII, which is the collation the rest of this program has — see the note on
/// sorting in CLAUDE.md. A separator sorts below every other character, so a folder's children come
/// straight after it and before any sibling that merely starts with the same letters: `a\b` before
/// `a-b`. Which is what makes [`parents`]' binary search find the folder it is looking for.
pub(crate) fn key_cmp(a: &str, b: &str) -> Ordering {
    let fold = |byte: u8| match byte {
        b'\\' | b'/' => 0,
        _ => byte.to_ascii_lowercase(),
    };
    a.bytes().map(fold).cmp(b.bytes().map(fold))
}

/// One side's counts, as the status line says them.
pub fn summary(counts: Counts) -> String {
    let mut parts: Vec<String> = Vec::new();
    if counts.only > 0 {
        parts.push(format!("{} only here", counts.only));
    }
    if counts.differ > 0 {
        parts.push(format!("{} different", counts.differ));
    }
    if parts.is_empty() {
        parts.push("No differences".to_owned());
    }
    parts.push(format!("{} identical", counts.same));
    parts.join("  ·  ")
}

// ---------------------------------------------------------------------------
// What a tab holds
// ---------------------------------------------------------------------------

/// What makes a tab one half of a folder diff. See [`crate::pane::Tab::diff`].
///
/// The tab in the strip is the **left** half and carries the pane its other half lives in; that pane
/// is not in the layout — see [`crate::pane::Pane::twin`] — and exists so the right half is addressed
/// the way every listing is, by a [`PaneId`], and every action a listing can push works on it
/// unchanged.
#[derive(Default)]
pub struct Side {
    /// The pane holding the right half, on the left half. `None` on the right half itself.
    pub twin: Option<PaneId>,
    /// The comparison both halves are drawn from, once there is one.
    pub comparison: Option<Arc<Comparison>>,
    /// The listings a comparison has been asked for and has not landed yet, on the left half. Let go
    /// of the moment either half has no listing — see `App::tend_diffs` — so a tree that has been left
    /// is not kept alive by it.
    pub asked: Option<Pair>,
    /// Which question is the latest, on the left half: a worker whose number is no longer this one's
    /// stops rather than sorting two trees for an answer nobody will read. See [`Differ::ask`].
    pub ticket: Arc<AtomicU64>,
    /// Which rows are shown. The same on both halves — see `Action::SetDiffShow`, the one place it is
    /// set.
    pub show: Show,
    /// The path bar is to be picked out for a moment — the right half of a diff opened on the same
    /// folder twice, which is waiting for the second folder to be chosen. See [`Flash`].
    pub flash: Flash,
    /// Where this side's rows are, for the other side to scroll to. See [`Lookup`].
    pub lookup: Lookup,
    /// Where this side's listing was scrolled to the last time the two were compared — so a change
    /// in it is a scroll, and which side moved is which side the other one follows.
    pub seen: f32,
    /// Whether the last change of scroll here was the other side's doing, and so is not one to
    /// follow back. See `App::sync_scroll`.
    pub steered: bool,
}

impl Side {
    pub fn is_left(&self) -> bool {
        self.twin.is_some()
    }

    /// This side's half of the comparison — **only if `dir` is the listing it was made of**. Anything
    /// else is a folder since left or read again, and nothing is claimed about it.
    pub fn half(&self, dir: &Arc<Dir>) -> Option<&Half> {
        let (of, half) = self.comparison.as_ref()?.half(self.is_left());
        Arc::ptr_eq(of, dir).then_some(half)
    }

    /// The mark for entry `entry` of `dir`. See [`Side::half`].
    #[inline]
    pub fn mark(&self, dir: &Arc<Dir>, entry: usize) -> Option<Mark> {
        self.half(dir)?.marks.get(entry).copied()
    }

    /// The entry of `dir` called `name`, matched the way the comparison matches — ignoring case — so
    /// the entry found is the one the diff paired up.
    pub fn entry_named(&self, dir: &Arc<Dir>, name: &str) -> Option<usize> {
        let half = self.half(dir)?;
        half.search(dir, name).ok().map(|at| half.sorted[at] as usize)
    }

    /// The test [`crate::pane::Tab::rebuild_order`] puts each row through, or `None` when every row
    /// is shown — which is also the answer while there is no comparison of `dir` to ask, so a half
    /// whose diff is still being worked out shows its whole tree rather than nothing.
    pub fn keeper(&self, dir: &Arc<Dir>) -> Option<Box<dyn Fn(usize) -> bool>> {
        if self.show == Show::All {
            return None;
        }
        self.half(dir)?;
        let comparison = self.comparison.clone()?;
        let (left, show) = (self.is_left(), self.show);
        Some(Box::new(move |entry| comparison.half(left).1.keeps(show, entry)))
    }
}

/// A path bar picked out for a few seconds, and then not.
///
/// **A highlight rather than the keyboard.** The right half of a diff that opened on the folder the
/// left already shows has to be pointed somewhere else, and saying where is this; putting the caret
/// in its path field instead took the keyboard off the listing the user had just been working in.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub enum Flash {
    #[default]
    Off,
    /// Asked for, and not drawn yet: the clock starts on the first frame it is on screen, so a tab
    /// opened behind another does not use its seconds up out of sight.
    Wanted,
    /// On screen since this time, on the frame clock.
    Since(f64),
}

impl Flash {
    /// How long the highlight holds at full strength, and then how long it takes to fade.
    pub const HOLD: f64 = 2.0;
    pub const FADE: f64 = 1.0;

    /// How strong the highlight is at `now`, from 1 to nothing — and `None` once it is over, which
    /// also turns it off.
    pub fn strength(&mut self, now: f64) -> Option<f32> {
        let since = match *self {
            Self::Off => return None,
            Self::Wanted => {
                *self = Self::Since(now);
                now
            }
            Self::Since(since) => since,
        };
        let age = now - since;
        if age >= Self::HOLD + Self::FADE {
            *self = Self::Off;
            return None;
        }
        Some((1.0 - ((age - Self::HOLD) / Self::FADE).max(0.0)) as f32)
    }
}

/// Where each entry of a side's listing is in its display order, so the row showing a given relative
/// path is a binary search away rather than a walk down two hundred thousand rows on every frame of a
/// scroll.
///
/// **The names are already sorted** — [`Half::sorted`] is the order the merge walked — so all this
/// keeps is one position per entry, refilled in a single pass when the order is rebuilt:
/// [`crate::pane::Tab::order_gen`] says when, which is per collapse, filter or listing and never per
/// frame.
#[derive(Default)]
pub struct Lookup {
    built: Option<u64>,
    /// Per entry, its position in the order plus one; `0` is not in it — filtered, or inside a folder
    /// that is shut.
    positions: Vec<u32>,
}

impl Lookup {
    /// The row of `order` that best stands for `name`: the row showing it, and when there is none —
    /// it is only on the other side, or inside a folder shut on this one — the nearest row before
    /// where it would be by name. Which is a sibling above it, or the folder it is in: the same place
    /// in the tree, approximately, and that is what scrolling the two sides together is for.
    pub fn position_of(
        &mut self,
        half: &Half,
        dir: &Dir,
        order: &[u32],
        generation: u64,
        name: &str,
    ) -> Option<usize> {
        if self.built != Some(generation) {
            self.positions.clear();
            self.positions.resize(dir.len(), 0);
            for (position, &entry) in order.iter().enumerate() {
                self.positions[entry as usize] = position as u32 + 1;
            }
            self.built = Some(generation);
        }
        let shown = |entry: &u32| {
            self.positions
                .get(*entry as usize)
                .copied()
                .filter(|&position| position > 0)
                .map(|position| position as usize - 1)
        };
        let end = match half.search(dir, name) {
            Ok(at) => at + 1,
            Err(at) => at,
        };
        half.sorted[..end]
            .iter()
            .rev()
            .find_map(shown)
            // Before everything on show: the first row by name.
            .or_else(|| half.sorted[end..].iter().find_map(shown))
    }
}

// ---------------------------------------------------------------------------
// Off the UI thread
// ---------------------------------------------------------------------------

/// Makes comparisons on a thread of its own, one thread per comparison.
///
/// A comparison is asked for when either half of a diff lands a new listing, which is a handful of
/// times in a session and never per frame, so there is no pool to keep: a thread per question and a
/// channel back. A question overtaken by a newer one for the same diff stops before sorting — see
/// [`Side::ticket`] — and an answer that lands anyway is dropped by whoever polls, by [`Pair::is`].
pub struct Differ {
    tx: Sender<Arc<Comparison>>,
    rx: Receiver<Arc<Comparison>>,
    ctx: egui::Context,
}

impl Differ {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            ctx: ctx.clone(),
        }
    }

    /// Compare `pair`, as the latest question on `ticket`.
    pub fn ask(&self, pair: Pair, ticket: &Arc<AtomicU64>) {
        let number = ticket.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        let ticket = ticket.clone();
        let latest = move || ticket.load(AtomicOrdering::Relaxed) == number;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::Builder::new()
            .name("folder-diff".to_owned())
            .spawn(move || {
                if !latest() {
                    return;
                }
                let comparison = compare(pair);
                // Dropped with its listings, rather than sent, if the window has gone or a newer
                // question was asked while this one was being answered.
                if latest() && tx.send(Arc::new(comparison)).is_ok() {
                    ctx.request_repaint();
                }
            })
            .ok();
    }

    pub fn poll(&self) -> Option<Arc<Comparison>> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests;
