//! The parts of this program that *are* the shell rather than merely near it.
//!
//! Everything here goes through the same interfaces Explorer uses, so the results
//! are the same results — the same icons, the same context menu including whatever
//! extensions are installed, the same copy/move/delete with the same progress
//! dialogs, the same conflict prompts, the same undo, and the same Recycle Bin.
//! Re-implementing any of that would mean a file manager whose Delete is *nearly*
//! Explorer's Delete, and nearly is the wrong target for something that moves a
//! user's files.
//!
//! | module | what it is |
//! | --- | --- |
//! | [`icons`] | the system image list, cached by file type |
//! | [`cloud`] | `System.StorageProviderState` — the sync provider's Status column |
//! | [`links`] | `IShellLink` — what a shortcut points at |
//! | [`clipboard`] | `CF_HDROP` and `Preferred DropEffect` — cut, copy, paste |
//! | [`ops`] | `IFileOperation` — copy, move, delete, rename, new folder |
//! | [`menu`] | `IContextMenu` — the real menu, extensions included |
//! | [`dnd`] | OLE drag and drop, as a source and as a target |
//! | [`providers`] | which file types this machine can draw a picture of, by registration |
//! | [`winkey`] | `Win+E` — whether Windows' folder key opens this program |
//!
//! # Threading
//!
//! COM is apartment-threaded and the shell is emphatic about it: `IContextMenu` and
//! `IFileOperation` both put up windows, so both have to run on a thread with a
//! message pump — which for this program means the UI thread, initialised as an STA
//! by [`init`]. The icon lookups are the exception: they call one function that is
//! documented as free-threaded and touch nothing else, so they run on workers.
//!
//! An apartment is a promise, and the promise has a second half that is easy to miss:
//! anything a thread hands out can be called back into, and the caller *blocks* until it is
//! answered. A thread of this program ’s that parks in `recv` answers nothing, and what that
//! breaks is not obvious from the failure. See [`answering_calls`], which is what every wait
//! here goes through, and the note on [`Modal`].

pub mod clipboard;
pub mod cloud;
pub mod dnd;
pub mod icons;
pub mod links;
pub mod menu;
pub mod ops;
pub mod providers;
pub mod thumbs;
pub mod winkey;

#[cfg(windows)]
#[path = "../windows/com.rs"]
mod win;
#[cfg(windows)]
pub(crate) use win::{
    answering_calls, display_name, global_medium, hdrop_paths, hglobal_format, nul_terminated,
    parse_item, parsed_pidl, pidl_of, take_co_string, token_information, until_nul, wide,
    wide_text, win32_message, Apartment, Pidl, CF_HDROP, FSCTL_SET_REPARSE_POINT,
};
#[cfg(windows)]
#[path = "../windows/registry.rs"]
pub(crate) mod registry;

/// Initialise COM for the calling thread as a single-threaded apartment.
///
/// Called once, from the UI thread, before anything else here. The shell requires an
/// STA for the interfaces that show UI, and `OleInitialize` rather than
/// `CoInitializeEx` because drag and drop and the clipboard need OLE's own setup on
/// top of COM's.
pub fn init() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Ole::OleInitialize;
        // SAFETY: called once, on the thread that will own every call below. A failure
        // means the shell features degrade to nothing, which the callers all handle.
        let _ = unsafe { OleInitialize(None) };
    }
}

#[cfg(not(windows))]
pub(crate) fn answering_calls(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

/// Render whatever this program has put on the clipboard, so it outlives the process.
///
/// `OleSetClipboard` copies nothing. It leaves the clipboard holding a reference to the data
/// object *here*, and another process asking for the bytes is a marshalled call back into this
/// apartment — which is why a copy taken here pastes into Explorer while this window is open,
/// and why it would paste into nothing at all a moment after the window closed. `OleFlushClipboard`
/// renders every format for real, so the clipboard keeps the files rather than a pointer to a
/// process that has gone.
///
/// Called on the way out and not after every copy, deliberately: while the object is still
/// this program's, a paste elsewhere can hand back `CFSTR_PASTESUCCEEDED` and finish a cut
/// properly. A flush replaces it with a snapshot, and that conversation is over.
pub fn flush() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Ole::OleFlushClipboard;
        // SAFETY: documented as safe to call whether or not this process owns the clipboard;
        // it does nothing when it does not.
        let _ = unsafe { OleFlushClipboard() };
    }
}

