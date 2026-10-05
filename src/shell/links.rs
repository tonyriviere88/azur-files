//! What a shortcut points at, found without the listing waiting for it.
//!
//! Two kinds of row are a shortcut, and they are unrelated to each other:
//!
//! - **A `.lnk` file**, which is the shell's own shortcut and the one people mean. Its target
//!   is not in the file system at all, it is in the file's contents, and reading it means
//!   `IShellLink` — a COM object per file.
//! - **A reparse point**, which is a symlink or a junction. Its target is metadata and comes
//!   back from one syscall.
//!
//! Both are asked about the same way and answered into the same column, because from the row's
//! point of view they are one thing: a name that stands for somewhere else.
//!
//! # Why this is a service and not a function
//!
//! [`crate::fs`]'s rule is that the listing never goes back to the disk for something the
//! enumeration already told it — and a shortcut's target is the one thing on a row that the
//! enumeration cannot tell it. So this is the exception, built the way the other exception is
//! ([`crate::shell::icons`]): asked once per row per view, answered on a worker, delivered to
//! the tab that asked, and dropped if that tab has moved on.
//!
//! It has to be off the UI thread for the same reason the per-file icons do. A `.lnk` pointing
//! at a share that is not currently reachable is the classic Explorer hang, and `IPersistFile
//! ::Load` is where it happens. Nothing here is allowed to make the window wait.
//!
//! `SLGP_RAWPATH` is the other half of that: it returns the path the shortcut *stores* rather
//! than asking the shell to find where the target has moved to, which is what makes a stale
//! shortcut cost a local file read instead of a network timeout. The cost is that a shortcut
//! whose target has moved shows where it used to be — which is the honest answer to "what does
//! this point at", and the same one `Properties` shows.

use std::path::{Path, PathBuf};

#[cfg(windows)]
#[path = "../windows/links.rs"]
mod win;
#[cfg(windows)]
use win::read_shortcut;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// What a shortcut leads to, as a row shows it.
///
/// The arguments are their own field rather than part of the path, because the two are read in
/// different places for different reasons: the tooltip names them separately — a path and a
/// command line are not one string — and only [`Target::line`] runs them together, for the one
/// line of dimmed text the Name column has room for.
///
/// A reparse point never has any: a junction leads somewhere, it does not run anything.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Target {
    /// The path the shortcut stores. Never empty — a shortcut with nothing to show is `None`
    /// rather than a `Target` with an empty path.
    pub path: String,
    /// The command line the target is run with, empty when there is none.
    pub arguments: String,
}

impl Target {
    /// The one line a row's dimmed half shows: where it points, and what it runs it with.
    ///
    /// Borrowed for the ordinary case, which is a shortcut with no arguments — the allocation is
    /// only for the ones that have something more to say.
    pub fn line(&self) -> std::borrow::Cow<'_, str> {
        if self.arguments.is_empty() {
            std::borrow::Cow::Borrowed(&self.path)
        } else {
            std::borrow::Cow::Owned(format!("{} {}", self.path, self.arguments))
        }
    }
}

/// Everything a `.lnk` is read for at once, which is one COM object's worth of answers.
///
/// [`Target`] is the display half of it; `folder` is the half [`folder_target`] wants and the
/// display never shows.
pub(crate) struct Shortcut {
    pub target: String,
    pub arguments: String,
    /// Whether the attributes the shortcut stores say its target is a directory.
    pub folder: bool,
}

/// Which kind of shortcut a row is, which is what decides how its target is found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A `.lnk` file: `IShellLink`, on a thread that is allowed to block.
    Shortcut,
    /// A symlink or a junction: one syscall.
    Reparse,
}

/// What kind of shortcut, if any, a row is — from the enumeration alone, so this costs nothing
/// to ask about every row of every listing.
///
/// `.url` files are deliberately not included: an internet shortcut's target is a URL rather
/// than a path, it is an `.ini` file rather than a shell object, and a listing is not a browser.
pub fn kind_of(ext: &str, is_reparse: bool) -> Option<Kind> {
    if is_reparse {
        Some(Kind::Reparse)
    } else if ext.eq_ignore_ascii_case("lnk") {
        Some(Kind::Shortcut)
    } else {
        None
    }
}

