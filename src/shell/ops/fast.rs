//! Copy, move and permanent delete by this program's own hand, for when **Fast copy** is ticked in
//! the application menu. Off by default; [`super`] is what runs when it is off, and what this hands
//! back to whenever a job is not one it can do as well as the shell.
//!
//! # Where Explorer's time goes, and what is taken back
//!
//! `IFileOperation` moves bytes through the same kernel copy engine as `CopyFile2` does. What makes
//! it slow is everything around the bytes:
//!
//! | the shell | here |
//! | --- | --- |
//! | walks the whole selection first, to count and estimate, before one byte moves | the walk *feeds* the copy: the first file is copying while the rest are still being found |
//! | one file at a time — a queue depth of one, which an SSD or an SMB link spends idle | several files in flight, on a small pool of threads |
//! | an `IShellItem`, copy hooks, undo bookkeeping and a progress update per item | `CopyFile2` on two paths |
//!
//! The thread count is the one thing decided per job — see [`crate::windows::fast`]: more for a
//! share, where every file is a handful of round trips, and **one** for a disk that seeks, where a
//! second file in flight only sends the head back and forth between them.
//!
//! The bytes themselves stay the kernel's. `CopyFile2` already copies alternate streams, attributes
//! and sparse ranges, picks cached or unbuffered I/O, and block-clones on ReFS. A loop of reads and
//! writes here would be a slower and less correct copy of it.
//!
//! # What still goes to the shell
//!
//! [`win::run`] answers `None` and the job goes to `IFileOperation` exactly as it would with the
//! setting off, for anything whose correct handling is the shell's and not a path's:
//!
//! - a copy or move into the folder an item is already in — the shell's `one - Copy.txt` is
//!   localised, and nothing here can produce the same name;
//! - a folder into itself or below itself — under its own name or another, a `subst` drive, a
//!   mapped drive, a junction on the way — which the shell refuses in words of its own;
//! - an item or a destination that is a reparse point at the top: a cloud placeholder, a junction, a
//!   link. *Inside* a tree a link is copied as the link and never followed, which is the rule
//!   everywhere else in this program;
//! - a destination only an administrator may write into, or items only an administrator may take
//!   out of where they are: the shell has the elevation prompt, and this engine would fail file by
//!   file;
//! - anything this program does not see as a plain drive or UNC path.
//!
//! A test cannot get here without [`super::FOR_REAL`], because [`super::Operations::start_then`]
//! decides between the two engines after that guard and the sandbox guard both.
//!
//! # Permanent delete
//!
//! Shift+Delete with the setting on comes here too — never Delete, which is the Recycle Bin's and
//! stays the shell's: putting something in the bin correctly is the shell's own `$I…`/`$R…`
//! bookkeeping, and undo depends on the shell saying what each item became there. A permanent
//! delete has neither. What it does have is the cost this engine exists for: a tree like
//! `node_modules` is tens of thousands of files, each a separate delete, one after another.
//!
//! So the walk lists and the pool deletes, files first and in parallel, and the folders go at the
//! end, deepest first — a folder that still holds something because a file in it failed or was
//! skipped stays, and nothing above it is touched. A link inside the tree is removed as a link and
//! never followed: following one would delete what it points at, which is somewhere else entirely.
//!
//! **It asks first**, as the shell does, and nothing is touched until it has been answered — see
//! [`Transfer::ask_confirm`]. Enter and Escape answer it, the shell's dialog's own keys; see
//! [`crate::app::App::keyboard`].
//!
//! # What the shell gave for free that has to be done here
//!
//! - **Progress, pause and cancel** — [`Transfer`], drawn by [`crate::ui::transfers`].
//! - **The questions**, each through [`Talk`]: a name already taken ([`Transfer::ask`]: Replace,
//!   Skip or Keep both — a folder onto a folder merges without asking, as in Explorer), a file
//!   another program has open or an encryption the destination cannot keep
//!   ([`Transfer::ask_snag`]), a destination running out of room ([`Transfer::ask_room`]), and the
//!   yes or no before a permanent delete ([`Transfer::ask_confirm`]).
//! - **An [`Outcome`] for undo.** Undo goes through the shell whichever engine did the work, so all
//!   this has to do is say what it did. A folder that already existed is never reported, so undo
//!   cannot recycle a folder that was there first; what *is* reported is the first new thing at
//!   each place in the tree.
//! - **Telling Explorer** which folders changed, which `IFileOperation` does as part of the job.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::{Job, Outcome, Ran};

