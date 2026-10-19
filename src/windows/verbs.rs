//! `ShellExecuteW`: opening a file with whatever owns it, revealing one in Explorer, and
//! starting a terminal.
//!
//! The Windows half of [`crate::fs::shell`].

use super::*;

#[cfg(windows)]
pub(super) fn run(verb: Option<&str>, file: &std::ffi::OsStr, args: Option<&std::ffi::OsString>) {
    run_ok(verb, file, args, None);
}

/// `ShellExecuteW`, with every string null-terminated and the working directory
/// optional. Returns whether the shell managed to start something.
#[cfg(windows)]
pub(super) fn run_ok(
    verb: Option<&str>,
    file: &std::ffi::OsStr,
    args: Option<&std::ffi::OsString>,
    dir: Option<&Path>,
) -> bool {
    use crate::shell::nul_terminated;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb = verb.map(nul_terminated);
    let file = nul_terminated(file);
    let args = args.map(nul_terminated);
    let dir = dir.map(nul_terminated);

    let ptr = |v: &Option<Vec<u16>>| v.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());

    // SAFETY: every pointer is either null or into a buffer that outlives the
    // call, and each is null-terminated.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            ptr(&verb),
            file.as_ptr(),
            ptr(&args),
            ptr(&dir),
            SW_SHOWNORMAL,
        )
    };
    // Documented contract: anything above 32 is success.
    result as isize > 32
}

/// Open a picture, a clip or a track **from a folder view**, so that the viewer can step to the
/// next one — and `ShellExecuteW` if that cannot be done.
///
/// # Why a whole Explorer view, for one double click
///
/// The Photos app's previous and next walk **the view it was opened from**. Explorer's `open` hands
/// the AppX handler its folder view as the context menu's site, the handler asks that site for
/// `SID_SFolderView`, and Photos reads the neighbours out of what it gets — once, at startup, in the
/// view's order. `ShellExecuteW` has no view to hand over, so the arrows are simply not there.
///
/// Measured by driving Photos with `SendInput` on three sandbox pictures, before this existed:
///
/// | how it was opened | ← and → |
/// | --- | --- |
/// | Explorer, double click | step through all three |
/// | `ShellExecuteW` | nothing |
/// | WinRT `Launcher` with a `NeighboringFilesQuery` — the documented way | nothing: the launch succeeds and the query is ignored for an unpackaged caller |
/// | a context menu sited on an `IFolderView2` of this program's own | nothing: the handler also asks for a private interface only Explorer's view implements |
/// | `InvokeVerbOnSelection` on a hidden `IExplorerBrowser` | all three, in the order the view was sorted |
///
/// So the last row: Explorer's own view, in a window nobody sees, sorted the way the pane is, with
/// the file selected — not an imitation of what Explorer does but the same object doing it. Any
/// other handler is launched exactly as `ShellExecuteW` would: it is the same default verb.
///
/// **On a thread of its own**, in a single-threaded apartment with its own message loop, because
/// the view lists the folder through that loop — a few hundred milliseconds here, as long as a share
/// takes on a remote one — and none of that is the window's to wait for.
#[cfg(windows)]
pub(super) fn launch_among(path: &Path, sort: crate::fs::Sort) {
    let owned = path.to_owned();
    let spawned = std::thread::Builder::new()
        .name("launch".into())
        .spawn(move || {
            // Declared first so it is left last, after the view below has gone.
            let Some(_apartment) = crate::shell::Apartment::enter_sta() else {
                run(None, owned.as_os_str(), None);
                return;
            };
            // SAFETY: on this thread, inside the apartment it just joined; `Hidden` owns everything
            // the call creates and gives it back when it drops.
            match unsafe { Hidden::open_from(&owned, sort) } {
                // Held, and the loop run, until the viewer has had what it came for.
                Ok(_view) => pump(KEPT),
                Err(_) => run(None, owned.as_os_str(), None),
            }
        });
    if spawned.is_err() {
        run(None, path.as_os_str(), None);
    }
}

/// How long the hidden view outlives the launch.
///
/// **It has to be there when the viewer asks**, and that is after the launch rather than during
/// it: torn down the moment `InvokeVerbOnSelection` returned, Photos came up with no neighbours at
/// all; kept for three seconds, it had them and went on stepping long after the view had gone. A
/// cold start of Photos is what is being waited out, so this is generous rather than tight — one
/// hidden window and one folder's view, for half a minute, per picture opened.
#[cfg(windows)]
const KEPT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long the view has to list the file before the launch goes to `ShellExecuteW` instead.
///
/// A file the view never shows — a hidden picture, in a view that does not show hidden files — is
/// the case this is for, and it would otherwise wait for ever.
#[cfg(windows)]
const LISTED: std::time::Duration = std::time::Duration::from_secs(3);

/// An Explorer view in a window that is never shown. Destroyed, window and all, when it drops.
#[cfg(windows)]
struct Hidden {
    window: windows::Win32::Foundation::HWND,
    browser: windows::Win32::UI::Shell::IExplorerBrowser,
}

