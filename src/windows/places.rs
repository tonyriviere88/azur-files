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
