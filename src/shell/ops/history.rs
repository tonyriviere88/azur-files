//! Ctrl+Z and Ctrl+Y: the operations this window has performed, and how to take each one back.
//!
//! # Why this exists at all
//!
//! Every operation in [`super`] is issued with `FOF_ALLOWUNDO | FOFX_ADDUNDORECORD`, which is not
//! decorative — it puts the operation in the shell's own undo stack, so Ctrl+Z **in Explorer**
//! takes back a copy this program made. What shell32 does not export is any way to *replay* that
//! stack: there is no `SHUndo`, no undo method on `IFileOperation`, and no namespace object
//! standing for it. Explorer's Edit ▸ Undo is Explorer's own code reading its own list.
//!
//! So the list is this program's, and Windows performs the individual reversals. The division is
//! worth being precise about, because the interesting half is not the stack:
//!
//! | | |
//! | --- | --- |
//! | remembering what happened | here |
//! | knowing what actually happened | [`Outcome`], filled by the progress sink — **not** inferred from the [`Job`] |
//! | performing the reversal | `IFileOperation` again, or the Recycle Bin's `undelete` verb |
//!
//! # What each operation is undone by
//!
//! | operation | undone by | redone by |
//! | --- | --- | --- |
//! | new folder | recycling what was made | making it again |
//! | copy, paste of a copy, drag-copy | recycling the copies that arrived | copying again |
//! | move, paste of a cut, drag-move | [`Job::PutBack`] — each item to the folder it came from | moving again |
//! | rename | [`Job::PutBack`] — one item, one folder, the old name | renaming again |
//! | delete to the Recycle Bin | [`Job::Restore`] — the `undelete` verb | deleting again |
//! | **Shift+Delete** | **nothing.** The file is gone; an entry here would be an offer this program cannot honour | — |
//!
//! Redo is the original job run again, unchanged, and that is not a shortcut: undoing puts every
//! item back where the job first found it, so the job's own description of what to do is true
//! again. What redo does *not* do is reproduce the shell's answers — a copy redone into a folder
//! that now has a name clash asks again, and may land as `one (2).txt`. The new outcome is what
//! gets recorded, so a second Ctrl+Z takes back what the second copy did rather than what the
//! first one did.
//!
//! # What it deliberately does not promise
//!
//! **A replaced file is not brought back.** Answering the shell's conflict dialog with Replace
//! overwrites the file that was there, and nothing — not this history, not Explorer's — can
//! produce it again. Undo puts back what this program *moved*; it does not resurrect what the
//! move landed on. The shell asked before doing it, which is where that decision was made.
//!
//! **Nothing survives the window closing.** A path that was true this session may be a different
//! file next session, and offering to move something back into a folder that has since been
//! rearranged is worse than not offering. Explorer's own undo stack dies with the window too.
//!
//! **Only what this window did.** An operation another program performed, or one the shell's own
//! context menu performed — `New ▸ Text Document`, an extension's *Extract here* — is not here,
//! because nothing reported what it did. Ctrl+Z takes back this window's last operation, which
//! may not be the last thing that happened to the folder on screen.

use super::{Done, Job, Outcome};

/// How many operations back Ctrl+Z reaches.
///
/// A cap rather than a session's worth, because an entry holds every path the operation touched:
/// a select-all move of a photo library is a hundred thousand `PathBuf`s, and thirty of those in
/// a stack nobody is going to walk to the bottom of is memory spent on a promise. Deep enough
/// that reaching the end of it is a surprise, which is what the number is for.
const KEEP: usize = 30;

/// One operation, and everything needed to take it back or do it again.
#[derive(Clone, Debug)]
struct Entry {
    /// The job as it was asked for. Redo asks for it again.
    job: Job,
    /// What the shell reported it actually did. Undo works from this and never from `job`.
    outcome: Outcome,
}

/// An undo or a redo that has been started and has not reported back.
///
/// The entry travels with it rather than staying on a stack, so that a reversal the shell refuses
/// — or that the user cancels at a conflict dialog — puts it back where it came from instead of
/// losing it or moving it to the other side.
#[derive(Clone, Debug)]
enum Running {
    Undoing(Entry),
    Redoing(Entry),
}