#[cfg(windows)]
#[path = "../../windows/fast.rs"]
pub(crate) mod win;

/// What to do about a file that is already at the name a copy or move wants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    Replace,
    Skip,
    /// Land it beside the one already there, as `name (2).ext`.
    KeepBoth,
}

/// A file that is already where one is going. What the conflict question shows.
#[derive(Clone, Debug)]
pub struct Clash {
    /// The file's name, which is the same at both ends.
    pub name: String,
    /// The folder it is going into.
    pub into: PathBuf,
    pub incoming: Facts,
    pub existing: Facts,
}

/// The two things worth comparing about either file in a [`Clash`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    pub size: u64,
    /// A raw `FILETIME`; `0` for not known.
    pub modified: u64,
}

/// A permanent delete waiting to be confirmed. What the question shows.
#[derive(Clone, Debug)]
pub struct Confirm {
    /// How many items were selected — not what is inside them, which is not counted until the
    /// answer is yes.
    pub count: usize,
    /// The one item's name, when there is only one.
    pub name: Option<String>,
    /// Whether any of them is a folder, which is everything inside it as well.
    pub folders: bool,
}

/// Not enough room where a copy is going, for what the walk has found so far.
///
/// Asked by the walk the moment the count passes what the destination had free, rather than up
/// front as the shell asks it: there is no count up front, which is what makes this engine start at
/// once. The workers stop picking up files while it is on screen, so the files already copying are
/// the most that can run the disk out before the answer.
#[derive(Clone, Debug)]
pub struct Short {
    pub into: PathBuf,
    /// What the job needs written from here, by what it has found so far.
    pub needed: u64,
    pub free: u64,
}

/// The answers to a [`Short`]. Cancel is the panel's own button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Room {
    /// Look again, after something has been cleared off the disk.
    TryAgain,
    /// Go on regardless, and stop asking: replacing files frees room, and a compressed or
    /// deduplicated volume holds more than its free space says.
    Anyway,
}

/// Why a file cannot be copied or moved right now, of the kinds worth asking about rather than
/// writing down as failed — the two where the shell stops and asks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trouble {
    /// Another program has it open in a way that keeps this one out: a sharing or a lock
    /// violation. Closing that program and trying again is the usual way through.
    InUse,
    /// It is encrypted, and the destination cannot hold it encrypted — FAT, exFAT, a share without
    /// EFS. Copying it decrypted is the user's call, which is why it is asked and not done.
    Encrypted,
}

/// A file the job stopped on, and why. See [`Trouble`].
#[derive(Clone, Debug)]
pub struct Snag {
    pub path: PathBuf,
    pub trouble: Trouble,
    /// What Windows said, for the panel.
    pub why: String,
}

/// The answers to a [`Snag`]. Cancel is the panel's own button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mend {
    TryAgain,
    Skip,
    /// Copy it without its encryption. Only offered for [`Trouble::Encrypted`].
    Decrypt,
}

/// A button on the transfer panel, on its way to the job it is about. See
/// [`super::Operations::steer`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Steer {
    Pause(bool),
    Cancel,
    Answer { choice: Choice, every: bool },
    /// The answer to a [`Short`].
    Room(Room),
    /// The answer to a [`Snag`], and whether it is the answer for every snag of its kind from now
    /// on. Never for [`Mend::TryAgain`], which as a standing answer would retry for ever.
    Mend { mend: Mend, every: bool },
    /// Yes or no to a permanent delete. See [`Confirm`].
    Confirm(bool),
    /// Close the panel of a job that has finished.
    Dismiss,
}

