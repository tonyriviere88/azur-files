//! Copy, move, delete, rename and new-folder — through `IFileOperation`.
//!
//! This is the shell's own engine, which is not a detail. Going through it means this
//! program gets, for free and *correctly*:
//!
//! - the **Recycle Bin** — a delete that cannot be undone is a different feature from
//!   the one users expect Delete to be. Ctrl+Z is in [`history`], and it is worth reading
//!   why the shell can perform every reversal without being able to *remember* any of them;
//! - the **progress dialog**, with its estimate, its pause and its cancel;
//! - the **conflict prompts** — "there is already a file with this name", with the
//!   comparison of both and the keep-both option;
//! - the **elevation prompt**, when a destination needs administrator rights;
//! - **long path, junction, reparse point and cloud placeholder** handling, none of
//!   which a hand-rolled recursive copy gets right;
//! - correct behaviour on a collision that differs only in case, on a file in use, and
//!   on a path over 260 characters.
//!
//! Re-implementing that over `std::fs` would produce something that looks the same in
//! the easy cases and quietly loses data in the hard ones.
//!
//! What the shell puts on screen, and when, is worth knowing rather than assuming —
//! `what_the_shell_puts_on_screen` lists it:
//!
//! | | |
//! | --- | --- |
//! | a copy onto an existing name | `Replace or Skip Files`, in an `OperationStatusWindow` |
//! | Shift+Delete | `Delete File`, a plain `#32770` confirmation |
//! | Delete to the Recycle Bin | nothing, which is Windows’ own default |
//! | a copy into the folder it is already in | nothing, and a `one - Copy.txt` appears |
//!
//! The last row is the one that needed doing. Copying something into the folder it already
//! lives in cannot mean “replace it with itself”, so Explorer does not ask: it makes
//! `one - Copy.txt` and gets on with it. Without `FOF_RENAMEONCOLLISION` the same paste stopped
//! on the Replace-or-Skip dialog, which is not what Ctrl+C Ctrl+V does anywhere else in
//! Windows. The name is the shell’s own and localised — measured as `one - Copie.txt` here.
//!
//! # Why each operation gets its own thread
//!
//! `PerformOperations` is synchronous: it shows the progress dialog and does not
//! return until the work is done or cancelled. Called on the UI thread it would freeze
//! this window for the duration — so each operation runs on a thread of its own, which
//! initialises COM as its own apartment and pumps the dialog there. The window stays
//! live, the dialog is parented to it, and the affected folders are re-read when the
//! thread reports back.

use std::path::{Path, PathBuf};

pub mod history;

#[cfg(windows)]
#[path = "../../windows/ops.rs"]
mod win;
/// Taking something back out of the Recycle Bin, which is the one operation here that
/// `IFileOperation` has no call for. Declared beside [`win`] rather than inside it because it is
/// the other half of the same story and neither reads without the other.
#[cfg(windows)]
#[path = "../../windows/bin.rs"]
pub(crate) mod bin;
#[cfg(windows)]
pub(crate) use win::run;
// Reached by the tests next door and in [`super::clipboard`], which check what the shell
// actually answers rather than a mock of it.
#[cfg(all(test, windows))]
pub(crate) use win::{all_already_in, item, landed};
use std::sync::mpsc::{channel, Receiver, Sender};

use super::Owner;

/// What an operation did, once it has finished.
#[derive(Clone, Debug)]
pub struct Done {
    /// The job as it was asked for, so [`history`] can offer it again as Redo.
    ///
    /// `None` only when the operation's thread could not be started, which is the one path here
    /// that reports back without anything having been asked of the shell.
    pub job: Option<Job>,
    /// The folders whose contents may have changed, so they can be re-read.
    pub touched: Vec<PathBuf>,
    /// Empty when it worked — including when the user cancelled, which is an answer
    /// rather than a failure.
    ///
    /// So this is **not** the test for whether the operation happened. See [`Self::worked`].
    pub error: Option<String>,
    /// Whether the user stopped it — the shell's own `GetAnyOperationsAborted`.
    ///
    /// The third value the rest of the program was missing. See [`Self::worked`].
    pub aborted: bool,
    /// What still has to happen now that it has.
    pub after: After,
    /// What the shell says it actually did, item by item. See [`Outcome`].
    pub outcome: Outcome,
}

