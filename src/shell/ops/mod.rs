//! Copy, move, delete, rename and new-folder — through `IFileOperation`.
//!
//! This is the shell's own engine, which is not a detail. Going through it means this
//! program gets, for free and *correctly*:
//!
//! - the **Recycle Bin**, and with it undo — a delete that cannot be undone is a
//!   different feature from the one users expect Delete to be;
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

#[cfg(windows)]
#[path = "../../windows/ops.rs"]
mod win;
#[cfg(windows)]
pub(crate) use win::run;
// Reached by the tests next door and in [`super::clipboard`], which check what the shell
// actually answers rather than a mock of it.
#[cfg(all(test, windows))]
pub(crate) use win::{all_already_in, item};
use std::sync::mpsc::{channel, Receiver, Sender};

use super::Owner;

/// What an operation did, once it has finished.
#[derive(Clone, Debug)]
pub struct Done {
    /// The folders whose contents may have changed, so they can be re-read.
    pub touched: Vec<PathBuf>,
    /// Empty when it worked — including when the user cancelled, which is an answer
    /// rather than a failure.
    pub error: Option<String>,
    /// What still has to happen now that it has.
    pub after: After,
    /// The name the shell gave a newly created folder.
    ///
    /// Asked for rather than guessed. `NewItem` is given the name to *start* from and the shell
    /// picks the first free one, so what actually appears may be `New folder (3)` — and
    /// nothing on this side can know which without being told. `IFileOperationProgressSink`
    /// is how it is told.
    pub created: Option<String>,
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
    /// re-read brings it in. Which name that is comes back on [`Done::created`].
    NameIt(crate::pane::PaneId),
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
}

impl Job {
    /// A present-tense description, for the status line while it runs.
    pub fn describe(&self) -> String {
        let count = |items: &Vec<PathBuf>| {
            if items.len() == 1 {
                "1 item".to_owned()
            } else {
                format!("{} items", items.len())
            }
        };
        match self {
            Self::Copy { items, .. } => format!("Copying {}…", count(items)),
            Self::Move { items, .. } => format!("Moving {}…", count(items)),
            Self::Delete {
                items,
                to_bin: true,
            } => format!("Recycling {}…", count(items)),
            Self::Delete { items, .. } => format!("Deleting {}…", count(items)),
            Self::Rename { .. } => "Renaming…".to_owned(),
            Self::NewFolder { .. } => "Creating a folder…".to_owned(),
        }
    }

    /// The folders this will change, so they can be re-read afterwards.
    pub fn touches(&self) -> Vec<PathBuf> {
        let parents = |items: &Vec<PathBuf>| -> Vec<PathBuf> {
            items
                .iter()
                .filter_map(|p| p.parent().map(Path::to_path_buf))
                .collect()
        };
        let mut touched = match self {
            Self::Copy { items, into } | Self::Move { items, into } => {
                let mut all = parents(items);
                all.push(into.clone());
                all
            }
            Self::Delete { items, .. } => parents(items),
            Self::Rename { item, .. } => item.parent().map(Path::to_path_buf).into_iter().collect(),
            Self::NewFolder { parent, .. } => vec![parent.clone()],
        };
        touched.sort();
        touched.dedup();
        touched
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

        // **Not from a test, unless a test asked for it.** See [`FOR_REAL`]. This is the choke
        // point every copy, move, delete, rename and new folder goes through, which is why the
        // guard is here and not at the five call sites.
        #[cfg(test)]
        if !FOR_REAL.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = tx.send(Done {
                touched,
                error: None,
                after,
                created: None,
            });
            return;
        }

        // Past the guard above, so this job is about to become real work by the shell's hand.
        // `FOR_REAL` says a test *meant* to do that; it says nothing about *where*, and where is
        // the part that cost this repository its working tree. Every path the job names, not just
        // the folders it will re-read: a delete names the items, and it is the items that go.
        #[cfg(test)]
        {
            let mut paths = job.touches();
            match &job {
                Job::Copy { items, into } | Job::Move { items, into } => {
                    paths.extend(items.iter().cloned());
                    paths.push(into.clone());
                }
                Job::Delete { items, .. } => paths.extend(items.iter().cloned()),
                Job::Rename { item, .. } => paths.push(item.clone()),
                Job::NewFolder { parent, .. } => paths.push(parent.clone()),
            }
            crate::sandbox::guard(&format!("the shell job {:?}", job.describe()), &paths);
        }

        // Taken here and dropped on the job's own thread, so that a directory a drop claimed
        // into goes when the job that consumes it is done — and goes there rather than on the
        // UI thread, because removing a staged archive is real work. Built after the guard
        // above so a test can never make one.
        let scratch = claimed(&job).map(Scratch);
        let spawned = std::thread::Builder::new()
            .name("file-operation".to_owned())
            .spawn(move || {
                let _scratch = scratch;
                let (error, created) = run(&job, owner);
                let _ = tx.send(Done {
                    touched,
                    error,
                    after,
                    created,
                });
                ctx.request_repaint();
            });
        if spawned.is_err() {
            self.running.pop();
            let _ = self.tx.send(Done {
                touched: Vec::new(),
                error: Some("Could not start the operation".to_owned()),
                after: After::Nothing,
                created: None,
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
    let dir = items.first()?.parent()?;
    if crate::shell::dnd::is_staging(dir) {
        return Some(dir.to_path_buf());
    }
    // One level up as well, for an item that had to be nested to keep its name.
    let up = dir.parent()?;
    crate::shell::dnd::is_staging(up).then(|| up.to_path_buf())
}

/// A directory that goes when this does.
///
/// A guard rather than a statement so that a panic in the job, or a thread that never starts,
/// cannot leak an extracted archive into `%TEMP%`.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Everywhere else
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn run(_job: &Job, _owner: Owner) -> (Option<String>, Option<String>) {
    (
        Some("File operations are implemented against the Windows shell only".to_owned()),
        None,
    )
}

#[cfg(test)]
mod tests;
