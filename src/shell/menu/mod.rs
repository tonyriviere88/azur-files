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
//! states, the accelerator text, the item bitmaps and the submenus. `TrackPopupMenuEx` is
//! never called, and no native menu ever appears.
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
//! canonical name — `open`, `copy`, `delete`, `properties`, or a CLSID in braces for
//! anything registered as an `IExplorerCommand` — which is stable and can be used from any
//! thread against a freshly obtained `IContextMenu`. That matters because invoking a command
//! can put up a dialog, which has to happen off the UI thread; see [`crate::shell::Modal`].
//!
//! Where an extension offers no canonical verb the numeric id is used instead, and then the
//! *whole shape* of the menu has to be reproduced: identical flags, identical items, and the
//! submenu it came from populated, because that is when the ids inside a submenu are handed
//! out. All of which is [`invoke`], and all three of them were wrong at once — read the note
//! there before changing how a command is resolved.
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
//!
//! **It is not only the submenus.** Measured on a selected folder, five entries at the *top*
//! level are missing from the first menu of a process and present in every one after it:
//! `Ouvrir dans le Terminal`, `Open with Zed`, `Renommer avec PowerRename`, `Unlock with File
//! Locksmith` and one of the two `Déplacer vers OneDrive` — 30 entries against 35. Every one of
//! them is an `IExplorerCommand`, which is how anything written for Windows 11 registers, so
//! what the first menu is short of is the modern half of the menu.
//!
//! Two things follow. A `--shot --menu` capture only ever builds one menu per process, so a
//! screenshot of this program's context menu is a screenshot of the short one — which is not a
//! bug in the capture and is worth knowing before hunting for one. And the *ids* in that first
//! menu are five entries out from the ids in every later one, which matters when a command has
//! no canonical verb and can only be named by its number: see [`win::resolve`].

use std::path::{Path, PathBuf};

/// One of this program's own entries, which the shell knows nothing about.
///
/// There used to be a dozen of these above every shell menu — Open in new tab, Refresh, Select
/// all, Copy path and the rest. They are gone: a context menu on a file or a folder now shows
/// Windows' own menu and nothing else, which is what it claims to be. Every command they carried
/// is still on its keyboard shortcut, and most of them are in the shell's menu anyway under the
/// name Explorer gives it.
///
/// What is left is the two things the shell has no answer for, because neither is the shell's
/// question: where a right-button drag has just landed, and Paste on empty space.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Own {
    /// The four a right-button drag offers when it lands, which is how Windows has asked
    /// "copy, move, or a shortcut?" since it stopped guessing.
    CopyHere,
    MoveHere,
    /// Explorer's *Create shortcuts here*, and the same operation an Alt-drag performs —
    /// [`crate::shell::ops::Job::Link`]. Plural in Explorer's own wording whatever the count,
    /// which is worth keeping: the label is a menu entry rather than a sentence about this drag.
    LinkHere,
    Cancel,
    /// Paste, on a folder's **background** menu — the one gap the shell leaves.
    ///
    /// Every other entry in that menu comes from Windows, and this one has to be ours because
    /// Windows does not put it there. `CreateViewObject`'s menu carries no `paste` verb — see
    /// [`win::context_of`] for the two menus a folder has and what is in each — and Explorer's own
    /// Paste on empty space is synthesised by its view, around the shell's menu rather than out of
    /// it. So there is nothing to redirect the way `cut`, `copy` and the `paste` on a *selected*
    /// folder are redirected (see [`crate::app::App::ours_rather_than_the_shell_s`]): a right click
    /// on empty space either shows this entry or shows no Paste at all.
    ///
    /// It goes through the same [`crate::shell::ops`] engine as Ctrl+V, so it is the same paste
    /// with the same progress, conflicts and undo — the shortcut and the entry cannot drift.
    Paste,
}

impl Own {
    pub fn label(self) -> &'static str {
        match self {
            Self::CopyHere => "Copy here",
            Self::MoveHere => "Move here",
            Self::LinkHere => "Create shortcuts here",
            Self::Cancel => "Cancel",
            // English, among a menu Windows has filled in French. Deliberate, and the same
            // choice as the three above: these entries are this program's, and this program's
            // interface is in English throughout — the sidebar says Drives and Bookmarks. What
            // is in French is what Windows wrote, which is everything else in this menu.
            Self::Paste => "Paste",
        }
    }
}