#[cfg(windows)]
impl Hidden {
    /// Browse to the file's folder, sort it, select the file, and run its default verb from there.
    unsafe fn open_from(path: &Path, sort: crate::fs::Sort) -> windows::core::Result<Self> {
        use windows::core::{w, PCSTR};
        use windows::Win32::Foundation::{E_FAIL, RECT};
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::{
            ExplorerBrowser, IExplorerBrowser, IFolderView2, IShellView, ILFindLastID,
            EBO_NOBORDER, FOLDERSETTINGS, FVM_DETAILS, FWF_NOWEBVIEW, SBSP_ABSOLUTE,
            SVGIO_SELECTION, SVSI_DESELECTOTHERS, SVSI_FOCUSED, SVSI_SELECT,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP,
        };

        let fail = || windows::core::Error::from(E_FAIL);
        let folder = path.parent().and_then(crate::shell::pidl_of).ok_or_else(fail)?;
        let item = crate::shell::pidl_of(path).ok_or_else(fail)?;

        // A popup that is never shown: the view needs a parent window, and nothing more of it.
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WS_POPUP,
            0,
            0,
            400,
            300,
            None,
            None,
            None,
            None,
        )?;
        let browser: IExplorerBrowser =
            match CoCreateInstance(&ExplorerBrowser, None, CLSCTX_INPROC_SERVER) {
                Ok(browser) => browser,
                Err(error) => {
                    let _ = DestroyWindow(window);
                    return Err(error);
                }
            };
        // Owned from here, so every early return below cleans up after itself.
        let hidden = Self { window, browser };
        let settings = FOLDERSETTINGS {
            ViewMode: FVM_DETAILS.0 as u32,
            fFlags: FWF_NOWEBVIEW.0 as u32,
        };
        let room = RECT {
            left: 0,
            top: 0,
            right: 400,
            bottom: 300,
        };
        hidden.browser.Initialize(window, &room, Some(&settings))?;
        let _ = hidden.browser.SetOptions(EBO_NOBORDER);
        hidden.browser.BrowseToIDList(folder.0, SBSP_ABSOLUTE)?;

        let view: IFolderView2 = hidden.browser.GetCurrentView()?;
        let shell_view: IShellView = hidden.browser.GetCurrentView()?;
        // The pane's order is the whole point: it is what the viewer's "next" will mean.
        let _ = view.SetSortColumns(&columns(sort));

        // The view lists the folder through this thread's message loop, so the file can only be
        // selected once the loop has run long enough to reach it.
        let child = ILFindLastID(item.0);
        let flags = (SVSI_SELECT.0 | SVSI_DESELECTOTHERS.0 | SVSI_FOCUSED.0) as u32;
        let deadline = std::time::Instant::now() + LISTED;
        loop {
            let _ = shell_view.SelectItem(child, flags);
            if view.ItemCount(SVGIO_SELECTION).unwrap_or(0) == 1 {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(fail());
            }
            pump(std::time::Duration::from_millis(15));
        }
        // No verb is the default verb, which is what a double click in Explorer runs.
        view.InvokeVerbOnSelection(PCSTR::null())?;
        Ok(hidden)
    }
}

#[cfg(windows)]
impl Drop for Hidden {
    fn drop(&mut self) {
        // SAFETY: both were created on this thread by `open_from`, and are released once, here.
        unsafe {
            let _ = self.browser.Destroy();
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.window);
        }
    }
}

/// The pane's sort as the shell's: the column's nearest property, then the name, both the same way
/// round — which is the listing's own tie-break. Keywords, Status and Hard links have no property
/// Explorer sorts by, and fall back to the name.
#[cfg(windows)]
fn columns(sort: crate::fs::Sort) -> Vec<windows::Win32::UI::Shell::SORTCOLUMN> {
    use crate::fs::Column;
    use windows::core::GUID;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::UI::Shell::{SORTCOLUMN, SORT_ASCENDING, SORT_DESCENDING};

    // `PKEY_ItemTypeText`, `PKEY_ItemNameDisplay`, `PKEY_Size` and `PKEY_DateModified`, which share
    // a format id. Spelled out rather than imported, because the `windows` crate files them under a
    // feature this program has no other use for.
    let key = |pid| PROPERTYKEY {
        fmtid: GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac),
        pid,
    };
    let name = key(10);
    let keys = match sort.by {
        Column::Type => vec![key(4), name],
        Column::Size => vec![key(12), name],
        Column::Modified => vec![key(14), name],
        _ => vec![name],
    };
    let direction = if sort.ascending {
        SORT_ASCENDING
    } else {
        SORT_DESCENDING
    };
    keys.into_iter()
        .map(|propkey| SORTCOLUMN { propkey, direction })
        .collect()
}

/// Run this thread's message loop for `how_long`, sleeping whenever it has nothing to do.
#[cfg(windows)]
fn pump(how_long: std::time::Duration) {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG,
        PM_REMOVE, QS_ALLINPUT,
    };
    let until = std::time::Instant::now() + how_long;
    let mut message = MSG::default();
    loop {
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return;
        }
        // SAFETY: an ordinary message loop on the thread that owns the windows it dispatches to.
        unsafe {
            let _ = MsgWaitForMultipleObjects(None, false, left.as_millis() as u32, QS_ALLINPUT);
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}
