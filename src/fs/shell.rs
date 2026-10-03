//! Handing a path to the operating system.
//!
//! Everything here is a fire-and-forget request to the shell. Nothing waits for
//! the launched program, and nothing in this program ever deletes, moves or
//! overwrites a file — a first release that can browse fast is worth more than one
//! that can also destroy things, and destructive operations want undo,
//! progress and a recycle bin before they want to exist at all.

use std::path::Path;

/// Open a file or folder with whatever the shell thinks owns it.
pub fn open(path: &Path) {
    #[cfg(windows)]
    {
        run(None, path.as_os_str(), None);
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

/// Open the shell's own window on a path, with the entry selected.
///
/// For the Recycle Bin and anything else that is a namespace extension rather
/// than a directory.
pub fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        use std::ffi::OsString;

        let text = path.to_string_lossy();
        // `shell:` monikers and `::{GUID}` paths are not files, so `/select,`
        // would be nonsense — open them directly.
        if text.starts_with("shell:") || text.starts_with("::{") {
            let mut args = OsString::from("\"");
            args.push(path.as_os_str());
            args.push("\"");
            run(Some("open"), std::ffi::OsStr::new("explorer.exe"), Some(&args));
            return;
        }
        let mut args = OsString::from("/select,\"");
        args.push(path.as_os_str());
        args.push("\"");
        run(Some("open"), std::ffi::OsStr::new("explorer.exe"), Some(&args));
    }
    #[cfg(not(windows))]
    {
        // No portable "reveal": show the containing directory instead.
        open(path.parent().unwrap_or(path));
    }
}

/// Open a terminal in a folder.
pub fn open_terminal(dir: &Path) {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        // Windows Terminal if it is installed, and `cmd` if the shell cannot find
        // it. `ShellExecuteW` reports failure through its return value, which is
        // an `HINSTANCE` for historical reasons and <= 32 on error.
        if !run_ok(Some("open"), OsStr::new("wt.exe"), None, Some(dir)) {
            run(Some("open"), OsStr::new("cmd.exe"), None);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("x-terminal-emulator")
            .current_dir(dir)
            .spawn();
    }
}

#[cfg(windows)]
fn run(verb: Option<&str>, file: &std::ffi::OsStr, args: Option<&std::ffi::OsString>) {
    run_ok(verb, file, args, None);
}

/// `ShellExecuteW`, with every string null-terminated and the working directory
/// optional. Returns whether the shell managed to start something.
#[cfg(windows)]
fn run_ok(
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
