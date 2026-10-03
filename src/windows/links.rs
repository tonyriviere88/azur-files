//! `IShellLink`: what a shortcut points at.
//!
//! The Windows half of [`crate::shell::links`].

use super::*;

/// The path a `.lnk` stores, and whether the attributes it stores with it say directory.
#[cfg(windows)]
pub(super) fn read_shortcut(path: &Path) -> Option<(String, bool)> {
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
    let mut find = WIN32_FIND_DATAW::default();

    // SAFETY: every pointer below is to a local that outlives the call, and each call's result
    // is checked before the next one uses what it produced.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        link.GetPath(&mut buffer, &mut find, SLGP_RAWPATH.0 as u32).ok()?;
    }

    let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
    // Empty means the shortcut points at something with no path: the Recycle Bin, Control
    // Panel, a printer. There is nothing useful to show, so the row keeps its name alone.
    if len == 0 {
        return None;
    }
    // The attributes the shortcut *stores*, filled in when it was made. Nothing is asked of the
    // target itself, which is what keeps a shortcut to an unreachable share cheap.
    let folder = find.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
    Some((String::from_utf16_lossy(&buffer[..len]), folder))
}
