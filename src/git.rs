//! Git, by asking `git`.
//!
//! What the window wants to know is small — the branch, how far it is from its remote, how much is
//! changed, what each row's own file is, and what changed inside the file a preview is showing — and
//! all of it comes out of four commands. Two of them answer for a **folder** and are what [`read`]
//! runs together: `status` and `ls-tree`. The other two answer for the **one file a preview panel is
//! showing**, and only ever that one: `diff` for a file with lines, and `show` for a picture, whose
//! older version has to be handed over whole because two versions of a `.png` have no lines to
//! compare.
//!
//! # Why the program and not a library
//!
//! `libgit2` (through `git2`) and `gitoxide` are both real options, and this deliberately takes
//! neither. The argument is not about elegance:
//!
//! - **`git status` is the hard part, and git is the fastest thing that does it.** A status is a walk
//!   of the worktree compared against the index, and git has spent twenty years making that cheap —
//!   the untracked cache, the index's stat data, `core.fsmonitor` where it is switched on, the sparse
//!   index. A library reimplements the walk and gets none of the user's own configuration for it. On
//!   a repository big enough for this to matter, the library is the slower answer, not the faster one.
//! - **It is the same answer the user's own tooling gives.** Their `git` reads their config, their
//!   `.gitignore` chain, their attributes, their worktrees, their submodules, their credential
//!   helper. A second implementation is a second set of answers, and the first time those differ is
//!   the last time this window is believed.
//! - **Everything deeper is one more command.** Log, blame, diff, stash, a commit — each is an
//!   argument list rather than an API to grow into. The shape here is already the shape those need: a
//!   worker, a `Command`, a parser for one text format.
//!
//! What it costs is a process per question and a text format to parse. **Measured** — see
//! `what_a_git_query_costs` — a folder in a repository is 113 ms of it on this machine, nearly all of
//! that being `git`'s own startup: a Windows `git` that does nothing at all costs about 80 ms. Which
//! is why [`read`] starts its two commands together and works the third answer out for itself, and why
//! the gate below matters more than any of it. The format is `--porcelain=v2`, which exists precisely
//! so that programs can read it and is documented as stable.
//!
//! The one place a library would win is a hot loop over object contents — a graph walk, a diff per
//! file. If this window ever draws a commit graph, that part can move to `gix` without any of this
//! changing: the answer would still arrive as [`Repo`] on the same channel.
//!
//! # There is no cache, and that is the feature
//!
//! TortoiseSVN's overlays are the cautionary tale: a status cache with its own lifetime, its own
//! invalidation and its own opinion of when to believe itself — and the failure mode is a green tick
//! on a file you just changed, which is worse than no tick at all.
//!
//! So there is nothing here that outlives the listing it belongs to. A [`Repo`] is asked for when a
//! folder's listing lands, it hangs off that tab's view of that folder, and it dies with it. Every
//! event that re-reads a folder — a file operation, `F5`, the watcher noticing somebody else's
//! change, a navigation — asks git again, because it is the same event that asked the filesystem
//! again. One source of truth, one lifetime, and no third state where the two disagree.
//!
//! What keeps that affordable is not caching but **not asking**: a folder with no `.git` anywhere
//! above it is answered by two or three `stat` calls and no process at all — 0.23 ms measured, against
//! 113 ms for one that is — which is what makes browsing a disk with no repositories on it free.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering as Atomic};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

/// What one row's file is to git.
///
/// One state per path, because a row can only wear one mark. Where git has two answers — a change
/// staged and then changed again, `MM` — the **worktree** wins: it is what is on disk, which is what
/// the row is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Tracked, and the same as HEAD.
    Clean,
    /// Changed, and the change is entirely staged.
    Staged,
    /// Changed on disk, staged or not.
    Modified,
    /// Gone from disk, still in the index.
    Deleted,
    /// Moved or copied, and git recognised it as the same content.
    Renamed,
    /// Not tracked and not ignored.
    Untracked,
    /// A merge left it with both sides in it.
    Conflicted,
}

impl State {
    /// Which state a folder wears when its subtree holds several.
    ///
    /// The most urgent thing under it, in the order somebody scanning a listing wants to be told:
    /// a conflict before a change, a change before something not yet added, and anything at all
    /// before "nothing to report".
    fn rank(self) -> u8 {
        match self {
            State::Clean => 0,
            State::Untracked => 1,
            State::Staged => 2,
            State::Renamed => 3,
            State::Deleted => 4,
            State::Modified => 5,
            State::Conflicted => 6,
        }
    }

    fn or_stronger(self, other: Self) -> Self {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// Everything the window knows about the repository a folder is in.
///
/// The counts are the **repository's**, because that is what a branch line means everywhere else it
/// is shown — a prompt, an editor's status bar — and because one `git status` answers for all of it
/// anyway. The marks are the **folder's**: they are per row, and a row only exists here.
#[derive(Debug, Default)]
pub struct Repo {
    /// The branch, or the short commit when the head is detached.
    pub head: String,
    /// Whether [`Repo::head`] is a commit rather than a branch name.
    pub detached: bool,
    /// The remote-tracking branch, when the head has one. `None` on a branch that has never been
    /// pushed, which is also why [`Repo::ahead`] and [`Repo::behind`] mean nothing without it.
    pub upstream: Option<String>,
    /// Commits this branch has that its upstream does not, and the other way round.
    pub ahead: u32,
    pub behind: u32,
    /// Paths git has anything to say about: changed, staged, untracked or conflicted, each counted
    /// once however many of those it is.
    pub changed: u32,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicted: u32,
    /// What the three commands took, end to end. For `--trace`, and for the same reason the scan's
    /// own figure is in the status line: a claim about speed nobody can check is not a claim.
    pub micros: u64,
    /// The repository's own `.git`.
    ///
    /// Watched while this folder is on screen, because **a commit changes the repository without
    /// touching the folder**: `git commit` in the console panel, a rebase in a terminal, a pull —
    /// every one of them leaves the working tree exactly as the listing last read it, so nothing that
    /// watches the *folder* would ever notice. Marks that survive their own commit are precisely the
    /// stale-overlay failure this design exists to avoid, so the one thing worth watching besides the
    /// folder is this.
    pub dot_git: PathBuf,
    /// State by path *relative to the folder that was asked about*, separators normalised to `/`.
    ///
    /// Every ancestor is in here too, wearing the strongest state under it — so a folder row is one
    /// lookup rather than a scan, and a listing costs one hash lookup per row per frame instead of
    /// a walk over the repository's changes.
    marks: HashMap<String, State>,
}

impl Repo {
    /// What the row named `name` is, if git has anything to say about it.
    ///
    /// `name` is the name as the listing shows it, which in a flattened listing is a path with the
    /// platform's separators in it — normalised here, since git speaks in `/` on every platform.
    pub fn state(&self, name: &str) -> Option<State> {
        if name.contains('\\') {
            let flat = name.replace('\\', "/");
            self.marks.get(flat.as_str()).copied()
        } else {
            self.marks.get(name).copied()
        }
    }

    /// Whether the working tree has anything in it that HEAD does not.
    pub fn dirty(&self) -> bool {
        self.changed > 0
    }

    /// A repository that says exactly this and nothing else.
    ///
    /// For the tests in the modules that *ask* a `Repo` rather than read one — the `@git` filter's,
    /// the listing's — which need an answer to hand and not a repository on disk. The marks are the
    /// only field they read; everything else is [`Default`].
    #[cfg(test)]
    pub fn of<'a>(marks: impl IntoIterator<Item = (&'a str, State)>) -> Self {
        Self {
            marks: marks
                .into_iter()
                .map(|(path, state)| (path.to_owned(), state))
                .collect(),
            ..Self::default()
        }
    }
}