/// Serialises the tests that reach into the shell.
///
/// Cargo runs a crate's tests as parallel threads of one process, and some of what the
/// shell does on the way to filling a context menu — an extension asking whether there is
/// anything to paste, say — takes the **process-wide** clipboard lock. That is enough to
/// make [`clipboard`]'s round trip come back "OpenClipboard refused", intermittently and
/// only ever in a full run, which is the worst way to learn it. Every test that goes
/// through the shell takes this first.
///
/// The production code does not need it: the retry in [`clipboard`] covers the contention
/// a real desktop produces, and there is one UI thread rather than eight.
///
/// **Thumbnails are the other measurement behind it.** Two `GetImage` calls in flight from one
/// process answer *one of them* with a failure, whether or not they are about the same file —
/// reproduced by letting two of [`thumbs`]' tests run in parallel, in both orders and on both
/// files. It is a property of the tests rather than of the service, which has exactly one worker;
/// and it is this lock rather than one of their own because `GetImage` is called from more than
/// one module, and two private locks would exclude each test from its own neighbours and from
/// nothing else.
#[cfg(test)]
pub(crate) static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`ONE_AT_A_TIME`], ignoring a poisoning from some other test's panic: a failing test has
/// a failure to report, and the next one refusing to run because the first panicked would hide it.
#[cfg(test)]
pub(crate) fn serialised() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every visible top-level window this process owns, as `(HWND, class, title)` — for the tests
/// that watch what the shell puts on screen.
#[cfg(all(test, windows))]
pub(crate) fn windows_of_this_process() -> Vec<(isize, String, String)> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let found = &mut *(lparam.0 as *mut Vec<(isize, String, String)>);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != GetCurrentProcessId() || !IsWindowVisible(hwnd).as_bool() {
            return TRUE;
        }
        let mut class = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut class);
        let mut title = [0u16; 512];
        let m = GetWindowTextW(hwnd, &mut title);
        found.push((
            hwnd.0 as isize,
            String::from_utf16_lossy(&class[..n.max(0) as usize]),
            String::from_utf16_lossy(&title[..m.max(0) as usize]),
        ));
        TRUE
    }

    let mut found: Vec<(isize, String, String)> = Vec::new();
    // SAFETY: the callback only writes through the pointer it is handed, which outlives
    // the enumeration.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut found as *mut _ as isize));
    }
    found
}

/// Ask a window to go away, which for a shell dialog is a cancel.
#[cfg(all(test, windows))]
pub(crate) fn close_window(hwnd: isize) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
    // SAFETY: posting is asynchronous and safe against a window that has already gone.
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut std::ffi::c_void)),
            WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// Book a frame a tenth of a second from now, for an answer that has landed or a request that has
/// been turned away and has to be able to ask again.
///
/// Delayed rather than immediate: answers arrive in bursts, and one frame after the burst is worth
/// sixty-four during it — and a hundred refused requests are one wake-up rather than a hundred.
pub(crate) fn nudge(ctx: &egui::Context) {
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

/// `image` padded into a transparent `cell`×`cell` square from its top-left corner, cropped where it
/// is bigger, and the size of the part that went in.
///
/// The whole cell, not only the part the picture covers: an atlas slot is reused, and a smaller
/// picture arriving in it would otherwise be drawn over the corner of whatever was there before. A
/// row at a time rather than a pixel at a time: a 96-pixel cell is 9,216 bounds-checked index pairs.
pub(crate) fn into_cell(image: &egui::ColorImage, cell: usize) -> (egui::ColorImage, [usize; 2]) {
    let mut out = egui::ColorImage::filled([cell, cell], egui::Color32::TRANSPARENT);
    let [across, h] = image.size;
    let (w, h) = (across.min(cell), h.min(cell));
    for y in 0..h {
        let from = &image.pixels[y * across..y * across + w];
        out.pixels[y * cell..y * cell + w].copy_from_slice(from);
    }
    (out, [w, h])
}

/// `dir\stem (2).ext`, `dir\stem (3).ext` and up, for a `name` in `dir` — the names Explorer gives a
/// second copy of something, the number before the extension.
///
/// Unbounded and unchecked: the callers decide whether the plain name is free, what counts as
/// taken, and how far to count. A dot at the very start is a name and not an extension, so it is
/// `.gitignore (2)`; a name ending in a dot keeps it, `a (2).`.
pub(crate) fn numbered<'a>(
    dir: &'a std::path::Path,
    name: &std::ffi::OsStr,
) -> impl Iterator<Item = std::path::PathBuf> + 'a {
    let name = std::path::Path::new(name);
    let stem = name.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = name
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    (2u32..).map(move |n| dir.join(format!("{stem} ({n}){ext}")))
}