struct Job {
    view: u64,
    row: u32,
    path: PathBuf,
    kind: Kind,
}

struct Ready {
    view: u64,
    row: u32,
    /// `None` when there is nothing to show: an unreadable shortcut, one pointing at a shell
    /// folder with no path behind it, or a platform where this does not apply.
    target: Option<Target>,
}

/// The shortcut-target service. One per application.
pub struct Links {
    /// The worker, started on the first request — a window that never opens a folder of
    /// shortcuts never starts a thread.
    jobs: Option<Sender<Job>>,
    answers: Receiver<Ready>,
    /// Kept so the worker can be started later with a live channel to answer on.
    replies: Sender<Ready>,
    /// How many requests are outstanding, so a folder of ten thousand shortcuts cannot queue
    /// ten thousand of them. A row that is turned away is simply asked again next frame.
    queued: Arc<AtomicUsize>,
    ctx: egui::Context,
}

/// The most requests that may be in flight at once.
///
/// Only the rows on screen are ever asked about, so this is reached by scrolling fast rather
/// than by opening a big folder. Turning a request away costs nothing: the row draws without
/// its context for one frame and asks again.
const QUEUE_CAP: usize = 64;

impl Links {
    pub fn new(ctx: &egui::Context) -> Self {
        let (replies, answers) = channel();
        Self {
            jobs: None,
            answers,
            replies,
            queued: Arc::new(AtomicUsize::new(0)),
            ctx: ctx.clone(),
        }
    }

