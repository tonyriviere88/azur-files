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