impl State {
    /// What this state is, in words, for a row's tooltip.
    ///
    /// The same sentences the badge table in the README uses, because they are the same facts and a
    /// second wording would be a second answer. `for_folder` is what turns them into sentences about a
    /// *folder*: a folder's mark is the strongest thing anywhere beneath it — see [`mark`] — so
    /// "changed" about a folder means something under it is, and saying so is the difference between a
    /// mark that reads as informative and one that reads as wrong.
    pub fn describe(self, for_folder: bool) -> &'static str {
        match (self, for_folder) {
            (Self::Clean, false) => "Committed, and the same as the last commit",
            (Self::Clean, true) => "Nothing under it has changed",
            (Self::Staged, false) => "Staged",
            (Self::Staged, true) => "Something under it is staged",
            (Self::Modified, false) => "Changed on disk",
            (Self::Modified, true) => "Something under it has changed",
            (Self::Deleted, false) => "Gone from disk, still in the index",
            (Self::Deleted, true) => "Something under it is gone from disk",
            (Self::Renamed, false) => "Moved or copied, and git saw it",
            (Self::Renamed, true) => "Something under it moved",
            (Self::Untracked, false) => "Untracked, and not ignored",
            (Self::Untracked, true) => "Something under it is untracked",
            (Self::Conflicted, false) => "A merge left both sides in it",
            (Self::Conflicted, true) => "A merge left a conflict under it",
        }
    }

    /// Whether `HEAD` has a version of this file that is not the one on disk — which is the
    /// question "is there anything to compare it with".
    ///
    /// Staged and modified both mean yes, and a conflicted file certainly does. The other four are
    /// no, each for its own reason: a clean file *is* `HEAD`'s, an untracked one and a renamed one
    /// have no path in `HEAD` for git to look up, and a deleted one is not there to preview.
    pub fn differs_from_head(self) -> bool {
        matches!(self, Self::Staged | Self::Modified | Self::Conflicted)
    }
}

/// What `git diff` says about one file: which of its lines are new, and the text of the ones it no
/// longer has.
///
/// **Against `HEAD`, not against the index.** The question a preview answers is "what is different
/// about this file", and staged-or-not is a distinction about *how* the change is being handled rather
/// than about the file — so a preview that showed only the unstaged half would hide work somebody had
/// already staged.
#[derive(Debug, Default, Clone)]
pub struct Changes {
    /// In file order, as git emits them.
    pub hunks: Vec<Hunk>,
}

/// One run of changed lines.
#[derive(Debug, Default, Clone)]
pub struct Hunk {
    /// The first line of the *file* this hunk adds, 1-based, and how many.
    ///
    /// `count` is zero for a hunk that only takes lines away.
    pub added: u32,
    pub added_count: u32,
    /// The line of the file the removed lines belong *after*. Zero means before the first line.
    ///
    /// Worked out from the header rather than guessed: git gives a hunk that removes and adds
    /// nothing at the same place two different new-side numbers, and the difference is exactly this.
    pub after: u32,
    /// The lines `HEAD` has here that the file does not, in order, and the first of their line
    /// numbers in `HEAD`.
    pub removed: Vec<String>,
    pub removed_at: u32,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }
}

/// Ask git what changed in one file, or answer `None` because nothing did or nobody could say.
///
/// One process, and it is only ever asked for the file a preview panel is *showing* — so at most one
/// per selection, and none at all for a file outside a repository.
pub fn changes(file: &Path) -> Option<Changes> {
    let dir = file.parent()?;
    repo_root(dir)?;
    // `-U0`, so the output is the changed lines and nothing else: a hunk of context around every
    // change would be most of a large file's diff, and the file itself is already on screen.
    //
    // `--no-ext-diff` because a configured `diff.external` would answer in its own format, and
    // `--no-color` because a configured `color.diff` would answer in escape codes. Both are somebody
    // else's preference about reading a diff, and neither survives being parsed.
    let out = finish_git(start_git(
        dir,
        &[
            "--no-optional-locks",
            "diff",
            "HEAD",
            "-U0",
            "--no-color",
            "--no-ext-diff",
            "--ignore-submodules",
            "--",
            &file.to_string_lossy(),
        ],
    ))?;
    Some(parse_diff(&out))
}

