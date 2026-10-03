//! `ReadDirectoryChangesW`: hearing about a folder changing without polling it.
//!
//! The Windows half of [`crate::watch`].

use super::*;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
    FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

/// Everything worth being told about a folder's contents.
///
/// Not `FILE_NOTIFY_CHANGE_LAST_ACCESS`: reading a file in the folder would then re-read the
/// folder, and merely showing the listing reads it.
const FILTER: windows::Win32::Storage::FileSystem::FILE_NOTIFY_CHANGE =
    windows::Win32::Storage::FileSystem::FILE_NOTIFY_CHANGE(
        FILE_NOTIFY_CHANGE_FILE_NAME.0
            | FILE_NOTIFY_CHANGE_DIR_NAME.0
            | FILE_NOTIFY_CHANGE_ATTRIBUTES.0
            | FILE_NOTIFY_CHANGE_SIZE.0
            | FILE_NOTIFY_CHANGE_LAST_WRITE.0,
    );

/// One watched folder: the handle, the event it completes on, and the read in flight.
struct Watched {
    path: PathBuf,
    dir: HANDLE,
    event: HANDLE,
    /// Boxed because the kernel keeps the address until the read completes, and the struct
    /// itself moves when it goes into the vector.
    overlapped: Box<OVERLAPPED>,
    /// `u32` so the buffer is `DWORD`-aligned, which the API requires even though nothing
    /// here ever reads it.
    buffer: Box<[u32; 256]>,
    /// Whether a read is in flight — so [`Drop`] knows whether there is a completion to wait for
    /// before it frees the two boxes the kernel is writing into.
    ///
    /// Tracked rather than assumed because both answers are common: a folder that has been deleted
    /// fails to re-arm and is dropped with nothing pending, and waiting on that would cost the
    /// grace period below for nothing.
    armed: bool,
}

impl Watched {
    fn open(path: &Path) -> Option<Self> {
        let wide = crate::shell::wide(path);
        // SAFETY: a null-terminated path that outlives the call. `FILE_SHARE_DELETE` is not
        // optional: without it this handle would stop the folder from being deleted or
        // renamed, so watching a folder would break the very operations it is watching for.
        let dir = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
            .ok()?
        };
        if dir == INVALID_HANDLE_VALUE {
            return None;
        }
        // Auto-reset: the wait that observes it clears it, so re-arming needs nothing.
        // SAFETY: an unnamed event, released in `Drop`.
        let event = match unsafe { CreateEventW(None, false, false, PCWSTR::null()) } {
            Ok(event) => event,
            Err(_) => {
                // SAFETY: opened just above and not handed anywhere.
                let _ = unsafe { CloseHandle(dir) };
                return None;
            }
        };
        let mut overlapped = Box::new(OVERLAPPED::default());
        overlapped.hEvent = event;
        let mut watched = Self {
            path: path.to_path_buf(),
            dir,
            event,
            overlapped,
            buffer: Box::new([0u32; 256]),
            armed: false,
        };
        watched.arm().then_some(watched)
    }

    /// Ask for the next change. False if the folder has gone.
    fn arm(&mut self) -> bool {
        let len = std::mem::size_of_val(self.buffer.as_ref()) as u32;
        // SAFETY: the buffer and the `OVERLAPPED` are boxed, so their addresses stand still
        // until the read completes — which is what makes this sound at all. The kernel
        // writes into the buffer and nothing here reads it while the read is in flight.
        self.armed = unsafe {
            ReadDirectoryChangesW(
                self.dir,
                self.buffer.as_mut().as_mut_ptr().cast(),
                len,
                false,
                FILTER,
                None,
                Some(self.overlapped.as_mut() as *mut OVERLAPPED),
                None,
            )
            .is_ok()
        };
        // Recorded rather than returned alone, because [`Drop`] is the other reader of it: from here
        // until the completion, the kernel owns the two boxes.
        self.armed
    }

    /// Complete the read that just signalled, and ask for the next.
    fn consume(&mut self) -> bool {
        let mut written = 0u32;
        // SAFETY: the read this completes was issued by `arm` on this same `OVERLAPPED`.
        // The byte count is ignored: zero means the buffer overflowed, which is still a
        // change, and any other value describes files this does not need to know about.
        unsafe {
            let _ = GetOverlappedResult(self.dir, self.overlapped.as_ref(), &mut written, false);
        }
        // That read is finished with the buffer, whatever it reported. The next one is not yet
        // asked for.
        self.armed = false;
        self.arm()
    }
}