/// What this window has done, and what it has taken back.
#[derive(Default)]
pub struct History {
    /// Operations that can be taken back, oldest first. The last is what Ctrl+Z acts on.
    done: Vec<Entry>,
    /// Operations that have been taken back, oldest first. The last is what Ctrl+Y acts on.
    undone: Vec<Entry>,
    /// The one reversal in flight, if there is one. See [`Running`].
    running: Option<Running>,
}

impl History {
    /// Note an operation that has finished, if there is anything to take back.
    ///
    /// Everything that reaches here is filtered on what the shell *did*, never on what it was
    /// asked to do, so all four of these fall out without a special case: a paste answered with
    /// Skip, a copy cancelled at the first conflict, a rename to the name it already had, and a
    /// permanent delete. Each leaves an empty [`Outcome`] and each is correctly not undoable.
    ///
    /// Taken by value, which is why the caller hands the operation over last: an entry *is* the job
    /// and the outcome, and both name every path the operation touched. A select-all move across a
    /// photo library is a hundred thousand of them, and there is no reason to copy those only to
    /// drop the originals on the next line.
    pub fn record(&mut self, done: Done) {
        // An undo or a redo, which moves an entry between the stacks rather than making one.
        if done.after == super::After::Settle {
            self.settle(done);
            return;
        }
        let Done { job, outcome, .. } = done;
        let Some(job) = job else { return };
        self.keep(Entry { job, outcome });
        // Fresh work invalidates the redo stack, as it does in every editor: what was undone was
        // undone against a folder that has since changed underneath it.
        self.undone.clear();
    }

    /// Put an entry on the undo stack, if there is anything left to take it back with.
    ///
    /// The invertibility is decided **here and not when Ctrl+Z is pressed**, so that an entry
    /// which cannot be reversed never reaches the stack at all. The other way round, an undo that
    /// came up empty after the keystroke would take the user's Ctrl+Z and quietly do nothing with
    /// it — and, worse, would have consumed the entry doing so.
    fn keep(&mut self, entry: Entry) {
        if inverse(&entry.outcome).is_none() {
            return;
        }
        self.done.push(entry);
        if self.done.len() > KEEP {
            self.done.remove(0);
        }
    }

    /// The job that takes the last operation back, if there is one to take back.
    ///
    /// The entry leaves the stack now and lands on the other one only when the shell says the
    /// reversal worked — see [`Self::settle`].
    pub fn undo(&mut self) -> Option<Job> {
        if self.running.is_some() {
            return None;
        }
        // Read before it is taken, so that a refusal leaves the stack as it was rather than
        // swallowing the entry on the way to answering `None`. [`Self::keep`] means this cannot
        // refuse; a `pop` that could lose an operation is not worth being one bug away from.
        let job = inverse(&self.done.last()?.outcome)?;
        let entry = self.done.pop()?;
        self.running = Some(Running::Undoing(entry));
        Some(job)
    }

    /// The job that does the last undone operation again.
    pub fn redo(&mut self) -> Option<Job> {
        if self.running.is_some() {
            return None;
        }
        let entry = self.undone.pop()?;
        let job = entry.job.clone();
        self.running = Some(Running::Redoing(entry));
        Some(job)
    }

