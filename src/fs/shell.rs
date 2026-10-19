//! Handing a path to the operating system.
//!
//! Everything here is a fire-and-forget request to the shell. Nothing waits for
//! the launched program, and nothing in this program ever deletes, moves or
//! overwrites a file — a first release that can browse fast is worth more than one
//! that can also destroy things, and destructive operations want undo,
//! progress and a recycle bin before they want to exist at all.

use std::path::Path;

#[cfg(windows)]
#[path = "../windows/verbs.rs"]
mod win;
#[cfg(windows)]
use win::{launch_among, run, run_ok};

/// Open a file from a listing, telling a viewer which files are beside it and in what order, so
/// that its previous and next walk the folder the way the pane shows it — as they do for a file
/// opened in Explorer.
///
/// Only for pictures, clips and tracks, which are the files a viewer steps through. Everything else
/// goes to [`open`] unchanged: a document has no "next", and what this costs — a hidden Explorer
/// view of the whole folder, for half a minute — is only worth paying for the files that do.
pub fn open_among(path: &Path, sort: super::Sort) {
    #[cfg(windows)]
    {
        use super::fmt::Kind;
        let ext = path.extension().unwrap_or_default().to_string_lossy();
        if matches!(super::fmt::kind_of(&ext, false), Kind::Image | Kind::Video | Kind::Audio) {
            launch_among(path, sort);
            return;
        }
    }
    #[cfg(not(windows))]
    let _ = sort;
    open(path);
}

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
        if super::is_shell_name(&text) {
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