/// One item that could not be done, and why.
#[derive(Clone, Debug)]
pub struct Failure {
    pub path: PathBuf,
    pub why: String,
}

/// Which of the three jobs a [`Transfer`] is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Copy,
    Move,
    /// A permanent delete. [`Transfer::into`] is then the folder the first item was in, which is
    /// what the panel names.
    Delete,
}

impl Kind {
    /// What was done to an item, for "could not be …".
    pub fn done(self) -> &'static str {
        match self {
            Self::Copy => "copied",
            Self::Move => "moved",
            Self::Delete => "deleted",
        }
    }
}

/// How far a job has got: whether it is still this engine's at all, then whether it is done.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Deciding whether it can do the job. Nothing is drawn yet.
    Starting,
    Running,
    /// Not a job for this engine. The shell has it, so there is nothing to draw.
    Shell,
    Finished,
}

impl Phase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Running,
            2 => Self::Shell,
            3 => Self::Finished,
            _ => Self::Starting,
        }
    }
}

/// How many failures are kept to show. The count goes on past this; the list does not.
const KEPT: usize = 200;

/// One copy, move or delete in progress, shared between the threads doing it and the panel drawing
/// it.
///
/// Counters are atomics, so a worker never waits on the frame. Everything that needs an answer —
/// a pause, a question — goes through [`Talk`] and its condition variable, which the workers sleep
/// on and the window wakes.
pub struct Transfer {
    pub id: u64,
    pub kind: Kind,
    /// Where a copy or move is going; for a delete, where the first item was. See [`Kind::Delete`].
    pub into: PathBuf,
    phase: AtomicU8,
    /// Whether the walk has finished, so the totals below are the job's and not merely what has
    /// been found so far.
    counted: AtomicBool,
    files_found: AtomicU64,
    bytes_found: AtomicU64,
    files_done: AtomicU64,
    bytes_done: AtomicU64,
    /// What has actually landed on the destination — not what was skipped, refused or renamed,
    /// which [`Self::bytes_done`] counts as progress all the same. What the check for room goes by.
    bytes_written: AtomicU64,
    failed: AtomicU64,
    /// A `BOOL`, because `CopyFile2` is handed a pointer to it and reads it while it copies. See
    /// [`crate::windows::fast`].
    cancel: AtomicI32,
    talk: Mutex<Talk>,
    wake: Condvar,
    started: Instant,
    /// To book a frame when something here needs drawing at once — a question, the end.
    ctx: Option<egui::Context>,
}

/// The part of a [`Transfer`] that is a conversation rather than a counter.
#[derive(Default)]
struct Talk {
    paused: bool,
    /// When the current pause began, and how long every earlier one lasted, so the rate is the
    /// rate while it was moving.
    paused_at: Option<Instant>,
    paused_for: Duration,
    /// The conflict waiting on an answer. One at a time: a second worker that hits one waits for
    /// the first to be answered, because two questions on screen at once is one too many.
    question: Option<Clash>,
    reply: Option<Choice>,
    /// Not enough room, and its answer. Beside the conflict rather than instead of it, because the
    /// walk asks this one while a worker may be asking the other.
    short: Option<Short>,
    room: Option<Room>,
    /// A file that could not be got at, and its answer. One at a time, like a conflict.
    snag: Option<Snag>,
    mend: Option<Mend>,
    /// The standing answers for each kind of snag, once "for every file like it" was ticked.
    every_in_use: Option<Mend>,
    every_encrypted: Option<Mend>,
    /// A permanent delete waiting on its yes or no, and the answer.
    confirm: Option<Confirm>,
    confirmed: Option<bool>,
    /// The answer to give every conflict from here on, once "for every conflict" was ticked.
    every: Option<Choice>,
    /// The file most recently started, for the panel.
    current: String,
    failures: Vec<Failure>,
}