    /// A reversal has finished. Move its entry to the other stack, or put it back.
    ///
    /// Whether it worked is the shell's answer, and a **cancel counts as not working** — which is
    /// the case this exists for. A user who reaches a conflict dialog on the way back and answers
    /// Cancel has undone nothing, and the entry has to still be there for the Ctrl+Z they are
    /// about to press again.
    ///
    /// # Why a redo replaces the outcome it came with
    ///
    /// A redo is the original job run *again*, and the second run is not the first one. Nothing
    /// guarantees it lands the same way: the folder has changed since, so a copy that was
    /// `one - Copy.txt` may this time be `one - Copy (2).txt`, and a delete produces an entirely
    /// new `$R…` file in the Recycle Bin. The old outcome describes the *first* run.
    ///
    /// Keeping it would be worse than losing the undo. The stale paths from a delete are merely
    /// dead — the next Ctrl+Z would report the item is no longer in the bin. The stale path from a
    /// copy is live and belongs to **something else**: `one - Copy.txt` still exists, it was not
    /// made by the redo, and a Ctrl+Z built on the old outcome would recycle a file this program
    /// never touched. So the entry takes on what the redo actually did, and [`Self::keep`] drops it
    /// if that turns out to be nothing.
    ///
    /// # What it does not fix
    ///
    /// A reversal that **partly** worked. `PerformOperations` reports success when it ran, so a
    /// `PutBack` of three items where one could not be moved comes back as worked, and the entry
    /// crosses over with one item still on the wrong side. This is where the trail stops being
    /// exact, and it is the same place Explorer's does; the alternative — splitting an entry in
    /// two — makes a Ctrl+Y that means half of something, which is harder to reason about than a
    /// history that is one operation behind.
    fn settle(&mut self, done: Done) {
        // [`Done::worked`] and not `error.is_none()`, which is what this used to ask and which is
        // never false for a cancel — so the promise three paragraphs up was the opposite of what
        // the code did. See there.
        let worked = done.worked();
        match (self.running.take(), worked) {
            (Some(Running::Undoing(entry)), true) => self.undone.push(entry),
            (Some(Running::Redoing(entry)), true) => self.keep(Entry {
                outcome: done.outcome,
                ..entry
            }),
            // Refused or cancelled: back where it was, so the same keystroke can be tried again.
            // Through `keep` on the undo side, since that is the stack Ctrl+Z reads.
            (Some(Running::Undoing(entry)), false) => self.keep(entry),
            (Some(Running::Redoing(entry)), false) => self.undone.push(entry),
            (None, _) => {}
        }
    }

    /// Whether Ctrl+Z has anything to act on.
    ///
    /// **Only the tests ask.** Nothing in the window is greyed out or ticked according to the
    /// history — the two shortcuts are the whole of the interface, and one that has nothing to do
    /// says so through [`Self::why_not`] when it is pressed. A menu entry offering Undo would want
    /// this, and would be the reason to make it public.
    #[cfg(test)]
    pub fn can_undo(&self) -> bool {
        self.running.is_none() && !self.done.is_empty()
    }

    /// Whether Ctrl+Y has anything to act on. As [`Self::can_undo`], and as test-only.
    #[cfg(test)]
    pub fn can_redo(&self) -> bool {
        self.running.is_none() && !self.undone.is_empty()
    }

    /// Why a keystroke did nothing, in words, for the status line.
    ///
    /// A reversal already running is a different sentence from an empty stack, because they are
    /// different situations: one is "wait", the other is "there is nothing there". Both are worth
    /// saying — a Ctrl+Z that silently does nothing reads as a Ctrl+Z that is not wired up.
    pub fn why_not(&self, redo: bool) -> &'static str {
        match (&self.running, redo) {
            // Which way the one in flight is going, not which key was just pressed: what the user
            // needs to know is what the window is busy with.
            (Some(Running::Undoing(_)), _) => "Still undoing the last one…",
            (Some(Running::Redoing(_)), _) => "Still redoing the last one…",
            (None, true) => "Nothing to redo",
            (None, false) => "Nothing to undo",
        }
    }
}

/// The job that takes an outcome back.
///
/// One kind of reversal per operation, which is what [`Outcome::one_kind`] establishes: a job
/// whose outcome mixed two of them would need two jobs, and doing the first and calling it undone
/// is worse than declining.
fn inverse(outcome: &Outcome) -> Option<Job> {
    Some(match outcome.one_kind()? {
        // Things that were not there before go where anything else goes when it is deleted: the
        // Recycle Bin, never `to_bin: false`. An undo is not a decision to destroy anything —
        // most of all not the folder somebody made, filled, and is now undoing the *making* of.
        Kind::Created => Job::Delete {
            items: outcome.created.clone(),
            to_bin: true,
        },
        // Each item from where it is now to where it was, which is the pair the sink recorded
        // read the other way round.
        Kind::Moved => Job::PutBack {
            items: outcome
                .moved
                .iter()
                .map(|(was, now)| (now.clone(), was.clone()))
                .collect(),
        },
        Kind::Recycled => Job::Restore {
            items: outcome.recycled.clone(),
        },
    })
}

