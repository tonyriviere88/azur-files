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
/// Test-only, and the only way to have a real shortcut to resolve: a fixture checked into the
/// repository would be a binary blob nobody could read, and one from `C:\Users` is not the
/// same on two machines. It uses the same two interfaces the reading does, from the other end.
#[cfg(all(test, windows))]
pub(crate) fn write_shortcut(at: &Path, target: &Path, arguments: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    fn made(at: &[u16], target: &[u16], arguments: &[u16]) -> Option<()> {
        // SAFETY: every slice is a NUL-terminated local of the caller, alive for the calls.
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            link.SetPath(PCWSTR(target.as_ptr())).ok()?;
            // Set unconditionally: an empty command line is what a shortcut without one has,
            // and the call is the same either way.
            link.SetArguments(PCWSTR(arguments.as_ptr())).ok()?;
            let file: IPersistFile = link.cast().ok()?;
            file.Save(PCWSTR(at.as_ptr()), true).ok()?;
        }
        Some(())
    }

    // The writing thread needs an apartment as much as the reading one does.
    apartment();
    let wide = |path: &Path| -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    };
    let argv: Vec<u16> = arguments.encode_utf16().chain(Some(0)).collect();
    made(&wide(at), &wide(target), &argv).is_some()
}

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
