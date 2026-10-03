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
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    let verb = verb.map(|v| wide(std::ffi::OsStr::new(v)));
    let file = wide(file);
    let args = args.map(|a| wide(a));
    let dir = dir.map(|d| wide(d.as_os_str()));

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