impl std::ops::Drop for Watched {
    /// **`CancelIoEx` asks; it does not finish.** The comment that used to be here said the cancel
    /// came first "or the kernel would complete into a buffer this is about to free", which is the
    /// right worry and was not what the code did: nothing waited for the completion, so the boxed
    /// `OVERLAPPED` and its kilobyte of buffer were freed while a cancellation was still outstanding.
    /// A driver that defers it then writes the `IO_STATUS_BLOCK` and up to a kilobyte of
    /// `FILE_NOTIFY_INFORMATION` into heap this process has handed back and very likely reused.
    ///
    /// Usually it lands before `CancelIoEx` returns, which is why this never showed. **The case that
    /// defers is the network redirector**, and this program watches shares — reached whenever a
    /// watch is retired: the folder left the wanted set, the list overflowed, or the window is
    /// closing.
    ///
    /// So the completion is waited for, and the wait is *bounded*. An unbounded one is the other
    /// failure this file already knows about — see [`finished`], which leaks the wake event rather
    /// than close it under a thread stuck in the redirector. The same trade is made here: if the
    /// completion has not landed in time, the memory the kernel may still write into is **leaked**
    /// rather than freed. A kilobyte per abandoned watch on a share that has stopped answering is a
    /// far better outcome than a heap the kernel writes into afterwards, and it is bounded by
    /// [`MAX_WATCHED`].
    fn drop(&mut self) {
        if !self.armed {
            // Nothing in flight — a folder that was deleted and failed to re-arm. Nothing to wait
            // for, and nothing the kernel still holds.
            // SAFETY: opened by `open` and not handed anywhere else.
            unsafe {
                let _ = CloseHandle(self.dir);
                let _ = CloseHandle(self.event);
            }
            return;
        }

        /// Long enough for a cancellation to be noticed, short enough not to be a hang. A
        /// cancelled read completes almost at once even across a redirector; this is the ceiling
        /// on being wrong about that, and it is paid once per watch being retired.
        const GRACE: u32 = 1_000;

        // SAFETY: the read was issued by `arm` on this same handle and `OVERLAPPED`. The event is
        // the one in that `OVERLAPPED`, so it signals when the read completes however it completes
        // — including with `ERROR_OPERATION_ABORTED`, which is the answer being waited for.
        let completed = unsafe {
            use windows::Win32::System::Threading::WaitForSingleObject;

            let _ = CancelIoEx(self.dir, Some(self.overlapped.as_ref()));
            WaitForSingleObject(self.event, GRACE) == WAIT_OBJECT_0
        };
        if completed {
            // The kernel is done with both boxes; they drop with the struct as usual.
            // SAFETY: no I/O outstanding on either handle now.
            unsafe {
                let _ = CloseHandle(self.dir);
                let _ = CloseHandle(self.event);
            }
            return;
        }

        // Still outstanding. The directory handle is closed anyway — that is what stops the watch,
        // and a completion arriving afterwards writes only into what is leaked below. The *event*
        // is left open with them, because the kernel may still signal it.
        // SAFETY: closing a handle with I/O pending is allowed; it completes the read as aborted.
        unsafe {
            let _ = CloseHandle(self.dir);
        }
        let _ = Box::leak(std::mem::replace(
            &mut self.overlapped,
            Box::new(OVERLAPPED::default()),
        ));
        let _ = Box::leak(std::mem::replace(&mut self.buffer, Box::new([0u32; 256])));
    }
}

/// The wake event and the thread waiting on it.
pub fn start(
    shared: Arc<Mutex<Shared>>,
    ctx: egui::Context,
) -> (isize, Option<std::thread::JoinHandle<()>>) {
    // Manual reset, so a signal is not lost if the thread is between waits.
    // SAFETY: an unnamed event, closed by `Watch::drop`.
    let Ok(wake) = (unsafe { CreateEventW(None, true, false, PCWSTR::null()) }) else {
        return (0, None);
    };
    // Carried across as an integer: a `HANDLE` is a raw pointer and so not `Send`, which
    // is a rule about the type rather than about this handle — a kernel object is shared
    // between threads by design, and this one is closed only after the thread is joined.
    let raw = wake.0 as isize;
    let thread = std::thread::Builder::new()
        .name("folder-watch".to_owned())
        .spawn(move || run(shared, raw, ctx))
        .ok();
    (raw, thread)
}