/// What the panel draws, read once a frame.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: u64,
    pub kind: Kind,
    pub into: PathBuf,
    pub phase: Phase,
    pub counted: bool,
    pub files_found: u64,
    pub bytes_found: u64,
    pub files_done: u64,
    pub bytes_done: u64,
    pub failed: u64,
    pub cancelled: bool,
    pub paused: bool,
    pub question: Option<Clash>,
    pub short: Option<Short>,
    pub snag: Option<Snag>,
    pub confirm: Option<Confirm>,
    pub current: String,
    pub failures: Vec<Failure>,
    /// How long it has been running, pauses left out.
    pub running_for: Duration,
}

impl Snapshot {
    /// Bytes a second, over the whole run so far.
    pub fn rate(&self) -> Option<f64> {
        let secs = self.running_for.as_secs_f64();
        (secs > 0.5 && self.bytes_done > 0).then(|| self.bytes_done as f64 / secs)
    }

    /// How long is left, once the walk has finished and there is a rate to go on.
    pub fn remaining(&self) -> Option<Duration> {
        let rate = self.rate()?;
        if !self.counted {
            return None;
        }
        let left = self.bytes_found.saturating_sub(self.bytes_done) as f64;
        Some(Duration::from_secs_f64(left / rate))
    }

    /// How much of it is done, from `0` to `1`: by bytes, or by files when there are no bytes to
    /// speak of — a move inside one volume, and a delete.
    pub fn fraction(&self) -> f32 {
        if self.bytes_found > 0 {
            (self.bytes_done as f64 / self.bytes_found as f64).clamp(0.0, 1.0) as f32
        } else if self.files_found > 0 {
            (self.files_done as f64 / self.files_found as f64).clamp(0.0, 1.0) as f32
        } else {
            0.0
        }
    }
}

