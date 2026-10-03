//! How much is inside each folder, off the UI thread.
//!
//! A directory's own `size` is noise — see [`crate::fs::dir::Entry::size`] — so the Size column
//! leaves a folder's cell blank, and the one question a file manager is asked more than any other
//! goes unanswered: *what is taking the space in here.* The measure button on the status line is
//! that answer, and this module is what it costs.
//!
//! # Why this is a service and not a function
//!
//! Summing a folder is a walk of its whole tree, which is the one read in this program with no
//! natural bound: `C:\Users\you` is minutes, and there is no way to know that before starting. So
//! nothing here is allowed to happen on the UI thread and nothing is allowed to be waited for —
//! the numbers arrive **one folder at a time**, each on its own answer, and the column fills in
//! under the reader while the rest is still being counted. A folder that has not answered yet
//! draws exactly what it drew before the button was pressed.
//!
//! # The unit of work is a directory, not a folder
//!
//! **This is the whole of why it is fast, and it was the other way round first.** The obvious
//! arrangement is one worker per folder on show: hand thread 1 `Windows`, thread 2 `Program Files`,
//! and let each walk its own tree. It is simpler, and it is wrong for the case that actually hurts —
//! `C:\` has a handful of children and two of them are nearly all of it, so two threads work and the
//! rest idle, and the answer takes as long as the largest single tree takes *serially*.
//!
//! So the queue holds **directories**, each tagged with the folder whose total it contributes to.
//! Every worker takes whatever directory is next, whoever it belongs to, and pushes the
//! subdirectories it finds back for anyone to pick up. One enormous folder therefore saturates
//! every thread, and so does a folder of ten thousand small ones. A folder is finished when the
//! last of its directories is read, which is a counter per folder rather than a thread per folder —
//! see [`Job::outstanding`].
//!
//! Measured warm over this crate's own folder — `target` included, which is most of it — by
//! [`tests::measuring_speed`]. The first three rows are the arrangement this replaced:
//!
//! | threads | unit of work | warm |
//! | --- | --- | --- |
//! | 1 | a folder | 81 ms |
//! | 4 | a folder | 74 ms |
//! | 8 | a folder | 72 ms |
//! | 1 | a directory | 78 ms |
//! | 2 | a directory | 51 ms |
//! | 4 | a directory | 34 ms |
//! | **8** | **a directory** | **26 ms** |
//! | 16 | a directory | 24 ms |
//!
//! **Threads bought almost nothing at all while the unit was a folder** — 81 ms down to 74, which is
//! nine percent for four times the threads — because `target` is most of the tree and one thread had
//! to carry the whole of it however many others were idle. The same four threads over directories
//! come back **2.4× sooner**, and the single-threaded rows either side (81 against 78) say that none
//! of that is the change in traversal order: it is only what the work being handed out is.
//!
//! Sixteen is a wash against eight, which is where [`hands`] stops — this is bound by the file system
//! rather than by the CPU, and it is the same figure [`crate::fs::scan::scan_deep`] settled on for the
//! same reason. On a tree big enough to have to wait for, the multiplier is what matters rather than
//! these absolute figures: the shape holds, and the folder-at-a-time rows get *worse* as one subtree
//! comes to dominate more.
//!
//! # Three rules the walk keeps
//!
//! - **A reparse point is not followed.** `C:\Users\All Users` is a junction to `C:\ProgramData`,
//!   and a walk that follows one on a system drive never finishes. It is also the only way a
//!   *loop* can exist in a Windows tree. So a junction is not descended into and gets no number
//!   at all — [`NO_NUMBER`] — because `0 B` for a link pointing at a full folder would be a wrong
//!   answer where a blank cell is merely a quiet one.
//! - **Depth first, and the earliest rows first.** Subdirectories go on the *front* of the queue
//!   and a fresh folder's root on the back, so the frontier in memory is the siblings along one
//!   path rather than a whole level of the tree — on `C:\Windows\WinSxS` that is a few dozen paths
//!   against tens of thousands — while the rows at the top of the listing are still the ones
//!   started first.
//! - **It is abandoned rather than finished.** Navigating away, refreshing, or pressing the button
//!   again drops the generation the work was started under, and a worker that picks up a directory
//!   belonging to a dead generation throws it away without retiring it — so the folder's counter
//!   never reaches zero and no half-counted answer is ever sent. That is what keeps a mistaken
//!   press on `C:\` from costing minutes of disk after the reader has moved on. See
//!   [`Sizes::only`].
//!
//! Nothing is cached between folders. A total is only as good as the moment it was taken and the
//! whole point of the button is to be pressed where you are standing; a map keyed by path would
//! be a store of numbers that go quietly wrong as files are written, which is the same argument
//! [`crate::git`] makes about a status cache.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as Atomic};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use crate::fs::{scan, Dir};

/// A folder nothing has asked about yet.
///
/// The three states here and a real byte count share one `u64` per row rather than an
/// `Option<u64>` or a parallel `Vec<bool>`: the column is one allocation, sized when the listing
/// lands and dropped when it goes, and a flattened tree of 200,000 rows is 1.6 MB either way — but
/// only if the states cost nothing. See [`Measurement::slots`].
///
/// **All three are private, and so is the arithmetic on them.** Nothing outside this module names
/// them or compares against them: [`Measurement`] owns every transition, so adding a fourth state
/// is a change here and nowhere else. They were `pub` for an afternoon and a `bytes.min(NO_NUMBER -
/// 1)` immediately appeared in `pane`, which is a second copy of where the state space ends — on
/// exactly the rows most likely to be large.
const UNMEASURED: u64 = u64::MAX;

/// A folder that has been asked about and has not answered.
///
/// The distinction from [`UNMEASURED`] is what stops a listing asking about the same folder on
/// every frame, and what lets a filter that *reveals* a folder ask about it without re-asking
/// about the rest.
const MEASURING: u64 = u64::MAX - 1;

/// A folder there is no number for: a junction or a symlink, which the walk does not follow.
const NO_NUMBER: u64 = u64::MAX - 2;

/// The byte count in a slot, if it holds one rather than one of the three states above.
///
/// The one place that comparison is made, so nothing can come to disagree with it about whether
/// `u64::MAX - 1` is a very large folder.
#[inline]
fn measured(slot: u64) -> Option<u64> {
    (slot < NO_NUMBER).then_some(slot)
}

