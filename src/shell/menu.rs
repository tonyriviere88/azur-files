//! Explorer's context menu content, read out and handed over to be drawn.
//!
//! The menu the user sees is this program's own — [`crate::ui::menu`] draws it with the
//! design system, at the pointer, keyboard-navigable, themed with everything else.
//! What is *in* it comes from the shell, so 7-Zip, TortoiseGit, Open With, Send To,
//! Properties and whatever else is installed are all there and all work.
//!
//! # How the content is obtained without showing a Win32 menu
//!
//! `IContextMenu::QueryContextMenu` does not display anything: it *populates an
//! `HMENU`*. So this creates one, lets the shell and every extension fill it, and then
//! reads it back out with `GetMenuItemInfoW` — labels, separators, disabled and checked
//! states, the default (bold) item, the accelerator text, the item bitmaps and the
//! submenus. `TrackPopupMenuEx` is never called, and no native menu ever appears.
//!
//! Two things make that read faithful rather than approximate:
//!
//! **Submenus have to be asked for.** An extension fills its submenu lazily, when the
//! menu is about to pop up, via `IContextMenu2::HandleMenuMsg(WM_INITMENUPOPUP)`. A
//! menu that is never shown never gets that message — so [`Live::fill`] sends it itself
//! before reading a submenu. Without it, Send To and New come back empty, which is the
//! usual way a re-drawn shell menu ends up looking finished and being broken.
//!
//! **Commands are invoked by verb where there is one.** `GetCommandString` gives the
//! canonical name — `open`, `copy`, `delete`, `properties` — which is stable and can be
//! used from any thread against a freshly obtained `IContextMenu`. That matters because
//! invoking a command can put up a dialog, which has to happen off the UI thread; see
//! [`crate::shell::Modal`]. Where an extension offers no canonical verb, the numeric id
//! is used instead, against a menu re-queried with identical flags and items so the
//! numbering is identical.
//!
//! # Why none of it happens in a frame
//!
//! `QueryContextMenu` shows nothing, so for a long time it was called during the frame
//! that opened the menu. It is also **slow**, and not only the first time. Measured on this
//! machine with a fairly ordinary set of extensions installed, by
//! `what_the_shell_menu_takes_to_build --nocapture`:
//!
//! | | `QueryContextMenu` | submenu prefill | reading the `HMENU` | bitmaps | verbs |
//! | --- | --- | --- | --- | --- | --- |
//! | a file, first of a session | 690 ms | 116 ms | 0.2 ms | 0.2 ms | 0.0 ms |
//! | a file, after that | 130–570 ms | 41–51 ms | 0.2 ms | 0.1 ms | 0.0 ms |
//! | a folder | 130–265 ms | 3 ms | 0.2 ms | 0.3 ms | 0.0 ms |
//!
//! Empty space is the cheapest of them and is the one case that got faster rather than slower:
//! it is the folder's *background* menu — a different shell object, see [`win::context_of`] —
//! and far fewer extensions register on it. `QueryContextMenu` there measures 55–60 ms, with
//! New's own fill costing 27–31 ms on the hover that opens it.
//!
//! So a right click froze the window for a sixth of a second at best and most of a second
//! at worst: the shell asks every installed extension to contribute, and each one goes to
//! the registry and the disk to decide what to offer. The spread is wide and it is not
//! ours — the same call on the same file measured 567 ms in one session and 140 ms in the
//! next — so the figures above are worth reading as an order of magnitude and not as a
//! benchmark. What is *not* wide is everything this file does with the answer: reading the
//! `HMENU` back and converting the item bitmaps come to under half a millisecond together,
//! every time, and are not worth moving anywhere.
//!
//! And those are the *cheap* menus. On an executable on a mapped network share the same call
//! takes twenty-four seconds, every time — one extension reading the whole file — which is
//! measured, and is the reason for the shape of [`Builder`]. Read the note there before
//! changing anything about how it is threaded.
//!
//! Two things follow, and both are here:
//!
//! **The query runs on a thread of its own, one per menu.** [`Builder`] gives each menu a
//! worker with an STA of its own and answers by channel, so the window keeps running frames
//! while the shell takes its time. Whatever an extension does on the way, it does it where no
//! frame is waiting for it — and a worker that is taking too long is *left*, rather than
//! becoming the thing every later menu queues behind. The menu itself is not drawn until the
//! answer is in — see [`crate::ui::menu`] for why it is not shown early and grown.
//!
//! **Submenus are filled when they are opened.** Prefilling all of them cost 41–116 ms on
//! a file — Open With alone was nearly all of it — for menus the user usually never opens.
//! [`Live`] keeps the `HMENU` and the `IContextMenu` alive for as long as the menu is on
//! screen, so a submenu can be filled on the hover that opens it.
//!
//! # What the first menu of a session is missing
//!
//! Some submenus populate themselves *after* `WM_INITMENUPOPUP` and are not finished when it
//! returns. Read immediately, Send To comes back with only `Desktop (create shortcut)` in it
//! and Include in library with the shell's own `Retrieving libraries...` placeholder. Read
//! from the *next* `IContextMenu` in the same process, both are complete — Send To with seven
//! entries — because the shell has cached its enumeration by then.
//!
//! Waiting does not fix it: measured, the same submenu re-read 300 ms and 1 s later, and
//! re-sent `WM_INITMENUPOPUP` a second and third time, gives the same short answer every
//! time. It is fixed at the moment that `IContextMenu` first initialised it. So the first
//! right click of a session has a short Send To and every one after it does not.
//!
//! Both ways out cost more than the symptom. Building a throwaway menu at startup to warm the
//! shell loads every installed extension into the process — 11 MB of private bytes, measured
//! by `what_the_shell_menu_costs` — whether or not anybody ever right-clicks. Warming on the
//! first menu instead doubles the wait for that one menu. Neither is worth it for one short
//! submenu once per run, so this is left as it is, and written down.

use std::path::{Path, PathBuf};

/// One of this program's own entries, which the shell knows nothing about.
///
/// There used to be a dozen of these above every shell menu — Open in new tab, Refresh, Select
/// all, Copy path and the rest. They are gone: a context menu on a file or a folder now shows
/// Windows' own menu and nothing else, which is what it claims to be. Every command they carried
/// is still on its keyboard shortcut, and most of them are in the shell's menu anyway under the
/// name Explorer gives it.
///
/// What is left is the one menu Windows has no answer for, because it is not the shell's question:
/// where a right-button drag has just landed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Own {
    /// The three a right-button drag offers when it lands, which is how Windows has asked
    /// "copy or move?" since it stopped guessing.
    CopyHere,
    MoveHere,
    Cancel,
}

impl Own {
    pub fn label(self) -> &'static str {
        match self {
            Self::CopyHere => "Copy here",
            Self::MoveHere => "Move here",
            Self::Cancel => "Cancel",
        }
    }
}

/// What activating an entry does.
#[derive(Clone, Debug)]
pub enum Command {
    /// One of this program's own.
    Own(Own),
    /// A shell command: its canonical verb if it has one, and its id either way.
    Shell { verb: Option<String>, id: u32 },
}

impl Command {
    /// Whether this is one of the shell's New entries, which answers by *creating something*.
    ///
    /// Recognised by verb, because the verbs are the shell's own and are the same on every
    /// Windows: `NewFolder`, `NewLink`, and one per registered file type named after the
    /// extension it makes — `.txt`, `.bmp`, `.docx`. The labels are not; read off a French
    /// Windows the same three entries are `Dossier`, `Raccourci` and `Document texte`.
    ///
    /// It has to be recognised at all because the shell will not say what it made. Unlike this
    /// program's own New folder, which goes through `IFileOperation` and is *told* the name on
    /// [`crate::shell::ops::Done::created`], a New verb creates the file inside `InvokeCommand`
    /// and reports nothing but success — so the way to find `Nouveau document texte (2).txt` is
    /// to know a file is coming and see what turns up. See [`crate::pane::Tab::name_the_new`].
    pub fn creates_an_item(&self) -> bool {
        let Self::Shell { verb: Some(verb), .. } = self else {
            return false;
        };
        verb == "NewFolder"
            || verb == "NewLink"
            // An extension, spelled the way the registry spells one. Tested rather than just
            // `starts_with('.')` so that an extension of somebody else's offering a verb that
            // happens to begin with a dot is not mistaken for a file the shell is about to make.
            || (verb.len() > 1
                && verb.starts_with('.')
                && verb[1..]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
    }
}

/// What an entry is.
#[derive(Clone, Debug)]
pub enum Kind {
    Command(Command),
    Separator,
    /// A submenu, which starts out empty.
    ///
    /// Filling one means asking the extension that owns it to populate its `HMENU`, and
    /// that costs real time — up to a tenth of a second for Open With. So it is left
    /// unfilled until the user opens it.
    ///
    /// `source` is what the shell knows this submenu by, and `None` means there is nothing
    /// to ask: either it has been filled already, or it never had a shell behind it.
    ///
    /// It is a bare number and not a path through the entries on purpose. It *was* a path,
    /// and that was wrong the moment the drawing code put this program's own entries above
    /// the shell's: the path the menu on screen would have asked with was six entries and a
    /// divider further along than the one the shell had filed the submenu under, so every
    /// lookup missed and every submenu in the program came back empty. An opaque id cannot
    /// go wrong that way, because neither side can compute it.
    Submenu {
        children: Vec<Entry>,
        source: Option<u32>,
    },
}

impl Kind {
    /// A submenu nobody has asked the shell about yet.
    pub fn unfilled(source: u32) -> Self {
        Self::Submenu {
            children: Vec::new(),
            source: Some(source),
        }
    }