pub fn signal(wake: isize) {
    if wake == 0 {
        return;
    }
    // SAFETY: the handle belongs to `Watch`, which outlives every call to this.
    let _ = unsafe { SetEvent(HANDLE(wake as *mut std::ffi::c_void)) };
}

pub fn close(wake: isize) {
    if wake == 0 {
        return;
    }
    // SAFETY: closed once, from `Watch::drop`, and only once that has established the thread has
    // ended — see [`finished`], which is what makes that true even when the thread is stuck.
    let _ = unsafe { CloseHandle(HANDLE(wake as *mut std::ffi::c_void)) };
}

/// Whether the watcher thread has ended, waiting up to `grace` for it.
///
/// **The whole reason `Watch::drop` does not simply join.** The thread opens each watched folder
/// with `CreateFileW`, and on a share that has gone away that call blocks for a redirector timeout —
/// around twenty seconds. It cannot see the quit flag while it is in there and the wake event cannot
/// interrupt it, so a join is a twenty-second wait *after* the window has gone, which is the freeze
/// this exists to avoid.
///
/// `false` means "still in there", and the caller's answer to that is to leave the thread alone and
/// leak the event rather than close a handle the thread may still wait on — a closed handle value is
/// free to be reused by the next object the process opens, which is the one way this could go wrong
/// quietly. One event handle at exit costs nothing; the OS takes it back with the process.
///
/// The grace only has to cover *noticing*: a thread in `WaitForMultipleObjects` is released by the
/// signal immediately, so this returns in microseconds in the normal case, and the tests that make
/// and drop a `Watch` therefore still close their handle.
pub fn finished(thread: &std::thread::JoinHandle<()>, grace: std::time::Duration) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::WaitForSingleObject;

    let handle = HANDLE(thread.as_raw_handle());
    let ms = grace.as_millis().min(u128::from(u32::MAX)) as u32;
    // SAFETY: the handle belongs to `thread`, which is borrowed for the whole call, so it cannot
    // have been closed by the `JoinHandle` being dropped.
    unsafe { WaitForSingleObject(handle, ms) == WAIT_OBJECT_0 }
}

fn run(shared: Arc<Mutex<Shared>>, wake: isize, ctx: egui::Context) {
    use windows::Win32::System::Threading::{ResetEvent, INFINITE};

    let wake = HANDLE(wake as *mut std::ffi::c_void);

    let mut watches: Vec<Watched> = Vec::new();
    loop {
        // ---- Reconcile the set -------------------------------------
        let wanted = {
            let Ok(shared) = shared.lock() else { return };
            if shared.quit {
                return;
            }
            shared.wanted.clone()
        };
        // SAFETY: manual-reset, so it is cleared here rather than by the wait — before the
        // set is read, so a change arriving during reconciliation wakes the next wait
        // instead of being dropped.
        let _ = unsafe { ResetEvent(wake) };
        // Dropping a `Watched` cancels its read and closes its handles.
        watches.retain(|w| wanted.contains(&w.path));
        for path in wanted.iter().take(MAX_WATCHED) {
            if !watches.iter().any(|w| &w.path == path) {
                if let Some(watched) = Watched::open(path) {
                    watches.push(watched);
                }
            }
        }

        // ---- Wait --------------------------------------------------
        let mut handles = Vec::with_capacity(watches.len() + 1);
        handles.push(wake);
        handles.extend(watches.iter().map(|w| w.event));
        // SAFETY: every handle is live — the wake event belongs to `Watch`, which joins this
        // thread before closing it, and the rest are owned by `watches`.
        let got = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        if got == WAIT_FAILED {
            // Nothing to retry against, and spinning on it would burn a core.
            std::thread::sleep(std::time::Duration::from_millis(200));
            continue;
        }
        let index = got.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        if index == 0 || index > watches.len() {
            // The wake event, or something unexpected: reconcile and wait again.
            continue;
        }

        // ---- Report ------------------------------------------------
        let watched = &mut watches[index - 1];
        let path = watched.path.clone();
        if !watched.consume() {
            // The folder has gone. Its own disappearance is a change in its parent, which
            // is watched separately if it is on screen.
            watches.remove(index - 1);
        }
        if let Ok(mut shared) = shared.lock() {
            if shared.quit {
                return;
            }
            if !shared.changed.contains(&path) {
                shared.changed.push(path);
            }
        }
        ctx.request_repaint();
    }
}