impl Transfer {
    pub fn new(id: u64, job: &Job, ctx: Option<egui::Context>) -> Self {
        let (kind, into) = match job {
            Job::Move { into, .. } => (Kind::Move, into.clone()),
            Job::Delete { items, .. } => (
                Kind::Delete,
                items
                    .first()
                    .and_then(|item| item.parent())
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
            ),
            Job::Copy { into, .. } => (Kind::Copy, into.clone()),
            _ => (Kind::Copy, PathBuf::new()),
        };
        Self {
            id,
            kind,
            into,
            phase: AtomicU8::new(Phase::Starting as u8),
            counted: AtomicBool::new(false),
            files_found: AtomicU64::new(0),
            bytes_found: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            cancel: AtomicI32::new(0),
            talk: Mutex::new(Talk::default()),
            wake: Condvar::new(),
            started: Instant::now(),
            ctx,
        }
    }

    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Acquire))
    }

    pub(crate) fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Release);
        self.repaint();
    }

    fn talk(&self) -> MutexGuard<'_, Talk> {
        // A worker that panicked mid-copy poisons the lock; what is behind it is counters and
        // text, and still worth drawing.
        self.talk.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Sleep until the window or another worker changes something in [`Talk`].
    fn sleep<'a>(&self, talk: MutexGuard<'a, Talk>) -> MutexGuard<'a, Talk> {
        self.wake
            .wait(talk)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Sleep until `answer` finds what it is looking for in [`Talk`] — `None` if the job is
    /// cancelled first, which every question's own Cancel is.
    fn wait_for<'a, T>(
        &self,
        mut talk: MutexGuard<'a, Talk>,
        mut answer: impl FnMut(&mut Talk) -> Option<T>,
    ) -> (MutexGuard<'a, Talk>, Option<T>) {
        loop {
            if self.cancelled() {
                return (talk, None);
            }
            if let Some(found) = answer(&mut talk) {
                return (talk, Some(found));
            }
            talk = self.sleep(talk);
        }
    }

    fn repaint(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let talk = self.talk();
        let mut running_for = self.started.elapsed().saturating_sub(talk.paused_for);
        if let Some(at) = talk.paused_at {
            running_for = running_for.saturating_sub(at.elapsed());
        }
        Snapshot {
            id: self.id,
            kind: self.kind,
            into: self.into.clone(),
            phase: self.phase(),
            counted: self.counted.load(Ordering::Acquire),
            files_found: self.files_found.load(Ordering::Relaxed),
            bytes_found: self.bytes_found.load(Ordering::Relaxed),
            files_done: self.files_done.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            cancelled: self.cancelled(),
            paused: talk.paused,
            question: talk.question.clone(),
            short: talk.short.clone(),
            snag: talk.snag.clone(),
            confirm: talk.confirm.clone(),
            current: talk.current.clone(),
            failures: talk.failures.clone(),
            running_for,
        }
    }

    // ---- From the window ---------------------------------------------------

    /// Stop. Every worker finishes the call it is in — `CopyFile2` reads [`Self::cancel`] as it
    /// goes and removes the half-written file itself — and picks up nothing more.
    pub fn cancel(&self) {
        // Under the lock, so a worker between checking the flag and going to sleep cannot miss it.
        let _talk = self.talk();
        self.cancel.store(1, Ordering::SeqCst);
        self.wake.notify_all();
    }

    pub fn pause(&self, paused: bool) {
        let mut talk = self.talk();
        if talk.paused == paused {
            return;
        }
        talk.paused = paused;
        if paused {
            talk.paused_at = Some(Instant::now());
        } else if let Some(at) = talk.paused_at.take() {
            talk.paused_for += at.elapsed();
        }
        self.wake.notify_all();
    }

    /// The answer to the conflict on screen, and whether it is the answer to all of them from now
    /// on.
    pub fn answer(&self, choice: Choice, every: bool) {
        let mut talk = self.talk();
        if talk.question.is_none() {
            return;
        }
        talk.reply = Some(choice);
        if every {
            talk.every = Some(choice);
        }
        self.wake.notify_all();
    }

    /// Yes or no to the permanent delete on screen.
    pub fn confirm(&self, yes: bool) {
        let mut talk = self.talk();
        if talk.confirm.is_none() {
            return;
        }
        talk.confirmed = Some(yes);
        self.wake.notify_all();
    }

    /// Whether this job is waiting to be told yes or no, for the keys that answer it.
    pub fn confirming(&self) -> bool {
        self.talk().confirm.is_some()
    }

    /// The answer to the snag on screen. A standing answer is kept only for one that ends the
    /// question: see [`Steer::Mend`].
    pub fn mend(&self, mend: Mend, every: bool) {
        let mut talk = self.talk();
        let Some(trouble) = talk.snag.as_ref().map(|snag| snag.trouble) else {
            return;
        };
        talk.mend = Some(mend);
        if every && mend != Mend::TryAgain {
            match trouble {
                Trouble::InUse => talk.every_in_use = Some(mend),
                Trouble::Encrypted => talk.every_encrypted = Some(mend),
            }
        }
        self.wake.notify_all();
    }

    /// The answer to the shortage of room on screen.
    pub fn make_room(&self, room: Room) {
        let mut talk = self.talk();
        if talk.short.is_none() {
            return;
        }
        talk.room = Some(room);
        self.wake.notify_all();
    }

    // ---- From the workers ----------------------------------------------------

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) != 0
    }

    /// The flag itself, for `CopyFile2`'s `pfCancel`.
    pub(crate) fn cancel_flag(&self) -> *mut i32 {
        self.cancel.as_ptr()
    }

    /// Wait here for as long as the job is paused, or short of room.
    pub(crate) fn hold(&self) {
        let talk = self.talk();
        let _ = self.wait_for(talk, |talk| (!talk.paused && talk.short.is_none()).then_some(()));
    }

    /// Ask what to do about a clash, and wait for the answer. `None` when the job was cancelled
    /// instead — which the question's own Cancel is.
    pub(crate) fn ask(&self, clash: Clash) -> Option<Choice> {
        // Another worker's question first — unless a standing answer makes asking unnecessary.
        let (mut talk, turn) = self.wait_for(self.talk(), |talk| match talk.every {
            Some(every) => Some(Some(every)),
            None => talk.question.is_none().then_some(None),
        });
        if let Some(every) = turn? {
            return Some(every);
        }
        talk.question = Some(clash);
        talk.reply = None;
        self.repaint();
        let (mut talk, choice) = self.wait_for(talk, |talk| talk.reply.take());
        talk.question = None;
        // Whoever is waiting for a turn to ask.
        self.wake.notify_all();
        choice
    }

    /// Ask whether to go ahead with a permanent delete, and wait. `false` for no, and for a job
    /// cancelled while it waited.
    pub(crate) fn ask_confirm(&self, confirm: Confirm) -> bool {
        let mut talk = self.talk();
        talk.confirm = Some(confirm);
        talk.confirmed = None;
        self.repaint();
        let (mut talk, yes) = self.wait_for(talk, |talk| talk.confirmed.take());
        talk.confirm = None;
        yes.unwrap_or(false)
    }

    /// Say a file cannot be got at, and wait for what to do about it. `None` when the job was
    /// cancelled instead. Serialised like [`Self::ask`]: a second worker waits its turn.
    pub(crate) fn ask_snag(&self, snag: Snag) -> Option<Mend> {
        let trouble = snag.trouble;
        let (mut talk, turn) = self.wait_for(self.talk(), |talk| {
            let standing = match trouble {
                Trouble::InUse => talk.every_in_use,
                Trouble::Encrypted => talk.every_encrypted,
            };
            match standing {
                Some(mend) => Some(Some(mend)),
                None => talk.snag.is_none().then_some(None),
            }
        });
        if let Some(standing) = turn? {
            return Some(standing);
        }
        talk.snag = Some(snag);
        talk.mend = None;
        self.repaint();
        let (mut talk, mend) = self.wait_for(talk, |talk| talk.mend.take());
        talk.snag = None;
        self.wake.notify_all();
        mend
    }

    /// Say there is not enough room, and wait for what to do about it. `None` when the job was
    /// cancelled instead. Only the walk asks, so there is never a second one waiting its turn.
    pub(crate) fn ask_room(&self, short: Short) -> Option<Room> {
        let mut talk = self.talk();
        talk.short = Some(short);
        talk.room = None;
        self.repaint();
        let (mut talk, room) = self.wait_for(talk, |talk| talk.room.take());
        talk.short = None;
        // The workers, held while it was up.
        self.wake.notify_all();
        room
    }

    /// Bytes that have landed on the destination.
    pub(crate) fn wrote(&self, bytes: u64) {
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
    }

    /// How much has landed so far, for the walk's sum of what is still to come.
    pub(crate) fn written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    pub(crate) fn found(&self, files: u64, bytes: u64) {
        self.files_found.fetch_add(files, Ordering::Relaxed);
        self.bytes_found.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn counted(&self) {
        self.counted.store(true, Ordering::Release);
    }

    pub(crate) fn moved_bytes(&self, bytes: u64) {
        self.bytes_done.fetch_add(bytes, Ordering::Relaxed);
    }

    /// One file finished with — copied, skipped or failed alike, since all three are progress.
    pub(crate) fn finished_one(&self) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
    }

    /// One item found and finished with at once: a rename, a skip — nothing that waits in a queue.
    pub(crate) fn passed_one(&self, bytes: u64) {
        self.found(1, bytes);
        self.moved_bytes(bytes);
        self.finished_one();
    }

    pub(crate) fn now_on(&self, name: &str) {
        let mut talk = self.talk();
        talk.current.clear();
        talk.current.push_str(name);
    }

    pub(crate) fn failed(&self, path: &Path, why: String) {
        self.failed.fetch_add(1, Ordering::Relaxed);
        let mut talk = self.talk();
        if talk.failures.len() < KEPT {
            talk.failures.push(Failure {
                path: path.to_path_buf(),
                why,
            });
        }
    }

    /// Answer every conflict this way without asking, as a tick on *for every conflict* would.
    #[cfg(test)]
    pub(crate) fn always(&self, choice: Choice) {
        self.talk().every = Some(choice);
    }

    /// The same for a file in use.
    #[cfg(test)]
    pub(crate) fn always_in_use(&self, mend: Mend) {
        self.talk().every_in_use = Some(mend);
    }

    /// What the job comes back to [`super::Done`] as.
    pub(crate) fn ran(&self, outcome: Outcome) -> Ran {
        let failed = self.failed.load(Ordering::Relaxed);
        let error = (failed > 0).then(|| {
            let talk = self.talk();
            let verb = self.kind.done();
            let first = talk.failures.first().map_or(String::new(), |f| {
                format!(": {} — {}", crate::fs::display_name(&f.path), f.why)
            });
            if failed == 1 {
                format!("1 item could not be {verb}{first}")
            } else {
                format!("{failed} items could not be {verb}{first}")
            }
        });
        Ran {
            error,
            outcome,
            aborted: self.cancelled(),
        }
    }
}