    /// Ask what the shortcut at `path` points at. `false` if it was turned away.
    ///
    /// `view` and `row` are how the answer finds its way back: the tab's current view of its
    /// current folder, and the entry index within it. An answer to a view nobody holds any more
    /// is dropped on delivery, so nothing about a folder outlives looking at it.
    pub fn request(&mut self, view: u64, row: u32, path: PathBuf, kind: Kind) -> bool {
        if self.queued.load(Ordering::Relaxed) >= QUEUE_CAP {
            return false;
        }
        let jobs = self.worker().clone();
        self.queued.fetch_add(1, Ordering::Relaxed);
        if jobs
            .send(Job {
                view,
                row,
                path,
                kind,
            })
            .is_err()
        {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// Everything resolved since the last call, as `(view, row, target)`. Drained.
    pub fn answers(&mut self) -> Vec<(u64, u32, Option<Target>)> {
        self.answers
            .try_iter()
            .map(|ready| (ready.view, ready.row, ready.target))
            .collect()
    }

    /// The worker, started on demand.
    ///
    /// One thread with a queue rather than a thread per file, for the reason
    /// [`crate::shell::icons`] gives at length: a folder of shortcuts scrolled quickly would
    /// otherwise be a thread per row, each with its own stack.
    fn worker(&mut self) -> &Sender<Job> {
        let replies = self.replies.clone();
        let queued = self.queued.clone();
        let ctx = self.ctx.clone();
        self.jobs.get_or_insert_with(|| {
            let (send, receive) = channel::<Job>();
            let spawned = std::thread::Builder::new()
                .name("shell-links".to_owned())
                .spawn(move || work(receive, replies, queued, ctx));
            // A machine that will not give us a thread gets rows without their context, which
            // is what they looked like before this existed.
            let _ = spawned;
            send
        })
    }
}

/// Resolve until the channel closes, which is when the application is gone.
fn work(jobs: Receiver<Job>, replies: Sender<Ready>, queued: Arc<AtomicUsize>, ctx: egui::Context) {
    apartment();
    // Any read can touch an empty removable drive, which would otherwise raise "Please insert
    // a disk into drive E:" from inside the syscall.
    crate::fs::scan::silence_device_dialogs();

    while let Ok(job) = jobs.recv() {
        let target = match job.kind {
            Kind::Shortcut => shortcut_target(&job.path),
            Kind::Reparse => reparse_target(&job.path),
        };
        queued.fetch_sub(1, Ordering::Relaxed);
        if replies
            .send(Ready {
                view: job.view,
                row: job.row,
                target,
            })
            .is_err()
        {
            return;
        }
        ctx.request_repaint();
    }
}

/// Join the multi-threaded apartment, once, for the life of the thread.
///
/// **Not a single-threaded one**, which is the interesting choice and the one this program's own
/// rule decides: an STA is a promise to answer calls back into it, and a worker parked in
/// `recv` answers nothing — see [`crate::shell::answering_calls`] for what that breaks. Nothing
/// here hands an interface out, so there is nothing to be called back about, and the MTA needs
/// no message pump. `ShellLink` is registered as `Both`, so it is created in this apartment
/// rather than marshalled into one.
///
/// Never uninitialised, because the thread lives as long as the process does.
fn apartment() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        // SAFETY: called once, on this thread, before any COM call on it.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }
}

/// What a `.lnk` points at and what it runs it with.
fn shortcut_target(path: &Path) -> Option<Target> {
    read_shortcut(path).map(|link| Target {
        path: link.target,
        arguments: link.arguments,
    })
}

/// **The folder a shortcut leads to, if it leads to one.**
///
/// This is what makes a folder shortcut open *in this window* rather than in Explorer, and it
/// is the one thing in this module that runs on the caller's thread — the UI thread, at the
/// moment a row is opened. Three reasons that is the right way round here, where it would not
/// be for the display:
///
/// - **It has to be an answer, not an answer later.** Double-clicking a shortcut has to do the
///   same thing every time. Reaching for the column [`Links`] fills in would mean the gesture
///   depended on whether the row had been on screen long enough, which is the kind of
///   intermittent that is never reported and never fixed.
/// - **It is one small local read**, of a file in the folder already being listed, and only for
///   a row whose name ends in `.lnk`. The alternative on that path is `ShellExecute`, which
///   does considerably more on the same thread.
/// - **Nothing is asked of the *target*.** Whether it is a folder comes from the attributes the
///   shortcut itself stores, so a shortcut to a share that is not currently reachable costs
///   nothing to classify. A shortcut whose stored attributes have gone stale — the target
///   replaced by a file since — falls through to the shell, which is what used to happen to all
///   of them.
///
/// `None` for anything this cannot answer: not a `.lnk`, a shortcut to a file, or one pointing
/// at a shell object with no path at all — a library, the Recycle Bin, Control Panel. Those go
/// to the shell, which is the only thing that can open them.
pub fn folder_target(path: &Path) -> Option<PathBuf> {
    // The extension first, so nothing but a shortcut ever costs a COM object.
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
    {
        return None;
    }
    let link = read_shortcut(path)?;
    link.folder.then(|| PathBuf::from(link.target))
}

#[cfg(not(windows))]
fn read_shortcut(_path: &Path) -> Option<Shortcut> {
    None
}

/// Where a symlink or a junction leads.
fn reparse_target(path: &Path) -> Option<Target> {
    let target = std::fs::read_link(path).ok()?;
    let text = target.to_string_lossy();
    // A junction stores `\\?\C:\…`. The prefix is there to get the path past the Win32 parser
    // and means nothing to a person reading a row.
    Some(Target {
        path: text
            .strip_prefix(r"\\?\")
            .unwrap_or(&text)
            .trim_end_matches('\\')
            .to_owned(),
        // Nothing is run: a reparse point is a place, not a command.
        arguments: String::new(),
    })
}

/// Write a `.lnk` at `at` pointing at `target`, with `arguments` as its command line — `""` for
/// none. `false` if the shell refused.
///
/// The only way to have a real shortcut to resolve, which is what the tests want it for: a fixture
/// checked into the repository would be a binary blob nobody could read, and one from `C:\Users` is
/// not the same on two machines.
pub fn write_shortcut(at: &Path, target: &Path, arguments: &str) -> bool {
    #[cfg(windows)]
    {
        // The writing thread needs an apartment as much as the reading one does, and asking for one
        // it already has is not a mistake: `CoInitializeEx` with a *different* model answers
        // `RPC_E_CHANGED_MODE` and initialises nothing, which is exactly what should happen on the
        // operation thread — that one is an STA of its own making and uninitialises itself. So this
        // is the links worker's MTA when it is the caller, and a no-op when it is not.
        apartment();
        win::write_shortcut(at, target, arguments)
    }
    #[cfg(not(windows))]
    {
        let _ = (at, target, arguments);
        false
    }
}

/// Make a shortcut in `into` for each of `items`, and say where each one landed.
///
/// What [`crate::shell::ops::Job::Link`] runs, on the operation's own thread.
///
/// # The names are the shell's, and they were measured rather than chosen
///
/// `the_names_match_the_shell_s_own_link_drop` hands the destination folder's *own* `IDropTarget` a
/// `DROPEFFECT_LINK` drop — the call Explorer makes for an Alt-drag — and compares what appears with
/// what this produces. Measured on this machine:
///
/// | dropped | appeared | and again |
/// | --- | --- | --- |
/// | `one.txt` | `one.txt.lnk` | `one.txt (2).lnk` |
/// | `a folder` | `a folder.lnk` | `a folder (2).lnk` |
///
/// So: **the whole name including its extension, with `.lnk` on the end** — not the stem, and with
/// no ` - Shortcut` suffix. That suffix is real but it belongs elsewhere: to the context menu's
/// *Create shortcut* and to a drop on the Desktop, neither of which is this gesture. A collision
/// takes ` (2)` before the extension, counting up.
///
/// # Why this rather than handing the drop to the shell
///
/// Because that route reports nothing back. It works — the test named above is the proof, since it
/// uses it — but *what* it created is not knowable afterwards, and an operation this program cannot
/// describe is one Ctrl+Z cannot take back. Made here, every path is known as it is written, so a
/// link drop goes into [`crate::shell::ops::history`] beside the copy and the move. The cost is that
/// the naming rule above is this program's, which is why it is held to the shell's by a test rather
/// than by this comment.
pub fn shortcuts_into(items: &[PathBuf], into: &Path) -> (Option<String>, Vec<PathBuf>) {
    let mut made = Vec::new();
    let mut refused = 0usize;
    for item in items {
        // **A path with no last component is counted as refused rather than skipped**, and the case
        // is a drive root: `C:\` has no `file_name`, so there is no name to put a `.lnk` after.
        // Explorer names that one after the volume label — `Local Disk (C:).lnk` — which is a rule
        // this cannot check against anything, and inventing one is the thing the rest of this
        // function is at pains not to do. So it is reported instead of guessed at, and a drag of a
        // drive says so rather than quietly doing nothing.
        let name = item.file_name().map(|name| name.to_string_lossy());
        match name.and_then(|name| free_name(into, &name)) {
            Some(at) if write_shortcut(&at, item, "") => made.push(at),
            _ => refused += 1,
        }
    }
    // Reported per item rather than as one failure, because a partial answer is what the user has
    // to be told about: the shortcuts that *were* made are real, are on screen, and are what Ctrl+Z
    // will take back. Silence here would leave somebody counting rows to find the one that is
    // missing.
    let error = match (refused, made.is_empty()) {
        (0, _) => None,
        (_, true) if items.len() == 1 => Some("Could not make the shortcut".to_owned()),
        (_, true) => Some("Could not make the shortcuts".to_owned()),
        (n, false) => Some(format!("{n} of {} shortcuts could not be made", items.len())),
    };
    (error, made)
}

/// `into\name.lnk`, or `into\name (2).lnk` and up until one is free.
///
/// `None` when the folder somehow holds every name this will try, which is not a real folder and is
/// not worth spinning over — see [`TRIES`].
fn free_name(into: &Path, name: &str) -> Option<PathBuf> {
    let first = into.join(format!("{name}.lnk"));
    if !first.exists() {
        return Some(first);
    }
    // From two, because the first one has no number: the shell's own `one.txt.lnk` is followed by
    // `one.txt (2).lnk` and never by `one.txt (1).lnk`.
    (2..=TRIES)
        .map(|n| into.join(format!("{name} ({n}).lnk")))
        .find(|candidate| !candidate.exists())
}

/// How far [`free_name`] counts before giving up.
///
/// A bound rather than a loop, because the alternative is a folder that has been dropped into a
/// thousand times freezing the operation thread on `exists` calls. A thousand shortcuts to the same
/// file is already past anything intentional.
const TRIES: usize = 1000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_lnk_or_a_reparse_point_is_a_shortcut() {
        assert_eq!(kind_of("lnk", false), Some(Kind::Shortcut));
        // Extensions are stored as they were on disk, so the comparison cannot be `==`.
        assert_eq!(kind_of("LNK", false), Some(Kind::Shortcut));
        assert_eq!(kind_of("Lnk", false), Some(Kind::Shortcut));
        assert_eq!(kind_of("", true), Some(Kind::Reparse));
        // A reparse point wins: a `.lnk` that is also a symlink is asked about as the symlink,
        // which is the answer that says why the row is not where you expect it to be.
        assert_eq!(kind_of("lnk", true), Some(Kind::Reparse));
        assert_eq!(kind_of("txt", false), None);
        assert_eq!(kind_of("", false), None);
        // Not a shortcut as far as this is concerned — see `kind_of`.
        assert_eq!(kind_of("url", false), None);
    }

    /// A real shortcut, written and then read back: the folder one opens here, the file one
    /// does not.
    ///
    /// The point of the whole thing. A shortcut to a folder used to be handed to the shell,
    /// which opened a second file manager over the top of this one.
    #[cfg(windows)]
    #[test]
    fn a_shortcut_to_a_folder_resolves_to_the_folder_and_one_to_a_file_does_not() {
        let root = crate::sandbox::dir("lnk");
        let folder = root.join("somewhere");
        let file = root.join("something.txt");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&folder).expect("a directory in the temp folder");
        std::fs::write(&file, b"x").expect("a file in it");

        let to_folder = root.join("folder.lnk");
        let to_file = root.join("file.lnk");
        assert!(
            write_shortcut(&to_folder, &folder, "") && write_shortcut(&to_file, &file, "/x \"a b\""),
            "the shell would not write a shortcut here"
        );

        assert_eq!(
            folder_target(&to_folder).as_deref(),
            Some(folder.as_path()),
            "a shortcut to a folder has to resolve to that folder"
        );
        assert_eq!(
            folder_target(&to_file),
            None,
            "a shortcut to a file is the shell's business, not a place to navigate to"
        );
        // And the display half agrees about where both of them point — and says what the
        // shortcut runs it with, which is the difference between two `.lnk`s to the same
        // program that do different things.
        let shown = shortcut_target(&to_file).expect("a shortcut to a file has a target");
        assert_eq!(shown.path, file.to_string_lossy());
        assert_eq!(shown.arguments, "/x \"a b\"");
        assert_eq!(
            shown.line(),
            format!("{} /x \"a b\"", file.to_string_lossy()),
            "the Name column's dimmed half is the target and the command line, in that order"
        );
        // The one with nothing to run says so with an empty string rather than with a space at
        // the end of its line.
        let plain = shortcut_target(&to_folder).expect("a shortcut to a folder has a target");
        assert_eq!(plain.arguments, "");
        assert_eq!(plain.line(), folder.to_string_lossy());

        // Nothing that is not a shortcut costs a COM object, which is what the extension test
        // in `folder_target` is for.
        assert_eq!(folder_target(&file), None);
        assert_eq!(folder_target(&folder), None);
        crate::sandbox::remove(&root);
    }