/// One folder's total, tagged with the measurement it belongs to.
pub struct Answer {
    /// Which measurement asked — [`Measurement::gen`]. An answer for a generation no tab still
    /// holds is dropped, which is what makes a refresh safe: the row indices in an answer are
    /// indices into the listing that was on screen when it was asked for.
    pub gen: u64,
    /// The entry index the total is for.
    pub row: u32,
    pub bytes: u64,
}

// ---------------------------------------------------------------------------
// What one tab knows about counting its folders
// ---------------------------------------------------------------------------

/// A number no measurement has had before. See [`Measurement::gen`].
///
/// Its own counter rather than a share of `pane::next_view`'s, because the two are not the same
/// clock: a view survives a re-read of its folder and a measurement must not, which is the one
/// distinction the generation exists to make.
fn next_gen() -> u64 {
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Atomic::Relaxed)
}

/// What is owed before a measured listing can be drawn.
///
/// The answer to "what has to happen next", asked once a frame. It exists because a *Size-sorted*
/// listing owes more than a total when a folder answers: the order was built from numbers that have
/// just changed, and only the tab can rebuild one. See [`Measurement::owed`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owed {
    /// Everything is in step.
    Nothing,
    /// The total the bars are a share of needs recomputing, and nothing else.
    Total,
    /// And the order with it, because the sort is by Size and the numbers under it moved.
    Order,
}

/// What a listing wants counted, once it has been asked.
///
/// Two shapes because there are two kinds of listing and only one of them needs the disk. Returned
/// from one method so that the fork lives with the state it reads rather than in `App` — which is
/// where it was, and the cost showed up immediately in the tests: they reimplemented the flat branch
/// by hand because there was nothing to call.
pub enum Work {
    /// Nothing to do: counted already, or the button is off, or there is no listing.
    None,
    /// A flattened listing, counted off itself with no disk at all. Already applied by the time this
    /// comes back — it is a pure pass over the listing, so there is nothing to wait for and nothing
    /// to hand to a worker. See [`measure_listing`].
    Done,
    /// The folders on show that nothing has asked about yet, in display order, for the service.
    Walk(Vec<(u32, PathBuf)>),
}

/// Everything one tab knows about counting the folders on show.
///
/// **One struct rather than nine fields on [`crate::pane::Tab`]**, which is the house shape for a
/// feature that owns more than a fact or two — `Tab::preview` and `Tab::grid` are both structs with
/// their own `impl` — and here it earns it twice over. The state was reset by hand in four places
/// and two of them had already drifted, leaving the previous folder's duration behind; now there is
/// one `restart` and the drift is not expressible.
///
/// It lives in this module and not in `pane` because of what it holds: the [`UNMEASURED`] /
/// [`MEASURING`] / [`NO_NUMBER`] encoding is this module's, and keeping the owner beside it is what
/// lets those three be private. Every transition is a method here, so a fourth state is a change to
/// this file and nowhere else.
#[derive(Default)]
pub struct Measurement {
    /// Whether the folders on show are being counted at all — what the status line's measure button
    /// turns on.
    ///
    /// Per tab, like [`crate::pane::Tab::flat`] and [`crate::pane::Lens`], because it is a question
    /// asked of the folder in front of you. But **unlike** those two it survives a navigation, which
    /// is the one place `pane`'s usual rule is deliberately broken: finding what is taking the space
    /// means opening the folder that turned out to be big and asking again, and a mode that switched
    /// itself off at every step would have to be pressed once per level for as long as the hunt went
    /// on. Nothing is remembered between sessions, and nothing is inherited by a new tab.
    pub on: bool,
    /// What is inside each folder, by entry index: a byte count, or one of the three states above.
    ///
    /// Empty while the button is off, so a listing nobody has asked to measure costs nothing at all:
    /// this is eight bytes a row, which on a flattened tree of 200,000 is 1.6 MB. Sized when the
    /// listing lands and dropped with it, exactly like [`crate::pane::Tab::file_icons`].
    slots: Vec<u64>,
    /// Which measurement the numbers in [`Self::slots`] belong to.
    ///
    /// **The whole of what makes a refresh safe.** An answer names a *row*, and a row is an index
    /// into the listing that was on screen when the question was asked — so an answer arriving after
    /// an `F5`, a file operation or [`crate::watch`] would land a real byte count on whatever file
    /// now sits at that index. A fresh generation on every listing, and an answer whose generation no
    /// tab still holds is dropped. See [`Sizes::only`].
    ///
    /// Not [`crate::pane::Tab::view`], which is the icons' and git's key: that one survives a re-read
    /// of the same folder, which is precisely the case this has to tell apart.
    gen: u64,
    /// How many folders on show have been asked about and not answered.
    ///
    /// Maintained rather than counted, which is the rule `Tab::selected_count` follows and for the
    /// same reason: the status line is drawn every frame, and a walk over the column would be a walk
    /// over the folder — on a flattened tree of 200,000 rows, per frame, to print one word.
    waiting: usize,
    /// When the folders still outstanding were first asked about.
    ///
    /// `Some` exactly while [`Self::waiting`] is above zero — one state, so one place sets and clears
    /// both. An `Instant` rather than the frame clock because this is wall time across worker
    /// threads, and it has to be right whether or not a frame happened to be drawn when the last
    /// answer landed.
    started: Option<std::time::Instant>,
    /// How long the last round took, in microseconds — `scan_micros`' unit, so the status line reads
    /// the same whichever figure it is printing.
    ///
    /// A **round**, not a session: it starts when the first folder is asked about with nothing
    /// outstanding and ends when the last of them answers, so a filter that later reveals one more
    /// folder times that one folder. `0` means nothing has been counted yet.
    micros: u64,
    /// The total the bars are drawn as a share of — `None` while it needs recomputing.
    ///
    /// An `Option` rather than a value beside a dirty flag, and that is not tidiness: the flag
    /// version had to be *un-set* after [`crate::pane::Tab::rebuild_order`] re-armed it on the way
    /// through, with a comment explaining that leaving it set would rebuild the order on every frame
    /// for ever. Assigning the total is what clears the debt, so the debt cannot outlive it.
    total: Option<u64>,
    /// Whether a Size-sorted order was built from numbers that have since changed.
    ///
    /// Set when a total lands, when the button goes on, and when a flattened listing counts itself —
    /// and deliberately **not** by a rebuilt order, which is what keeps a *user's* rebuild from buying
    /// a second identical one: a settled filter keystroke, a `Ctrl+H` or a column click invalidates
    /// [`Self::total`] and never this.
    order_stale: bool,
    /// And when it may be paid, for the debt a streaming total incurs.
    ///
    /// **Debounced exactly as the filter box is**, and for the same measured reason: a rebuild is one
    /// filter pass plus a sort of everything that survives, which `crate::pane::FILTER_DELAY`
    /// measures at 220–240 ms on a 188,690-row listing. Answers arrive one folder at a time and each
    /// wakes the window, so a rebuild per answer is a rebuild per frame for as long as the counting
    /// takes — minutes, on a drive. This is that same quarter-second, and the round finishing
    /// overrides it so the order you are finally left reading is never a step behind.
    ///
    /// `None` with [`Self::order_stale`] set means **at once**: pressing the button and counting a
    /// flattened listing are single events rather than a stream, and there is nothing to wait for.
    resort_at: Option<f64>,
    /// A flattened listing has been summed off itself. See [`Self::wanted`].
    ///
    /// The flat case is one pure pass with no per-row "asked" state to carry it, so it needs a latch
    /// of its own — where an ordinary listing's folders each say whether they have been asked. Private
    /// and cleared by [`Self::restart`], which is what keeps it from becoming the tenth thing a caller
    /// has to remember to reset.
    counted: bool,
    /// Which build of the display order [`Self::wanted`] last scanned — `crate::pane::Tab::order_gen`.
    ///
    /// **The per-frame early-out**, and it is not an optimisation. The scan is a pass over the rows on
    /// show, up to 200,000 of them, and [`Self::on`] deliberately survives a navigation — so a reader
    /// who leaves the button on would pay that pass on every frame of every tab for the rest of the
    /// session, long after every folder had answered. The rows on show change only when the order is
    /// rebuilt, and the order already carries a generation that says so.
    scanned: u64,
}