/// What activating an entry does.
#[derive(Clone, Debug)]
pub enum Command {
    /// One of this program's own.
    Own(Own),
    /// A shell command, and everything needed to run it later against a menu built afresh.
    ///
    /// The menu it was read from is gone by then — see [`invoke`] — so all four of these are
    /// carried rather than looked up again.
    Shell {
        /// Its canonical verb, where the shell gave one.
        verb: Option<String>,
        /// Its id, as an offset from the first command id this program handed out.
        id: u32,
        /// The submenus it sits inside, as positions in each enclosing `HMENU`, root first.
        ///
        /// Empty for a top-level entry. It is what lets an entry with **no** canonical verb be
        /// invoked at all: the id only exists in a menu whose submenu has been populated, and a
        /// menu built afresh has populated none of them. So `Send to > Documents` and
        /// `Open with > Notepad` — neither of which has a verb — used to invoke nothing
        /// whatsoever, because the id read from a populated submenu was handed to a menu where
        /// nothing had assigned it. See [`win::invoke`], which walks this and sends each level
        /// its `WM_INITMENUPOPUP` before using the id.
        path: Vec<u32>,
        /// The label it was shown under, so the id can be checked against the rebuilt menu
        /// before it is used. See [`win::invoke`] — a numeric id is a *position* in somebody
        /// else's numbering, and the one thing worse than a menu entry that does nothing is one
        /// that does something else.
        label: String,
    },
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
            icon: None,
        }
    }
}

/// Put this program's Paste at the top of a folder's **background** menu.
///
/// For the one menu that arrives from the shell with a gap in it. See [`Own::Paste`] for why the
/// gap is there and cannot be closed by redirecting a verb.
///
/// `can_paste` greys the entry rather than hiding it, which is what Explorer does and is the more
/// useful of the two: an entry that disappears when the clipboard is empty reads as a program that
/// has no Paste. It is passed in rather than read here so that the menu's shape can be tested
/// without taking over the desktop's one clipboard;
/// [`crate::app::App::pump_menu`] is the caller and asks
/// [`crate::shell::clipboard::has_files`], which is cheap enough for the once per menu it is asked.
///
/// # Why prepending is safe
///
/// It was not always. Putting this program's entries above the shell's used to break every submenu
/// in the program, because a submenu was identified by its *path* through the entries and the path
/// on screen no longer matched the one the shell had filed it under. That is why [`Kind::Submenu`]
/// carries an opaque `source` id instead — read the note there. Nothing else in an entry is a
/// position in this list either: a [`Command::Shell`]'s `id` and `path` are positions in the
/// shell's own `HMENU`, recorded when it was read and untouched by what is put in front of them.
pub fn with_our_paste(entries: Vec<Entry>, can_paste: bool) -> Vec<Entry> {
    let mut paste = Entry::own(Own::Paste);
    paste.enabled = can_paste;

    let mut ours = vec![paste];
    // Only when there is something to be divided from. The shell's background menu is never
    // actually empty, but a separator hanging off the bottom of a menu is the kind of thing that
    // shows up the one time it is.
    if !entries.is_empty() {
        ours.push(Entry::separator());
    }
    ours.extend(entries);
    ours
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
pub fn invoke(
    parent: &Path,
    items: &[PathBuf],
    command: &Command,
    depth: Depth,
    owner: super::Owner,
) {
    #[cfg(test)]
    {
        let mut paths = vec![parent.to_path_buf()];
        paths.extend(items.iter().cloned());
        crate::sandbox::guard("IContextMenu::InvokeCommand", &paths);
    }
    #[cfg(windows)]
    if matches!(command, Command::Shell { .. }) {
        win::invoke(parent, items, command, depth, owner);
    }
    #[cfg(not(windows))]
    let _ = (parent, items, command, depth, owner);
}

#[cfg(windows)]
#[path = "../../windows/menu.rs"]
mod win;

#[cfg(test)]
mod tests;