    /// A submenu with everything in it, which nothing will be asked about.
    pub fn complete(children: Vec<Entry>) -> Self {
        Self::Submenu {
            children,
            source: None,
        }
    }

    /// What the shell knows this submenu by, if it is still waiting to be filled.
    pub fn unasked(&self) -> Option<u32> {
        match self {
            Self::Submenu { source, .. } => *source,
            _ => None,
        }
    }
}

/// One line of the menu.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Ready to draw: accelerator ampersands removed, the tab-separated shortcut split
    /// off into `shortcut`.
    pub label: String,
    pub shortcut: String,
    pub kind: Kind,
    pub enabled: bool,
    pub checked: bool,
    /// The bold one — what a double click would have done.
    pub default: bool,
    /// The item's own bitmap, as RGBA, when the shell gave one.
    pub icon: Option<egui::ColorImage>,
}

impl Entry {
    fn separator() -> Self {
        Self {
            label: String::new(),
            shortcut: String::new(),
            kind: Kind::Separator,
            enabled: false,
            checked: false,
            default: false,
            icon: None,
        }
    }

    /// One of this program's own entries.
    pub fn own(which: Own) -> Self {
        Self {
            label: which.label().to_owned(),
            shortcut: String::new(),
            kind: Kind::Command(Command::Own(which)),
            enabled: true,
            checked: false,
            default: false,
            icon: None,
        }
    }
}