/// How long a Size sort waits before it re-orders itself around the totals that have landed.
///
/// `crate::pane::FILTER_DELAY`'s figure and its whole argument, applied to the other thing in this
/// program that changes a listing's order faster than anybody can read it. See
/// [`Measurement::resort_at`].
pub const RESORT_DELAY: f64 = 0.25;

impl Measurement {
    /// Turn the counting on or off.
    ///
    /// Turning it **on** keeps whatever was already counted, which is what makes the button cheap to
    /// press twice: the numbers are still true of the same listing, and only the folders left
    /// unanswered are asked about again. Turning it **off** keeps them too and merely stops drawing
    /// them — the walks are abandoned by the generation going out of use, not by anything here. See
    /// [`Sizes::only`].
    pub fn set(&mut self, on: bool, rows: usize) {
        self.on = on;
        self.total = None;
        if !on {
            return;
        }
        // A fresh generation, so nothing still in flight from the last time the button was on can
        // land on a listing that may have been re-read since.
        self.gen = next_gen();
        if self.slots.len() != rows {
            self.slots = vec![UNMEASURED; rows];
        }
        self.reopen();
        self.idle();
        // A Size-sorted listing was ordered without the folders in it, and now they are in it. At
        // once rather than on a deadline: this is one press, not a stream of answers.
        self.order_stale = true;
    }

    /// Put every folder that was waiting back to being a question, and ask for a fresh scan.
    ///
    /// What both the button and [`Self::duplicate`] need, and for the same reason: the walks those rows
    /// were waiting on belong to a generation that is no longer live, so nothing is ever going to
    /// answer them. A folder left marked as asked would simply stay blank for ever.
    fn reopen(&mut self) {
        for slot in &mut self.slots {
            if *slot == MEASURING {
                *slot = UNMEASURED;
            }
        }
        // Whatever the last scan concluded, it concluded it about slots that have just changed.
        // `order_gen` starts at one, so zero is "no build of any order".
        self.scanned = 0;
        self.counted = false;
    }

    /// A listing has landed: a new measurement over new rows.
    ///
    /// The totals do not survive it, and that is the point — see [`Self::gen`]. `rows` is the new
    /// listing's length; the column is only allocated while the button is on.
    pub fn for_listing(&mut self, rows: usize) {
        self.slots = if self.on {
            vec![UNMEASURED; rows]
        } else {
            Vec::new()
        };
        self.restart();
    }

    /// The listing has gone and there is not a new one yet — a navigation, or a flatten toggling.
    ///
    /// The column is released rather than resized: a flattened tree's is 1.6 MB, and nothing can be
    /// drawn from it before the next listing arrives anyway.
    pub fn forget(&mut self) {
        self.slots = Vec::new();
        self.slots.shrink_to_fit();
        self.restart();
    }

    /// Everything but [`Self::on`] and the column, back to nothing.
    ///
    /// **The one reset.** It was four hand-written lists and two of them had already dropped
    /// [`Self::started`] and [`Self::micros`], leaving a folder that was never counted able to
    /// report how long it took — invisible only because the paths that forgot happen to null the
    /// listing first. A fresh generation is part of it, so work under way is abandoned on this frame
    /// rather than at whatever point the next listing lands.
    fn restart(&mut self) {
        self.gen = next_gen();
        self.total = None;
        self.scanned = 0;
        self.counted = false;
        self.idle();
    }

    /// No round in progress, and no figure from one.
    fn idle(&mut self) {
        self.waiting = 0;
        self.started = None;
        self.micros = 0;
        self.order_stale = false;
        self.resort_at = None;
    }

    /// A copy for a duplicated tab, over the same `Arc<Dir>`.
    ///
    /// The numbers come across because it is the same listing and every entry index still names the
    /// same file — a copy that re-walked the tree would cost the seconds the original has already
    /// spent. Whatever the original is still *waiting* for does not: those answers are addressed to
    /// its generation, so the copy asks for itself.
    pub fn duplicate(&self) -> Self {
        let mut copy = Self {
            on: self.on,
            slots: self.slots.clone(),
            gen: next_gen(),
            micros: self.micros,
            ..Self::default()
        };
        copy.reopen();
        copy
    }

