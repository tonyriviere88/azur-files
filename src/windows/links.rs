//! `IShellLink`: what a shortcut points at.
//!
//! The Windows half of [`crate::shell::links`].

use super::*;

/// What a `.lnk` stores: the target, the command line it runs it with, and whether the
/// attributes stored alongside say directory.
///
/// The arguments come from the same COM object the path does, so they are free once it is open —
/// and they are the difference between two shortcuts to `cmd.exe` that do entirely different
/// things, which is exactly the case a row showing only the target cannot tell apart.
#[cfg(windows)]
pub(super) fn read_shortcut(path: &Path) -> Option<Shortcut> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Storage::FileSystem::{
        WIN32_FIND_DATAW, FILE_ATTRIBUTE_DIRECTORY,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH};

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // A target longer than `MAX_PATH` is what `SLGP_RAWPATH` can hand back — it is the string
    // the file holds, not a path the API has parsed — so the buffer is not `MAX_PATH`.
    let mut buffer = [0u16; 1024];
    // And the same again for the command line, which `INFOTIPSIZE` puts at 1024 characters.
    let mut argv = [0u16; 1024];
    let mut find = WIN32_FIND_DATAW::default();

    // SAFETY: every pointer below is to a local that outlives the call, and each call's result
    // is checked before the next one uses what it produced.
    let arguments = unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        link.GetPath(&mut buffer, &mut find, SLGP_RAWPATH.0 as u32).ok()?;
        // **Not `?`.** A shortcut with no arguments is the ordinary case and this is the one
        // call here whose failure is not the read failing: the target is what the row is about,
        // and losing the whole answer over the command line would be the wrong trade.
        link.GetArguments(&mut argv).is_ok()
    };

    let text = |units: &[u16]| {
        let len = units.iter().position(|&unit| unit == 0).unwrap_or(0);
        String::from_utf16_lossy(&units[..len])
    };
    let target = text(&buffer);
    // Empty means the shortcut points at something with no path: the Recycle Bin, Control
    // Panel, a printer. There is nothing useful to show, so the row keeps its name alone.
    if target.is_empty() {
        return None;
    }
    Some(Shortcut {
        target,
        arguments: if arguments { text(&argv) } else { String::new() },
        // The attributes the shortcut *stores*, filled in when it was made. Nothing is asked of
        // the target itself, which is what keeps a shortcut to an unreachable share cheap.
        folder: find.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
    })
}

/// Write a `.lnk` at `at` pointing at `target`, with `arguments` as its command line — `""` for
/// none. `false` if the shell refused.
///
/// The same two interfaces the reading above uses, from the other end, which is what makes the
/// round trip in `a_real_shortcut_is_read_back` a test of anything.
///
/// Nothing but the path and the command line is set. A shortcut made by dragging is a shortcut *to
/// a thing*, and a working directory or an icon index invented here would be this program's idea
/// rather than the shell's.
#[cfg(windows)]
pub(super) fn write_shortcut(at: &Path, target: &Path, arguments: &str) -> bool {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    let at = crate::shell::wide(at);
    let target = crate::shell::wide(target);
    let arguments: Vec<u16> = arguments.encode_utf16().chain(Some(0)).collect();
    // SAFETY: every string is a NUL-terminated local that outlives the calls, and each result is
    // checked before the next call depends on it.
    unsafe {
        let made = || -> Option<()> {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            link.SetPath(PCWSTR(target.as_ptr())).ok()?;
            // Set unconditionally: an empty command line is what a shortcut without one has, and
            // the call is the same either way.
            link.SetArguments(PCWSTR(arguments.as_ptr())).ok()?;
            let file: IPersistFile = link.cast().ok()?;
            // `true` is `fRemember`: the object takes this as the file it belongs to, which is
            // what a later `Save` with no name would write back to.
            file.Save(PCWSTR(at.as_ptr()), true).ok()
        };
        made().is_some()
    }
}