/// The file as it would be **on disk** at the last commit — not as the object database holds it.
///
/// **For the files a diff of text cannot describe**, which today means pictures: two versions of a
/// `.png` have no lines to put a band behind, so the panel decodes both and shows them side by side
/// with the difference between them. See [`crate::preview::against_head`].
///
/// `None` for an untracked or newly added file — `HEAD` has no such path, and git says so by failing —
/// and for a folder outside a repository, which is the cheap test done first. Every one of those means
/// the same thing to the panel: show the file, and nothing to compare it with.
///
/// # `cat-file --filters`, and why `show` is the wrong command
///
/// `git show HEAD:<path>` hands back the **stored object**, and a stored object is not always a file.
/// Two things stand between the two, and both are ordinary configuration in a repository big enough to
/// hold pictures:
///
/// - **Git LFS.** What is committed is a 130-byte pointer; the picture lives beside the repository.
///   Measured on the repository this was reported from, `show` gave **129 bytes** of
///   `version https://git-lfs.github.com/spec/v1` where the file is **154,544**. Nothing decodes that,
///   so the panel fell back to showing one picture — which is exactly the bug: "it works for `.svg`
///   and not for `.bmp`", because that repository's `.gitattributes` puts one through LFS and not the
///   other.
/// - **End-of-line conversion.** With `* text=auto` and `core.autocrlf=true`, git converts anything it
///   *decides* is text — and its decision is "no NUL byte in the first 8 KB", which a 24-bit bitmap of
///   a photograph can easily pass. The blob then has `\n` where the file has `\r\n`, and every pixel
///   row after the first is shifted.
///
/// `--filters` runs the smudge filter and the eol conversion, which is to say it answers the question
/// actually being asked: what would this file *be* if it were checked out. It is also why the path
/// matters to the command rather than only to the lookup — filters are configured per path.
///
/// `HEAD:./name` rather than a path worked out from the repository root, because git resolves a
/// `./`-prefixed spec against the process's own directory and [`start_git`] has already set that to
/// the file's folder. One less thing to get wrong in a worktree or a submodule.
pub fn blob(file: &Path) -> Option<Vec<u8>> {
    let dir = file.parent()?;
    let name = file.file_name()?.to_str()?;
    repo_root(dir)?;
    finish_git(start_git(
        dir,
        &[
            "--no-optional-locks",
            "cat-file",
            "--filters",
            &format!("HEAD:./{name}"),
        ],
    ))
}

/// Read `git diff -U0`.
///
/// ```text
/// @@ -2 +2 @@ a          one line replaced by one: a count of 1 is left out entirely
/// -b
/// +B
/// @@ -3,0 +4,2 @@ c      nothing removed, two lines added at 4
/// +NEW1
/// +NEW2
/// @@ -6,2 +7,0 @@ e      two lines removed, nothing added — and 7 is the line they sat *after*
/// -f
/// -g
/// ```
///
/// The zero-count side is the subtle one, and it is why [`Hunk::after`] exists: when the new side
/// has no lines, its number is the line the removal follows; when it has lines, the removal is
/// displayed in front of them.
fn parse_diff(out: &[u8]) -> Changes {
    let text = String::from_utf8_lossy(out);
    let mut changes = Changes::default();
    // Nothing before the first `@@` is a change — and `--- a/path` starts with a minus, so a parser
    // that does not wait for the header would read the file's own name as a removed line.
    let mut started = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@@ ") {
            started = true;
            let Some(hunk) = parse_hunk(rest) else {
                continue;
            };
            changes.hunks.push(hunk);
            continue;
        }
        if !started {
            continue;
        }
        let Some(hunk) = changes.hunks.last_mut() else {
            continue;
        };
        match line.as_bytes().first() {
            Some(b'-') => hunk.removed.push(shown_as(&line[1..])),
            // `\ No newline at end of file`, which is a note about the line above rather than a line.
            Some(b'+') | Some(b'\\') => {}
            _ => {}
        }
    }
    changes
}

/// `-a[,b] +c[,d] @@ …` into a hunk.
fn parse_hunk(rest: &str) -> Option<Hunk> {
    let mut fields = rest.split(' ');
    let (old_at, old_count) = span_of(fields.next()?.strip_prefix('-')?)?;
    let (new_at, new_count) = span_of(fields.next()?.strip_prefix('+')?)?;
    Some(Hunk {
        added: new_at,
        added_count: new_count,
        after: if new_count == 0 {
            new_at
        } else {
            new_at.saturating_sub(1)
        },
        removed: Vec::with_capacity(old_count as usize),
        removed_at: old_at,
    })
}

/// `12,3` or `12`, where a missing count means one line.
fn span_of(field: &str) -> Option<(u32, u32)> {
    match field.split_once(',') {
        Some((at, count)) => Some((at.parse().ok()?, count.parse().ok()?)),
        None => Some((field.parse().ok()?, 1)),
    }
}

/// A removed line as the preview would have drawn it, had the file still had it.
///
/// The same two substitutions [`crate::preview`] makes on the way in — tabs to four spaces, and no
/// stray `\r` — because a removed line is shown *among* the file's own lines and one of them laying
/// its indentation out differently is worse than not showing it.
fn shown_as(line: &str) -> String {
    line.trim_end_matches('\r').replace('\t', "    ")
}

/// A finished query, tagged with the view of the folder that asked.
pub struct Answer {
    /// [`crate::pane::Tab::view`] — this view of this folder. Any other is a question nobody is
    /// asking any more, and its answer is dropped.
    pub view: u64,
    /// `None` when the folder is not in a repository, or when git could not be asked at all.
    pub repo: Option<Arc<Repo>>,
}

struct Request {
    view: u64,
    dir: PathBuf,
}

/// How many questions may be waiting at once.
///
/// Arrow-keying down a tree of repositories queues one per folder passed through, and every one of
/// them but the last is already stale. The oldest go, which is the one that has been waiting longest
/// and is therefore the least likely to still be on screen.
const BACKLOG: usize = 4;

struct Queue {
    pending: Mutex<std::collections::VecDeque<Request>>,
    wake: Condvar,
    shutdown: AtomicBool,
}