    /// The generation to keep alive, while there is one. See [`Sizes::only`].
    pub fn live(&self) -> Option<u64> {
        self.on.then_some(self.gen)
    }

    /// Which measurement to file work under. See [`Self::gen`].
    #[inline]
    pub fn gen(&self) -> u64 {
        self.gen
    }

    /// What the tab owes before the listing can be drawn, and how long until it does.
    ///
    /// `Owed::Order` is only ever returned once the deadline has passed or the round has finished, so
    /// a caller that acts on it immediately is already debounced. The second half is the wait left,
    /// for a caller that has to book the frame which will notice — this program is idle between
    /// events, so without that the re-sort would happen whenever something else next wanted a frame.
    pub fn owed(&mut self, now: f64, sorted_by_size: bool) -> (Owed, Option<f64>) {
        // A re-sort owed by a listing that is no longer sorted by Size is not owed at all: the order it
        // would produce is the one already on screen.
        if !sorted_by_size {
            self.order_stale = false;
            self.resort_at = None;
        }
        if self.order_stale {
            // How long is left, if anything is waiting on a deadline at all. The round finishing wins
            // over it: the last answer is the one whose order somebody is going to sit and read.
            let left = self
                .resort_at
                .filter(|_| self.waiting > 0)
                .map(|at| RESORT_DELAY - (now - at))
                .filter(|left| *left > 0.0);
            if let Some(left) = left {
                // Owed, and not due yet. The *total* still settles on this frame, so the bars stay
                // right while the order waits — and the caller is handed the wait so it can book the
                // frame that will notice.
                return (Owed::Total, Some(left));
            }
            self.order_stale = false;
            self.resort_at = None;
            return (Owed::Order, None);
        }
        let owed = if self.wants_total() {
            Owed::Total
        } else {
            Owed::Nothing
        };
        (owed, None)
    }

    /// Note that the rows on show have changed, so the total has to be worked out again.
    ///
    /// Deliberately **not** the order: this is what a rebuilt order calls, and arming a re-sort here
    /// would have every settled filter keystroke rebuild the listing a second time on the next frame
    /// for an order that cannot have changed.
    pub fn rows_moved(&mut self) {
        if self.on {
            self.total = None;
        }
    }

    /// Whether the total needs working out, for a caller about to do it.
    pub fn wants_total(&self) -> bool {
        self.on && self.total.is_none()
    }

    /// What the bars are a share of.
    pub fn set_total(&mut self, total: u64) {
        self.total = Some(total);
    }

