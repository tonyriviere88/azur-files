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
    /// The directory this repository keeps its `HEAD` and its index in — see [`git_dir`], which is
    /// **not always the `.git` beside the folder**.
    ///
    /// Watched while this folder is on screen, because **a commit changes the repository without
    /// touching the folder**: `git commit` in the console panel, a rebase in a terminal, a pull —
    /// every one of them leaves the working tree exactly as the listing last read it, so nothing that
    /// watches the *folder* would ever notice. Marks that survive their own commit are precisely the
    /// stale-overlay failure this design exists to avoid, so the one thing worth watching besides the
    /// folder is this.
    pub dot_git: PathBuf,
    /// What [`dot_git`](Repo::dot_git) held when this answer was made. See [`stamp`].
    ///
    /// The whole of how a change under `.git` worth re-reading is told from **this program's own
    /// write of the index**, which arrives through the same watch and is otherwise
    /// indistinguishable from a commit somebody else made.
    pub stamp: u64,
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
    /// For the tests in the modules that *ask* a `Repo` rather than read one — [`crate::pane::Lens`]'s,
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
    //
    // And read with a ceiling on it, because the file being bounded does not bound this — see
    // [`DIFF_CAP`].
    let out = finish_capped(
        start_git(
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
        ),
        DIFF_CAP,
    )?;
    Some(parse_diff(&out))
}

/// How much of a diff is read.
///
/// **The body a preview shows is capped and the diff of it was not**, which is the same oversight in
/// the other half of one feature. `git diff HEAD -U0` runs over the whole file whatever
/// [`crate::preview::TEXT_CAP`] the panel read of it, the output was buffered entire by
/// `wait_with_output`, `from_utf8_lossy` then had a second copy of it, and every removed line was kept
/// after that as a `String` of its own. A 500 MB text file that a build regenerates is about a
/// gigabyte of diff, so previewing one file could cost more memory than the whole of the rest of the
/// window.
///
/// Four times the body's cap, and the arithmetic is the previewed megabyte's own worst case: a
/// megabyte in which every line changed is that megabyte twice over — once as `-`, once as `+` — plus
/// one `@@` header per changed run, and `-U0` makes every isolated line a run of its own. Four times
/// leaves room for that and stops well short of trouble. Past it the diff is describing a part of the
/// file the panel is not showing anyway.
const DIFF_CAP: u64 = 4 * crate::preview::TEXT_CAP as u64;