/// Asks git about folders, off the UI thread.
///
/// Modelled on [`crate::loader::Loader`] and for the same reason: a question that can take a second
/// on somebody else's repository does not get to hold up a repaint. Two workers, because a window
/// has two panes and one slow answer must not be the other's queue.
pub struct Git {
    queue: Arc<Queue>,
    answers: Receiver<Answer>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl Git {
    pub fn new(ctx: &egui::Context) -> Self {
        let queue = Arc::new(Queue {
            pending: Mutex::new(std::collections::VecDeque::new()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let (tx, answers) = channel();
        let workers = (0..2)
            .map(|i| {
                let queue = queue.clone();
                let tx = tx.clone();
                let ctx = ctx.clone();
                std::thread::Builder::new()
                    .name(format!("git-{i}"))
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

    /// Ask about a folder, for one view of it.
    pub fn request(&self, view: u64, dir: &Path) {
        let Ok(mut pending) = self.queue.pending.lock() else {
            return;
        };
        while pending.len() >= BACKLOG {
            pending.pop_front();
        }
        pending.push_back(Request {
            view,
            dir: dir.to_path_buf(),
        });
        self.queue.wake.notify_one();
    }

    /// Every answer that has arrived since the last call.
    pub fn drain(&self) -> impl Iterator<Item = Answer> + '_ {
        self.answers.try_iter()
    }
}

impl Drop for Git {
    fn drop(&mut self) {
        // The flag is set *while holding the queue lock*, which is what makes the handshake sound: a
        // worker checks `shutdown` with the lock held and then calls `wait`, which releases it.
        // Setting it without the lock leaves a window where the notify lands before the worker is
        // waiting for it, and the worker then sleeps for ever on a queue nothing will push to. The
        // same handshake as [`crate::loader::Loader`], for the same reason.
        {
            let _held = self.queue.pending.lock();
            self.queue.shutdown.store(true, Atomic::Release);
        }
        self.queue.wake.notify_all();
        for worker in self.workers.drain(..) {
            // A worker waiting on a `git status` cannot be interrupted, so this is as long as that
            // status takes — which on somebody's monorepo is seconds. Joined anyway, for the reason
            // the loader joins: a thread still running when the process tears down is holding a
            // channel and a `Context` that are about to be dropped underneath it.
            let _ = worker.join();
        }
    }
}

fn worker(queue: Arc<Queue>, tx: Sender<Answer>, ctx: egui::Context) {
    loop {
        let request = {
            let Ok(mut pending) = queue.pending.lock() else {
                return;
            };
            loop {
                if queue.shutdown.load(Atomic::Acquire) {
                    return;
                }
                if let Some(request) = pending.pop_front() {
                    break request;
                }
                let Ok(next) = queue.wake.wait(pending) else {
                    return;
                };
                pending = next;
            }
        };
        let repo = read(&request.dir).map(Arc::new);
        if tx
            .send(Answer {
                view: request.view,
                repo,
            })
            .is_ok()
        {
            ctx.request_repaint();
        } else {
            return;
        }
    }
}

/// Ask git about one folder, or decide not to ask at all.
///
/// **Two processes, started together.** Both were measured into being:
///
/// - It was three, the third being `rev-parse --show-prefix` to ask git where this folder sits
///   inside the repository. That answer is already in hand — the walk that found `.git` found the
///   root — and on Windows a `git` that does nothing at all still costs about **80 ms** of startup,
///   so asking it a question you can answer yourself is a third of the whole cost.
/// - And they ran one after the other, which is the sum of two startups for no reason: neither
///   answer feeds the other.
fn read(dir: &Path) -> Option<Repo> {
    // The gate, and the reason browsing a disk with no repositories on it is not a process storm: a
    // folder with no `.git` above it is not in a repository, and `stat` says so without starting
    // anything. A `.git` *file* counts as much as a directory — that is what a worktree and a
    // submodule have, and both are repositories.
    let root = repo_root(dir)?;
    let start = std::time::Instant::now();

    // Where this folder is inside the repository, in git's spelling: every path `status` prints is
    // relative to the root, and every name a row has is relative to here.
    //
    // Worked out from the root rather than asked, which is exact for a plain repository, a submodule
    // and a linked worktree — all three keep their `.git` at their own root. The layouts where it
    // would not be are the exotic ones: `GIT_DIR` in the environment, or a `core.worktree` pointing
    // somewhere else. There the prefix comes out wrong and `strip_prefix` then matches nothing, so
    // the failure is **marks missing**, never marks on the wrong row.
    let mut prefix = String::new();
    for part in dir.strip_prefix(&root).ok()?.components() {
        prefix.push_str(&part.as_os_str().to_string_lossy());
        prefix.push('/');
    }

    let mut repo = Repo {
        dot_git: root.join(".git"),
        ..Repo::default()
    };

    // `--porcelain=v2` is the format meant for programs; `-z` is what makes a path with a space, a
    // quote or a newline in it readable, since git otherwise C-quotes such names. `--branch` adds the
    // three header lines the status bar is made of, in the same call rather than another one.
    //
    // **No `--no-optional-locks` here**, and that is deliberate rather than an omission — it was
    // here once, and it made a real repository unusable. `status` disagreeing with the index's
    // cached stat data — a fresh checkout, an LFS smudge, a backup tool touching mtimes — makes git
    // recompute the truth and, ordinarily, write the answer back into the index so the next call is
    // cheap. `--no-optional-locks` is documented to suppress exactly that write, and on a repository
    // where the recompute is a `git-lfs` clean filter reading gigabytes of large binary samples back
    // off disk, throwing the answer away turns an **eleven-second, one-time** cost into an
    // eleven-second cost on every listing, forever — measured on a real repository of exactly this
    // shape. The lock this write takes is `hold_locked_index(..., 0)`, non-blocking by construction:
    // if two of this program's own workers reach the same repository at once, or the user is mid
    // `git rebase` in a terminal, the loser silently skips the write and answers correctly anyway —
    // confirmed by holding `index.lock` and by running two statuses at once, both against a real
    // repository. So there was never a contention problem here for the flag to be protecting against,
    // only a self-inflicted one.
    //
    // And `ls-tree` beside it: the names in *this* folder that git already tracks, which is the only
    // way to know that a file `status` said nothing about is committed rather than ignored.
    // Non-recursive and relative to the folder, so it costs the folder's own size rather than the
    // repository's — and it lists a subfolder as a name of its own, which is what makes a tracked
    // folder mark itself. Absent on a repository with no commit yet, which is not a failure: there is
    // nothing committed for a tick to be about. Left with `--no-optional-locks`, unlike `status`: it
    // never touches the index and takes no lock either way, so the flag is inert on it — nothing here
    // depends on that, it is just what makes the difference from `status` visible at the call site.
    let status = start_git(
        dir,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
            "-z",
        ],
    );
    let tree = start_git(
        dir,
        &[
            "--no-optional-locks",
            "ls-tree",
            "--name-only",
            "-z",
            "HEAD",
            "--",
            ".",
        ],
    );

    parse_status(&finish_git(status)?, &prefix, &mut repo);
    if let Some(tree) = finish_git(tree) {
        for name in tree.split(|byte| *byte == 0) {
            if name.is_empty() {
                continue;
            }
            let Ok(name) = std::str::from_utf8(name) else {
                continue;
            };
            repo.marks.entry(name.to_owned()).or_insert(State::Clean);
        }
    }

    repo.micros = start.elapsed().as_micros() as u64;
    Some(repo)
}

/// The folder holding the `.git` that governs `dir`, walking up from it.
fn repo_root(dir: &Path) -> Option<PathBuf> {
    let mut at = Some(dir);
    while let Some(here) = at {
        if std::fs::symlink_metadata(here.join(".git")).is_ok() {
            return Some(here.to_path_buf());
        }
        at = here.parent();
    }
    None
}

/// Whether `dir` is in a repository at all — the gate, without the answer.
#[cfg(test)]
fn under_a_git_dir(dir: &Path) -> bool {
    repo_root(dir).is_some()
}

/// Start git in `dir`, without waiting for it.
///
/// Both commands are started before either is read, so the window pays one git startup instead of
/// two. Draining them one after the other afterwards is safe: a child whose pipe fills up simply
/// stops writing until it is read, and each has its own.
fn start_git(dir: &Path, args: &[&str]) -> Option<std::process::Child> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::shell::no_window(&mut command);
    command.spawn().ok()
}

/// Its standard output, or `None` if it failed for any reason.
///
/// A failure is not worth reporting anywhere: no git on the `PATH`, a folder that has just been
/// deleted, a repository mid-rebase with a lock held. All of them mean the same thing to this
/// window — no git furniture this time round — and all of them are asked again by the next read of
/// the folder.
fn finish_git(child: Option<std::process::Child>) -> Option<Vec<u8>> {
    let out = child?.wait_with_output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// Read `git status --porcelain=v2 --branch -z`.
///
/// The format, one record per NUL-terminated string:
///
/// ```text
/// # branch.head main            the branch, or `(detached)`
/// # branch.upstream origin/main only when it has one
/// # branch.ab +2 -1             ahead of it, behind it
/// 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>              a changed path
/// 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>   renamed or copied, and then a
///                                                          second record with where from
/// u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>    unmerged
/// ? <path>                                                  untracked
/// ! <path>                                                  ignored, which is not asked for
/// ```
///
/// `XY` is the index's letter and the worktree's, `.` for unchanged. A path is the last field and
/// may contain spaces, so the fields in front of it are counted rather than split on.
fn parse_status(out: &[u8], prefix: &str, repo: &mut Repo) {
    let mut records = out.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        // A path git cannot spell in UTF-8 is a path this window cannot draw either. Counted, so
        // the totals stay true, and unmarked.
        let Ok(record) = std::str::from_utf8(record) else {
            repo.changed += 1;
            continue;
        };
        let (kind, rest) = record.split_at(1);
        let rest = rest.trim_start_matches(' ');
        match kind {
            "#" => header(rest, repo),
            "?" => {
                repo.untracked += 1;
                repo.changed += 1;
                // An untracked *directory* is one record with a trailing slash, however much is
                // inside it — git's own summary of it, and the same one this shows.
                mark(repo, prefix, rest.trim_end_matches('/'), State::Untracked);
            }
            "!" => {}
            "1" | "2" => {
                // The rename record's second half — where it came from — is a record of its own.
                // Taken off the front here whether or not it is used, or it would be read as the
                // next status line.
                //
                // A `2` carries one field more than a `1`: the `R100` that says how alike the two
                // sides are.
                let fields = if kind == "1" { 7 } else { 8 };
                let Some((xy, path)) = split_off(rest, fields) else {
                    continue;
                };
                if kind == "2" {
                    let _from = records.next();
                }
                let (index, work) = letters(xy);
                if index != '.' {
                    repo.staged += 1;
                }
                if work != '.' {
                    repo.unstaged += 1;
                }
                repo.changed += 1;
                mark(repo, prefix, path, state_of(index, work));
            }
            "u" => {
                let Some((_xy, path)) = split_off(rest, 9) else {
                    continue;
                };
                repo.conflicted += 1;
                repo.changed += 1;
                mark(repo, prefix, path, State::Conflicted);
            }
            _ => {}
        }
    }
}

/// One of the three `# branch.*` lines.
fn header(rest: &str, repo: &mut Repo) {
    let Some((key, value)) = rest.split_once(' ') else {
        return;
    };
    match key {
        // **After the oid, which git prints first.** So a detached head cannot simply take the
        // branch line's word for its name: it has to keep the commit from a line already gone past.
        // Setting `head` from the oid and letting `(detached)` overwrite it is the version of this
        // that was wrong, and it left a detached head with no name at all.
        "branch.head" => {
            repo.detached = value == "(detached)";
            if !repo.detached {
                repo.head = value.to_owned();
            }
        }
        // Seven characters, which is what git itself abbreviates to and what fits in a status line.
        // Kept whatever the head turns out to be, since it is only *used* when detached.
        "branch.oid" => repo.head = value.chars().take(7).collect(),
        "branch.upstream" => repo.upstream = Some(value.to_owned()),
        "branch.ab" => {
            for part in value.split(' ') {
                let (sign, count) = part.split_at(1);
                let Ok(count) = count.parse::<u32>() else {
                    continue;
                };
                match sign {
                    "+" => repo.ahead = count,
                    "-" => repo.behind = count,
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// The `XY` field and the path, given how many space-separated fields follow `XY` — the path being
/// the last of them.
///
/// Counted rather than split on, because **the path is whatever is left**: a name with spaces in it
/// is ordinary on Windows, and splitting on every space would take `my notes v2.txt` for three
/// fields and mark nothing.
fn split_off(rest: &str, fields: usize) -> Option<(&str, &str)> {
    let mut parts = rest.splitn(fields + 1, ' ');
    let xy = parts.next()?;
    let path = parts.nth(fields - 1)?;
    Some((xy, path))
}

fn letters(xy: &str) -> (char, char) {
    let mut chars = xy.chars();
    (chars.next().unwrap_or('.'), chars.next().unwrap_or('.'))
}

/// One state from git's two letters. The worktree's answer wins — see [`State`].
fn state_of(index: char, work: char) -> State {
    match (index, work) {
        (_, 'D') | ('D', '.') => State::Deleted,
        (_, 'R') | ('R', '.') | (_, 'C') | ('C', '.') => State::Renamed,
        (_, 'M') | (_, 'T') => State::Modified,
        ('A', '.') => State::Staged,
        ('.', _) => State::Modified,
        _ => State::Staged,
    }
}

/// Record a path's state against the folder that was asked about, and against every folder between.
///
/// `path` is relative to the repository's root; `prefix` is where the folder is inside it. Anything
/// outside the folder is somebody else's row and is only counted, not marked.
fn mark(repo: &mut Repo, prefix: &str, path: &str, state: State) {
    let Some(rel) = path.strip_prefix(prefix) else {
        return;
    };
    if rel.is_empty() {
        return;
    }
    // Every ancestor as well as the path itself, so that a folder row wears the strongest state
    // under it without anything having to walk the map to find out.
    for (at, _) in rel.match_indices('/') {
        let ancestor = &rel[..at];
        let now = repo
            .marks
            .get(ancestor)
            .copied()
            .unwrap_or(State::Clean)
            .or_stronger(state);
        repo.marks.insert(ancestor.to_owned(), now);
    }
    let now = repo
        .marks
        .get(rel)
        .copied()
        .unwrap_or(State::Clean)
        .or_stronger(state);
    repo.marks.insert(rel.to_owned(), now);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every sample here is real output, captured from a repository built to have one of each — a
    /// staged add, a staged-and-modified file, a deletion, a rename, an untracked file, an untracked
    /// directory, a change in a subfolder and a change further down. The NULs are written as `\0`.
    const REAL: &str = concat!(
        "# branch.oid ab41008277b4bce1dd164b4c7e49073939048bf2\0",
        "# branch.head main\0",
        "# branch.upstream origin/main\0",
        "# branch.ab +2 -1\0",
        "1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 2ce75e2a24f7d6841a504cf3616ef5a59edb3a2d added.txt\0",
        "1 .M N... 100644 100644 100644 fe7900bcbd294970da3296db5cf2020b4391a639 fe7900bcbd294970da3296db5cf2020b4391a639 deep/inner/buried.txt\0",
        "1 .D N... 100644 100644 000000 2bdf67abb163a4ffb2d7f3f0880c9fe5068ce782 2bdf67abb163a4ffb2d7f3f0880c9fe5068ce782 deleted.txt\0",
        "1 MM N... 100644 100644 100644 f719efd430d52bcfc8566a43b2eb655688d38871 f642f860d1de0bbe29b4f23400c1df3625454d08 modified.txt\0",
        "2 R. N... 100644 100644 100644 5626abf0f72e58d7a153368ba57db4c673c0e171 5626abf0f72e58d7a153368ba57db4c673c0e171 R100 moved.txt\0clean.txt\0",
        "1 .M N... 100644 100644 100644 ffe2fce498955b628014618b28c6bcf152466a4a ffe2fce498955b628014618b28c6bcf152466a4a sub/modified.txt\0",
        "? brandnew/\0",
        "? untracked.txt\0",
    );

    fn at(prefix: &str) -> Repo {
        let mut repo = Repo::default();
        parse_status(REAL.as_bytes(), prefix, &mut repo);
        repo
    }

    #[test]
    fn the_branch_line_is_the_status_bar() {
        let repo = at("");
        assert_eq!(repo.head, "main");
        assert!(!repo.detached);
        assert_eq!(repo.upstream.as_deref(), Some("origin/main"));
        assert_eq!((repo.ahead, repo.behind), (2, 1));
    }

    /// One count per path, whatever git had to say about it — and `MM` is one file, not two.
    #[test]
    fn every_path_is_counted_once() {
        let repo = at("");
        assert_eq!(repo.changed, 8, "six tracked paths and two untracked");
        assert_eq!(repo.staged, 3, "added, MM, and the rename");
        assert_eq!(repo.unstaged, 4, "buried, deleted, MM, sub/modified");
        assert_eq!(repo.untracked, 2);
        assert_eq!(repo.conflicted, 0);
    }

    /// The row's own name, from the folder that asked.
    #[test]
    fn a_row_wears_its_own_state() {
        let repo = at("");
        assert_eq!(repo.state("added.txt"), Some(State::Staged));
        assert_eq!(repo.state("modified.txt"), Some(State::Modified));
        assert_eq!(repo.state("deleted.txt"), Some(State::Deleted));
        assert_eq!(repo.state("moved.txt"), Some(State::Renamed));
        assert_eq!(repo.state("untracked.txt"), Some(State::Untracked));
        assert_eq!(repo.state("nothing-to-say.txt"), None);
    }

    /// A folder wears the strongest thing under it, however deep that is — which is the whole of what
    /// a row for a folder can usefully say.
    #[test]
    fn a_folder_wears_the_strongest_state_beneath_it() {
        let repo = at("");
        assert_eq!(repo.state("sub"), Some(State::Modified));
        assert_eq!(repo.state("deep"), Some(State::Modified), "two levels down");
        assert_eq!(
            repo.state("brandnew"),
            Some(State::Untracked),
            "an untracked directory is one record with a slash on it"
        );
    }

    /// The same output read from a subfolder: the paths are still the repository's, so the folder's
    /// own prefix is what turns them into rows.
    #[test]
    fn a_subfolder_sees_its_own_names() {
        let repo = at("sub/");
        assert_eq!(repo.state("modified.txt"), Some(State::Modified));
        assert_eq!(repo.state("sub/modified.txt"), None, "not from in here");
        assert_eq!(repo.state("added.txt"), None, "that one is upstairs");
        // Counted all the same: the branch line is the repository's, not the folder's.
        assert_eq!(repo.changed, 8);
    }

    /// A flattened listing's rows are paths with the platform's separators in them.
    #[test]
    fn a_flattened_row_finds_itself_too() {
        let repo = at("");
        assert_eq!(repo.state("deep\\inner\\buried.txt"), Some(State::Modified));
        assert_eq!(repo.state("deep\\inner"), Some(State::Modified));
    }

    /// A detached head has no branch name, so it wears the commit instead — and nothing about a
    /// remote, because there is no branch to be ahead of one.
    #[test]
    fn a_detached_head_shows_its_commit() {
        let mut repo = Repo::default();
        parse_status(
            concat!(
                "# branch.oid ed036ba1dab0b7a04bcfff16806b18543fadec4c\0",
                "# branch.head (detached)\0",
            )
            .as_bytes(),
            "",
            &mut repo,
        );
        assert!(repo.detached);
        assert_eq!(repo.head, "ed036ba");
        assert_eq!(repo.upstream, None);
    }

    /// A conflict outranks everything, both on the file and on the folders above it.
    #[test]
    fn a_conflict_is_the_loudest_thing_in_a_folder() {
        let mut repo = Repo::default();
        parse_status(
            concat!(
                "1 .M N... 100644 100644 100644 aaa bbb sub/quiet.txt\0",
                "u UU N... 100644 100644 100644 100644 aaa bbb ccc sub/both.txt\0",
            )
            .as_bytes(),
            "",
            &mut repo,
        );
        assert_eq!(repo.conflicted, 1);
        assert_eq!(repo.state("sub"), Some(State::Conflicted));
        assert_eq!(repo.state("sub/quiet.txt"), Some(State::Modified));
    }

    /// A path with a space in it is why `-z` and a counted field split: the path is whatever is
    /// left, spaces and all.
    #[test]
    fn a_name_with_spaces_survives() {
        let mut repo = Repo::default();
        parse_status(
            "1 .M N... 100644 100644 100644 aaa bbb my notes v2.txt\0".as_bytes(),
            "",
            &mut repo,
        );
        assert_eq!(repo.state("my notes v2.txt"), Some(State::Modified));
    }

    /// **The real thing, against a repository this test builds.**
    ///
    /// Everything above reads captured output; this reads git. It is what catches an argument list
    /// that has gone stale, a `git` that is not on the `PATH`, and the two commands whose output is
    /// *not* checked in above — `rev-parse --show-prefix`, which decides what every path means, and
    /// `ls-tree`, which is where a committed file's tick comes from.
    ///
    /// Read-only, on a repository in the temp directory, with no remote and no network. The `git`
    /// invoked is whatever the developer has, which is the point.
    #[test]
    fn a_real_repository_answers_for_its_own_files() {
        let root = crate::sandbox::dir("git");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(root.join("sub")).expect("a temp folder");

        let run = |args: &[&str]| {
            let mut command = Command::new("git");
            command.args(args).current_dir(&root);
            crate::shell::no_window(&mut command);
            command
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        };
        // No git on this machine is not a test failure; there is nothing to test.
        if !run(&["init", "--quiet"]) {
            eprintln!("no git: skipping");
            return;
        }
        // A repository of this program's own making, so nothing here depends on the developer's
        // name, editor or signing key.
        assert!(run(&["config", "user.email", "test@example.invalid"]));
        assert!(run(&["config", "user.name", "Test"]));
        assert!(run(&["config", "commit.gpgsign", "false"]));

        std::fs::write(root.join("clean.txt"), b"one\n").unwrap();
        std::fs::write(root.join("changed.txt"), b"two\n").unwrap();
        std::fs::write(root.join("sub/deep.txt"), b"three\n").unwrap();
        assert!(run(&["add", "-A"]));
        assert!(run(&["commit", "--quiet", "-m", "base"]));

        // One of each, from here on: a change on disk, a staged addition, an untracked file, and a
        // change one folder down.
        std::fs::write(root.join("changed.txt"), b"two and more\n").unwrap();
        std::fs::write(root.join("staged.txt"), b"four\n").unwrap();
        assert!(run(&["add", "staged.txt"]));
        std::fs::write(root.join("untracked.txt"), b"five\n").unwrap();
        std::fs::write(root.join("sub/deep.txt"), b"three and more\n").unwrap();

        let repo = read(&root).expect("the folder is a repository");
        assert!(!repo.head.is_empty(), "on some branch");
        assert_eq!(repo.upstream, None, "nowhere to push to");
        assert_eq!(repo.changed, 4, "changed, staged, untracked, and one below");
        assert_eq!(repo.state("clean.txt"), Some(State::Clean), "committed");
        assert_eq!(repo.state("changed.txt"), Some(State::Modified));
        assert_eq!(repo.state("staged.txt"), Some(State::Staged));
        assert_eq!(repo.state("untracked.txt"), Some(State::Untracked));
        assert_eq!(
            repo.state("sub"),
            Some(State::Modified),
            "a folder wears what is under it"
        );
        assert_eq!(repo.dot_git, root.join(".git"), "what to watch");
        assert!(repo.micros > 0, "and it was timed");

        // From inside the subfolder, the same repository answers about *its* names.
        let below = read(&root.join("sub")).expect("still a repository");
        assert_eq!(below.state("deep.txt"), Some(State::Modified));
        assert_eq!(below.dot_git, root.join(".git"), "the same repository");
        assert_eq!(below.changed, 4, "and the same counts");

        // The gate in front of all of it: this folder has a `.git`, so it is worth spawning git for.
        // The negative is deliberately not asserted here — whether the temp directory happens to sit
        // inside somebody's repository is not this test's to know.
        assert!(under_a_git_dir(&root.join("sub")));

        crate::sandbox::remove(&root);
    }

    /// What one folder's worth of git actually costs, on whatever repository this is run in.
    ///
    /// ```text
    /// cargo test --release -- --ignored --nocapture what_a_git_query_costs
    /// ```
    ///
    /// Three processes, and on Windows the startup is most of it. The number worth watching is not
    /// this one but the one beside it: a folder with no `.git` above it, which is what browsing a disk
    /// costs and which must be indistinguishable from zero.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn what_a_git_query_costs() {
        let here = std::env::current_dir().expect("a working directory");
        for round in 0..5 {
            let start = std::time::Instant::now();
            let repo = read(&here);
            let took = start.elapsed();
            match repo {
                Some(repo) => println!(
                    "round {round}: {:.1} ms for {} paths, branch {}",
                    took.as_secs_f64() * 1000.0,
                    repo.changed,
                    repo.head
                ),
                None => println!("round {round}: not a repository"),
            }
        }
        let nowhere = std::env::temp_dir();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            let _ = under_a_git_dir(&nowhere);
        }
        println!(
            "the gate: {:.1} µs per folder with no repository above it",
            start.elapsed().as_secs_f64() * 1e6 / 100.0
        );
    }

    /// Which states there is a `HEAD` version to compare against.
    ///
    /// The two `false`s worth stating are the ones that look like changes: an **untracked** file and a
    /// **renamed** one both differ from what is committed, and neither has a path `HEAD` can be asked
    /// about — so a comparison against one would be a comparison against nothing.
    #[test]
    fn only_some_kinds_of_change_have_something_to_compare_with() {
        for state in [State::Staged, State::Modified, State::Conflicted] {
            assert!(state.differs_from_head(), "{state:?} has a HEAD version");
        }
        for state in [
            State::Clean,
            State::Untracked,
            State::Renamed,
            State::Deleted,
        ] {
            assert!(
                !state.differs_from_head(),
                "{state:?} was offered a comparison it cannot have"
            );
        }
    }

    /// What `HEAD` has of a file, which is what a picture is compared against.
    ///
    /// The point of the test is the *version*: `blob` has to come back with what was committed and
    /// not with what is on disk, or the comparison would be a picture against itself.
    #[test]
    fn what_head_has_of_a_file() {
        // Its own folder, not [`a_real_repository_answers_for_its_own_files`]'s: two tests in one
        // process share a directory name at their peril.
        let root = crate::sandbox::dir("blob");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("a temp folder");

        let run = |args: &[&str]| {
            let mut command = Command::new("git");
            command.args(args).current_dir(&root);
            crate::shell::no_window(&mut command);
            command
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        };
        if !run(&["init", "--quiet"]) {
            eprintln!("no git: skipping");
            return;
        }
        assert!(run(&["config", "user.email", "test@example.invalid"]));
        assert!(run(&["config", "user.name", "Test"]));
        assert!(run(&["config", "commit.gpgsign", "false"]));

        // Bytes rather than text, because that is what this is for: a picture is not lines.
        std::fs::write(root.join("kept.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(root.join("moved.bin"), [9u8, 9]).unwrap();
        assert!(run(&["add", "-A"]));
        assert!(run(&["commit", "--quiet", "-m", "base"]));
        std::fs::write(root.join("moved.bin"), [7u8, 7, 7]).unwrap();
        std::fs::write(root.join("new.bin"), [5u8]).unwrap();

        assert_eq!(
            blob(&root.join("kept.bin")).as_deref(),
            Some(&[0u8, 1, 2, 3][..]),
            "an unchanged file is itself"
        );
        assert_eq!(
            blob(&root.join("moved.bin")).as_deref(),
            Some(&[9u8, 9][..]),
            "the committed version, not the one on disk"
        );
        assert_eq!(
            blob(&root.join("new.bin")),
            None,
            "HEAD has no such path, so there is nothing to compare with"
        );
        assert_eq!(blob(&root.join("gone.bin")), None, "nor of a file at all");

        // **And it is the file rather than the object**, which is the whole reason this runs
        // `cat-file --filters` and not `show`. Eol conversion stands in for Git LFS here: both are
        // filters configured per path, both make the stored object something other than the bytes a
        // checkout would write, and this one needs no external program to set up. `show` would answer
        // with the `\n` version — which for a picture is every pixel row after the first shifted by
        // however many `\r`s were in front of it.
        std::fs::write(root.join(".gitattributes"), b"*.crlf text eol=crlf\n").unwrap();
        std::fs::write(root.join("wrapped.crlf"), b"one\r\ntwo\r\n").unwrap();
        assert!(run(&["add", "-A"]));
        assert!(run(&["commit", "--quiet", "-m", "with an attribute"]));
        assert_eq!(
            blob(&root.join("wrapped.crlf")).as_deref(),
            Some(&b"one\r\ntwo\r\n"[..]),
            "the bytes a checkout would write, not the ones the object holds"
        );

        crate::sandbox::remove(&root);
    }

    /// Real `git diff -U0` output, from a file whose eight lines were changed in all three ways: one
    /// line replaced, two inserted, two taken away.
    ///
    /// `a b c d e f g h` became `a B c NEW1 NEW2 d e h`.
    const DIFFED: &str = concat!(
        "diff --git a/lines.txt b/lines.txt\n",
        "index 71ac1b5..5c16a65 100644\n",
        "--- a/lines.txt\n",
        "+++ b/lines.txt\n",
        "@@ -2 +2 @@ a\n",
        "-b\n",
        "+B\n",
        "@@ -3,0 +4,2 @@ c\n",
        "+NEW1\n",
        "+NEW2\n",
        "@@ -6,2 +7,0 @@ e\n",
        "-f\n",
        "-g\n",
    );

    #[test]
    fn a_diff_says_which_lines_are_new_and_what_the_old_ones_were() {
        let changes = parse_diff(DIFFED.as_bytes());
        assert_eq!(changes.hunks.len(), 3);

        // One line replaced by one: line 2 is new, and `b` was there before it.
        let one = &changes.hunks[0];
        assert_eq!((one.added, one.added_count), (2, 1));
        assert_eq!(one.removed, vec!["b".to_owned()]);
        assert_eq!(one.after, 1, "shown in front of the line that replaced it");
        assert_eq!(one.removed_at, 2);

        // Two lines inserted and nothing removed.
        let two = &changes.hunks[1];
        assert_eq!((two.added, two.added_count), (4, 2));
        assert!(two.removed.is_empty());

        // Two lines removed and nothing added — `+7,0` means they sat after line 7.
        let three = &changes.hunks[2];
        assert_eq!(three.added_count, 0);
        assert_eq!(three.after, 7);
        assert_eq!(three.removed, vec!["f".to_owned(), "g".to_owned()]);
        assert_eq!(three.removed_at, 6);
    }

    /// The file's own name is not a removed line, however much it looks like one.
    #[test]
    fn the_diff_header_is_not_read_as_content() {
        let changes = parse_diff(DIFFED.as_bytes());
        let removed: Vec<&String> = changes.hunks.iter().flat_map(|h| &h.removed).collect();
        assert!(
            !removed.iter().any(|line| line.starts_with("- a/")),
            "the `--- a/lines.txt` header came through as content: {removed:?}"
        );
    }

    /// A file git has nothing to say about, and one it cannot diff.
    #[test]
    fn nothing_to_report_is_no_hunks_rather_than_no_answer() {
        assert!(parse_diff(b"").is_empty());
        assert!(parse_diff(b"Binary files a/x.png and b/x.png differ\n").is_empty());
    }

    /// A removed line is drawn among the file's own, so it is spelled the way the file's lines are.
    #[test]
    fn a_removed_line_is_laid_out_like_the_rest() {
        let changes = parse_diff(b"@@ -1 +1 @@\n-\tindented\r\n+    indented\n");
        assert_eq!(changes.hunks[0].removed, vec!["    indented".to_owned()]);
    }

    /// The tracked names from `ls-tree` only fill in what `status` said nothing about: a file that is
    /// both tracked and changed keeps the change.
    #[test]
    fn a_tick_never_overwrites_a_change() {
        let mut repo = at("");
        for name in ["added.txt", "modified.txt", "quiet.txt"] {
            repo.marks.entry(name.to_owned()).or_insert(State::Clean);
        }
        assert_eq!(repo.state("modified.txt"), Some(State::Modified));
        assert_eq!(repo.state("added.txt"), Some(State::Staged));
        assert_eq!(repo.state("quiet.txt"), Some(State::Clean));
    }
}