    /// How much of the listing `bytes` is, as a fraction — what a row's bar is drawn to.
    ///
    /// `None` while the measurement is off and while there is nothing to be a share *of*: a folder of
    /// nothing but empty files has a total of zero, and every bar in it would otherwise be drawn full
    /// or drawn from a division by zero. Clamped, because a truncated flatten can leave the total
    /// short of a row that was counted before the walk stopped — a bar past the end of its own track
    /// would say the arithmetic had gone wrong rather than that the listing was cut off.
    pub fn share(&self, bytes: u64) -> Option<f32> {
        let total = self.total.filter(|_| self.on)?;
        if total == 0 {
            return None;
        }
        Some((bytes as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }

    /// What a row's Size cell has to say: a file's own bytes, or a folder's counted ones.
    ///
    /// `None` is a **blank cell**, and it covers every case where there is no number to print rather
    /// than a number that happens to be zero: a folder with the button off, one still being counted,
    /// a junction the walk will not follow, and a row that is not there. That is the same silence the
    /// column has always kept for a folder — `Entry::size` is noise for one — so the cell only ever
    /// gains text, never loses it.
    pub fn shown(&self, dir: &Dir, entry: usize) -> Option<u64> {
        let record = dir.entries.get(entry)?;
        // An entry whose size could only be learnt by decompressing it — see
        // [`crate::fs::dir::FLAG_UNSIZED`]. The same silence a folder keeps, for the same reason,
        // and here rather than at the cells because this function is the only thing they ask.
        if record.is_unsized() {
            return None;
        }
        if !record.is_dir() {
            return Some(record.size);
        }
        if !self.on {
            return None;
        }
        measured(self.slots.get(entry).copied()?)
    }

    /// The keys a Size sort orders by: what every row's cell is showing, by entry index.
    ///
    /// `None` unless the listing is actually being sorted by Size while measured, which is what keeps
    /// this off every other rebuild — and, in the sort, what says folders no longer lead. See
    /// [`crate::fs::sort::compare`].
    ///
    /// A folder with no total yet keys as **zero**, so it sits at the quiet end and rises as its
    /// answer lands. That is the honest place for "not known yet": the alternative puts an unmeasured
    /// folder at the top, claiming to be the biggest thing in the folder on the strength of nothing.
    pub fn keys(&self, dir: &Dir, sorted_by_size: bool) -> Option<Vec<u64>> {
        if !self.on || !sorted_by_size {
            return None;
        }
        Some(
            (0..dir.len())
                .map(|entry| self.shown(dir, entry).unwrap_or(0))
                .collect(),
        )
    }

    /// How many folders on show are still being counted, and how long the last round took.
    ///
    /// The status line's two states of one figure: `counting 12 folders`, then `counted in 2.4 s`.
    #[inline]
    pub fn waiting(&self) -> usize {
        self.waiting
    }

    #[inline]
    pub fn micros(&self) -> u64 {
        self.micros
    }

    /// What this listing wants counted, marking whatever it asks about as asked.
    ///
    /// **The fork between the two kinds of listing**, and it lives here rather than in `App` because
    /// this is where both inputs meet. `order` is the display order, so what gets counted is what is
    /// on show: a filter, `Ctrl+H` or a shut branch of a tree all narrow the work, and revealing a
    /// row later asks about that row alone — which is the whole reason [`MEASURING`] is a state
    /// distinct from [`UNMEASURED`].
    ///
    /// A link is given [`NO_NUMBER`] rather than asked about: the walk would not follow it, so there
    /// is no total to have and the service never sees a path it would refuse.
    pub fn wanted(&mut self, dir: &Dir, order: &[u32], order_gen: u64, flat: bool) -> Work {
        if !self.on || self.slots.len() != dir.len() {
            return Work::None;
        }
        // Nothing can have come on show since the last scan — see [`Self::scanned`]. This is the
        // branch nearly every frame takes, and it is the whole reason a `u64` is kept.
        if self.scanned == order_gen && (self.counted || !flat) {
            return Work::None;
        }
        self.scanned = order_gen;
        if flat {
            self.counted = true;
            // Counted off the listing itself, in one pass, with no disk at all. `micros` is taken
            // like the walk's is, so the status line reads the same whichever kind of listing is on
            // show — and this is the figure that says the flat case is milliseconds where the other
            // is seconds.
            let started = std::time::Instant::now();
            self.slots = measure_listing(dir);
            self.idle();
            self.micros = started.elapsed().as_micros() as u64;
            self.total = None;
            // Every folder got its number in one go, so a Size sort is owed its order at once — there
            // is no stream of answers here to debounce against.
            self.order_stale = true;
            return Work::Done;
        }
        let mut wanted = Vec::new();
        for &row in order {
            let entry = row as usize;
            if !dir.entries[entry].is_dir() || self.slots[entry] != UNMEASURED {
                continue;
            }
            if dir.entries[entry].is_link() {
                self.slots[entry] = NO_NUMBER;
                continue;
            }
            if self.waiting == 0 {
                // The start of a round: nothing was outstanding, and now something is.
                self.started = Some(std::time::Instant::now());
                self.micros = 0;
            }
            self.slots[entry] = MEASURING;
            self.waiting += 1;
            wanted.push((row, dir.target(entry)));
        }
        if wanted.is_empty() {
            Work::None
        } else {
            Work::Walk(wanted)
        }
    }

    /// Take one folder's total, if it is for this measurement and a row that is still there.
    ///
    /// `now` is the frame clock, and only the re-sort's deadline uses it: the *duration* is taken from
    /// an `Instant` because it has to be right whether or not a frame was drawn when the last answer
    /// landed. See [`Self::resort_at`] and [`Self::started`].
    pub fn take(&mut self, answer: &Answer, now: f64) -> bool {
        if answer.gen != self.gen {
            return false;
        }
        let Some(slot) = self.slots.get_mut(answer.row as usize) else {
            return false;
        };
        let was_waiting = *slot == MEASURING;
        // Clamped below the states, so a folder holding more bytes than any disk has ever held cannot
        // come back looking like a question. Nothing real is anywhere near it, and the clamp lives
        // here because the state space does.
        *slot = answer.bytes.min(NO_NUMBER - 1);
        self.total = None;
        // A Size sort was built from the number that just changed, so the order owes a rebuild — from
        // *this* moment, because the next answer is milliseconds away and rebuilding per answer is
        // rebuilding per frame. See [`Self::resort_at`].
        self.order_stale = true;
        self.resort_at.get_or_insert(now);
        if was_waiting {
            self.waiting -= 1;
            // The last folder of the round. The duration is taken here rather than on the frame that
            // notices, because a frame is not a clock — the window is idle between events, and this
            // answer is what woke it. `owed` then treats a finished round as due whatever the deadline
            // says, so the settled order is right on the next frame rather than a quarter-second later.
            if self.waiting == 0 {
                if let Some(started) = self.started.take() {
                    self.micros = started.elapsed().as_micros() as u64;
                }
            }
        }
        true
    }
}

/// One folder being counted: what has been added up so far, and how much is still to read.
///
/// Shared by every directory of that folder still in the queue or in a worker's hands, which is
/// what lets the work be split by directory while the *answer* is still per folder.
struct Job {
    gen: u64,
    row: u32,
    bytes: AtomicU64,
    /// Directories pushed for this folder and not yet retired.
    ///
    /// **It reaches zero exactly once**, on whichever thread reads the last directory — and that
    /// thread is the one that reports the total. It starts at one (the folder itself), every
    /// directory adds its subdirectories to it *before* retiring itself, and a directory belonging
    /// to a dead generation is dropped without retiring at all, so a cancelled folder simply never
    /// reaches zero and never answers.
    ///
    /// The counters are [`Atomic::SeqCst`] throughout, which is stronger than the acquire/release
    /// pair this needs and costs nothing that can be measured: it is a handful of atomic operations
    /// against a directory read of a hundred microseconds warm and a great deal more cold. The
    /// alternative is an ordering argument in a comment, which is a worse thing to have to trust.
    outstanding: AtomicUsize,
}

/// One directory to read, and the folder it counts towards.
struct Task {
    job: Arc<Job>,
    path: PathBuf,
}

struct Queue {
    pending: Mutex<VecDeque<Task>>,
    /// The measurements still wanted.
    ///
    /// Checked by every worker before it reads a directory, which is the whole of how the work is
    /// cancelled — there is nothing to signal and nothing to join. See [`Sizes::only`].
    live: Mutex<HashSet<u64>>,
    wake: Condvar,
    shutdown: AtomicBool,
}

impl Queue {
    /// Whether the measurement a piece of work belongs to is still wanted.
    fn wanted(&self, gen: u64) -> bool {
        self.live
            .lock()
            .map(|live| live.contains(&gen))
            .unwrap_or(false)
    }
}

/// The measuring service. One per application.
pub struct Sizes {
    queue: Arc<Queue>,
    answers: Receiver<Answer>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl Sizes {
    pub fn new(ctx: &egui::Context) -> Self {
        // The deep walk's own figure, and the same reasoning arrived at twice independently: bound by
        // the file system rather than by the CPU, one burst of thousands of reads rather than a steady
        // trickle of one, and sixteen a wash against eight. See [`scan::hands`], and this module's
        // header for the measurement that agrees with it.
        Self::with_hands(ctx, scan::hands())
    }

    /// The same, with the thread count spelled out, for [`tests::measuring_speed`] — where one
    /// worker is the serial walk the figures in this module's header are measured against.
    fn with_hands(ctx: &egui::Context, hands: usize) -> Self {
        let queue = Arc::new(Queue {
            pending: Mutex::new(VecDeque::new()),
            live: Mutex::new(HashSet::new()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let (tx, answers) = channel();
        let workers = (0..hands.max(1))
            .map(|i| {
                let queue = queue.clone();
                let tx = tx.clone();
                let ctx = ctx.clone();
                std::thread::Builder::new()
                    .name(format!("measure-{i}"))
                    .spawn(move || worker(queue, tx, ctx))
                    .expect("the OS refused a thread")
            })
            .collect();
        Self {
            queue,
            answers,
            workers,
        }
    }

    /// Count these folders, for one measurement.
    ///
    /// Queued in the order they are handed over, which is the display order: the rows at the top of
    /// the listing are the ones being looked at while the rest is still being counted. Only the
    /// *roots* go on the back — the subdirectories a worker finds go on the front, so the queue
    /// expands depth-first. See the module header.
    pub fn request(&self, gen: u64, folders: Vec<(u32, PathBuf)>) {
        if folders.is_empty() {
            return;
        }
        if let Ok(mut live) = self.queue.live.lock() {
            live.insert(gen);
        }
        if let Ok(mut pending) = self.queue.pending.lock() {
            for (row, path) in folders {
                pending.push_back(Task {
                    job: Arc::new(Job {
                        gen,
                        row,
                        bytes: AtomicU64::new(0),
                        // The folder itself. Every subdirectory found under it is added to this
                        // before the directory that found it retires — see [`Job::outstanding`].
                        outstanding: AtomicUsize::new(1),
                    }),
                    path,
                });
            }
        }
        self.queue.wake.notify_all();
    }

    /// Keep only these measurements, and abandon every piece of work belonging to any other.
    ///
    /// Called once a frame with the generation of every tab whose measure button is on, which is
    /// how all five ways a walk stops being wanted are handled by one rule rather than five: the
    /// tab moved on, the folder was refreshed, the button went off, the tab was closed, the pane
    /// was closed. Work already in a worker's hands is dropped at its next directory; work still in
    /// the queue is thrown away here, and with it the last references to the folder it was for.
    ///
    /// The two locks are taken one at a time and never nested, which is what makes the ordering
    /// against [`Self::request`] and [`worker`] a non-question.
    pub fn only(&self, gens: &[u64]) {
        if let Ok(mut live) = self.queue.live.lock() {
            live.retain(|gen| gens.contains(gen));
        }
        if let Ok(mut pending) = self.queue.pending.lock() {
            pending.retain(|task| gens.contains(&task.job.gen));
        }
    }

    /// Every total that has arrived since the last call.
    pub fn drain(&self) -> impl Iterator<Item = Answer> + '_ {
        self.answers.try_iter()
    }
}

impl Drop for Sizes {
    fn drop(&mut self) {
        // Emptied first, so a walk in progress sees its generation gone and stops at its next
        // directory instead of counting a tree the window will not be there to show.
        if let Ok(mut live) = self.queue.live.lock() {
            live.clear();
        }
        // The flag is set *while holding the queue lock*, which is what makes the handshake sound:
        // a worker checks `shutdown` with the lock held and then calls `wait`, which releases it.
        // Setting it without the lock leaves a window where the notify lands before the worker is
        // waiting for it, and the worker then sleeps for ever on a queue nothing will push to. The
        // same handshake as [`crate::loader::Loader`] and [`crate::git::Git`], for the same reason.
        {
            let _held = self.queue.pending.lock();
            self.queue.shutdown.store(true, Atomic::SeqCst);
        }
        self.queue.wake.notify_all();
        // Detached rather than joined, for the reason [`crate::loader::Loader::drop`] sets out: a
        // walk inside a directory read on a share that has gone away cannot see the flag above, so
        // joining it is a redirector timeout spent with no window left to show the answer in. The
        // channel and the `Context` a worker holds are its own clones and outlive this, which is why
        // there was never anything being dropped underneath it.
        self.workers.clear();
    }
}

fn worker(queue: Arc<Queue>, tx: Sender<Answer>, ctx: egui::Context) {
    // Per-thread, and the reason counting a folder with an empty card reader in it does not raise
    // "Please insert a disk" from inside a syscall.
    scan::silence_device_dialogs();

    loop {
        let task = {
            let Ok(mut pending) = queue.pending.lock() else {
                return;
            };
            loop {
                if queue.shutdown.load(Atomic::SeqCst) {
                    return;
                }
                if let Some(task) = pending.pop_front() {
                    break task;
                }
                let Ok(next) = queue.wake.wait(pending) else {
                    return;
                };
                pending = next;
            }
        };

        // Nobody is waiting for this any more. Dropped **without retiring it**, so the folder's
        // counter never reaches zero and no half-counted total is ever reported.
        if !queue.wanted(task.job.gen) {
            continue;
        }

        let dir = scan::scan(&task.path);
        let mut bytes: u64 = 0;
        let mut children: Vec<PathBuf> = Vec::new();
        for i in 0..dir.len() {
            let entry = dir.entries[i];
            // [`scan::descends`] and not a test of its own: it is the named, *tested* home of the rule
            // that keeps a walk finite, and this is the second unbounded walk in the program asking
            // it. A junction is content of the folder and is listed by whoever is listing it; it is
            // simply not opened here.
            if scan::descends(entry.flags) {
                children.push(dir.target(i));
            } else if !entry.is_dir() {
                bytes = bytes.saturating_add(entry.size);
            }
        }
        // A directory that could not be read contributes nothing and is not an error: a tree of ten
        // thousand folders where one is denied is not a failed measurement, which is the same
        // judgement [`crate::fs::scan::scan_deep`] makes about the middle of a walk.
        if bytes > 0 {
            task.job.bytes.fetch_add(bytes, Atomic::SeqCst);
        }
        // **Counted in before this directory retires**, or the count could reach zero with a whole
        // subtree still to read and the folder would answer short.
        if !children.is_empty() {
            task.job
                .outstanding
                .fetch_add(children.len(), Atomic::SeqCst);
            if let Ok(mut pending) = queue.pending.lock() {
                for path in children {
                    // On the front: the queue expands depth-first, so the frontier is one path's
                    // siblings rather than a level of the tree. See the module header.
                    pending.push_front(Task {
                        job: task.job.clone(),
                        path,
                    });
                }
            }
            queue.wake.notify_all();
        }
        if task.job.outstanding.fetch_sub(1, Atomic::SeqCst) != 1 {
            continue;
        }
        // The last directory of this folder, so this thread is the one that reports it.
        if tx
            .send(Answer {
                gen: task.job.gen,
                row: task.job.row,
                bytes: task.job.bytes.load(Atomic::SeqCst),
            })
            .is_ok()
        {
            ctx.request_repaint();
        } else {
            return; // The UI is gone.
        }
    }
}

/// Every folder's total in a **flattened** listing, from the listing itself.
///
/// The one case that needs no disk at all and must not be given any: a flatten has already read
/// the whole tree — see [`crate::fs::scan::scan_deep`] — so every file that would be counted is
/// already in hand, and asking the service to walk each of twenty thousand folder rows would be
/// the same tree read twenty thousand times over.
///
/// One pass over the entries, adding each file's size to every folder above it by trimming its
/// stored name one component at a time. A name in a flattened listing *is* the path relative to
/// the root — [`Dir::name`] — so the ancestors are a string operation rather than a tree walk, and
/// the cost is one hash lookup per file per level: measured over `C:\Program Files` flattened
/// (188,729 entries) that is a handful of milliseconds against the 561 ms the walk itself took.
///
/// **A truncated flatten gives a lower bound**, and deliberately says nothing about it: the status
/// line already reports that the listing stopped at its limit, and a second warning attached to
/// every folder in it would be the same fact forty times over.
fn measure_listing(dir: &Dir) -> Vec<u64> {
    use std::collections::HashMap;

    let mut totals = vec![UNMEASURED; dir.len()];
    // Where each folder is, by the name it is stored under. Directories only — nothing is ever
    // inside a file — and a link is marked rather than entered, so it is not in here to be
    // credited with anything either.
    let mut folders: HashMap<&str, usize> = HashMap::with_capacity(dir.dir_count as usize);
    for (i, slot) in totals.iter_mut().enumerate() {
        let entry = dir.entries[i];
        if !entry.is_dir() {
            continue;
        }
        if entry.is_link() {
            *slot = NO_NUMBER;
            continue;
        }
        *slot = 0;
        folders.insert(dir.name(i), i);
    }

    for i in 0..dir.len() {
        let entry = dir.entries[i];
        if entry.is_dir() || entry.size == 0 {
            continue;
        }
        let mut name = dir.name(i);
        while let Some(at) = name.rfind(['\\', '/']) {
            name = &name[..at];
            if let Some(&folder) = folders.get(name) {
                totals[folder] = totals[folder].saturating_add(entry.size);
            }
        }
    }
    totals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::dir::{DirBuilder, FLAG_DIR, FLAG_LINK};

    /// Count one folder on this thread.
    ///
    /// The same rules as the service — reparse points not followed, unreadable directories skipped —
    /// and no parallelism at all, which is the point: it is the answer the service's arithmetic is
    /// checked against, and the serial baseline the figures in this module's header start from.
    fn walk_here(root: &std::path::Path) -> u64 {
        let mut total: u64 = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            let dir = scan::scan(&path);
            for i in 0..dir.len() {
                let entry = dir.entries[i];
                if entry.is_dir() {
                    if !entry.is_link() {
                        stack.push(dir.target(i));
                    }
                } else {
                    total = total.saturating_add(entry.size);
                }
            }
        }
        total
    }

    /// Run a measurement to completion and hand back what every folder answered.
    fn measure(hands: usize, folders: Vec<(u32, PathBuf)>) -> Vec<(u32, u64)> {
        let ctx = egui::Context::default();
        let sizes = Sizes::with_hands(&ctx, hands);
        let gen = 7;
        let wanted = folders.len();
        sizes.request(gen, folders);
        let mut out = Vec::new();
        for _ in 0..20_000 {
            // The service is told on every "frame" that this generation is still wanted, which is
            // what `App::collect_sizes` does — and without it nothing would be counted at all.
            sizes.only(&[gen]);
            out.extend(sizes.drain().map(|answer| (answer.row, answer.bytes)));
            if out.len() == wanted {
                return out;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("only {} of {wanted} folders answered", out.len());
    }

    /// **The parallel walk agrees with a serial one, folder for folder.**
    ///
    /// The whole risk in counting a directory at a time rather than a folder at a time is the
    /// counter that decides when a folder is finished: retire a directory before its
    /// subdirectories are counted in and the folder answers with part of its tree, silently and
    /// only sometimes. So this measures real folders with several threads and compares each total
    /// against [`walk_here`], which is the same rules with no threads at all.
    ///
    /// Two folders at once, each with a tree deep enough to be split across the workers, so the
    /// answers interleave — which is the arrangement that would expose a counter shared by
    /// accident.
    #[test]
    fn counting_a_directory_at_a_time_totals_the_same_as_counting_a_folder_at_a_time() {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let folders: Vec<(u32, PathBuf)> = ["app", "ui", "fs", "shell", "pane"]
            .iter()
            .enumerate()
            .map(|(i, name)| (i as u32, src.join(name)))
            .collect();
        let serial: Vec<u64> = folders.iter().map(|(_, path)| walk_here(path)).collect();
        assert!(
            serial.iter().all(|&bytes| bytes > 0),
            "the fixture folders are empty, so this proves nothing: {serial:?}"
        );

        for hands in [1, 4, 8] {
            let mut answers = measure(hands, folders.clone());
            answers.sort_unstable_by_key(|&(row, _)| row);
            let got: Vec<u64> = answers.into_iter().map(|(_, bytes)| bytes).collect();
            assert_eq!(
                got, serial,
                "{hands} workers did not agree with a serial walk"
            );
        }
    }

    /// A folder with nothing in it answers, and it answers zero.
    ///
    /// The counter's edge case: its root is the only directory there is, so it starts at one and is
    /// retired by the thread that reads it. A folder that never answered would leave its cell blank
    /// for ever, which is indistinguishable from one still being counted.
    #[test]
    fn a_folder_with_nothing_under_it_still_answers() {
        let empty = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("measure-empty");
        // Under `target/sandbox` and nowhere else, which is this suite's standing rule.
        std::fs::create_dir_all(&empty).expect("the sandbox could not be made");
        assert_eq!(measure(4, vec![(3, empty)]), [(3, 0)]);
    }

    /// **Dropping the generation abandons the work, and no total arrives.**
    ///
    /// The property the whole design rests on: a mistaken press on a folder the size of a drive has
    /// to stop costing the disk anything the moment the reader moves on. A worker that retired its
    /// directory anyway would let the counter reach zero and report part of a tree as though it were
    /// the whole of one.
    #[test]
    fn a_measurement_nobody_wants_is_abandoned_rather_than_finished() {
        let ctx = egui::Context::default();
        let sizes = Sizes::with_hands(&ctx, 4);
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        sizes.request(11, vec![(0, src)]);
        // Never named as live, which is what `App::collect_sizes` stops doing when the button goes
        // off — so every directory of it is thrown away as it is picked up.
        sizes.only(&[]);
        for _ in 0..200 {
            assert_eq!(
                sizes.drain().count(),
                0,
                "a total arrived for a measurement nobody asked to keep"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// What the choice of unit is worth, and what the thread count is worth on top of it.
    ///
    /// The figures in this module's header. Ignored, because it reads a real tree several times over
    /// and the answer depends on what else the machine is doing:
    ///
    /// ```text
    /// cargo test --release -- --ignored --nocapture measuring_speed
    /// ```
    ///
    /// `walk_here` is the honest serial baseline — one thread, one folder at a time — and the rows
    /// under it are the same work handed out a directory at a time.
    #[test]
    #[ignore = "a benchmark, not a test"]
    fn measuring_speed() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let folders: Vec<(u32, PathBuf)> = std::fs::read_dir(&root)
            .expect("the crate root")
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .enumerate()
            .map(|(i, e)| (i as u32, e.path()))
            .collect();

        // Warm the file system's own cache, so the first row measured is not the one that paid for
        // every other row's cache hits.
        let expected: u64 = folders.iter().map(|(_, path)| walk_here(path)).sum();

        let started = std::time::Instant::now();
        let serial: u64 = folders.iter().map(|(_, path)| walk_here(path)).sum();
        println!(
            "  1 thread,  a folder each   {:>6} ms",
            started.elapsed().as_millis()
        );
        assert_eq!(serial, expected);

        // **The arrangement this module replaced**: N threads, each taking a whole folder and
        // walking it alone. It is what the obvious design gives, and the row worth comparing
        // against — the same thread count doing the same work in a coarser unit. `walk_here` is
        // the walk, so the only difference from the rows below is what is being handed out.
        for threads in [4, 8] {
            let started = std::time::Instant::now();
            let next = AtomicUsize::new(0);
            let total: u64 = std::thread::scope(|scope| {
                let workers: Vec<_> = (0..threads)
                    .map(|_| {
                        let next = &next;
                        let folders = &folders;
                        scope.spawn(move || {
                            let mut mine = 0u64;
                            loop {
                                let at = next.fetch_add(1, Atomic::SeqCst);
                                let Some((_, path)) = folders.get(at) else {
                                    return mine;
                                };
                                mine += walk_here(path);
                            }
                        })
                    })
                    .collect();
                workers.into_iter().filter_map(|w| w.join().ok()).sum()
            });
            println!(
                "  {threads} threads, a folder each   {:>6} ms",
                started.elapsed().as_millis()
            );
            assert_eq!(total, expected);
        }

        for hands in [1, 2, 4, 8, 16] {
            let started = std::time::Instant::now();
            let total: u64 = measure(hands, folders.clone())
                .into_iter()
                .map(|(_, bytes)| bytes)
                .sum();
            println!(
                "  {hands:>2} workers, a directory each  {:>6} ms",
                started.elapsed().as_millis()
            );
            // The poll below sleeps a millisecond between rounds, so every figure here carries up to
            // one of overhead the real thing does not — the window is woken by the answer arriving.
            // Named because it is the one thing that makes these rows not quite comparable with the
            // ones above, and it flatters the slow rows rather than the fast ones.
            assert_eq!(total, expected, "{hands} workers came to a different total");
        }
    }

    /// The flattened case, which is arithmetic rather than I/O: every folder carries what is under
    /// it, however deep, and each file is counted once per ancestor and never twice for the same
    /// one.
    #[test]
    fn a_flattened_listing_carries_its_own_totals() {
        let mut builder = DirBuilder::new("D:\\root");
        for (name, size, flags) in [
            ("top.txt", 100u64, 0u16),
            ("sub", 0, FLAG_DIR),
            ("sub\\a.txt", 20, 0),
            ("sub\\deep", 0, FLAG_DIR),
            ("sub\\deep\\b.txt", 3, 0),
            ("other", 0, FLAG_DIR),
            ("link", 0, FLAG_DIR | FLAG_LINK),
        ] {
            builder.push(name, size, 0, flags);
        }
        let dir = builder.finish(0);
        let totals = measure_listing(&dir);
        let of = |want: &str| {
            let i = (0..dir.len())
                .find(|&i| dir.name(i) == want)
                .expect("pushed above");
            totals[i]
        };

        // Both levels, and the deep file counted in each of the two folders above it.
        assert_eq!(measured(of("sub")), Some(23));
        assert_eq!(measured(of("sub\\deep")), Some(3));
        // A folder with nothing under it is a real answer, and it is zero.
        assert_eq!(measured(of("other")), Some(0));
        // A junction is not followed, so it has no number rather than a wrong one.
        assert_eq!(of("link"), NO_NUMBER);
        assert_eq!(measured(of("link")), None);
        // The top-level rows partition the tree, which is what makes `Dir::total_size` the
        // denominator a flattened listing's bars are measured against.
        assert_eq!(of("sub") + 100, dir.total_size);
    }

    /// The three states are not byte counts, and the boundary between them and a real answer is
    /// asked about in exactly one place.
    #[test]
    fn the_states_are_not_sizes() {
        assert_eq!(measured(UNMEASURED), None);
        assert_eq!(measured(MEASURING), None);
        assert_eq!(measured(NO_NUMBER), None);
        assert_eq!(measured(NO_NUMBER - 1), Some(NO_NUMBER - 1));
        assert_eq!(measured(0), Some(0));
    }
}