/// Do the job, or answer `None` if it is one for the shell. See the module header for which.
///
/// Called on the job's own thread, never on the UI's: deciding means a look at every top-level
/// item.
pub fn run(job: &Job, transfer: &Transfer) -> Option<Ran> {
    #[cfg(windows)]
    {
        win::run(job, transfer)
    }
    #[cfg(not(windows))]
    {
        let _ = (job, transfer);
        None
    }
}

/// The checks that need nothing but the paths: whether `items` going into `into` is a job this
/// engine could do at all. The rest is [`win::run`]'s, which has to look at the disk.
pub(crate) fn suits(items: &[PathBuf], into: &Path) -> bool {
    let fold = |path: &Path| {
        path.as_os_str()
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    if items.is_empty() || !plain(into) {
        return false;
    }
    let into_folded = fold(into);
    items.iter().all(|item| {
        let folded = fold(item);
        let parent = item.parent().map(fold);
        plain(item)
            && item.file_name().is_some()
            // Already there: the shell's `one - Copy.txt`, or a move to nowhere.
            && parent.as_deref() != Some(into_folded.as_str())
            // Into itself or below itself.
            && into_folded != folded
            && !into_folded.starts_with(&format!("{folded}\\"))
    })
}

/// A path this engine can act on at all: a drive or UNC path, and not one of this program's
/// synthetic places.
pub(crate) fn plain(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    let bytes = text.as_bytes();
    let drive = bytes.len() >= 3 && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/');
    let unc = text.starts_with(r"\\") && !text.starts_with(r"\\?\") && !text.starts_with(r"\\.\");
    (drive || unc) && !crate::fs::is_synthetic(path)
}

/// Whether these items are ones a permanent delete here may take: plain paths, every one of them
/// something inside a folder. A drive root has no name to be inside anything by, and is never a
/// thing to delete.
pub(crate) fn erasable(items: &[PathBuf]) -> bool {
    !items.is_empty()
        && items
            .iter()
            .all(|item| plain(item) && item.file_name().is_some() && item.parent().is_some())
}

/// `name (2).ext`, or the first number after it that is free.
///
/// Explorer's own Keep both, which is a number in brackets before the extension. Not the shell's
/// `- Copy`, which is what a copy into its own folder is called and is never asked here.
pub(crate) fn free_name(path: &Path, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new(""));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // A dot at the very start is a name, not an extension: `.gitignore (2)`.
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name.as_str(), ""),
    };
    (2u32..)
        .map(|n| parent.join(format!("{stem} ({n}){ext}")))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded range finds a free name")
}

#[cfg(test)]
mod tests;