/// The file as it would be **on disk** at the last commit — not as the object database holds it.
///
/// **For the files a diff of text cannot describe**, which today means pictures: two versions of a
/// `.png` have no lines to put a band behind, so the panel decodes both and shows them side by side
/// with the difference between them. See [`crate::preview::diff::against_head`].
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
    // The old side's count is parsed to check the field is a field and then thrown away — the removed
    // lines that follow are what say how many there were.
    let (old_at, _) = span_of(fields.next()?.strip_prefix('-')?)?;
    let (new_at, new_count) = span_of(fields.next()?.strip_prefix('+')?)?;
    Some(Hunk {
        added: new_at,
        added_count: new_count,
        after: if new_count == 0 {
            new_at
        } else {
            new_at.saturating_sub(1)
        },
        // **Not `with_capacity(old_count)`**, which reserves whatever number the header happens to
        // carry before a single line of the hunk has been read. `4000000000` in a header is ten
        // characters and 96 GB of reservation, and there is nothing to gain by trusting it: the lines
        // are pushed as they are parsed, so the vector grows to exactly what actually arrived.
        removed: Vec::new(),
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
        // Detached rather than joined, for the reason [`crate::loader::Loader::drop`] sets out: a
        // worker waiting on a `git status` cannot be interrupted, and that is seconds on a monorepo
        // and a redirector timeout on a repository that lives on a share which has gone away. The
        // channel and the `Context` a worker holds are its own clones and outlive this, which is why
        // there was never anything being dropped underneath it.
        self.workers.clear();
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
        dot_git: git_dir(&root),
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
    // **Last, and that is the point**: `status` above may have rewritten the index, so a stamp
    // taken before it would name a `.git` this answer is already out of date about — and the
    // notification that write raises would then read as somebody else's commit for ever after.
    repo.stamp = stamp(&repo.dot_git);
    Some(repo)
}

/// A fingerprint of `.git`'s direct children: their names, sizes and modification times.
///
/// **What a change under `.git` is compared against**, and the reason
/// [`crate::app::git::GIT_WRITE_SETTLE`] no longer guards that watch. `git::read` writes the
/// repository's index as a side effect of the question it was asked — see the note in [`read`] on
/// `--no-optional-locks` — and that write lands directly inside `.git`, where it is reported
/// exactly as a commit made in a terminal is. Telling the two apart used to be a *clock*: a change
/// arriving within two seconds of git's last answer was presumed to be our own echo and dropped.
///
/// Dropped, not deferred — which is the bug this replaces. A branch switched in the two seconds
/// after a folder was opened was thrown away with the echo, and nothing came back for it: the
/// branch on the status line stayed wrong until the folder was re-read for some other reason. And
/// the clock could not simply be extended into a delay instead, because re-applying our own echo
/// asks git again, which writes again — the loop `GIT_WRITE_SETTLE` was written to break.
///
/// A stamp answers the question the clock was standing in for. Our own write is stamped **into**
/// the answer it belongs to, so its echo compares equal and is ignored however late it arrives;
/// anything else — `HEAD` rewritten by a checkout, a ref by a commit or a fetch, the index by
/// somebody else's `git add`, `MERGE_MSG` and `REBASE_HEAD` by the operations that make them —
/// differs, at any distance from the last answer.
///
/// One `read_dir` of a folder with a dozen entries in it, on the UI thread, once per notification.
/// The subdirectories are not walked: `refs/` and `logs/` are entries here in their own right and
/// a ref added or removed changes the folder they are in, which is enough to say "ask again" — the
/// answer to every difference this can see is the same single `git status`.
///
/// Taken on [`Repo::dot_git`], which for a linked worktree or a submodule is the directory its
/// `.git` file *names* rather than the file — see [`git_dir`]. The file itself is still stamped, as
/// a file, for the case where that line cannot be followed: one `stat` instead of a listing, so
/// that such a repository does not fall into the one shape where "could not be read" would be both
/// readings' answer and every change would therefore compare equal.
///
/// `0` when neither can be read at all, which compares unequal to any real reading and so errs
/// towards asking again.
pub fn stamp(dot_git: &Path) -> u64 {
    use std::hash::{Hash, Hasher};

    let Ok(entries) = std::fs::read_dir(dot_git) else {
        return file_stamp(dot_git);
    };
    // Order-independent, because `read_dir` does not promise one and a folder whose entries came
    // back in a different order is not a folder that changed. Summed rather than hashed in
    // sequence, so the *set* is what is fingerprinted.
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        entry.file_name().hash(&mut hasher);
        if let Ok(meta) = entry.metadata() {
            meta.len().hash(&mut hasher);
            // A folder has no useful length, and on Windows its modification time moves when an
            // entry is added or removed — which is how a new branch under `refs/heads` is seen.
            if let Ok(at) = meta.modified() {
                at.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
                    .hash(&mut hasher);
            }
        }
        total = total.wrapping_add(hasher.finish());
    }
    // Never `0` for a `.git` that was read, so "could not be read" stays its own answer.
    total | 1
}