/// The window the shell should parent its dialogs to.
///
/// A progress dialog with no owner appears behind the window that started it and
/// looks like it has hung. `None` is safe — the shell then owns them to the desktop —
/// but it is worth having.
#[derive(Clone, Copy, Default)]
pub struct Owner(pub isize);

impl Owner {
    /// Take the handle out of whatever eframe was given by the platform.
    pub fn from_handle(handle: &dyn raw_window_handle::HasWindowHandle) -> Self {
        #[cfg(windows)]
        {
            use raw_window_handle::RawWindowHandle;
            if let Ok(handle) = handle.window_handle() {
                if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                    return Self(win32.hwnd.get());
                }
            }
        }
        #[cfg(not(windows))]
        let _ = handle;
        Self(0)
    }

    #[cfg(windows)]
    pub(crate) fn hwnd(self) -> windows::Win32::Foundation::HWND {
        windows::Win32::Foundation::HWND(self.0 as *mut std::ffi::c_void)
    }

    /// The same handle for the flat `windows-sys` calls, whose `HWND` is a bare pointer rather
    /// than the `windows` crate's newtype. Two spellings of one number, which is the price of
    /// keeping both crates — see `Cargo.toml`.
    #[cfg(windows)]
    pub(crate) fn raw(self) -> windows_sys::Win32::Foundation::HWND {
        self.0 as windows_sys::Win32::Foundation::HWND
    }
}

/// A GUID nothing has used before, in the form everything writes it in.
///
/// `Ctrl+G` in a rename field, which is the only caller: a name that has to be unique and mean
/// nothing is a thing developers type several times a day, and typing one by hand is not something
/// anybody does correctly.
///
/// **`CoCreateGuid` rather than arithmetic of this program's own**, which is what puts it in this
/// module: it is the platform's own version 4 generator, seeded by the platform's own entropy, and it
/// is the same call every tool on the machine that offers this makes. Writing a v4 by hand would mean
/// a random source, and a random source chosen for a keyboard shortcut is a dependency and a decision
/// about entropy that this has no business making.
///
/// Lowercase, hyphenated, and **no braces**: the form a source file, a config and a database column
/// want. Windows' own tools write `{...}` because a registry key needs the delimiters, which a
/// filename does not — and a name with braces in it is a name that has to be quoted in every shell
/// it is ever passed to.
///
/// `None` where there is no such call to make, which is the same answer [`over_network`] gives and
/// means the same thing: the shortcut inserts nothing rather than inventing something.
pub fn new_guid() -> Option<String> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Com::CoCreateGuid;

        // SAFETY: fills in one `GUID` of the size the signature declares. Documented as callable
        // from any thread, and it is called from the one that has an apartment anyway.
        let guid = unsafe { CoCreateGuid() }.ok()?;
        let [a, b, c, d, e, f, g, h] = guid.data4;
        Some(format!(
            "{:08x}-{:04x}-{:04x}-{a:02x}{b:02x}-{c:02x}{d:02x}{e:02x}{f:02x}{g:02x}{h:02x}",
            guid.data1, guid.data2, guid.data3
        ))
    }
    #[cfg(not(windows))]
    None
}