impl Done {
    /// Whether the operation did the whole of what it was asked for.
    ///
    /// **Not `error.is_none()`, and that distinction was a bug in two places.** A cancel is a
    /// decision rather than a fault, so [`win::friendly`] deliberately turns both cancel codes into
    /// no message at all — the shell's own dialog has already said everything there is to say, and
    /// the status line has nothing to add. Which left "did all of it" and "the user stopped it"
    /// indistinguishable to everything downstream, and two callers were computing
    /// `error.is_none()` for themselves and calling it success:
    ///
    /// - [`history::History::settle`], whose own documentation promises that "a cancel counts as
    ///   not working — which is the case this exists for". It did the opposite. Cut 500 photos,
    ///   Ctrl+Z, answer the conflict dialog with Cancel, and the entry moved to the *redo* stack:
    ///   Ctrl+Z then said "Nothing to undo" and the move could never be reversed again.
    /// - [`After::FinishCut`], which told the clipboard the cut had been honoured and emptied it,
    ///   for a paste that moved nothing. The comment where the call was moved to the end of the
    ///   frame says it was written for exactly this case; the end of the frame cannot see a cancel
    ///   either.
    ///
    /// So the shell is asked instead of guessed at — `GetAnyOperationsAborted`, on [`Self::aborted`].
    pub fn worked(&self) -> bool {
        self.error.is_none() && !self.aborted
    }
}

/// What the shell reported, once a job is over. See [`Done::worked`] for why `aborted` is not
/// merely the absence of an error.
pub struct Ran {
    pub error: Option<String>,
    pub outcome: Outcome,
    pub aborted: bool,
}

/// What an operation actually did, item by item, as the shell reported it.
///
/// **Asked for rather than assumed, and that is the whole reason undo can be trusted.** What a
/// job says it wants and what the shell does are not the same thing, and every difference
/// between them is a difference an undo built on the request would get wrong:
///
/// | asked for | what happens | what a guess would undo |
/// | --- | --- | --- |
/// | copy `one.txt` into a folder that has one | `one - Copy.txt` appears | `one.txt`, the file that was already there |
/// | copy ten items, cancelled after four | four exist | ten, six of which are not ours |
/// | copy onto an existing name, answered Skip | nothing appears | a file this program never made |
/// | move onto an existing name, answered Keep both | it lands as `two (2).txt` | a path with nothing at it |
///
/// So nothing here is inferred from [`Job`]. `IFileOperation` reports each item's fate to an
/// `IFileOperationProgressSink` — `PostCopyItem`, `PostMoveItem`, `PostRenameItem`,
/// `PostNewItem`, `PostDeleteItem`, each with the item it made and the `HRESULT` for that one
/// item — and this is those callbacks collected. An item the shell skipped, failed or was
/// never reached is simply absent, which is exactly the property undo needs.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// Items that exist now and did not before: a copy's destinations, a new folder.
    /// Undone by recycling them.
    pub created: Vec<PathBuf>,
    /// Items that changed place or name: where each was, and where it is now. A move and a
    /// rename are the same shape here, because putting either back is the same operation.
    /// Undone by [`Job::PutBack`].
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// Items that went to the Recycle Bin, and what to say to get each one back.
    /// Undone by [`Job::Restore`].
    pub recycled: Vec<Recycled>,
}

impl Outcome {
    /// The name the shell gave the item it made, when it made one.
    ///
    /// Asked for rather than guessed. `NewItem` is given the name to *start* from and the shell
    /// picks the first free one, so what actually appears may be `New folder (3)` — and nothing
    /// on this side can know which without being told.
    pub fn created_name(&self) -> Option<String> {
        self.created
            .first()?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }
}

