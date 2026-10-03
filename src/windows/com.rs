//! The two COM primitives every call in [`crate::shell`] is built on: waiting without
//! breaking the apartment's promise, and turning a path into something the shell will parse.
//!
//! The Windows half of [`crate::shell`].

/// Wait for a while, answering the cross-apartment calls this thread owes, and nothing else.
///
/// A single-threaded apartment is a promise: anything this thread hands out can be called back
/// into, and the caller blocks until it is answered. A thread that sleeps, or blocks on a
/// channel, breaks that promise — and on Windows the things it breaks are not obvious:
///
/// - **The clipboard stops working.** Whatever put data on the clipboard owns it, and reading
///   it from anywhere else is a call back into the owning apartment. A shell Copy run on the
///   modal thread put the files there correctly and *nothing could read them*, this program
///   included, because that thread was parked in `recv`.
/// - **A copy refuses the next copy.** The clipboard holds a reference rather than the bytes,
///   so a reader takes the clipboard lock and then calls in for the data; unanswered, the lock
///   stays taken. See the note on `retrying` in [`clipboard`].
///
/// `CoWaitForMultipleHandles` on an event nobody will ever signal, so the wait *is* the sleep.
/// Without `COWAIT_DISPATCH_WINDOW_MESSAGES` it services cross-apartment calls and leaves the
/// window queue alone — which matters on the UI thread, where dispatching a `WM_PAINT` from
/// inside an egui pass would have egui begin a pass while already inside one.
#[cfg(windows)]
pub(crate) fn answering_calls(ms: u64) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Com::{CoWaitForMultipleHandles, COWAIT_DISPATCH_CALLS};
    use windows::Win32::System::Threading::CreateEventW;

    thread_local! {
        /// One per thread, made once. The modal thread waits like this twenty times a second
        /// for as long as the program runs, and creating and closing a handle each time would
        /// be a needless million of them over an afternoon. Never signalled, so it is only
        /// ever a thing to wait on; leaked at thread exit, which is where the process is
        /// going anyway.
        static NEVER: Option<HANDLE> = {
            // SAFETY: a plain manual-reset event, unnamed and unsignalled.
            unsafe { CreateEventW(None, true, false, None).ok() }
        };
    }

    let waited = NEVER.with(|event| match event {
        Some(event) => {
            // SAFETY: the handle belongs to this thread and outlives the wait.
            unsafe {
                let _ = CoWaitForMultipleHandles(
                    COWAIT_DISPATCH_CALLS.0 as u32,
                    ms as u32,
                    &[*event],
                );
            }
            true
        }
        None => false,
    });
    // No event to wait on is no reason to spin.
    if !waited {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// A path as a null-terminated wide string, which every call here wants.
///
/// Forward slashes become backslashes on the way through, and that is not cosmetic. The
/// kernel accepts either — `std::fs` opens `C:/Windows/notepad.exe` quite happily — but the
/// shell *parses* paths, and `SHCreateItemFromParsingName` and `SHParseDisplayName` both
/// refuse one with a slash in it. So a path that arrived with slashes, which is exactly what
/// `--open=C:/Windows` gives, used to list its folder perfectly and then fail every single
/// shell call against it: no icons, no context menu, and copy, paste and delete all reporting
/// that the items could not be found. One conversion here fixes all of them, because every
/// call in this module comes through this function.
#[cfg(windows)]
pub(crate) fn wide(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    // Unit by unit is safe: `/` is U+002F, which never appears as either half of a surrogate
    // pair, so no character outside the BMP can be damaged by this.
    const SLASH: u16 = b'/' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    path.as_os_str()
        .encode_wide()
        .map(|unit| if unit == SLASH { BACKSLASH } else { unit })
        .chain(std::iter::once(0))
        .collect()
}
