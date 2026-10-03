//! The known folders the sidebar lists.
//!
//! The Windows half of [`crate::fs::places`]: `SHGetKnownFolderPath`, asked once per folder.

use super::*;

#[cfg(windows)]
pub(super) fn known_folders() -> Vec<(Option<PathBuf>, &'static str, PlaceIcon)> {
    use windows_sys::Win32::UI::Shell as shell;

    vec![
        (
            known_folder(&shell::FOLDERID_Profile),
            "Home",
            PlaceIcon::Home,
        ),
        (
            known_folder(&shell::FOLDERID_Desktop),
            "Desktop",
            PlaceIcon::Desktop,
        ),
        (
            known_folder(&shell::FOLDERID_Documents),
            "Documents",
            PlaceIcon::Documents,
        ),
        (
            known_folder(&shell::FOLDERID_Downloads),
            "Downloads",
            PlaceIcon::Downloads,
        ),
        (
            known_folder(&shell::FOLDERID_Pictures),
            "Pictures",
            PlaceIcon::Pictures,
        ),
        (
            known_folder(&shell::FOLDERID_Music),
            "Music",
            PlaceIcon::Music,
        ),
        (
            known_folder(&shell::FOLDERID_Videos),
            "Videos",
            PlaceIcon::Videos,
        ),
    ]
}

/// What the shell makes of one of its own names — `shell:Downloads`, `::{…}`. See [`ShellPlace`].
///
/// `SHCreateItemFromParsingName` is the parser the Run box and Explorer's bar use, so every name
/// either of those takes is taken here, including the localised and per-user ones nobody could
/// keep a table of. Asked on the UI thread, when a path is typed or passed in: a known folder is
/// a registry lookup and a CLSID a class lookup, and neither goes near a disk or a network.
///
/// `None` for a name the shell does not know, which leaves the text to be treated as a path — and
/// a path starting `shell:` names nothing, so it is then reported as not found.
#[cfg(windows)]
pub fn shell_place(name: &str) -> Option<ShellPlace> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItem, SHCreateItemFromParsingName, SHGetKnownFolderItem, FOLDERID_ComputerFolder,
        FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, SICHINT_CANONICAL, SIGDN_FILESYSPATH,
    };

    let wide = crate::shell::wide(std::path::Path::new(name));
    // SAFETY: `wide` is null-terminated and outlives every call that reads it; the display name is
    // read through `name_of`, which frees it. The apartment is joined for the length of this call
    // and left again — `S_FALSE` on a thread already in one, which is the UI thread's case, still
    // counts and is still balanced.
    unsafe {
        let joined = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let answer = (|| {
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
            let is = |folder: &windows::core::GUID| {
                SHGetKnownFolderItem::<IShellItem>(folder, KF_FLAG_DEFAULT, None)
                    .ok()
                    .and_then(|known| item.Compare(&known, SICHINT_CANONICAL.0 as u32).ok())
                    == Some(0)
            };
            if is(&FOLDERID_RecycleBinFolder) {
                return Some(ShellPlace::RecycleBin);
            }
            if is(&FOLDERID_ComputerFolder) {
                return Some(ShellPlace::ThisPc);
            }
            Some(
                match crate::shell::ops::bin::name_of(&item, SIGDN_FILESYSPATH) {
                    Some(path) if !path.as_os_str().is_empty() => ShellPlace::Folder(path),
                    _ => ShellPlace::Elsewhere,
                },
            )
        })();
        if joined {
            CoUninitialize();
        }
        answer
    }
}

/// Resolve one `FOLDERID_*`.
#[cfg(windows)]
pub(super) fn known_folder(id: &windows_sys::core::GUID) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

    let mut raw = std::ptr::null_mut();
    // SAFETY: `raw` is written only on success, and freed on every path out.
    let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw) };
    if hr < 0 || raw.is_null() {
        return None;
    }

    let mut len = 0;
    // SAFETY: the shell returns a null-terminated string it owns until we free it.
    unsafe {
        while *raw.add(len) != 0 {
            len += 1;
        }
    }
    let wide = unsafe { std::slice::from_raw_parts(raw, len) };
    let path = PathBuf::from(std::ffi::OsString::from_wide(wide));
    unsafe { CoTaskMemFree(raw.cast()) };

    (!path.as_os_str().is_empty()).then_some(path)
}