/// One item that went to the Recycle Bin, and both ways of finding it there again.
///
/// The bin is a namespace rather than a folder — what is in it cannot be restored by moving a
/// path, only by invoking its own `undelete` verb, and to invoke that the *item* has to be found.
/// See [`bin`], which is where both of these are used and where the measurements behind them are.
#[derive(Clone, Debug)]
pub struct Recycled {
    /// Where it was before it was deleted, which is where restoring puts it back.
    ///
    /// Kept whether or not [`Self::bin`] is: it is what the folder re-read needs, what the search
    /// falls back to, and what names the file to the user.
    pub from: PathBuf,
    /// The file the bin actually holds it as — `…\$Recycle.Bin\<SID>\$RKOFDIE.txt`.
    ///
    /// The exact answer, and it identifies one item and no other: a file deleted, remade and
    /// deleted again leaves two items in the bin claiming the same [`Self::from`], and this
    /// distinguishes them where nothing else can. `None` when the shell did not say —
    /// `psiNewlyCreated` is documented as optional on `PostDeleteItem`.
    pub bin: Option<PathBuf>,
}

/// What is left to do once an operation finishes, beyond re-reading the folders.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum After {
    #[default]
    Nothing,
    /// This was a paste of a cut: if it worked, the clipboard has to be told so, and then
    /// emptied. Carried through the operation rather than done when it starts, because a
    /// paste the user cancels at the conflict dialog has to leave the cut where it was. The
    /// number is the clipboard's sequence when the paste began, so a clipboard that has since
    /// changed is left alone.
    FinishCut(u32),
    /// A folder was created: select it in this pane and open its name for editing, once the
    /// re-read brings it in. Which name that is comes back on [`Outcome::created_name`].
    NameIt(crate::pane::PaneId),
    /// This *was* an undo or a redo. [`history::History`] is holding the entry it came off, and
    /// needs telling whether the shell managed it before the entry can move to the other stack.
    ///
    /// Carried on the job rather than remembered beside the history for the reason
    /// [`Self::FinishCut`] is: the answer arrives on a channel a frame or a minute later, and the
    /// only thing certain to still be true then is what travelled with it.
    Settle,
}

/// The work to do.
#[derive(Clone, Debug)]
pub enum Job {
    Copy { items: Vec<PathBuf>, into: PathBuf },
    Move { items: Vec<PathBuf>, into: PathBuf },
    /// `to_bin` sends them to the Recycle Bin, which is what Delete does; `false` is
    /// Shift+Delete.
    Delete { items: Vec<PathBuf>, to_bin: bool },
    Rename { item: PathBuf, name: String },
    NewFolder { parent: PathBuf, name: String },
    /// Make a shortcut in `into` for each item — the Alt-drag, and the right-drag menu's
    /// *Create shortcuts here*.
    ///
    /// The one job here that creates something without copying anything, which is why it is not a
    /// [`Self::Copy`] with a flag. See [`crate::shell::links::shortcuts_into`] for what it makes
    /// and what the names are measured against.
    Link { items: Vec<PathBuf>, into: PathBuf },

    // ---- The two that only undo asks for -------------------------------------
    //
    /// Put each item back where it was: `(where it is now, where it should go)`.
    ///
    /// The inverse of a move and of a rename both, which is why it is one job and not two. A
    /// pair whose two parents are the same folder is a rename and is performed as one — see
    /// [`crate::windows::ops`] — so undoing an F2 does not go through the move engine.
    ///
    /// Per item rather than one destination for all of them, unlike [`Self::Move`]: a paste can
    /// gather a selection out of several folders, and putting *that* back means as many
    /// destinations as there were sources.
    PutBack { items: Vec<(PathBuf, PathBuf)> },
    /// Take each of these out of the Recycle Bin and put it back where it came from.
    ///
    /// The inverse of a delete, and the one job here that is not `IFileOperation` at all — the
    /// bin is a namespace rather than a folder, and `undelete` is the only thing that empties an
    /// item out of it correctly. See [`bin`].
    Restore { items: Vec<Recycled> },
}