/// The one kind of thing an operation did. See [`Outcome::one_kind`].
///
/// Private, and the `impl Outcome` below it with it: both are the history's reading of an outcome
/// rather than anything the outcome itself is. Nothing outside this module has to know that undo
/// sorts operations into three shapes.
enum Kind {
    Created,
    Moved,
    Recycled,
}

impl Outcome {
    /// Which of the three kinds of change this operation made, when it made only one.
    ///
    /// Every job in [`Job`] produces exactly one: a copy and a new folder create, a move and a
    /// rename relocate, a recycle recycles. So `None` here means either nothing happened or the
    /// shell did something this program has not accounted for — a `Job::Move` that came back
    /// reported half as copies, say. Either way the honest answer is to offer no undo rather than
    /// an undo of part of it, and to say so where a debug build will be heard.
    fn one_kind(&self) -> Option<Kind> {
        let kinds = [
            (!self.created.is_empty(), Kind::Created),
            (!self.moved.is_empty(), Kind::Moved),
            (!self.recycled.is_empty(), Kind::Recycled),
        ];
        let mut only = None;
        for (present, kind) in kinds {
            if !present {
                continue;
            }
            if only.is_some() {
                debug_assert!(
                    false,
                    "one operation reported more than one kind of change, so it cannot be \
                     undone as one: {self:?}"
                );
                return None;
            }
            only = Some(kind);
        }
        only
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::ops::{After, Recycled};
    use std::path::PathBuf;

    /// A finished operation, as the shell would have reported it.
    fn done(job: Job, outcome: Outcome) -> Done {
        Done {
            job: Some(job),
            touched: Vec::new(),
            error: None,
            aborted: false,
            after: After::Nothing,
            outcome,
        }
    }

    /// The reversal reporting back, worked or refused.
    fn settled(worked: bool) -> Done {
        settled_with(worked, Outcome::default())
    }

    /// The same, for a redo — which did the work again and so has an outcome of its own.
    fn settled_with(worked: bool, outcome: Outcome) -> Done {
        Done {
            job: None,
            touched: Vec::new(),
            error: (!worked).then(|| "no".to_owned()),
            aborted: false,
            after: After::Settle,
            outcome,
        }
    }

    /// **A reversal the user stopped, in the shape the shell really reports it.**
    ///
    /// Which is the whole point of it being a separate helper from `settled(false)`: that one
    /// fakes an *error*, and a cancel is not one. `friendly` turns both cancel codes into no
    /// message at all, so `error` is `None` and only `aborted` says what happened — see
    /// [`Done::worked`]. Every test here used the faked error, which is why the stack spent two
    /// rounds of review looking correct while a real cancel did the opposite.
    fn cancelled() -> Done {
        Done {
            job: None,
            touched: Vec::new(),
            error: None,
            aborted: true,
            after: After::Settle,
            outcome: Outcome::default(),
        }
    }

    fn copy(into: &str, made: &[&str]) -> Done {
        done(
            Job::Copy {
                items: vec![PathBuf::from(r"C:\from\one.txt")],
                into: PathBuf::from(into),
            },
            Outcome {
                created: made.iter().map(PathBuf::from).collect(),
                ..Default::default()
            },
        )
    }

    /// A copy is taken back by recycling **what arrived**, which is not what was asked for.
    ///
    /// The case in one test: `one.txt` copied into a folder that already had one, which the shell
    /// answers by making `one - Copy.txt`. An undo built on the job would delete `one.txt` — the
    /// file that was there first and that this program never touched.
    #[test]
    fn a_copy_is_undone_by_recycling_what_actually_arrived() {
        let mut history = History::default();
        history.record(copy(r"C:\into", &[r"C:\into\one - Copy.txt"]));
        assert!(history.can_undo());

        match history.undo() {
            Some(Job::Delete { items, to_bin }) => {
                assert_eq!(items, [PathBuf::from(r"C:\into\one - Copy.txt")]);
                assert!(to_bin, "an undo must never delete anything permanently");
            }
            other => panic!("expected a recycle of the copy, got {other:?}"),
        }
    }

    /// A move and a rename both come back as [`Job::PutBack`], with the pairs reversed.
    #[test]
    fn a_move_is_undone_by_putting_each_item_back_where_it_came_from() {
        let mut history = History::default();
        history.record(done(
            Job::Move {
                items: vec![PathBuf::from(r"C:\a\one.txt"), PathBuf::from(r"C:\b\two.txt")],
                into: PathBuf::from(r"C:\dest"),
            },
            Outcome {
                moved: vec![
                    (
                        PathBuf::from(r"C:\a\one.txt"),
                        PathBuf::from(r"C:\dest\one.txt"),
                    ),
                    // The shell kept both, so this one landed under a different name — and it is
                    // that name the undo has to move.
                    (
                        PathBuf::from(r"C:\b\two.txt"),
                        PathBuf::from(r"C:\dest\two (2).txt"),
                    ),
                ],
                ..Default::default()
            },
        ));

        match history.undo() {
            Some(Job::PutBack { items }) => assert_eq!(
                items,
                [
                    (
                        PathBuf::from(r"C:\dest\one.txt"),
                        PathBuf::from(r"C:\a\one.txt")
                    ),
                    (
                        PathBuf::from(r"C:\dest\two (2).txt"),
                        PathBuf::from(r"C:\b\two.txt")
                    ),
                ],
                "each item has to go back to its own folder, under its own old name"
            ),
            other => panic!("expected a put-back, got {other:?}"),
        }
    }

    #[test]
    fn a_rename_is_undone_by_the_old_name() {
        let mut history = History::default();
        history.record(done(
            Job::Rename {
                item: PathBuf::from(r"C:\a\one.txt"),
                name: "two.txt".to_owned(),
            },
            Outcome {
                moved: vec![(
                    PathBuf::from(r"C:\a\one.txt"),
                    PathBuf::from(r"C:\a\two.txt"),
                )],
                ..Default::default()
            },
        ));
        match history.undo() {
            Some(Job::PutBack { items }) => assert_eq!(
                items,
                [(
                    PathBuf::from(r"C:\a\two.txt"),
                    PathBuf::from(r"C:\a\one.txt")
                )]
            ),
            other => panic!("expected a put-back, got {other:?}"),
        }
    }

    /// A recycle comes back as the restore, carrying whatever the delete learned about the bin.
    #[test]
    fn a_recycle_is_undone_by_restoring_it() {
        let mut history = History::default();
        history.record(done(
            Job::Delete {
                items: vec![PathBuf::from(r"C:\a\one.txt")],
                to_bin: true,
            },
            Outcome {
                recycled: vec![Recycled {
                    from: PathBuf::from(r"C:\a\one.txt"),
                    bin: None,
                }],
                ..Default::default()
            },
        ));
        match history.undo() {
            Some(Job::Restore { items }) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].from, PathBuf::from(r"C:\a\one.txt"));
            }
            other => panic!("expected a restore, got {other:?}"),
        }
    }

    /// **A permanent delete is not undoable**, and the reason it falls out rather than being
    /// special-cased here is worth a test: the sink records nothing for one, so there is nothing
    /// to record. The same shape covers a paste answered with Skip and a cancelled copy.
    #[test]
    fn nothing_the_shell_did_not_do_is_offered_back() {
        let mut history = History::default();
        history.record(done(
            Job::Delete {
                items: vec![PathBuf::from(r"C:\a\one.txt")],
                to_bin: false,
            },
            Outcome::default(),
        ));
        history.record(copy(r"C:\into", &[]));
        assert!(
            !history.can_undo(),
            "an operation that did nothing was put on the undo stack"
        );
        assert_eq!(history.why_not(false), "Nothing to undo");
    }

    /// Undo, then redo, then undo again — the entry going back and forth between the stacks, and
    /// only ever moving when the shell says the reversal worked.
    #[test]
    fn an_entry_moves_between_the_stacks_as_each_reversal_reports_back() {
        let mut history = History::default();
        history.record(copy(r"C:\into", &[r"C:\into\one.txt"]));

        assert!(history.undo().is_some());
        // In flight: neither key does anything until it reports back, and the reason is said out
        // loud rather than being a silent no-op.
        assert!(!history.can_undo() && !history.can_redo());
        // And it says which way the one in flight is going, not which key was pressed.
        assert_eq!(history.why_not(false), "Still undoing the last one…");
        assert_eq!(history.why_not(true), "Still undoing the last one…");
        assert!(history.undo().is_none(), "two undos ran at once");

        history.record(settled(true));
        assert!(history.can_redo(), "the undone copy is not redoable");
        assert!(!history.can_undo());

        // Redo is the original job again, not an inverse of the inverse.
        match history.redo() {
            Some(Job::Copy { into, .. }) => assert_eq!(into, PathBuf::from(r"C:\into")),
            other => panic!("expected the copy again, got {other:?}"),
        }
        // With what the second run did, which is what the next Ctrl+Z acts on — see
        // [`a_redo_takes_on_what_it_actually_did`].
        history.record(settled_with(
            true,
            Outcome {
                created: vec![PathBuf::from(r"C:\into\one.txt")],
                ..Default::default()
            },
        ));
        assert!(history.can_undo() && !history.can_redo());
    }

    /// **A redo lands where it lands, and the next undo has to take back *that*.**
    ///
    /// The trap, in one test. Undo a copy of `one - Copy.txt`, then redo it — into a folder that
    /// has changed since, so the copy comes back as `one - Copy (2).txt`. If the entry kept the
    /// outcome it was made with, the next Ctrl+Z would recycle `one - Copy.txt`: a file that still
    /// exists, that the redo did not make, and that this program has no business deleting.
    #[test]
    fn a_redo_takes_on_what_it_actually_did() {
        let mut history = History::default();
        history.record(copy(r"C:\into", &[r"C:\into\one - Copy.txt"]));
        history.undo();
        history.record(settled(true));

        history.redo().expect("the copy again");
        history.record(settled_with(
            true,
            Outcome {
                created: vec![PathBuf::from(r"C:\into\one - Copy (2).txt")],
                ..Default::default()
            },
        ));

        match history.undo() {
            Some(Job::Delete { items, .. }) => assert_eq!(
                items,
                [PathBuf::from(r"C:\into\one - Copy (2).txt")],
                "the undo went for the first copy's name, which belongs to another file now"
            ),
            other => panic!("expected a recycle, got {other:?}"),
        }
    }

    /// And a redo that turned out to do nothing leaves nothing to take back.
    ///
    /// Every conflict answered with Skip, say. The entry has to go rather than sit on the stack
    /// with a Ctrl+Z behind it that cannot act.
    #[test]
    fn a_redo_that_did_nothing_leaves_nothing_to_undo() {
        let mut history = History::default();
        history.record(copy(r"C:\into", &[r"C:\into\one.txt"]));
        history.undo();
        history.record(settled(true));
        history.redo().expect("the copy again");
        history.record(settled_with(true, Outcome::default()));
        assert!(!history.can_undo(), "an empty redo was left undoable");
        assert!(!history.can_redo());
    }

    /// A reversal the shell refused — or that the user cancelled at a conflict dialog — leaves
    /// the entry exactly where it was, so the same keystroke can be tried again.
    #[test]
    fn a_refused_undo_keeps_the_entry() {
        let mut history = History::default();
        history.record(copy(r"C:\into", &[r"C:\into\one.txt"]));
        assert!(history.undo().is_some());
        history.record(settled(false));
        assert!(
            history.can_undo(),
            "a cancelled undo threw the operation away"
        );
        assert!(
            !history.can_redo(),
            "a cancelled undo offered a redo of something that was never undone"
        );
    }

    /// **The same thing again, reported the way the shell actually reports it.**
    ///
    /// [`a_refused_undo_keeps_the_entry`] passed throughout the bug this is for, because it fakes
    /// an error and a cancel is not an error: `PerformOperations` answers `S_OK` for a job the user
    /// stopped, and the cancel `HRESULT` it does sometimes give is deliberately turned into no
    /// message. So `error.is_none()` — which is what `settle` used to ask — was true, and a
    /// cancelled Ctrl+Z moved its entry to the *redo* stack. Cut 500 photos into another folder,
    /// press Ctrl+Z, answer the conflict dialog with Cancel, and 498 files stayed where they were
    /// while Ctrl+Z reported "Nothing to undo" and the only key left re-ran the move.
    #[test]
    fn an_undo_the_user_cancelled_keeps_the_entry() {
        let mut history = History::default();
        history.record(copy(r"C:\into", &[r"C:\into\one.txt"]));
        assert!(history.undo().is_some());
        history.record(cancelled());
        assert!(
            history.can_undo(),
            "a cancelled undo was taken for a successful one, so the operation can never be \
             reversed again"
        );
        assert!(
            !history.can_redo(),
            "a cancelled undo offered a redo of something that was never undone"
        );
    }

    /// And a cancelled *redo* stays on the redo stack, for the same reason read the other way.
    #[test]
    fn a_redo_the_user_cancelled_keeps_the_entry() {
        let mut history = History::default();
        history.record(copy(r"C:\into", &[r"C:\into\one.txt"]));
        history.undo();
        history.record(settled(true));
        assert!(history.redo().is_some());
        history.record(cancelled());
        assert!(history.can_redo(), "a cancelled redo threw the entry away");
        assert!(
            !history.can_undo(),
            "a cancelled redo offered an undo of work it never did"
        );
    }

    /// Fresh work throws the redo stack away, as it does in every editor.
    #[test]
    fn doing_something_new_ends_the_redo_stack() {
        let mut history = History::default();
        history.record(copy(r"C:\into", &[r"C:\into\one.txt"]));
        history.undo();
        history.record(settled(true));
        assert!(history.can_redo());

        history.record(copy(r"C:\other", &[r"C:\other\one.txt"]));
        assert!(!history.can_redo(), "the redo stack survived new work");
        assert!(history.can_undo());
    }

    /// The stack is capped, and it is the oldest that goes.
    #[test]
    fn the_history_is_bounded() {
        let mut history = History::default();
        for n in 0..KEEP + 5 {
            history.record(copy(r"C:\into", &[&format!(r"C:\into\{n}.txt")]));
        }
        assert_eq!(history.done.len(), KEEP);
        match history.undo() {
            Some(Job::Delete { items, .. }) => assert_eq!(
                items,
                [PathBuf::from(format!(r"C:\into\{}.txt", KEEP + 4))],
                "the newest operation is the one Ctrl+Z takes back"
            ),
            other => panic!("expected a recycle, got {other:?}"),
        }
        // And the oldest went, rather than the stack silently keeping everything.
        assert!(
            !history.done.iter().any(|entry| matches!(
                &entry.outcome.created[..],
                [only] if only == std::path::Path::new(r"C:\into\0.txt")
            )),
            "the oldest entry was kept past the cap"
        );
    }

    /// An outcome that reports two kinds of change at once is refused rather than half-undone.
    ///
    /// Nothing in [`Job`] produces one, which is why the `debug_assert` in [`Outcome::one_kind`]
    /// is allowed to be loud — so this is checked in a release build only, where that assert is
    /// compiled out.
    #[test]
    #[cfg(not(debug_assertions))]
    fn a_mixed_outcome_is_not_offered_back() {
        let mut history = History::default();
        history.record(done(
            Job::Move {
                items: vec![PathBuf::from(r"C:\a\one.txt")],
                into: PathBuf::from(r"C:\dest"),
            },
            Outcome {
                created: vec![PathBuf::from(r"C:\dest\one.txt")],
                moved: vec![(
                    PathBuf::from(r"C:\a\two.txt"),
                    PathBuf::from(r"C:\dest\two.txt"),
                )],
                ..Default::default()
            },
        ));
        assert!(!history.can_undo());
    }
}