#[cfg(test)]
/// The whole menu, submenus and all, on the calling thread.
///
/// Half a second of it, for a file — see the note at the top of this file. Nothing in the
/// program calls this: the app goes through [`Builder`], which does the same work off the
/// UI thread and fills submenus only when they are opened. It is kept because it is the
/// shape the fidelity tests want — one call, everything present, nothing to wait for.
pub fn build(parent: &Path, items: &[PathBuf]) -> Vec<Entry> {
    #[cfg(windows)]
    {
        let Some((mut live, entries)) = win::Live::open(parent, items, Depth::Full) else {
            return Vec::new();
        };
        fn deepen(live: &mut win::Live, entries: &mut Vec<Entry>) {
            for entry in entries.iter_mut() {
                let Some(source) = entry.kind.unasked() else {
                    continue;
                };
                let mut children = live.fill(source);
                deepen(live, &mut children);
                entry.kind = Kind::complete(children);
            }
            // A submenu with nothing in it is worse than no entry at all: it looks like
            // something that failed rather than something absent.
            entries.retain(|e| !matches!(&e.kind, Kind::Submenu { children, .. } if children.is_empty()));
        }
        let mut entries = entries;
        deepen(&mut live, &mut entries);
        entries
    }
    #[cfg(not(windows))]
    {
        let _ = (parent, items);
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// The builder thread
// ---------------------------------------------------------------------------

/// How much of a menu to ask the shell for.
///
/// # Why there is a choice at all
///
/// `QueryContextMenu` is one call that lets every installed extension contribute, and the whole
/// cost is paid inside it — before a single entry exists. So there is nothing to filter
/// afterwards: an entry that took twenty-four seconds to decide on has already taken them by the
/// time this program can see it, and dropping it saves nothing. The only lever is asking for
/// less, and `CMF_` flags are the whole of that lever.
///
/// Measured back to back on a 6.4 MB executable on a mapped share, by `probe_menu_costs`:
///
/// | flags | `QueryContextMenu` | what came back |
/// | --- | --- | --- |
/// | `CMF_NORMAL \| CMF_EXPLORE` | 19.2 s | 29 entries — everything |
/// | `CMF_OPTIMIZEFORINVOKE` | 10.3 s | 20 entries, labelled `open`, `runas`, `pintohomefile` |
/// | `CMF_NORMAL \| CMF_EXPLORE \| CMF_DONOTPICKDEFAULT` | 21.0 s | 29 entries |
/// | **`CMF_DEFAULTONLY`** | **0.49 s** | Open, Run as administrator, Cut, Copy, Paste, Create shortcut, Delete, Rename, Properties |
/// | `CMF_NOVERBS` | 1.3 ms | Cut, Copy, Create shortcut, Delete, Properties |
///
/// `CMF_OPTIMIZEFORINVOKE` is the flag whose documented job is exactly this — "do not do work
/// that is only needed to display the menu" — and it is no use for a menu that will be
/// displayed: the labels come back as raw verb names because extensions skip building display
/// strings, and it is still ten seconds, because whichever extension reads the whole file does
/// not honour it.
///
/// `CMF_DEFAULTONLY` does, forty times over, and what it leaves is the shell's own verbs. That
/// is a real menu — everything anybody does to a file is in it — minus the third-party extras
/// (7-Zip, Send To, Open With, Copy as path, Previous Versions, and the several installed
/// "open with" entries). It is a request for the default verb rather than for a short menu, so
/// this is leaning on it a little sideways; what it does empirically is skip the extensions that
/// have to look at the file to decide what to offer, which is precisely the thing that is slow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth {
    /// Everything the machine has to offer. What a local file gets.
    Full,
    /// The shell's own verbs, and nothing that has to read the file to decide.
    Fast,
}

/// What the UI thread wants from the shell.
enum Ask {
    /// Query the shell for a selection, or for the folder when `items` is empty.
    Build {
        token: u64,
        parent: PathBuf,
        items: Vec<PathBuf>,
        depth: Depth,
    },
    /// Fill the submenu the shell knows by this id.
    Fill { token: u64, id: u32 },
    /// The menu is gone: let the `HMENU` and the `IContextMenu` go with it.
    Close { token: u64 },
}

/// What came back.
pub enum Said {
    /// The shell's entries, and how much of a menu they are.
    Built {
        token: u64,
        entries: Vec<Entry>,
        depth: Depth,
    },
    /// One submenu's contents. Empty means the extension really had nothing.
    Filled {
        token: u64,
        id: u32,
        children: Vec<Entry>,
    },
}

/// Builds context menus off the UI thread, on a worker that can be walked away from.
///
/// One menu at a time, which is all a pointer can be pointing at. Every request carries a
/// token; answers for a token the caller has stopped caring about are simply dropped,
/// which is what makes a right click during a slow build safe — the old menu's answer
/// arrives, does not match, and goes in the bin.
///
/// # Why a worker per menu, and not one thread with a queue
///
/// It *was* one long-lived thread reading a channel, and that is fine right up until the
/// shell takes a really long time over one menu. Measured by `probe_menu_costs`, on a 14 MB
/// executable on a mapped SMB share:
///
/// | | `QueryContextMenu` |
/// | --- | --- |
/// | a folder on the share | 0.20 s |
/// | a 41 MB `.lib` on the share | 1.8 s |
/// | a 6.4 MB `.exe` on the share | 11.7 s |
/// | a 11.5 MB `.exe` on the share | 19.8 s |
/// | a 14.7 MB `.exe` on the share | **23.9 s** |
///
/// Every time, not just the first. That is one extension reading the whole executable — the
/// time is linear in its size at about 570 kB/s, while a 41 MB file that is *not* an
/// executable comes back in under two seconds, so the link is doing 20 MB/s and the slow read
/// is small-chunk and latency-bound rather than short of bandwidth. Nothing else in a menu
/// costs anything at all: on that same file, parsing the path was 0.26 s, binding to the
/// parent 0.19 s, and reading the whole `HMENU` back out, item bitmaps and all twenty-six
/// canonical verbs included, came to **0.4 ms**.
///
/// None of that is ours to make faster. What *was* ours is that the queue made it everybody
/// else's problem: a second right click — on a local file, on anything — sat behind the first
/// for the rest of those twenty-four seconds, so one slow menu meant no menus at all until it
/// finished. `QueryContextMenu` is a blocking call into somebody else's code and there is no
/// asking it to stop, so the only cancellation available is to stop waiting: [`Builder::build`]
/// **retires** a worker that has not answered yet and serves the new menu on a fresh one. The
/// abandoned thread finishes whenever the shell lets go, drops its `Live`, and exits; its
/// answer arrives with a stale token and is discarded.
///
/// The cost is that two workers can briefly hold two sets of extension interfaces, which is
/// the thing the single thread was carefully avoiding. That is the right way round: a few
/// megabytes for a few seconds, against a program whose context menu stops working.
///
/// # Why not [`crate::shell::Modal`]
///
/// The modal thread blocks for as long as a Properties sheet is open. A menu that queued
/// behind one would arrive when the user closed a dialog, which is not when they asked for
/// it. And the two have opposite lifetimes: a modal request is over when it returns,
/// whereas a menu's `IContextMenu` has to stay alive — on the thread that made it — for as
/// long as a submenu might still be opened.
pub struct Builder {
    /// The worker serving the menu asked for most recently. Spawned on the first menu, so a
    /// session where nobody right-clicks has no thread and no shell extensions in it.
    current: Option<Worker>,
    /// Workers abandoned inside a call nobody is waiting for. Reaped as they end.
    retired: Vec<std::thread::JoinHandle<()>>,
    /// Handed to every worker, live and retired. Answers are told apart by token.
    says: std::sync::mpsc::Sender<Said>,
    rx: std::sync::mpsc::Receiver<Said>,
    ctx: egui::Context,
    next: u64,
    /// The token of the build that has not been answered yet — which is the same thing as
    /// "the current worker is inside `QueryContextMenu` and will be for as long as it takes".
    waiting: Option<u64>,
}

/// One thread with one apartment and at most one menu open on it.
struct Worker {
    /// Dropping this ends the thread's loop, which is how a worker is told to finish.
    tx: std::sync::mpsc::Sender<Ask>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Ends the threads, and waits for them — but not for ever.
///
/// The waiting is the point. A menu still open when the program quits leaves a worker holding
/// an `IContextMenu`, and through it a live object inside every shell extension that
/// contributed to it. Joining releases all of that on the thread that owns it, while that
/// thread is still there — rather than leaving it to process teardown, where an `HMENU` and
/// a set of apartment-threaded interfaces are freed by nobody in particular.
///
/// The bound is the point too, and it is new. An unconditional join here hands the
/// twenty-four seconds straight back, this time to closing the window: measured, closing while
/// a worker was still inside `QueryContextMenu` on that network executable took **19.9 s**
/// joining and **1.9 s** with the bound, which is the rest of shutting down and not this. So
/// the wait is brief, and it is spent in [`crate::shell::answering_calls`] rather than asleep,
/// because this is the UI thread's own apartment and a worker on the way out may yet call into
/// it. Whatever has not finished by then is left to the process, which is where it was going
/// anyway.
///
/// It does *not* fix the intermittent failure to exit that a `--shot --menu` run shows about
/// one time in eight. That was the first guess, and it was wrong: the same run against the
/// commit before any of this hangs at the same rate, and a `--shot` run with no menu in it
/// does not hang at all. So the cause is the extensions being in the process at all, which
/// predates all of this and is not this file's to fix.
impl Drop for Builder {
    fn drop(&mut self) {
        // The sender goes with the `Worker`; the handle is kept so it can be waited for.
        let live = self.current.take().and_then(|mut w| w.thread.take());
        let mut threads = std::mem::take(&mut self.retired);
        threads.extend(live);

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while !threads.is_empty() {
            threads.retain(|thread| !thread.is_finished());
            if threads.is_empty() || std::time::Instant::now() >= deadline {
                break;
            }
            crate::shell::answering_calls(5);
        }
    }
}

impl Builder {
    pub fn new(ctx: &egui::Context) -> Self {
        let (says, rx) = std::sync::mpsc::channel::<Said>();
        Self {
            current: None,
            retired: Vec::new(),
            says,
            rx,
            ctx: ctx.clone(),
            next: 1,
            waiting: None,
        }
    }

    /// A fresh worker with an apartment of its own.
    fn spawn(&self) -> Worker {
        let (tx, asks) = std::sync::mpsc::channel::<Ask>();
        let says = self.says.clone();
        let ctx = self.ctx.clone();
        let thread = std::thread::Builder::new()
            .name("shell-menu".to_owned())
            .spawn(move || serve(asks, says, ctx))
            .ok();
        Worker { tx, thread }
    }

    /// Send an ask, if there is a worker to hear it.
    fn ask(&self, ask: Ask) {
        if let Some(worker) = &self.current {
            let _ = worker.tx.send(ask);
        }
    }

    /// Ask for a menu. The token identifies its answers.
    ///
    /// If the previous menu has not come back yet, its worker is abandoned rather than queued
    /// behind — see the note on [`Builder`]. So this returns immediately and the new menu is
    /// built immediately, whatever the shell is still doing about the old one.
    ///
    /// The app happens to reach [`Builder::abandon`] first, because opening a menu closes
    /// whatever was on its way. That does not make this redundant: a build queued behind a
    /// worker that has twenty seconds left to run is never the right thing, and whether it can
    /// happen should not depend on a caller elsewhere getting the order right.
    pub fn build(&mut self, parent: &Path, items: &[PathBuf], depth: Depth) -> u64 {
        let token = self.next;
        self.next += 1;
        if self.waiting.is_some() {
            self.retire();
        }
        if self.current.is_none() {
            self.current = Some(self.spawn());
        }
        self.waiting = Some(token);
        self.ask(Ask::Build {
            token,
            parent: parent.to_owned(),
            items: items.to_vec(),
            depth,
        });
        token
    }

    /// Stop waiting for the menu that is still being built.
    ///
    /// Not the same as [`Builder::close`], which tells a worker to let go of a menu it has
    /// already made. There is nothing to tell here: the thread is inside somebody else's
    /// code. So it is let go of instead — the loop ends when the call returns, the `Live`
    /// goes with it, and the answer lands on a channel nobody is reading.
    pub fn abandon(&mut self) {
        if self.waiting.take().is_some() {
            self.retire();
        }
    }

    /// Whether a build is outstanding, which is the one state a worker cannot be talked out of.
    ///
    /// The app has no use for this — it knows whether it is waiting, because it is holding the
    /// `Asking`. It is here for the test that has to establish that the slow build really was
    /// still running when the second one was asked for.
    #[cfg(test)]
    pub fn busy(&self) -> bool {
        self.waiting.is_some()
    }

    fn retire(&mut self) {
        if let Some(mut worker) = self.current.take() {
            if let Some(thread) = worker.thread.take() {
                self.retired.push(thread);
            }
            // And `worker` drops here, taking its sender with it — which is what ends the
            // thread's loop once the shell finally returns.
        }
        self.reap();
    }

    /// Join whichever abandoned workers have since finished, so their handles do not pile up.
    fn reap(&mut self) {
        if self.retired.is_empty() {
            return;
        }
        let mut still = Vec::with_capacity(self.retired.len());
        for thread in std::mem::take(&mut self.retired) {
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                still.push(thread);
            }
        }
        self.retired = still;
    }

    /// Ask for one submenu's contents.
    pub fn fill(&self, token: u64, id: u32) {
        self.ask(Ask::Fill { token, id });
    }

    /// The menu has closed; nothing more will be asked of it.
    pub fn close(&self, token: u64) {
        self.ask(Ask::Close { token });
    }

    /// Whatever has arrived, without waiting.
    pub fn poll(&mut self) -> Option<Said> {
        self.reap();
        let said = self.rx.try_recv().ok()?;
        if let Said::Built { token, .. } = &said {
            if self.waiting == Some(*token) {
                self.waiting = None;
            }
        }
        Some(said)
    }
}

/// The builder thread.
fn serve(
    asks: std::sync::mpsc::Receiver<Ask>,
    says: std::sync::mpsc::Sender<Said>,
    ctx: egui::Context,
) {
    // Its own apartment: the `HMENU` and every interface reached through it belong to this
    // thread and are only ever touched from here.
    super::init();
    let mut held = Held::default();

    while let Ok(ask) = asks.recv() {
        let said = match ask {
            Ask::Build {
                token,
                parent,
                items,
                depth,
            } => Some(Said::Built {
                token,
                entries: held.open(token, &parent, &items, depth),
                depth,
            }),
            Ask::Fill { token, id } => Some(Said::Filled {
                token,
                id,
                children: held.fill(token, id),
            }),
            Ask::Close { token } => {
                held.close(token);
                None
            }
        };
        if let Some(said) = said {
            if says.send(said).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    }
}

/// Makes the next build take this long instead of asking the shell anything at all.
///
/// For `a_slow_menu_does_not_hold_up_the_next_one`, which needs a build that is still running
/// when the second one is asked for. A stall touches the shell not at all, deliberately: the
/// worker abandoned in the middle of it must not be able to collide with another test over the
/// process-wide clipboard lock — see [`crate::shell::serialised`].
///
/// Every build stalls while it is set, so a test can stall the retry as well as the first
/// attempt. Whichever test sets it must put it back, and must be holding
/// [`crate::shell::serialised`] while it does.
#[cfg(test)]
pub(crate) static STALL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many builds have stalled, so the test can wait until one really has.
#[cfg(test)]
pub(crate) static STALLED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The one menu the builder thread currently has open, if any.
#[derive(Default)]
struct Held {
    #[cfg(windows)]
    live: Option<(u64, win::Live)>,
    #[cfg(not(windows))]
    live: Option<u64>,
}

impl Held {
    fn open(&mut self, token: u64, parent: &Path, items: &[PathBuf], depth: Depth) -> Vec<Entry> {
        #[cfg(test)]
        {
            use std::sync::atomic::Ordering;
            let ms = STALL_MS.load(Ordering::SeqCst);
            if ms > 0 {
                STALLED.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(ms));
                return Vec::new();
            }
        }
        // The previous menu goes first, so this worker holds at most one `HMENU` and one set
        // of extension interfaces at a time.
        self.live = None;
        #[cfg(windows)]
        {
            match win::Live::open(parent, items, depth) {
                Some((live, entries)) => {
                    self.live = Some((token, live));
                    entries
                }
                None => Vec::new(),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (token, parent, items, depth);
            Vec::new()
        }
    }

    fn fill(&mut self, token: u64, id: u32) -> Vec<Entry> {
        #[cfg(windows)]
        {
            match &mut self.live {
                Some((held, live)) if *held == token => live.fill(id),
                _ => Vec::new(),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (token, id);
            Vec::new()
        }
    }

    fn close(&mut self, token: u64) {
        #[cfg(windows)]
        if matches!(&self.live, Some((held, _)) if *held == token) {
            self.live = None;
        }
        #[cfg(not(windows))]
        let _ = token;
    }
}

/// Run a shell command. Puts up dialogs, so it belongs on the modal thread.
///
/// Under `cfg(test)` the folder and the selection must be inside [`crate::sandbox`]. A verb list
/// read from a real folder contains Delete, and `InvokeCommand` runs whatever it is handed without
/// a confirmation this program ever sees — which is how `probe_invokable`, pointed at this
/// repository, deleted it. `shell::ops::FOR_REAL` guards `IFileOperation` and never saw this call.
pub fn invoke(parent: &Path, items: &[PathBuf], command: &Command, owner: super::Owner) {
    #[cfg(test)]
    {
        let mut paths = vec![parent.to_path_buf()];
        paths.extend(items.iter().cloned());
        crate::sandbox::guard("IContextMenu::InvokeCommand", &paths);
    }
    #[cfg(windows)]
    if let Command::Shell { verb, id } = command {
        win::invoke(parent, items, verb.as_deref(), *id, owner);
    }
    #[cfg(not(windows))]
    let _ = (parent, items, command, owner);
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::{Interface, PCSTR, PCWSTR, PSTR, PWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IContextMenu, IContextMenu2, IShellFolder, SHBindToObject, SHBindToParent,
        SHParseDisplayName, CMF_EXPLORE, CMF_NORMAL, CMF_OPTIMIZEFORINVOKE, CMINVOKECOMMANDINFOEX,
        GCS_VERBW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, HMENU, MENUITEMINFOW,
        MFS_CHECKED, MFS_DEFAULT, MFS_DISABLED, MFS_GRAYED, MFT_SEPARATOR, MIIM_BITMAP, MIIM_FTYPE,
        MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SW_SHOWNORMAL, WM_INITMENUPOPUP,
    };

    /// The shell's commands are numbered from here, so an id read back out of the menu
    /// is turned into a command offset by subtracting it.
    const FIRST: u32 = 0x1000;
    const LAST: u32 = 0x7FFF;
    /// Deep enough for anything real; a guard against a malformed extension.
    const MAX_DEPTH: u32 = 4;

    /// A PIDL that frees itself.
    struct Pidl(*mut ITEMIDLIST);

    impl Drop for Pidl {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: allocated by `SHParseDisplayName`, freed exactly once.
                unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
            }
        }
    }

    fn pidl_of(path: &Path) -> Option<Pidl> {
        let wide = crate::shell::wide(path);
        let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
        // SAFETY: `wide` is null-terminated and outlives the call.
        let ok =
            unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut raw, 0, None).is_ok() };
        (ok && !raw.is_null()).then_some(Pidl(raw))
    }

    /// The `IContextMenu` for a selection, or for the folder's background when the selection
    /// is empty — which is where New comes from.
    ///
    /// # Why the empty case is a different call and not the same one with no items
    ///
    /// A folder has *two* context menus and they are different objects. `GetUIObjectOf` on the
    /// folder's own PIDL, taken from its parent, gives the menu Explorer shows for the folder
    /// **as an item in a listing** — Open, Cut, Copy, Create shortcut, Delete, Rename,
    /// Properties, and every extension registered under `Directory` and `Folder`.
    /// `CreateViewObject` gives the menu Explorer shows for the folder's **background**, and
    /// only that one has New in it.
    ///
    /// This was the first of those, and New was simply absent. Not empty — absent, which is
    /// why none of the `WM_INITMENUPOPUP` machinery in [`Live::fill`] had anything to do with
    /// it: the handler is registered in exactly one place,
    /// `HKCR\Directory\Background\shellex\ContextMenuHandlers\New`, so nothing asked for under
    /// `Directory` or `Folder` can produce it at all. Measured against a project folder on
    /// this machine, both menus read out with every submenu filled:
    ///
    /// | | entries | New |
    /// | --- | --- | --- |
    /// | `GetUIObjectOf` on the folder item | 29 | absent |
    /// | `CreateViewObject` | 12 | Folder, Shortcut and ten registered file types |
    ///
    /// What the view object does *not* carry is Paste, Refresh, View and Sort by. Those belong
    /// to Explorer's own view rather than to the shell folder — Explorer synthesises them
    /// around this menu — so they are this program's to provide, and it does, on their
    /// shortcuts.
    unsafe fn context_of(parent: &Path, items: &[PathBuf]) -> Option<IContextMenu> {
        if items.is_empty() {
            let pidl = pidl_of(parent)?;
            // Bound from the desktop, because what is wanted is the folder itself as an
            // `IShellFolder` — not the folder as an item inside its parent, which is what
            // `SHBindToParent` gives and what the selection path below wants.
            let folder: IShellFolder = SHBindToObject(None, pidl.0, None).ok()?;
            return folder.CreateViewObject(HWND::default()).ok();
        }

        // Every item has to be a child of one folder for one `IContextMenu`, which a
        // selection in this program always is.
        let pidls: Vec<Pidl> = items.iter().filter_map(|p| pidl_of(p)).collect();
        let first = pidls.first()?;
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let folder: IShellFolder = SHBindToParent(first.0, Some(&mut child)).ok()?;

        let mut children: Vec<*const ITEMIDLIST> = Vec::with_capacity(pidls.len());
        children.push(child as *const ITEMIDLIST);
        for pidl in pidls.iter().skip(1) {
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHBindToParent::<IShellFolder>(pidl.0, Some(&mut child)).is_ok() && !child.is_null() {
                children.push(child as *const ITEMIDLIST);
            }
        }
        folder.GetUIObjectOf(HWND::default(), &children, None).ok()
    }

    /// What to ask `QueryContextMenu` for. See [`Depth`] for the measurements behind this.
    ///
    /// `CMF_EXPLORE` on the full one asks for the menu Explorer shows rather than the shorter
    /// one a file dialog gets.
    fn flags(depth: Depth) -> u32 {
        match depth {
            Depth::Full => CMF_NORMAL | CMF_EXPLORE,
            Depth::Fast => windows::Win32::UI::Shell::CMF_DEFAULTONLY,
        }
    }

    /// One open menu's shell state, on the thread that made it.
    ///
    /// `IContextMenu` is apartment-threaded and the `HMENU` belongs to whoever created it,
    /// so this never leaves the thread it was made on — which is
    /// [`super::Builder`]'s, not the UI's.
    pub struct Live {
        context: IContextMenu,
        hmenu: HMENU,
        /// Every submenu found so far, under the id handed out with it.
        submenus: std::collections::HashMap<u32, Sub>,
        /// The next id. Only ever grows, so an id names one submenu for this menu's life.
        next: u32,
    }

    /// What is needed to fill one submenu.
    #[derive(Clone, Copy)]
    struct Sub {
        menu: HMENU,
        /// The item's position inside its *parent* `HMENU`, which is what
        /// `WM_INITMENUPOPUP` wants — and which is not the entry index, since separators are
        /// coalesced and unusable items dropped on the way out.
        position: u32,
        /// How deep the parent was, for the guard against a malformed extension.
        depth: u32,
    }

    impl Drop for Live {
        fn drop(&mut self) {
            // Destroys the submenus with it, which is why they are not tracked for freeing.
            // SAFETY: created by `CreatePopupMenu` here, destroyed exactly once.
            unsafe {
                let _ = DestroyMenu(self.hmenu);
            }
        }
    }

    impl Live {
        /// Ask the shell, and read the top level. Submenus are left for [`Live::fill`].
        pub fn open(parent: &Path, items: &[PathBuf], depth: Depth) -> Option<(Self, Vec<Entry>)> {
            // SAFETY: the menu is destroyed by `Drop` on every path out, and the
            // interfaces are reference counted.
            unsafe {
                let context = context_of(parent, items)?;
                let hmenu = CreatePopupMenu().ok()?;
                let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags(depth));
                let mut live = Self {
                    context,
                    hmenu,
                    submenus: std::collections::HashMap::new(),
                    next: 0,
                };
                let entries = live.read(hmenu, 0);
                Some((live, entries))
            }
        }

        /// The contents of the submenu with this id, asking the extension to fill it first.
        pub fn fill(&mut self, id: u32) -> Vec<Entry> {
            let Some(sub) = self.submenus.get(&id).copied() else {
                return Vec::new();
            };
            if sub.depth >= MAX_DEPTH {
                // Deep enough for anything real; a guard against a malformed extension.
                return Vec::new();
            }
            // SAFETY: both handles came out of this menu and are alive as long as it is.
            unsafe {
                // An extension fills its submenu when the menu is about to pop up. This
                // menu never pops up, so it is told to anyway — otherwise Send To and New
                // come back empty.
                if let Ok(two) = self.context.cast::<IContextMenu2>() {
                    let _ = two.HandleMenuMsg(
                        WM_INITMENUPOPUP,
                        WPARAM(sub.menu.0 as usize),
                        LPARAM(sub.position as isize),
                    );
                }
                self.read(sub.menu, sub.depth + 1)
            }
        }

        /// Read one `HMENU` the shell has filled into something drawable.
        unsafe fn read(&mut self, hmenu: HMENU, depth: u32) -> Vec<Entry> {
            let count = GetMenuItemCount(Some(hmenu));
            if count <= 0 {
                return Vec::new();
            }
            let mut entries: Vec<Entry> = Vec::with_capacity(count as usize);

            for position in 0..count {
                let mut info = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE
                        | MIIM_STATE
                        | MIIM_ID
                        | MIIM_SUBMENU
                        | MIIM_STRING
                        | MIIM_BITMAP,
                    ..Default::default()
                };
                // Two calls: the first to learn the length, the second to get the text. A
                // fixed buffer would truncate a long "Open with <application>".
                if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
                    continue;
                }
                let mut text = vec![0u16; info.cch as usize + 1];
                info.dwTypeData = PWSTR(text.as_mut_ptr());
                info.cch = text.len() as u32;
                if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
                    continue;
                }

                if info.fType.0 & MFT_SEPARATOR.0 != 0 {
                    // Two separators running together, or one at either end, are what a
                    // menu assembled from several extensions looks like before anyone
                    // tidies it.
                    if !matches!(entries.last(), None | Some(Entry { kind: Kind::Separator, .. })) {
                        entries.push(Entry::separator());
                    }
                    continue;
                }

                let raw = String::from_utf16_lossy(&text[..info.cch as usize]);
                let (label, shortcut) = split_label(&raw);
                if label.is_empty() {
                    continue;
                }

                let enabled = info.fState.0 & (MFS_DISABLED.0 | MFS_GRAYED.0) == 0;
                let checked = info.fState.0 & MFS_CHECKED.0 != 0;
                let default = info.fState.0 & MFS_DEFAULT.0 != 0;
                let icon = menu_bitmap(info.hbmpItem);

                let kind = if !info.hSubMenu.is_invalid() && depth < MAX_DEPTH {
                    // Noted rather than read. Whether there is anything in it is not known
                    // until the extension is asked, and asking is the expensive part.
                    let id = self.next;
                    self.next += 1;
                    self.submenus.insert(
                        id,
                        Sub {
                            menu: info.hSubMenu,
                            position: position as u32,
                            depth,
                        },
                    );
                    Kind::unfilled(id)
                } else if info.wID >= FIRST && info.wID <= LAST {
                    let offset = info.wID - FIRST;
                    Kind::Command(Command::Shell {
                        verb: canonical_verb(&self.context, offset as usize),
                        id: offset,
                    })
                } else {
                    // An id outside the range this program handed out is not ours to
                    // invoke.
                    continue;
                };

                entries.push(Entry {
                    label,
                    shortcut,
                    kind,
                    enabled,
                    checked,
                    default,
                    icon,
                });
            }

            while matches!(entries.last(), Some(Entry { kind: Kind::Separator, .. })) {
                entries.pop();
            }
            entries
        }
    }

    /// A menu label as the shell writes it, split into what to draw.
    ///
    /// `&` marks the keyboard accelerator and `&&` is a literal ampersand; a tab
    /// separates the label from its shortcut text.
    pub(super) fn split_label(raw: &str) -> (String, String) {
        let (left, right) = match raw.split_once('\t') {
            Some((left, right)) => (left, right.trim().to_owned()),
            None => (raw, String::new()),
        };
        let mut label = String::with_capacity(left.len());
        let mut chars = left.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '&' {
                if chars.peek() == Some(&'&') {
                    chars.next();
                    label.push('&');
                }
                continue;
            }
            label.push(c);
        }
        (label.trim().to_owned(), right)
    }

    /// The canonical verb for a command, if it has one.
    ///
    /// Stable across processes and threads, which is what lets the command be invoked
    /// later against a freshly obtained `IContextMenu`.
    unsafe fn canonical_verb(context: &IContextMenu, offset: usize) -> Option<String> {
        let mut buffer = [0u8; 260];
        // `GCS_VERBW` writes wide characters into the same buffer, so it is sized for
        // them and read back as such.
        context
            .GetCommandString(
                offset,
                GCS_VERBW,
                None,
                PSTR(buffer.as_mut_ptr()),
                (buffer.len() / 2) as u32,
            )
            .ok()?;
        let wide: Vec<u16> = buffer
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect();
        let verb = String::from_utf16_lossy(&wide);
        (!verb.is_empty()).then_some(verb)
    }

    /// A menu item's bitmap as RGBA, when it is a real one.
    ///
    /// `hbmpItem` doubles as a slot for a dozen `HBMMENU_*` magic values — small
    /// integers, not handles — which is what the bound below is filtering out.
    unsafe fn menu_bitmap(
        bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    ) -> Option<egui::ColorImage> {
        const LAST_MAGIC: isize = 16;
        if bitmap.is_invalid() || (bitmap.0 as isize) <= LAST_MAGIC {
            return None;
        }
        crate::shell::icons::bitmap_of(bitmap)
    }

    /// What each `CMF_` flag combination costs, and what it leaves out.
    ///
    /// The expensive work happens *inside* `QueryContextMenu`, before any entry exists, so
    /// there is nothing to filter afterwards. The only lever is asking for less. This is what
    /// asking for less actually buys.
    #[cfg(test)]
    pub(super) fn probe_flags(parent: &Path, items: &[PathBuf]) {
        use std::time::Instant;
        use windows::Win32::UI::Shell::{CMF_DEFAULTONLY, CMF_DONOTPICKDEFAULT, CMF_NOVERBS};

        for (label, flags) in [
            ("NORMAL|EXPLORE (what it uses)", CMF_NORMAL | CMF_EXPLORE),
            ("OPTIMIZEFORINVOKE", CMF_OPTIMIZEFORINVOKE),
            (
                "NORMAL|EXPLORE|DONOTPICKDEFAULT",
                CMF_NORMAL | CMF_EXPLORE | CMF_DONOTPICKDEFAULT,
            ),
            ("DEFAULTONLY", CMF_DEFAULTONLY),
            ("NOVERBS", CMF_NOVERBS),
        ] {
            // SAFETY: the menu is destroyed before the next round, and the interfaces are
            // reference counted.
            unsafe {
                let Some(context) = context_of(parent, items) else {
                    return;
                };
                let Ok(hmenu) = CreatePopupMenu() else { return };
                let at = Instant::now();
                let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags);
                let took = at.elapsed().as_secs_f32() * 1e3;
                let mut live = Live {
                    context,
                    hmenu,
                    submenus: std::collections::HashMap::new(),
                    next: 0,
                };
                let entries = live.read(hmenu, 0);
                eprintln!(
                    "  {label:<32} {took:>9.1} ms  {} entries",
                    entries.len()
                );
                eprintln!(
                    "      {}",
                    entries
                        .iter()
                        .filter(|e| !e.label.is_empty())
                        .map(|e| e.label.as_str())
                        .collect::<Vec<_>>()
                        .join(" | ")
                );
            }
        }
    }

    /// Time every shell call one menu costs, call by call.
    ///
    /// Not a test of anything — a measurement, and the only way to find out which of these is
    /// the slow one when the file is on a share. See `probe_menu_costs`.
    #[cfg(test)]
    pub(super) fn probe(parent: &Path, items: &[PathBuf]) {
        use std::time::Instant;
        let ms = |at: Instant| at.elapsed().as_secs_f32() * 1e3;

        unsafe {
            let at = Instant::now();
            let pidl = pidl_of(items.first().unwrap_or(&parent.to_owned()));
            eprintln!("  SHParseDisplayName      {:>9.1} ms", ms(at));
            drop(pidl);

            let at = Instant::now();
            let Some(context) = context_of(parent, items) else {
                eprintln!("  no IContextMenu");
                return;
            };
            eprintln!("  bind + GetUIObjectOf    {:>9.1} ms", ms(at));

            let at = Instant::now();
            let Ok(hmenu) = CreatePopupMenu() else { return };
            let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, CMF_NORMAL | CMF_EXPLORE);
            eprintln!("  QueryContextMenu        {:>9.1} ms", ms(at));

            // The read, split into the three things it does per item.
            let count = GetMenuItemCount(Some(hmenu));
            let mut reading = 0.0f32;
            let mut bitmaps = 0.0f32;
            let mut verbs: Vec<(String, f32, Option<String>)> = Vec::new();
            for position in 0..count {
                let at = Instant::now();
                let mut info = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE
                        | MIIM_STATE
                        | MIIM_ID
                        | MIIM_SUBMENU
                        | MIIM_STRING
                        | MIIM_BITMAP,
                    ..Default::default()
                };
                if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
                    continue;
                }
                let mut text = vec![0u16; info.cch as usize + 1];
                info.dwTypeData = PWSTR(text.as_mut_ptr());
                info.cch = text.len() as u32;
                if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
                    continue;
                }
                let raw = String::from_utf16_lossy(&text[..info.cch as usize]);
                reading += ms(at);

                let at = Instant::now();
                let _ = menu_bitmap(info.hbmpItem);
                bitmaps += ms(at);

                if info.hSubMenu.is_invalid() && info.wID >= FIRST && info.wID <= LAST {
                    let at = Instant::now();
                    let verb = canonical_verb(&context, (info.wID - FIRST) as usize);
                    verbs.push((split_label(&raw).0, ms(at), verb));
                }
            }
            eprintln!("  GetMenuItemInfoW x{count:<3}   {reading:>9.1} ms");
            eprintln!("  item bitmaps            {bitmaps:>9.1} ms");
            let total: f32 = verbs.iter().map(|(_, ms, _)| ms).sum();
            eprintln!("  GetCommandString x{:<3}   {total:>9.1} ms", verbs.len());
            verbs.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (label, took, verb) in verbs.iter().take(8) {
                eprintln!("      {took:>9.1} ms  {label:<32} -> {verb:?}");
            }
            let _ = DestroyMenu(hmenu);
        }
    }

    /// TEMPORARY probe: which entries survive into the menu `invoke` resolves against.
    #[cfg(test)]
    pub(super) fn probe_invokable(parent: &Path, items: &[PathBuf]) {
        unsafe fn collect(
            live: &mut Live,
            entries: Vec<Entry>,
            prefix: &str,
            out: &mut Vec<(String, Option<String>, u32)>,
        ) {
            for entry in entries {
                if let Some(source) = entry.kind.unasked() {
                    let children = live.fill(source);
                    let deeper = format!("{prefix}{} > ", entry.label);
                    collect(live, children, &deeper, out);
                    continue;
                }
                if let Kind::Command(Command::Shell { verb, id }) = entry.kind {
                    out.push((format!("{prefix}{}", entry.label), verb, id));
                }
            }
        }

        let gather = |flags: u32| -> Vec<(String, Option<String>, u32)> {
            unsafe {
                let Some(context) = context_of(parent, items) else {
                    return Vec::new();
                };
                let Ok(hmenu) = CreatePopupMenu() else {
                    return Vec::new();
                };
                let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags);
                let mut live = Live {
                    context,
                    hmenu,
                    submenus: std::collections::HashMap::new(),
                    next: 0,
                };
                let entries = live.read(hmenu, 0);
                let mut out = Vec::new();
                collect(&mut live, entries, "", &mut out);
                out
            }
        };

        let shown = gather(CMF_NORMAL | CMF_EXPLORE);
        let optimised = gather(CMF_OPTIMIZEFORINVOKE);
        let have: std::collections::HashSet<String> = optimised
            .iter()
            .filter_map(|(_, verb, _)| verb.clone())
            .collect();
        eprintln!(
            "  as shown: {} commands;  rebuilt with OPTIMIZEFORINVOKE: {} commands",
            shown.len(),
            optimised.len()
        );
        for (label, verb, id) in &shown {
            match verb {
                Some(verb) if have.contains(verb) => eprintln!("    ok       {label:<46} {verb}"),
                Some(verb) => eprintln!("    MISSING  {label:<46} {verb}  (id {id})"),
                None => eprintln!("    no verb  {label:<46} (id {id})"),
            }
        }
    }

    /// Run a shell command, by verb where there is one.
    pub fn invoke(
        parent: &Path,
        items: &[PathBuf],
        verb: Option<&str>,
        id: u32,
        owner: crate::shell::Owner,
    ) {
        // SAFETY: the menu is destroyed before returning, and every string outlives the
        // call that reads it.
        unsafe {
            let Some(context) = context_of(parent, items) else {
                return;
            };
            let Ok(hmenu) = CreatePopupMenu() else {
                return;
            };
            // A verb needs no numbering, so the shell is told not to build a menu it will
            // never show: `CMF_OPTIMIZEFORINVOKE` is what lets an extension skip the
            // registry and disk work that `QueryContextMenu` otherwise costs, and that
            // work was measured at half a second on a file. Without a verb the flag cannot
            // be used — the command is a *position* in the menu, so the menu has to be
            // built the same way it was when the position was read, or the wrong thing
            // runs.
            let flags = match verb {
                Some(_) => CMF_OPTIMIZEFORINVOKE,
                None => CMF_NORMAL | CMF_EXPLORE,
            };
            let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags);

            let verb_bytes: Option<Vec<u8>> = verb.map(|verb| {
                let mut bytes = verb.as_bytes().to_vec();
                bytes.push(0);
                bytes
            });
            let verb_wide: Option<Vec<u16>> = verb.map(|verb| {
                verb.encode_utf16().chain(std::iter::once(0)).collect()
            });

            let mut info = CMINVOKECOMMANDINFOEX {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                fMask: 0,
                hwnd: if owner.0 != 0 {
                    owner.hwnd()
                } else {
                    HWND::default()
                },
                // A verb is a string; an id is a small integer pretending to be one,
                // which is how `IContextMenu` has taken numeric commands since it was
                // introduced.
                lpVerb: match &verb_bytes {
                    Some(bytes) => PCSTR(bytes.as_ptr()),
                    None => PCSTR(id as usize as *const u8),
                },
                lpVerbW: match &verb_wide {
                    Some(wide) => PCWSTR(wide.as_ptr()),
                    None => PCWSTR(id as usize as *const u16),
                },
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let _ = context.InvokeCommand(&mut info as *mut _ as *const _);
            let _ = DestroyMenu(hmenu);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The *folder's* menu — the one an empty-space right click asks for, which is a different
    /// shell object from a selection's and not the same one with no items in it.
    ///
    /// What this checks is *which object came back*, and it checks it by verb, because a count
    /// cannot tell them apart and for a long time nothing did. Asking `GetUIObjectOf` for the
    /// folder-as-an-item returns a perfectly good menu — 29 entries, Cut, Copy, Delete, Rename,
    /// 7-Zip, Send To — with no New anywhere in it, and every length assertion that used to be
    /// here passed on it comfortably. So the background menu is identified by what it *lacks*:
    /// it has Properties, and it has none of the item verbs. That is true of it on every Windows
    /// and in every language, which is more than can be said for looking for a label.
    #[test]
    #[cfg(windows)]
    fn a_folder_with_nothing_selected_gets_the_background_menu() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();

        let dir = crate::sandbox::dir("folder-menu");
        std::fs::create_dir_all(&dir).expect("temp dir");

        // `build` fills every submenu on the way, which is what makes New's contents visible.
        let entries = build(&dir, &[]);
        // Worth printing whole on any failure: which menu this is, is the entire question.
        let shown = || {
            entries
                .iter()
                .map(|e| match &e.kind {
                    Kind::Submenu { children, .. } => format!(
                        "{} > [{}]",
                        e.label,
                        children
                            .iter()
                            .map(|c| c.label.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    Kind::Separator => "--".to_owned(),
                    Kind::Command(_) => e.label.clone(),
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        assert!(!entries.is_empty(), "the folder's background menu came back empty");

        let verbs: Vec<String> = entries
            .iter()
            .filter_map(|e| match &e.kind {
                Kind::Command(Command::Shell { verb: Some(verb), .. }) => Some(verb.to_lowercase()),
                _ => None,
            })
            .collect();
        assert!(
            verbs.iter().any(|v| v == "properties"),
            "no Properties on it, so this is not a folder's menu at all: {}",
            shown()
        );
        for item_verb in ["cut", "copy", "delete", "rename", "link"] {
            assert!(
                !verbs.iter().any(|v| v == item_verb),
                "`{item_verb}` is on the background menu, so this is the folder-as-an-item menu \
                 and New cannot be in it -- see `context_of`: {}",
                shown()
            );
        }

        // And New itself, found by verb and not by label -- it is "New" here and "Nouveau" on a
        // French Windows, whereas `NewFolder` and `NewLink` are the shell's own names for the two
        // entries it always starts with and are the same in every language.
        let new = entries
            .iter()
            .filter_map(|e| match &e.kind {
                Kind::Submenu { children, .. } => Some(children),
                _ => None,
            })
            .find(|children| {
                let has = |wanted: &str| {
                    children.iter().any(|c| {
                        matches!(&c.kind, Kind::Command(Command::Shell { verb: Some(verb), .. })
                            if verb == wanted)
                    })
                };
                has("NewFolder") && has("NewLink")
            })
            .unwrap_or_else(|| panic!("no New submenu on the folder's menu: {}", shown()));

        // The registered file types under it, which are the rest of what New is for. Each comes
        // with its extension as the canonical verb -- `.txt`, `.bmp` -- and that is also what
        // makes them safe to run: [`invoke`] takes the verb path, so it never has to reproduce
        // the numbering of a submenu it would have had to populate all over again to get right.
        let types: Vec<&str> = new
            .iter()
            .filter_map(|c| match &c.kind {
                Kind::Command(Command::Shell { verb: Some(verb), .. }) if verb.starts_with('.') => {
                    Some(verb.as_str())
                }
                _ => None,
            })
            .collect();
        assert!(
            !types.is_empty(),
            "New offers Folder and Shortcut and no file type at all, so the submenu was never \
             filled: {}",
            shown()
        );
        eprintln!("New offers {} file types: {types:?}", types.len());

        crate::sandbox::remove(&dir);
    }

    /// A temp folder with a file and a subfolder in it, for the tests that need something
    /// real to right-click.
    #[cfg(windows)]
    fn scratch(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = crate::sandbox::dir(name);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("one.txt");
        std::fs::write(&file, b"x").expect("write");
        let sub = dir.join("folder");
        std::fs::create_dir_all(&sub).expect("subdir");
        (dir, file, sub)
    }

    /// The lazy half of the design: opening the menu must *not* fill the submenus, and
    /// filling one afterwards must give what the eager path would have.
    ///
    /// This is the saving that matters on a file -- Open With alone was 41-104 ms of the
    /// build -- so a change that quietly went back to prefilling would show up here as a
    /// submenu that already had children.
    #[test]
    #[cfg(windows)]
    fn submenus_stay_empty_until_they_are_asked_for() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        let (dir, file, _) = scratch("lazy");

        let (mut live, entries) =
            super::win::Live::open(&dir, std::slice::from_ref(&file), Depth::Full)
                .expect("the shell's menu");

        let submenus: Vec<(u32, String)> = entries
            .iter()
            .filter_map(|e| e.kind.unasked().map(|id| (id, e.label.clone())))
            .collect();
        assert!(
            !submenus.is_empty(),
            "a text file on any Windows has at least Open With or Send To: {:?}",
            entries.iter().map(|e| &e.label).collect::<Vec<_>>()
        );
        for entry in &entries {
            if let Kind::Submenu { children, source } = &entry.kind {
                assert!(
                    children.is_empty() && source.is_some(),
                    "`{}` came back already filled -- opening the menu paid for a submenu \
                     nobody had opened",
                    entry.label
                );
            }
        }

        // And asking works: at least one of them has something in it.
        let filled: Vec<(String, usize)> = submenus
            .iter()
            .map(|(id, label)| (label.clone(), live.fill(*id).len()))
            .collect();
        assert!(
            filled.iter().any(|(_, count)| *count > 0),
            "every submenu came back empty when asked, so the fill never reached the \
             extensions: {filled:?}"
        );

        crate::sandbox::remove(&dir);
    }

    /// The asynchronous half: the builder answers by channel, and a submenu asked for
    /// after the fact comes back against the same token.
    #[test]
    #[cfg(windows)]
    fn the_builder_answers_by_channel() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        let (dir, file, _) = scratch("builder");

        let ctx = egui::Context::default();
        let mut builder = Builder::new(&ctx);

        // Wait for one answer. Generous, because the whole point is that this is slow.
        fn next(builder: &mut Builder) -> Said {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                if let Some(said) = builder.poll() {
                    return said;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the builder thread never answered"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        let token = builder.build(&dir, std::slice::from_ref(&file), Depth::Full);
        let entries = match next(&mut builder) {
            Said::Built { token: got, entries, .. } => {
                assert_eq!(got, token, "answered for a menu nobody asked for");
                entries
            }
            _ => panic!("the first answer to a build should be the menu"),
        };
        assert!(
            entries.len() > 3,
            "the builder came back with only {} entries",
            entries.len()
        );

        // The submenu is still on the builder's thread, and can be filled from here.
        let (id, label) = entries
            .iter()
            .find_map(|e| e.kind.unasked().map(|id| (id, e.label.clone())))
            .expect("a submenu");
        builder.fill(token, id);
        match next(&mut builder) {
            Said::Filled { token: got, id: got_id, children } => {
                assert_eq!(got, token);
                assert_eq!(got_id, id);
                assert!(!children.is_empty(), "`{label}` filled to nothing");
            }
            _ => panic!("expected the submenu"),
        }

        // Closing frees the shell's side; a fill against a closed token is still answered,
        // and answered emptily, rather than reaching a menu that is gone.
        builder.close(token);
        builder.fill(token, id);
        match next(&mut builder) {
            Said::Filled { children, .. } => assert!(children.is_empty()),
            _ => panic!("expected the submenu"),
        }

        crate::sandbox::remove(&dir);
    }

    /// Where the time in a context menu actually goes.
    ///
    /// Not an assertion -- a measurement, and the one that decided the design of this
    /// file. The numbers it printed are in the note at the top. What to look at: the whole
    /// menu against the part the user waits for, which is now only `Live::open` reading the
    /// top level, and the submenu fill that a hover pays for instead.
    #[test]
    #[ignore = "measures the shell; run explicitly, single-threaded, with --nocapture"]
    #[cfg(windows)]
    fn what_the_shell_menu_takes_to_build() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        let (dir, file, sub) = scratch("menu-cost");

        for (what, parent, items) in [
            ("a text file", dir.clone(), vec![file.clone()]),
            ("a folder", dir.clone(), vec![sub.clone()]),
            ("empty space", dir.clone(), Vec::new()),
            (
                r"a folder in C:\",
                PathBuf::from(r"C:\"),
                vec![PathBuf::from(r"C:\Windows")],
            ),
        ] {
            for round in 1..=3 {
                let whole = std::time::Instant::now();
                let all = build(&parent, &items);
                let whole = whole.elapsed();

                let root = std::time::Instant::now();
                let Some((mut live, entries)) = super::win::Live::open(&parent, &items, Depth::Full) else {
                    continue;
                };
                let root = root.elapsed();

                // Every submenu, one at a time, the way a hover pays for it.
                let mut fills = Vec::new();
                for entry in &entries {
                    if let Some(id) = entry.kind.unasked() {
                        let at = std::time::Instant::now();
                        let children = live.fill(id);
                        fills.push((
                            entry.label.clone(),
                            children.len(),
                            at.elapsed().as_secs_f32() * 1e3,
                        ));
                    }
                }

                eprintln!(
                    "{what} #{round}: whole menu {:>7.1}ms ({} entries)   top level only \
                     {:>7.1}ms ({} entries)",
                    whole.as_secs_f32() * 1e3,
                    all.len(),
                    root.as_secs_f32() * 1e3,
                    entries.len()
                );
                for (label, count, ms) in fills {
                    eprintln!("    fill `{label}` {ms:>7.1}ms ({count} children)");
                }
            }
        }

        crate::sandbox::remove(&dir);
    }

    /// The reduced menu has to still be a menu — the shell's own verbs, all of them working.
    ///
    /// [`Depth::Fast`] exists because on an executable on a share the full query takes
    /// twenty-four seconds and this one takes half of one. What it buys is worth nothing if what
    /// comes back cannot cut, copy, delete, rename or open. Checked against a local file, where
    /// both are fast, because what is being tested is the *content* of the reduced menu; the
    /// timings are in the note on [`Depth`].
    #[test]
    #[cfg(windows)]
    fn the_reduced_menu_is_still_a_menu() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        let (dir, file, _) = scratch("reduced");

        let verbs = |depth: Depth| -> Vec<String> {
            let (_live, entries) = super::win::Live::open(&dir, std::slice::from_ref(&file), depth)
                .expect("the shell's menu");
            entries
                .iter()
                .filter_map(|e| match &e.kind {
                    Kind::Command(Command::Shell { verb: Some(verb), .. }) => {
                        Some(verb.to_lowercase())
                    }
                    _ => None,
                })
                .collect()
        };
        let full = verbs(Depth::Full);
        let fast = verbs(Depth::Fast);

        // Everything anybody actually does to a file. `open` is deliberately in here: it is the
        // default verb, and a menu whose default is missing is not a context menu.
        for wanted in ["open", "cut", "copy", "delete", "rename", "properties"] {
            assert!(
                fast.iter().any(|v| v == wanted),
                "the reduced menu has no `{wanted}` in it, so it is not usable: {fast:?}"
            );
        }
        // And it really is reduced, or there would be no point to any of this.
        assert!(
            fast.len() < full.len(),
            "the reduced menu came back with as much as the full one ({} against {}), so the \
             flags did nothing: {fast:?}",
            fast.len(),
            full.len()
        );
        eprintln!("full {} verbs, reduced {} verbs", full.len(), fast.len());

        crate::sandbox::remove(&dir);
    }

    /// A menu the shell is taking for ever over must not be able to hold up the next one.
    ///
    /// This is the whole reason [`Builder`] spawns a worker per menu. Right-clicking a 14 MB
    /// executable on a mapped share puts `QueryContextMenu` inside one extension for
    /// twenty-four seconds — measured; see the note on [`Builder`] — and with one thread and a
    /// queue, every menu asked for in that time waited behind it. A right click on a local file
    /// did nothing at all, which is what it looks like from the outside: the context menu has
    /// stopped working.
    ///
    /// The stall stands in for the share. What is being tested is not the shell's speed but the
    /// scheduling: that the second answer arrives while the first build is demonstrably still
    /// running, and that the first one's answer never turns up.
    #[test]
    #[cfg(windows)]
    fn a_slow_menu_does_not_hold_up_the_next_one() {
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        let _serialised = crate::shell::serialised();
        crate::shell::init();
        let (dir, file, _) = scratch("cancel");

        let ctx = egui::Context::default();
        let mut builder = Builder::new(&ctx);

        /// Long enough that a real menu built inside it cannot be the stall finishing early,
        /// short enough that the abandoned worker is gone before the suite is.
        const STALL: u64 = 8_000;
        STALLED.store(0, Ordering::SeqCst);
        STALL_MS.store(STALL, Ordering::SeqCst);
        let slow = builder.build(&dir, std::slice::from_ref(&file), Depth::Full);

        // Wait until the worker is really inside it, so what follows is a menu asked for
        // during a slow build rather than after one.
        let deadline = Instant::now() + Duration::from_secs(5);
        while STALLED.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < deadline,
                "the worker never started the slow build"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // Cleared only now: the stall is a property of the *first* build, and clearing it before
        // that build had picked it up would race it.
        STALL_MS.store(0, Ordering::SeqCst);
        assert!(builder.busy(), "a build is outstanding and the builder says it is not");
        assert!(
            builder.poll().is_none(),
            "the slow build answered in no time, so it was not slow and this proves nothing"
        );

        // The second menu, asked for with the first still running.
        let at = Instant::now();
        let quick = builder.build(&dir, std::slice::from_ref(&file), Depth::Full);
        let entries = loop {
            if let Some(said) = builder.poll() {
                match said {
                    Said::Built { token, entries, .. } => {
                        assert_ne!(
                            token, slow,
                            "the abandoned build answered, and its answer was taken"
                        );
                        assert_eq!(token, quick);
                        break entries;
                    }
                    Said::Filled { .. } => {}
                }
            }
            assert!(
                at.elapsed() < Duration::from_millis(STALL / 2),
                "the second menu has been {:?} and has not arrived -- it is queued behind the \
                 first, which is the bug",
                at.elapsed()
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let took = at.elapsed();

        assert!(
            entries.len() > 3,
            "the second menu came back with only {} entries, so it was not really built",
            entries.len()
        );
        assert!(
            !builder.busy(),
            "the second menu has been delivered and the builder still thinks it is waiting"
        );
        eprintln!("the second menu took {took:?} while the first had {STALL} ms left to run");

        crate::sandbox::remove(&dir);
    }

    /// Where the time goes on a file the shell has to reach across a network for.
    ///
    /// `YAFE_PROBE="H:\some\file.exe" cargo test probe_menu_costs -- --ignored --nocapture`
    #[test]
    #[ignore = "measures the shell against a path of your choosing"]
    #[cfg(windows)]
    fn probe_menu_costs() {
        let Some(path) = std::env::var_os("YAFE_PROBE") else {
            eprintln!("set YAFE_PROBE to a file or folder");
            return;
        };
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        for path in path.to_string_lossy().split(';').filter(|p| !p.is_empty()) {
            let path = PathBuf::from(path);
            let parent = path.parent().unwrap_or(&path).to_owned();

            // The decision in `App::menu_depth` is made on the UI thread before anything slow is
            // allowed to happen, so what it costs is part of the claim.
            let at = std::time::Instant::now();
            let remote = crate::shell::over_network(&parent);
            let cold = at.elapsed();
            let at = std::time::Instant::now();
            let _ = crate::shell::over_network(&parent);
            eprintln!(
                "  over_network -> {remote}: {:.1} µs uncached, {:.1} µs cached",
                cold.as_secs_f64() * 1e6,
                at.elapsed().as_secs_f64() * 1e6
            );
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            eprintln!("\n--- {} ({} bytes)", path.display(), size);
            for _ in 1..=2 {
                let at = std::time::Instant::now();
                let opened = super::win::Live::open(&parent, std::slice::from_ref(&path), Depth::Full);
                let took = at.elapsed().as_secs_f32() * 1e3;
                match opened {
                    Some((live, entries)) => {
                        eprintln!("  Live::open {took:>9.1} ms -> {} entries", entries.len());
                        drop(live);
                    }
                    None => eprintln!("  no menu at all ({took:.1} ms)"),
                }
            }
            super::win::probe(&parent, std::slice::from_ref(&path));
            super::win::probe_flags(&parent, std::slice::from_ref(&path));
        }
    }

    /// Invoking a shell command has to actually do it, and the result has to be usable.
    ///
    /// The menu's own Cut, Copy, Paste, Delete and Rename are the *shell's* entries, so they go
    /// through [`invoke`] by canonical verb, on [`crate::shell::Modal`]. Copy is the one to test
    /// with, because whether it worked is a fact about the clipboard rather than an opinion.
    ///
    /// It did not work, and the way it failed is the point. `InvokeCommand` put the file on the
    /// clipboard perfectly well -- read from the invoking thread it was right there -- and
    /// `IsClipboardFormatAvailable` from anywhere else said the clipboard held no files at all.
    /// Clipboard data belongs to the apartment that put it there, and reading it from elsewhere
    /// is a call back into that apartment. The modal thread was parked in `recv()` and answered
    /// nothing, so a Copy from the context menu was a copy nobody could paste. Hence the wait in
    /// `Modal`'s loop; see [`crate::shell::answering_calls`].
    ///
    /// Goes through the real `Modal` rather than a thread of its own, because a thread of its
    /// own is what made this look like it worked: read the clipboard on the thread that wrote it
    /// and everything is fine.
    #[test]
    #[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
    #[cfg(windows)]
    fn a_shell_verb_from_the_menu_actually_runs() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        crate::shell::clipboard::settle_for_tests();

        let (dir, file, _) = scratch("verb");

        let entries = build(&dir, std::slice::from_ref(&file));
        let copy = entries
            .iter()
            .find_map(|e| match &e.kind {
                Kind::Command(command @ Command::Shell { verb: Some(verb), .. })
                    if verb.eq_ignore_ascii_case("copy") =>
                {
                    Some(command.clone())
                }
                _ => None,
            })
            .expect("every file's menu has a Copy with a canonical verb");

        let ctx = egui::Context::default();
        let mut modal = crate::shell::Modal::new(&ctx);
        assert!(modal.send(crate::shell::Request::Invoke {
            parent: dir.clone(),
            items: vec![file.clone()],
            command: copy,
            owner: crate::shell::Owner::default(),
        }));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while modal.poll().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the modal thread never came back"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(
            crate::shell::clipboard::has_files(),
            "the shell's Copy ran and the clipboard says it holds no files -- the thread that \
             owns them is not answering for them"
        );
        let on_clipboard = crate::shell::clipboard::get()
            .expect("and it has to read back, since that is what a paste does");
        assert_eq!(on_clipboard.items.len(), 1, "{:?}", on_clipboard.items);
        assert!(
            on_clipboard.items[0]
                .to_string_lossy()
                .to_lowercase()
                .ends_with("one.txt"),
            "{:?}",
            on_clipboard.items
        );

        crate::shell::clipboard::clear();
        crate::sandbox::remove(&dir);
    }

    /// New is spotted by verb, because a label is whatever language Windows is in.
    #[test]
    fn the_shell_s_new_entries_are_recognised_by_verb_and_not_by_label() {
        let shell = |verb: Option<&str>| Command::Shell {
            verb: verb.map(str::to_owned),
            id: 0,
        };
        // The New submenu as it reads out of this machine, where the labels are `Dossier`,
        // `Raccourci`, `Document texte` and the verbs are these.
        for verb in ["NewFolder", "NewLink", ".txt", ".bmp", ".docx", ".library-ms", ".7z"] {
            assert!(shell(Some(verb)).creates_an_item(), "`{verb}` makes a file");
        }
        // The rest of a background menu, and the item menu's verbs with it.
        for verb in [
            "properties", "paste", "open", "cut", "copy", "delete", "rename", "link", "view", ".",
            "..", ".a b", "Open with Code",
        ] {
            assert!(
                !shell(Some(verb)).creates_an_item(),
                "`{verb}` does not make a file"
            );
        }
        // An extension offering no canonical verb cannot be told apart, and is not guessed at.
        assert!(!shell(None).creates_an_item());
        assert!(!Command::Own(Own::CopyHere).creates_an_item());
    }

    /// TEMPORARY probe.
    #[test]
    #[ignore = "probe"]
    #[cfg(windows)]
    fn probe_invokable() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        let (dir, file, _) = scratch("invokable");
        // **`YAFE_PROBE` is checked before it is used, not after.** This test used to take the
        // variable, enumerate the menu, and finish with `std::fs::remove_dir_all(&dir)` to clear
        // the scratch folder up. Given `YAFE_PROBE=D:\Sources\MyTools\yet-another-file-explorer`
        // that last line deleted the repository. The cleanup is guarded now
        // ([`crate::sandbox::remove`]), so the damage cannot repeat either way — but failing here
        // says which variable is wrong before anything has run, rather than after.
        let dir = match std::env::var_os("YAFE_PROBE") {
            Some(given) => {
                let given = PathBuf::from(given);
                crate::sandbox::guard("YAFE_PROBE", &[given.clone()]);
                given
            }
            None => dir,
        };
        eprintln!("--- the folder's background menu of {}", dir.display());
        super::win::probe_invokable(&dir, &[]);
        eprintln!("--- a text file");
        super::win::probe_invokable(&dir, std::slice::from_ref(&file));
        crate::sandbox::remove(&dir);
    }

    #[test]
    fn the_right_drag_entries_are_the_only_ones_of_our_own() {
        // Everything else this program used to put above the shell's menu is gone; these three
        // are here because Windows has no answer for "a right-button drag just landed".
        assert_eq!(Own::CopyHere.label(), "Copy here");
        assert_eq!(Own::MoveHere.label(), "Move here");
        assert_eq!(Own::Cancel.label(), "Cancel");
        let entry = Entry::own(Own::CopyHere);
        assert!(entry.enabled);
        assert!(!entry.checked && !entry.default);
        assert!(
            entry.shortcut.is_empty(),
            "a drop answer is not on a shortcut"
        );
    }

    #[test]
    #[cfg(windows)]
    fn shell_labels_lose_their_ampersands_and_keep_their_shortcuts() {
        use super::win::split_label;
        assert_eq!(split_label("&Open"), ("Open".to_owned(), String::new()));
        assert_eq!(
            split_label("Cu&t\tCtrl+X"),
            ("Cut".to_owned(), "Ctrl+X".to_owned())
        );
        // `&&` is a literal ampersand, which "Scan && Repair" depends on.
        assert_eq!(
            split_label("Scan && Repair"),
            ("Scan & Repair".to_owned(), String::new())
        );
        assert_eq!(split_label(""), (String::new(), String::new()));
    }

    /// The chain that actually breaks: paths, the parent folder, the shell's
    /// `IContextMenu`, a populated `HMENU`, and reading it back into entries.
    #[test]
    #[cfg(windows)]
    fn the_shell_fills_a_menu_that_reads_back() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();

        let dir = crate::sandbox::dir("menu");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("one.txt");
        std::fs::write(&file, b"x").expect("write");

        let entries = build(&dir, std::slice::from_ref(&file));
        assert!(
            entries.len() > 3,
            "the shell offered only {} entries -- Open, Cut, Copy, Delete, Rename and \
             Properties should all be there at a minimum",
            entries.len()
        );

        // Every entry has to be drawable and doable.
        for entry in &entries {
            match &entry.kind {
                Kind::Separator => {}
                Kind::Submenu { children, .. } => {
                    assert!(!children.is_empty(), "{}", entry.label)
                }
                Kind::Command(_) => assert!(!entry.label.is_empty()),
            }
            assert!(
                !entry.label.contains('&') || entry.label.matches('&').count() == 1,
                "`{}` still has an accelerator marker in it",
                entry.label
            );
            assert!(!entry.label.contains('\t'), "`{}`", entry.label);
        }

        // The commands worth having should be recognisable by verb.
        let verbs: Vec<String> = entries
            .iter()
            .filter_map(|e| match &e.kind {
                Kind::Command(Command::Shell { verb, .. }) => verb.clone(),
                _ => None,
            })
            .collect();
        assert!(
            verbs.iter().any(|v| v.eq_ignore_ascii_case("properties")),
            "no Properties verb among {verbs:?}"
        );
        assert!(
            verbs.iter().any(|v| v.eq_ignore_ascii_case("copy")),
            "no Copy verb among {verbs:?}"
        );

        // And the folder's own menu, which is a different shell object and not this one with the
        // items left out. Only that it answers at all is checked here; *which* object answered is
        // `a_folder_with_nothing_selected_gets_the_background_menu`, since a count cannot say.
        assert!(
            !build(&dir, &[]).is_empty(),
            "the folder's background menu came back empty"
        );

        crate::sandbox::remove(&dir);
    }

    #[test]
    #[cfg(windows)]
    fn submenus_are_populated_rather_than_left_empty() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        let dir = crate::sandbox::dir("submenu");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("one.txt");
        std::fs::write(&file, b"x").expect("write");

        let entries = build(&dir, std::slice::from_ref(&file));
        let submenus: Vec<&Entry> = entries
            .iter()
            .filter(|e| matches!(e.kind, Kind::Submenu { .. }))
            .collect();
        // A plain text file on any Windows has at least "Open with" or "Send to".
        assert!(
            !submenus.is_empty(),
            "no submenu came back at all, which means `WM_INITMENUPOPUP` is not reaching \
             the extensions: {:?}",
            entries.iter().map(|e| &e.label).collect::<Vec<_>>()
        );
        for submenu in submenus {
            if let Kind::Submenu { children, .. } = &submenu.kind {
                assert!(
                    !children.is_empty(),
                    "`{}` came back empty -- the submenu was never asked to fill itself",
                    submenu.label
                );
            }
        }

        crate::sandbox::remove(&dir);
    }
}