impl Job {
    /// A present-tense description, for the status line while it runs.
    pub fn describe(&self) -> String {
        match self {
            Self::Copy { items, .. } => format!("Copying {}…", plural(items.len())),
            Self::Move { items, .. } => format!("Moving {}…", plural(items.len())),
            Self::Delete {
                items,
                to_bin: true,
            } => format!("Recycling {}…", plural(items.len())),
            Self::Delete { items, .. } => format!("Deleting {}…", plural(items.len())),
            Self::Rename { .. } => "Renaming…".to_owned(),
            Self::NewFolder { .. } => "Creating a folder…".to_owned(),
            // Counted in shortcuts rather than in items, because that is what is being made and
            // "a shortcut for 1 item" says the number twice.
            Self::Link { items, .. } if items.len() == 1 => "Making a shortcut…".to_owned(),
            Self::Link { items, .. } => format!("Making {} shortcuts…", items.len()),
            // Both undo jobs say "Putting back", which is what they are from where the user is
            // standing: they pressed Ctrl+Z, and whether the shell is being asked to move a file
            // or to empty one out of the Recycle Bin is not their problem.
            Self::PutBack { items } => format!("Putting {} back…", plural(items.len())),
            Self::Restore { items } => format!("Putting {} back…", plural(items.len())),
        }
    }

    /// The folders this will change, so they can be re-read afterwards.
    pub fn touches(&self) -> Vec<PathBuf> {
        let parents = |items: &[PathBuf]| -> Vec<PathBuf> {
            items
                .iter()
                .filter_map(|p| p.parent().map(Path::to_path_buf))
                .collect()
        };
        let mut touched = match self {
            Self::Copy { items, .. } | Self::Move { items, .. } | Self::Delete { items, .. } => {
                parents(items)
            }
            Self::Rename { item, .. } => item.parent().map(Path::to_path_buf).into_iter().collect(),
            // The destination and nothing else, and it is added below. What a shortcut points at is
            // not touched — that is the whole difference between this and a copy.
            Self::NewFolder { .. } | Self::Link { .. } => Vec::new(),
            // Both ends of every pair: an item leaves one folder and arrives in another, and a
            // pane showing either has to be told.
            Self::PutBack { items } => items
                .iter()
                .flat_map(|(from, to)| [from.parent(), to.parent()])
                .flatten()
                .map(Path::to_path_buf)
                .collect(),
            // Where each one is going. The bin it came out of is [`Self::changes_the_bin`]'s.
            Self::Restore { items } => items
                .iter()
                .filter_map(|item| item.from.parent().map(Path::to_path_buf))
                .collect(),
        };
        // The folder written into, for the four jobs that have one — from [`Self::destination`], so
        // that answer is written once rather than in an arm here and another in [`Self::every_path`].
        touched.extend(self.destination().map(Path::to_path_buf));
        touched.sort();
        touched.dedup();
        touched
    }

    /// Whether this job puts something into the Recycle Bin or takes something out, so a pane
    /// showing the bin has to be re-read once it is done.
    ///
    /// Beside [`Self::touches`] rather than in it, because the bin is not a folder: `touches` is also
    /// every path a test's job is held to the sandbox by, and [`crate::fs::recycle::LOCATION`] is a
    /// shell name that no sandbox contains. The watcher on the bin's own folders would usually
    /// notice anyway — this is for the volume whose folder the delete has only just created.
    pub fn changes_the_bin(&self) -> bool {
        matches!(
            self,
            Self::Delete { to_bin: true, .. } | Self::Restore { .. }
        )
    }

    /// Every path this job acts **on**: the items, and both ends of an undo pair.
    ///
    /// The counterpart to [`Self::destination`], and the split matters — see
    /// [`Self::archive_refusal`], which asks a different question of each.
    pub(crate) fn sources(&self) -> Vec<&Path> {
        match self {
            Self::Copy { items, .. }
            | Self::Move { items, .. }
            | Self::Delete { items, .. }
            | Self::Link { items, .. } => items.iter().map(PathBuf::as_path).collect(),
            Self::Rename { item, .. } => vec![item.as_path()],
            // Nothing yet exists to act on; the name is not a path until the shell has made it.
            Self::NewFolder { .. } => Vec::new(),
            Self::PutBack { items } => items
                .iter()
                .flat_map(|(from, to)| [from.as_path(), to.as_path()])
                .collect(),
            Self::Restore { items } => items.iter().map(|item| item.from.as_path()).collect(),
        }
    }