/// Whether reading this path means going over a network.
///
/// True for a UNC path, and for a drive letter that is a mapped share. Used by [`menu`] to
/// decide how much of a context menu it is worth asking the shell for, which is a decision
/// that has to be made before anything slow is allowed to happen — so this must be cheap.
///
/// `GetDriveTypeW` is: it answers from the mount table and is documented not to touch the
/// network, so a share that has gone away still comes back `DRIVE_REMOTE` rather than hanging.
/// Measured by `probe_menu_costs` at **376 µs** for a mapped `H:\` the first time and 26 µs
/// after, against 41 µs and 6 µs for a local drive — so the first look at a share costs about a
/// third of a millisecond, and the cache is what keeps every right click after it from doing so.
/// Either way it is four orders of magnitude under the menu it is deciding about.
pub fn over_network(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Storage::FileSystem::GetDriveTypeW;
        use windows::Win32::System::WindowsProgramming::DRIVE_REMOTE;

        // `\\server\share\...`, which is a network path by construction and needs no asking.
        if crate::fs::is_network_unc(path) {
            return true;
        }

        static KNOWN: PerRoot = std::sync::Mutex::new(None);
        per_root(path, &KNOWN, |root| {
            let root_wide = wide(root);
            // SAFETY: null-terminated by `wide`, and it outlives the call.
            let kind = unsafe { GetDriveTypeW(windows::core::PCWSTR(root_wide.as_ptr())) };
            kind == DRIVE_REMOTE
        })
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// One yes-or-no answer per drive root, remembered for the session. See [`per_root`].
#[cfg(windows)]
pub(crate) type PerRoot =
    std::sync::Mutex<Option<std::collections::HashMap<std::path::PathBuf, bool>>>;

/// What `ask` says about the root `path` is on, asked once per root and remembered in `known`.
///
/// `H:\a\b` -> `H:\`, which is what the volume calls want. Anything without a root — a relative
/// path, or the empty path that means This PC — is not ours to judge, and is `false`. The lock is
/// held while `ask` runs, so two threads asking about one new drive ask the system once.
#[cfg(windows)]
pub(crate) fn per_root(
    path: &std::path::Path,
    known: &PerRoot,
    ask: impl FnOnce(&std::path::Path) -> bool,
) -> bool {
    let Some(root) = path.ancestors().last().filter(|r| !r.as_os_str().is_empty()) else {
        return false;
    };
    let mut known = known.lock().unwrap_or_else(|e| e.into_inner());
    let known = known.get_or_insert_with(std::collections::HashMap::new);
    if let Some(&answer) = known.get(root) {
        return answer;
    }
    let answer = ask(root);
    known.insert(root.to_owned(), answer);
    answer
}

/// Start a child process with no console window behind it.
///
/// `CREATE_NO_WINDOW`. Without it a **debug** build — which is a console-subsystem binary, so that
/// `--trace` and the probes have somewhere to print — hands its console to the child and a black
/// window flashes over the pane. A release build has no console to hand over and would not flash,
/// which is exactly what makes this the kind of bug that ships.
///
/// Here rather than in either caller because this program has two: the shell behind the console
/// panel, and `git`. One statement about how a child is started, so the second one cannot be
/// written without it.
pub fn no_window(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Start a child process **with** a console window of its own.
///
/// The opposite of [`no_window`], and the one thing this program cannot give a child any other way: a
/// release build is a `windows` subsystem binary with no console to share, so a program that wants a
/// terminal — `claude`, `vim`, anything that draws over the screen — has to be handed a new one. See
/// [`crate::console::in_terminal`], which is the only caller and the only reason this exists.
///
/// Said outright rather than by leaving `CREATE_NO_WINDOW` off: a debug build *does* have a console,
/// so the absent flag would mean two different things in the two builds, and the one that worked would
/// be the one nobody ships.
pub fn new_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        command.creation_flags(CREATE_NEW_CONSOLE);
    }
    #[cfg(not(windows))]
    let _ = command;
}

// ---------------------------------------------------------------------------
// The modal thread
// ---------------------------------------------------------------------------

/// Something that puts up a window of its own and does not return until the user is done
/// with it.
pub enum Request {
    /// Run a shell command chosen from the context menu.
    Invoke {
        parent: std::path::PathBuf,
        items: Vec<std::path::PathBuf>,
        command: menu::Command,
        /// How much of a menu the one it was chosen from was, so the menu it is resolved
        /// against is built the same way. See [`menu::invoke`].
        depth: menu::Depth,
        owner: Owner,
    },
}

/// What one of those turned out to be.
pub enum Reply {
    /// A command ran. What it did is unknown, and deliberately not acted on: the folder is
    /// watched, so a command that changed it is noticed like any other change on disk, and one
    /// that did not costs no scan. See `crate::app::App::collect_modal`.
    Invoked,
}

/// Runs the shell calls that put up a window of their own, on a thread of their own.
///
/// # Why not on the UI thread
///
/// `InvokeCommand` can open anything from a Properties sheet to an installer. Called from
/// inside the frame — which is where every other action is performed — it would dispatch
/// messages to this window's own procedure; winit would turn a `WM_PAINT` into a redraw
/// request; eframe would call straight back into `Context::run_ui`; and egui would be asked
/// to begin a pass while already inside one. Whether that panics or deadlocks depends on
/// timing, which is the worst kind of bug to ship: it works while you are testing it.
///
/// It is documented as belonging to a *thread* rather than to a window, so it runs here,
/// where there is no egui pass to re-enter, and the answer comes back by channel like any
/// other background result.
///
/// # What is deliberately *not* here
///
/// **Building the menu.** `QueryContextMenu` only fills an `HMENU` and shows nothing, so
/// [`menu::build`] is safe to call during a frame — which is what lets the menu be drawn by
/// this program rather than by Windows.
///
/// **Starting a drag.** `DoDragDrop` is modal in the same way and for the same reason, but it
/// gets a thread of its own per drag rather than sharing this one: it has to join the UI
/// thread's input queue for the length of the drag to see the button that started it, and this
/// thread is long-lived and shared. See [`dnd::Drag`].
pub struct Modal {
    tx: std::sync::mpsc::Sender<Request>,
    rx: std::sync::mpsc::Receiver<Reply>,
    /// Whether a request is outstanding. One at a time: they are modal, and a second
    /// menu behind the first would be a menu nobody asked for.
    busy: bool,
}