/// [`stamp`] for a `.git` that is a file: its size and modification time.
fn file_stamp(dot_git: &Path) -> u64 {
    use std::hash::{Hash, Hasher};

    let Ok(meta) = std::fs::symlink_metadata(dot_git) else {
        return 0;
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    meta.len().hash(&mut hasher);
    if let Ok(at) = meta.modified() {
        at.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .hash(&mut hasher);
    }
    hasher.finish() | 1
}

/// Where the repository rooted at `root` keeps the things a change to them is worth re-reading for:
/// `HEAD`, the index, `MERGE_MSG`, `refs/`.
///
/// `<root>/.git` for a plain repository, and the reason this function exists is that **for a linked
/// worktree and for a submodule that `.git` is a file**, holding one line — `gitdir: <path>` — that
/// names the directory instead. `git worktree add` makes one per worktree under the main
/// repository's `.git/worktrees/<name>`, and a submodule's lives under `.git/modules/<name>`.
///
/// Following it is what makes [`crate::watch`] work at all in a worktree, and the bug that says so
/// is worth writing down: `ReadDirectoryChangesW` takes a **directory**. Handed a file it opens the
/// handle and then refuses the read, silently — so the watch sat there armed at nothing, and a
/// branch switched in a worktree never reached the status line unless it happened to change a file
/// in the folder on screen, which is the *other* watch noticing. Which is to say the feature looked
/// like it worked, and did, for every repository that was not one of the two shapes this program's
/// author actually works in.
///
/// A relative `gitdir:` — which is what a submodule has — is relative to the folder holding the
/// file, and is left with its `..` in it rather than canonicalised: Windows resolves it on the way
/// into every call, and every comparison against this path is with another copy of *this* value.
/// `std::fs::canonicalize` would answer a `\\?\` path, which is a different string for the same
/// folder and one this program hands to the shell elsewhere.
///
/// Falls back to `<root>/.git` whenever the file cannot be read or says something else, which is the
/// answer this had before and no worse than it: a watch on a path that is not a directory reports
/// nothing, and everything else about the repository still works.
fn git_dir(root: &Path) -> PathBuf {
    let dot = root.join(".git");
    let Ok(meta) = std::fs::symlink_metadata(&dot) else {
        return dot;
    };
    // The common case, and no read at all: a directory is its own answer.
    //
    // The size cap is for the other one — this is about to be read as text, and a `.git` *file* is
    // one short line. Anything larger is not one, whatever it is.
    if meta.is_dir() || meta.len() > 4_096 {
        return dot;
    }
    let Ok(text) = std::fs::read_to_string(&dot) else {
        return dot;
    };
    let Some(named) = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))
        .map(|rest| Path::new(rest.trim()))
    else {
        return dot;
    };
    let joined = if named.is_absolute() {
        named.to_path_buf()
    } else {
        root.join(named)
    };
    // Git writes that line with forward slashes even on Windows. See [`crate::fs::normalize`],
    // which is what every other path in this program has been through by the time anything
    // compares one.
    crate::fs::normalize(&joined)
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

/// The same, for a command whose answer has no natural size: at most `cap` bytes of it, and the child
/// ended rather than waited for once that is reached.
///
/// **Killed and not drained**, which is the whole point: a child whose pipe fills up stops writing and
/// sits there, so reading the first `cap` bytes and then waiting for it to exit is a deadlock, and
/// draining the rest to be polite is paying for the gigabyte this exists to refuse.
///
/// What comes back is then a *prefix* of a text format, cut back to the last complete line — because
/// half an `@@` header parses as a different hunk rather than as no hunk at all. The exit status is
/// only asked for when the pipe reached its own end: git's failure is what says `HEAD` has no such
/// path, and a git this killed has failed for a reason of this program's own making.
fn finish_capped(child: Option<std::process::Child>, cap: u64) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut child = child?;
    let mut out = Vec::new();
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    // One byte past the cap, so an answer that lands exactly on it is not read as "there was more".
    let read = stdout.take(cap + 1).read_to_end(&mut out).is_ok();
    if out.len() as u64 > cap {
        out.truncate(cap as usize);
        let whole = out.iter().rposition(|byte| *byte == b'\n');
        out.truncate(whole.map_or(0, |at| at + 1));
        let _ = child.kill();
        let _ = child.wait();
        return Some(out);
    }
    let status = child.wait().ok()?;
    (read && status.success()).then_some(out)
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
        // `split_at_checked`, because `split_at(1)` panics on a record whose first character is more
        // than one byte long. Every record git writes begins with an ASCII byte it chose itself, so
        // this is a guard against a format nobody has seen rather than a fix for one — but a parser
        // that can panic on the output of a program it does not control is not worth the argument.
        let Some((kind, rest)) = record.split_at_checked(1) else {
            continue;
        };
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
                // `split_at_checked` for the same reason as in [`parse_status`], and here the shape
                // that would reach it is duller still: `split(' ')` over a value with two spaces in it
                // hands back an empty part, and `split_at(1)` on an empty string is a panic.
                let Some((sign, count)) = part.split_at_checked(1) else {
                    continue;
                };
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
mod tests;