    /// The folder this job writes **into**, for the jobs that have one.
    ///
    /// The counterpart to [`Self::sources`]. Distinct from [`Self::touches`], which is every folder
    /// to re-read afterwards and takes in the sources' own parents as well.
    pub(crate) fn destination(&self) -> Option<&Path> {
        match self {
            Self::Copy { into, .. } | Self::Move { into, .. } | Self::Link { into, .. } => {
                Some(into)
            }
            Self::NewFolder { parent, .. } => Some(parent),
            // A rename writes into the folder its item is already in, which the item test covers.
            // A delete has no destination, and the two undo jobs name real paths at both ends:
            // nothing was ever moved *into* an archive for them to put back.
            Self::Delete { .. }
            | Self::Rename { .. }
            | Self::PutBack { .. }
            | Self::Restore { .. } => None,
        }
    }

    /// The paths this job **empties**: what is no longer there once it has worked.
    ///
    /// A subset of [`Self::sources`], and it is what the window drops [`crate::archive`]'s cached
    /// index for — see [`crate::app::Explorer::collect_operations`].
    ///
    /// # Asked after the job, never before
    ///
    /// An index dropped while the archive is still on the disk takes
    /// [`crate::archive::is_virtual_item`] out from under a pane that is still showing the archive's
    /// rows, and a delete of one of those rows would then reach `IFileOperation` with a path that
    /// is not a file. Afterwards there is nothing at the path for it to resolve to, which is what
    /// makes the same call safe.
    pub(crate) fn emptied(&self) -> Vec<&Path> {
        match self {
            Self::Delete { items, .. } | Self::Move { items, .. } => {
                items.iter().map(PathBuf::as_path).collect()
            }
            Self::Rename { item, .. } => vec![item.as_path()],
            Self::PutBack { items } => items.iter().map(|(from, _)| from.as_path()).collect(),
            // A copy and a shortcut add something and take nothing, a new folder takes nothing, and
            // a restore's source is an item in the Recycle Bin, which is never an archive's index.
            Self::Copy { .. }
            | Self::Link { .. }
            | Self::NewFolder { .. }
            | Self::Restore { .. } => Vec::new(),
        }
    }

    /// The archive path that stops this job reaching the shell, if there is one.
    ///
    /// **Two questions, and the difference between them is `pkg.zip` itself.** An *item* must not be
    /// inside an archive, because there is no such file; a *destination* must not be one, because
    /// there is nothing to write into. Each is asked only of the paths that play that role, and
    /// asking either of the other role reintroduces a bug — the item question over a destination
    /// lets a paste into `pkg.zip` reach `IFileOperation`, and the location question over an item
    /// is what once cost the archive its own delete. See [`crate::archive::is_virtual_item`].
    pub(crate) fn archive_refusal(&self) -> Option<Refused> {
        if let Some(item) = self
            .sources()
            .into_iter()
            .find(|path| crate::archive::is_virtual_item(path))
        {
            return Some(Refused::Item(item.to_path_buf()));
        }
        self.destination()
            .filter(|folder| crate::archive::is_virtual_location(folder))
            .map(|into| Refused::Destination(into.to_path_buf()))
    }

    /// The Recycle Bin path that stops this job reaching the shell, if there is one.
    ///
    /// The same two questions as [`Self::archive_refusal`], asked of the bin. **An item the bin
    /// holds** — a `$R…` file — is a real file, so `IFileOperation` would take it and do exactly
    /// what it was told: move it out and leave its `$I…` behind, rename it out of its pair, or
    /// recycle it a second time. Every one of those corrupts the bin. The bin's own verbs are the
    /// only ones that act on its items correctly, and those are not jobs — see
    /// [`crate::fs::recycle`]. **The bin as a destination** is not a folder: what dropping onto it
    /// means is a delete, and [`crate::app::App::land`] asks for one before anything gets here.
    ///
    /// [`Self::Restore`] is the one job whose paths may be held items, since taking one out of the
    /// bin is the point of it — and it goes through `undelete` rather than `IFileOperation`.
    pub(crate) fn bin_refusal(&self) -> Option<Refused> {
        if matches!(self, Self::Restore { .. }) {
            return None;
        }
        if self
            .sources()
            .into_iter()
            .any(crate::fs::recycle::is_held)
        {
            return Some(Refused::Held);
        }
        self.destination()
            .filter(|folder| crate::fs::is_synthetic(folder))
            .map(|into| Refused::NotAFolder(into.to_path_buf()))
    }