impl Modal {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, requests) = std::sync::mpsc::channel::<Request>();
        let (replies, rx) = std::sync::mpsc::channel::<Reply>();
        let ctx = ctx.clone();

        let spawned = std::thread::Builder::new()
            .name("shell-modal".to_owned())
            .spawn(move || {
                // This thread's own apartment: the menu's window and every interface it
                // touches belong to it.
                init();
                // Not `recv()`. This thread runs shell commands, and one of them is Copy: the
                // clipboard data then belongs to *this* apartment, and every read of it — by
                // this program or by any other — is a call back into here. Parked in `recv()`
                // this thread answered none of them, so a Copy from the context menu put the
                // files on the clipboard and nothing whatsoever could read them back. Fifty
                // milliseconds of latency on a menu click is not perceptible; a clipboard that
                // silently does nothing is.
                loop {
                    let request = match requests.try_recv() {
                        Ok(request) => request,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            answering_calls(50);
                            continue;
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    };
                    let reply = match request {
                        Request::Invoke {
                            parent,
                            items,
                            command,
                            depth,
                            owner,
                        } => {
                            menu::invoke(&parent, &items, &command, depth, owner);
                            Reply::Invoked
                        }
                    };
                    if replies.send(reply).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        let _ = spawned;

        Self {
            tx,
            rx,
            busy: false,
        }
    }

    /// Ask for a modal gesture. `false` when one is already running.
    pub fn send(&mut self, request: Request) -> bool {
        if self.busy {
            return false;
        }
        // **Never the user's Recycle Bin from a test.** Its items are real deletions in
        // `$RECYCLE.BIN`, which no sandbox contains, and the verbs on them — Delete, Empty — are
        // permanent. [`ops::FOR_REAL`] is the same rule for `IFileOperation`; this is the one route
        // to a shell verb that does not pass through it, and a pane showing the bin sends its
        // Delete this way. See [`crate::fs::recycle`].
        #[cfg(test)]
        {
            let Request::Invoke { parent, .. } = &request;
            if crate::fs::recycle::is_bin(parent) {
                return false;
            }
        }
        if self.tx.send(request).is_ok() {
            self.busy = true;
            return true;
        }
        false
    }

    /// The answer, once there is one.
    pub fn poll(&mut self) -> Option<Reply> {
        match self.rx.try_recv() {
            Ok(reply) => {
                self.busy = false;
                Some(reply)
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    /// The names a second copy is given, as each of [`super::numbered`]'s callers gave them before
    /// they shared it: the number before the last extension, a leading dot part of the name, and a
    /// trailing one kept where it was.
    #[test]
    fn the_number_goes_before_the_last_extension() {
        let names = |name: &str| -> Vec<PathBuf> {
            super::numbered(Path::new(r"C:\d"), OsStr::new(name)).take(2).collect()
        };
        let want = |a: &str, b: &str| vec![PathBuf::from(a), PathBuf::from(b)];
        assert_eq!(names("one.txt"), want(r"C:\d\one (2).txt", r"C:\d\one (3).txt"));
        assert_eq!(names("one.txt.lnk"), want(r"C:\d\one.txt (2).lnk", r"C:\d\one.txt (3).lnk"));
        assert_eq!(names("a folder.lnk"), want(r"C:\d\a folder (2).lnk", r"C:\d\a folder (3).lnk"));
        assert_eq!(names("a folder"), want(r"C:\d\a folder (2)", r"C:\d\a folder (3)"));
        assert_eq!(names(".gitignore"), want(r"C:\d\.gitignore (2)", r"C:\d\.gitignore (3)"));
        assert_eq!(names("a."), want(r"C:\d\a (2).", r"C:\d\a (3)."));
        assert_eq!(names("..x"), want(r"C:\d\. (2).x", r"C:\d\. (3).x"));
    }
}