    /// A real reparse point, where the machine allows one to be made.
    ///
    /// Creating a symlink needs either Developer Mode or an elevated process, so this skips
    /// rather than fails where neither is true — and says so, because a test that quietly does
    /// nothing is worse than one that is not there. The junctions this is really for
    /// (`C:\Users\All Users`) cannot be made without shelling out to `mklink`, which a test has
    /// no business doing; they are read by the same one syscall.
    #[test]
    fn a_symlink_reports_where_it_leads() {
        let root = crate::sandbox::dir("link");
        let target = root.join("target");
        let link = root.join("link");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&target).expect("a directory in the temp folder");

        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&target, &link);
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(&target, &link);

        if made.is_err() {
            println!("no privilege to create a symlink here; skipping");
            crate::sandbox::remove(&root);
            return;
        }
        let got = reparse_target(&link).expect("a symlink has a target");
        assert_eq!(
            got.path,
            target.to_string_lossy(),
            "the target came back as something else"
        );
        assert_eq!(got.arguments, "", "a place does not run anything");
        // And the enumeration agrees that the row is one, which is what asks the question.
        let dir = crate::fs::scan::scan(&root);
        let row = (0..dir.len())
            .find(|&i| dir.name(i) == "link")
            .expect("the link is in the listing");
        assert_eq!(
            kind_of(dir.ext(row), dir.entries[row].is_link()),
            Some(Kind::Reparse)
        );
        crate::sandbox::remove(&root);
    }

    /// A junction's target comes back without the prefix that gets a path past the parser.
    #[test]
    fn a_reparse_target_is_shown_the_way_a_person_writes_it() {
        // `read_link` is what produces these; this checks the tidying done to its answer, which
        // is the part that is this program's own.
        let tidy = |raw: &str| {
            let text = std::borrow::Cow::Borrowed(raw);
            text.strip_prefix(r"\\?\")
                .unwrap_or(&text)
                .trim_end_matches('\\')
                .to_owned()
        };
        assert_eq!(tidy(r"\\?\C:\ProgramData"), r"C:\ProgramData");
        assert_eq!(tidy(r"C:\ProgramData"), r"C:\ProgramData");
        assert_eq!(tidy(r"\\?\C:\Users\"), r"C:\Users");
        assert_eq!(tidy(r"\\server\share"), r"\\server\share");
    }
}

#[cfg(all(test, windows))]
mod link_tests {
    use super::*;

    /// **The names this makes are the shell's own, compared against the shell.**
    ///
    /// The rule in [`shortcuts_into`] — the whole file name, `.lnk` on the end, ` (2)` before it on
    /// a collision — is this program's arithmetic, and the only thing that makes it more than a
    /// guess is this: the same two items are dropped twice into one folder by
    /// [`crate::shell::dnd::win::link_drop_through_the_shell`], which is the call Explorer makes for
    /// an Alt-drag, and twice into another by [`shortcuts_into`]. The two folders then have to hold
    /// the same names.
    ///
    /// A file **and** a folder, because the two are named differently by every other operation here
    /// — `one.txt` has an extension to put the `.lnk` after, and `a folder` does not — and twice
    /// each, because the second time is what shows the collision rule.
    #[test]
    #[ignore = "fails on the GitHub Actions runner, passes on a desktop: compares against a link drop made by the real shell"]
    fn the_names_match_the_shell_s_own_link_drop() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();

        let root = crate::sandbox::fresh("link-names");
        let from = root.join("from");
        let ours = root.join("ours");
        let theirs = root.join("theirs");
        for dir in [&from, &ours, &theirs] {
            std::fs::create_dir_all(dir).expect("sandbox");
        }
        let file = from.join("one.txt");
        std::fs::write(&file, b"one").expect("write");
        let folder = from.join("a folder");
        std::fs::create_dir_all(&folder).expect("sandbox");
        let items = vec![file, folder];

        let names = |dir: &Path| -> Vec<String> {
            let mut found: Vec<String> = std::fs::read_dir(dir)
                .expect("read the folder back")
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            found.sort();
            found
        };

        // Twice each, so the collision rule is exercised as well as the plain one.
        for round in 1..=2 {
            let (error, made) = shortcuts_into(&items, &ours);
            assert_eq!(error, None, "round {round}");
            assert_eq!(made.len(), 2, "round {round}: {made:?}");
            for at in &made {
                assert!(at.is_file(), "round {round}: {} was not written", at.display());
            }

            assert!(
                crate::shell::dnd::win::link_drop_through_the_shell(&items, &theirs),
                "round {round}: the shell would not take a link drop, so there is nothing to \
                 compare against"
            );
            // The shell's drop is asynchronous in a way `shortcuts_into` is not: `Drop` returns
            // before the `.lnk` is necessarily on disk, exactly as `NewItem` does.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while names(&theirs).len() < round * 2 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }

        assert_eq!(
            names(&ours),
            names(&theirs),
            "the shortcut names have drifted from the shell's own"
        );
        // And what they are, written down, so a change in either is legible in the diff rather
        // than only in a mismatch.
        assert_eq!(
            names(&ours),
            [
                "a folder (2).lnk",
                "a folder.lnk",
                "one.txt (2).lnk",
                "one.txt.lnk",
            ]
        );

        crate::sandbox::remove(&root);
    }

    /// A shortcut this makes resolves to the item it was made for — read back through the same
    /// interface a row uses, which is the only thing that makes it a shortcut rather than a file.
    #[test]
    fn a_shortcut_points_at_what_it_was_made_for() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();

        let root = crate::sandbox::fresh("link-target");
        let into = root.join("into");
        std::fs::create_dir_all(&into).expect("sandbox");
        let file = root.join("one.txt");
        std::fs::write(&file, b"one").expect("write");

        let (error, made) = shortcuts_into(std::slice::from_ref(&file), &into);
        assert_eq!(error, None);
        let [at] = &made[..] else {
            panic!("expected one shortcut, got {made:?}")
        };
        assert_eq!(at, &into.join("one.txt.lnk"));

        // Read back through `shortcut_target`, which is exactly what fills a row's dimmed half —
        // so this checks the shortcut against the thing that will display it.
        let target = shortcut_target(at).expect("the shortcut should resolve");
        assert_eq!(Path::new(&target.path), file, "it points somewhere else");
        assert!(
            target.arguments.is_empty(),
            "a shortcut made by dragging runs nothing: {:?}",
            target.arguments
        );

        crate::sandbox::remove(&root);
    }

    /// Nothing to make a shortcut *of* is not an error, and nothing is written.
    #[test]
    fn no_items_makes_no_shortcuts() {
        let root = crate::sandbox::fresh("link-none");
        let (error, made) = shortcuts_into(&[], &root);
        assert_eq!(error, None);
        assert!(made.is_empty());
        assert_eq!(std::fs::read_dir(&root).into_iter().flatten().count(), 0);
        crate::sandbox::remove(&root);
    }

    /// An item that cannot be named is **said** to have been refused, not passed over.
    ///
    /// A drive root is the case — see [`shortcuts_into`]. What matters here is that the count comes
    /// back rather than the folder quietly staying empty, because a drag that appears to do nothing
    /// is indistinguishable from one that is not wired up.
    #[test]
    fn an_item_with_no_name_is_reported_rather_than_skipped() {
        let root = crate::sandbox::fresh("link-unnamed");
        let (error, made) = shortcuts_into(&[PathBuf::from(r"C:\")], &root);
        assert!(made.is_empty(), "a drive root has no name to make a .lnk from");
        assert_eq!(error.as_deref(), Some("Could not make the shortcut"));
        crate::sandbox::remove(&root);
    }

    /// The collision rule on its own, without the shell: the first has no number, and the count
    /// starts at two.
    #[test]
    fn a_free_name_counts_from_two() {
        let root = crate::sandbox::fresh("link-free");
        assert_eq!(free_name(&root, "one.txt"), Some(root.join("one.txt.lnk")));
        std::fs::write(root.join("one.txt.lnk"), b"").expect("write");
        assert_eq!(
            free_name(&root, "one.txt"),
            Some(root.join("one.txt (2).lnk")),
            "the second one is (2), never (1)"
        );
        std::fs::write(root.join("one.txt (2).lnk"), b"").expect("write");
        assert_eq!(free_name(&root, "one.txt"), Some(root.join("one.txt (3).lnk")));
        crate::sandbox::remove(&root);
    }
}