    /// Every path this job could write to, for the sandbox guard in [`Operations::start_then`].
    ///
    /// The folders from [`Self::touches`] are not enough: a delete names the items, and it is
    /// the items that go. Kept here beside them so a job added later has one obvious place to
    /// say what it would touch, rather than a `match` in the middle of a guard nobody reads
    /// until it is too late — which is why this is the union of the accessors above and not a
    /// fourth `match` over the same variants.
    #[cfg(test)]
    pub(crate) fn every_path(&self) -> Vec<PathBuf> {
        let mut paths = self.touches();
        paths.extend(self.sources().into_iter().map(Path::to_path_buf));
        paths
    }
}

/// Why a job cannot be handed to the shell: the path that stops it, and which role it played.
///
/// From [`Job::archive_refusal`], which is where the two questions are set out.
pub(crate) enum Refused {
    /// An item that is inside an archive, so there is no such file to act on.
    Item(PathBuf),
    /// A destination that this program only reads, so there is nothing to write into.
    Destination(PathBuf),
    /// An item the Recycle Bin holds. See [`Job::bin_refusal`].
    Held,
    /// This PC or the Recycle Bin, as somewhere to write into.
    NotAFolder(PathBuf),
}

impl Refused {
    /// The sentence for the status line, beside the case it is about.
    fn why(&self) -> String {
        match self {
            Self::Item(path) => {
                let what = crate::fs::display_name(path);
                format!(
                    "{what} is inside an archive, which this program only reads. \
                     Copy it out first."
                )
            }
            // Not "is an archive": the destination may be a folder *inside* one, and a sentence
            // that names the kind would be wrong for one of the two.
            Self::Destination(path) => {
                let what = crate::fs::display_name(path);
                format!("Nothing can be written into {what}: this program only reads archives.")
            }
            Self::Held => crate::fs::recycle::RESTORE_FIRST.to_owned(),
            Self::NotAFolder(path) => {
                let what = crate::fs::display_name(path);
                format!("{what} is not a folder to put anything in")
            }
        }
    }
}

/// `1 item` or `n items`, which four of the descriptions above want.
fn plural(count: usize) -> String {
    if count == 1 {
        "1 item".to_owned()
    } else {
        format!("{count} items")
    }
}

/// Whether a test is allowed to hand a job to the real shell. **Off by default.**
///
/// This is not caution, it is a bug that was shipped and found — the same one, in the same shape,
/// as the guard on [`crate::config::Config::save`]. `the_clipboard_events_map_to_the_right_actions`
/// selects the first row of the harness's folder, which is `CARGO_MANIFEST_DIR`, and feeds the
/// window a Shift+Delete to check that the event maps to `Action::Delete` rather than to a cut.
/// The frame it does that in *applies* the action, so a green test quietly asked `IFileOperation`
/// to **permanently delete `.cargo` from this repository** and put the shell's confirmation dialog
/// up to ask about it. It ran on its own thread, so the test finished and passed while the prompt
/// was still on screen; whether the folder survived came down to a human not clicking Yes.
///
/// A test process has no business asking the shell to move or delete a user's files. The one test
/// that must — `copy_cut_paste_and_delete_end_to_end`, which works inside `target/sandbox` — turns
/// this on for its duration through [`for_real`].
#[cfg(test)]
pub(crate) static FOR_REAL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Let the real shell do the work for as long as the returned guard is alive.
///
/// A guard rather than a pair of calls, so a test that fails an assertion half way through still
/// leaves the flag off for whatever runs next.
#[cfg(test)]
pub(crate) fn for_real() -> impl Drop {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            FOR_REAL.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    FOR_REAL.store(true, std::sync::atomic::Ordering::SeqCst);
    Guard
}

/// Runs jobs, one thread each, and reports back.
pub struct Operations {
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// What is running, oldest first, for the status line.
    running: Vec<String>,
}

impl Operations {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            running: Vec::new(),
        }
    }

    /// Start a job. Returns immediately; the window stays live while it runs.
    pub fn start(&mut self, job: Job, owner: Owner, ctx: &egui::Context) {
        self.start_then(job, After::Nothing, owner, ctx);
    }

    /// The same, with something to do once it has finished.
    pub fn start_then(&mut self, job: Job, after: After, owner: Owner, ctx: &egui::Context) {
        self.running.push(job.describe());
        let touched = job.touches();
        let tx = self.tx.clone();
        let ctx = ctx.clone();

        // **Nothing inside an archive is the shell's to touch.** `IFileOperation` is being handed
        // `D:\dl\pkg.zip\src\main.rs`, which is not a file — so a delete would either fail with a
        // number or, far worse, resolve to the *archive* and take all of it. Nothing in this program
        // writes into an archive at all: see [`crate::archive`], where that is scope and format
        // both, a `.tar.gz` having no way to change one member without being rebuilt whole.
        //
        // Which of its paths, and which sentence, is [`Job::archive_refusal`] — asked here rather
        // than at the arms in [`crate::app::App::perform`] for the same reason the two guards below
        // are here: this is the one funnel every copy, move, delete, rename, shortcut and new folder
        // passes through, and a rule at the call sites is a rule the next call site forgets. Refused
        // through the same channel a failure comes back on, so the words reach the status line by
        // the route that was already built for them — and with `job: None`, so a refusal cannot be
        // offered to `Ctrl+Z` as something to undo.
        if let Some(refused) = job.archive_refusal().or_else(|| job.bin_refusal()) {
            let _ = tx.send(Done {
                job: None,
                touched,
                error: Some(refused.why()),
                aborted: false,
                after,
                outcome: Outcome::default(),
            });
            return;
        }

        // **Not from a test, unless a test asked for it.** See [`FOR_REAL`]. This is the choke
        // point every copy, move, delete, rename and new folder goes through, which is why the
        // guard is here and not at the five call sites.
        #[cfg(test)]
        if !FOR_REAL.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = tx.send(Done {
                job: Some(job),
                touched,
                error: None,
                aborted: false,
                after,
                outcome: Outcome::default(),
            });
            return;
        }

        // Past the guard above, so this job is about to become real work by the shell's hand.
        // `FOR_REAL` says a test *meant* to do that; it says nothing about *where*, and where is
        // the part that cost this repository its working tree. Every path the job names, not just
        // the folders it will re-read: a delete names the items, and it is the items that go.
        #[cfg(test)]
        crate::sandbox::guard(
            &format!("the shell job {:?}", job.describe()),
            &job.every_path(),
        );

        // Taken here and dropped on the job's own thread, so that a directory a drop claimed
        // into goes when the job that consumes it is done — and goes there rather than on the
        // UI thread, because removing a staged archive is real work. Built after the guard
        // above so a test can never make one.
        let scratch = claimed(&job).map(Scratch::at);
        // Kept back from the closure below, because the failure path needs it. See there.
        let unstarted = after.clone();
        let spawned = std::thread::Builder::new()
            .name("file-operation".to_owned())
            .spawn(move || {
                let mut scratch = scratch;
                let ran = run(&job, owner);
                // **The staging directory goes only if the job that consumed it worked.** It holds
                // the *only* copy of whatever the drop claimed — the claim is a rename, so the
                // source no longer has it — and removing it after a copy that failed or was
                // cancelled destroyed the lot. A source's own extraction is no loss; a file of the
                // user's that legitimately lives under `%TEMP%` is, and nothing here can tell them
                // apart. So a failure leaves it where it is: the next launch's
                // [`crate::shell::dnd::sweep`] is then the one that clears it, which is late but
                // is not the middle of the user asking for a copy.
                if let Some(scratch) = &mut scratch {
                    scratch.keep = ran.error.is_some() || ran.aborted;
                }
                let _ = tx.send(Done {
                    job: Some(job),
                    touched,
                    error: ran.error,
                    aborted: ran.aborted,
                    after,
                    outcome: ran.outcome,
                });
                ctx.request_repaint();
            });
        if spawned.is_err() {
            self.running.pop();
            let _ = self.tx.send(Done {
                // No job: it was moved into a closure that will never run, and there is nothing
                // for the history to offer back anyway. See [`Done::job`].
                job: None,
                touched: Vec::new(),
                error: Some("Could not start the operation".to_owned()),
                aborted: false,
                // **The job's own `after`, and not `Nothing`.** A thread that never started still
                // has to be reported as the thing it was: dropping `After::Settle` here left
                // [`history::History`] with a reversal permanently in flight, so `undo` and `redo`
                // both answered `None` and every Ctrl+Z for the rest of the session was told
                // "Still undoing the last one…". The entry went with it.
                after: unstarted,
                outcome: Outcome::default(),
            });
        }
    }

    /// Anything that has finished since the last frame.
    pub fn drain(&mut self) -> Vec<Done> {
        let finished: Vec<Done> = self.rx.try_iter().collect();
        for _ in 0..finished.len() {
            if !self.running.is_empty() {
                self.running.remove(0);
            }
        }
        finished
    }

    /// What is in progress, for the status line.
    pub fn in_progress(&self) -> Option<&str> {
        self.running.first().map(String::as_str)
    }
}

impl Default for Operations {
    fn default() -> Self {
        Self::new()
    }
}

/// The staging directory a drop claimed its items into, if that is where they live.
///
/// Recognised from the path rather than carried down from the drop, so that every route which
/// ends in a job gets the cleanup without knowing about it — including the right-button menu,
/// which can sit open for as long as the user likes before it becomes one. What makes that safe
/// to act on is [`crate::shell::dnd::is_staging`], which asks both for the name and for the
/// directory to be sitting in `%TEMP%`.
fn claimed(job: &Job) -> Option<PathBuf> {
    let items = match job {
        Job::Copy { items, .. } | Job::Move { items, .. } => items,
        _ => return None,
    };
    // Every item, and not just the first. A drop can mix a source's materialisation with the
    // user's own files — see [`crate::shell::dnd::claim`], which stages the one and leaves the
    // other where it is — so the staged items are not necessarily at the front of the list.
    // Asking only the first left the directory sitting in `%TEMP%` until the next startup sweep.
    items.iter().find_map(|item| {
        let dir = item.parent()?;
        if crate::shell::dnd::is_staging(dir) {
            return Some(dir.to_path_buf());
        }
        // One level up as well, for an item that had to be nested to keep its name.
        let up = dir.parent()?;
        crate::shell::dnd::is_staging(up).then(|| up.to_path_buf())
    })
}

/// A directory that goes when this does — unless the job it was claimed for did not work.
///
/// A guard rather than a statement so that a panic in the job, or a thread that never starts,
/// cannot leak an extracted archive into `%TEMP%`. Those are the cases [`Self::keep`] is *not*
/// for: nothing was copied anywhere, so there is nothing to be recovered from the staging
/// directory and leaving it would be litter. It is a failed or cancelled *copy* that has to keep
/// it, because then the staged items are the only ones left.
struct Scratch {
    dir: PathBuf,
    /// Set by the job's thread once the shell has answered. See [`Operations::start_then`].
    keep: bool,
}

impl Scratch {
    /// Removed by default: a claim that is never settled one way or the other is a panic or an
    /// abandoned thread, and neither leaves anything worth keeping.
    fn at(dir: PathBuf) -> Self {
        Self { dir, keep: false }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// Everywhere else
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn run(_job: &Job, _owner: Owner) -> Ran {
    Ran {
        error: Some("File operations are implemented against the Windows shell only".to_owned()),
        outcome: Outcome::default(),
        aborted: false,
    }
}

#[cfg(test)]
mod tests;
