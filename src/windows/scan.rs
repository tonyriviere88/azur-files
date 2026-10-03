//! `FindFirstFileExW` at `FindExInfoBasic`, which is the whole reason the listing is fast.
//!
//! The Windows half of [`crate::fs::scan`]. The name comes out of the syscall's own buffer and
//! goes straight into the arena; nothing here asks a second question about a file it has already
//! been told about. See the module header next door for what that is worth.

use super::*;

#[cfg(windows)]
pub(super) fn scan_real(path: &Path, started: Instant) -> Dir {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
        FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
    };

    let pattern = search_pattern(path);
    let mut data = WIN32_FIND_DATAW::default();

    let handle = unsafe {
        FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            (&mut data) as *mut WIN32_FIND_DATAW as *mut c_void,
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let code = unsafe { GetLastError() };
        // An empty directory reports "no more files" from the *first* call rather
        // than coming back with zero entries, so it lands here and is not an error.
        const ERROR_FILE_NOT_FOUND: u32 = 2;
        const ERROR_NO_MORE_FILES: u32 = 18;
        if matches!(code, ERROR_FILE_NOT_FOUND | ERROR_NO_MORE_FILES) {
            return DirBuilder::new(path).finish(elapsed_micros(started));
        }
        // Whether a sign-in would fix it, decided here because this is the last place the *code*
        // exists — a sentence cannot be asked whether it was about credentials.
        return Dir::failed(path, error_text(code))
            .wanting_credentials(super::wants_credentials(code, path));
    }

    let mut builder = DirBuilder::new(path);
    loop {
        let name = trimmed_name(&data.cFileName);
        if !is_dot_entry(name) {
            builder.push_wide(
                name,
                (data.nFileSizeHigh as u64) << 32 | data.nFileSizeLow as u64,
                filetime(&data.ftLastWriteTime),
                flags_of(data.dwFileAttributes, data.dwReserved0),
            );
        }
        if unsafe { FindNextFileW(handle, &mut data) } == 0 {
            break;
        }
    }
    unsafe { FindClose(handle) };

    builder.finish(elapsed_micros(started))
}

/// `\\?\C:\some\dir\*`, ready for `FindFirstFileExW`.
///
/// The `\\?\` prefix lifts the 260-character limit and skips the Win32 path
/// parser, which is a per-call cost this makes on every directory. It also turns
/// off `.`/`..` collapsing — fine here, because every path in this program comes
/// from a canonicalised navigation, never from user text that was not resolved
/// first.
#[cfg(windows)]
pub(super) fn search_pattern(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    let raw: Vec<u16> = path.as_os_str().encode_wide().collect();
    let mut out: Vec<u16> = Vec::with_capacity(raw.len() + 8);

    const SEP: u16 = b'\\' as u16;
    let starts_with = |prefix: &str| {
        raw.len() >= prefix.len()
            && raw
                .iter()
                .zip(prefix.encode_utf16())
                .take(prefix.len())
                .all(|(a, b)| *a == b || (*a == '/' as u16 && b == SEP))
    };

    if starts_with(r"\\?\") || starts_with(r"\\.\") {
        out.extend_from_slice(&raw);
    } else if starts_with(r"\\") {
        // `\\server\share` -> `\\?\UNC\server\share`.
        out.extend(r"\\?\UNC".encode_utf16());
        out.extend_from_slice(&raw[1..]);
    } else if raw.len() >= 2 && raw[1] == b':' as u16 {
        out.extend(r"\\?\".encode_utf16());
        out.extend_from_slice(&raw);
    } else {
        // Relative, or something exotic. Hand it to the normal parser.
        out.extend_from_slice(&raw);
    }

    // The extended prefix takes backslashes only.
    for unit in &mut out {
        if *unit == '/' as u16 {
            *unit = SEP;
        }
    }
    if out.last() != Some(&SEP) {
        out.push(SEP);
    }
    out.push(b'*' as u16);
    out.push(0);
    out
}

/// The name out of a `cFileName`, without the padding after the terminator.
#[cfg(windows)]
#[inline]
pub(super) fn trimmed_name(buf: &[u16; 260]) -> &[u16] {
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    &buf[..len]
}

/// `.` and `..`, which every directory reports and nobody wants to see.
#[cfg(windows)]
#[inline]
pub(super) fn is_dot_entry(name: &[u16]) -> bool {
    const DOT: u16 = b'.' as u16;
    match name.len() {
        1 => name[0] == DOT,
        2 => name[0] == DOT && name[1] == DOT,
        _ => false,
    }
}

#[cfg(windows)]
#[inline]
pub(super) fn filetime(ft: &windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

/// The `FLAG_*` set for one find record.
///
/// `tag` is the find data's `dwReserved0`, which holds the reparse tag when the attributes say there
/// is one and is meaningless otherwise. It is read for one family: `IO_REPARSE_TAG_CLOUD_*` is
/// `0x9000_001A` with a provider's own nibble at bits 12–15, and every one of them is a placeholder.
#[cfg(windows)]
#[inline]
pub(super) fn flags_of(attrs: u32, tag: u32) -> u16 {
    use windows_sys::Win32::Storage::FileSystem as fs;
    let mut flags = 0;
    if attrs & fs::FILE_ATTRIBUTE_DIRECTORY != 0 {
        flags |= FLAG_DIR;
    }
    if attrs & fs::FILE_ATTRIBUTE_HIDDEN != 0 {
        flags |= FLAG_HIDDEN;
    }
    if attrs & fs::FILE_ATTRIBUTE_SYSTEM != 0 {
        flags |= FLAG_SYSTEM;
    }
    if attrs & fs::FILE_ATTRIBUTE_READONLY != 0 {
        flags |= FLAG_READONLY;
    }
    if attrs & fs::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        const IO_REPARSE_TAG_CLOUD: u32 = 0x9000_001A;
        // **A placeholder is not a link.** It is the file itself, kept by a sync provider, and a
        // synced folder has to be walked, measured and flattened like any other — which is how every
        // one of them was treated before [`expose_placeholders`], while Windows hid the tag.
        flags |= if tag & 0xFFFF_0FFF == IO_REPARSE_TAG_CLOUD {
            FLAG_PLACEHOLDER
        } else {
            FLAG_LINK
        };
    }
    if attrs
        & (fs::FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
            | fs::FILE_ATTRIBUTE_RECALL_ON_OPEN
            | fs::FILE_ATTRIBUTE_OFFLINE)
        != 0
    {
        flags |= FLAG_ONLINE;
    }
    // Both at once is what OneDrive leaves on a file it does not sync at all — its own
    // `desktop.ini` reads `0x180026` — so it is neither of the two states.
    if attrs & (fs::FILE_ATTRIBUTE_PINNED | fs::FILE_ATTRIBUTE_UNPINNED) == fs::FILE_ATTRIBUTE_PINNED {
        flags |= FLAG_PINNED;
    }
    flags
}

/// The handful of failures a file manager actually meets, in words.
///
/// `FormatMessageW` would give the localised text for all of them, at the cost of
/// another feature of `windows-sys` and a `LocalFree` on every path; these five
/// cover everything a user can act on.
#[cfg(windows)]
pub(crate) fn error_text(code: u32) -> String {
    match code {
        2 | 3 => "This folder no longer exists".to_owned(),
        5 => "Access denied".to_owned(),
        15 => "The drive is not available".to_owned(),
        21 => "The device is not ready".to_owned(),
        53 | 67 => "The network path was not found".to_owned(),
        // What is left on screen when the credential prompt these raise is cancelled — so it has to
        // say something the person who cancelled it can act on. Windows' own words for 1265 blame a
        // domain controller, which is true and useless to somebody typing the path of a NAS on the
        // same desk: the sign-in is what did not happen. See [`super::wants_credentials`].
        1265 | 1311 => "Windows could not sign in to this share".to_owned(),
        1223 => "Cancelled".to_owned(),
        other => format!("Could not read this folder (error {other})"),
    }
}

/// See cloud-files placeholders for what they are, the way Explorer does.
///
/// **Windows disguises them from this process otherwise.** An executable with no manifest runs in
/// `PHCM_DISGUISE_PLACEHOLDER`, measured: the find data of a OneDrive folder then carries no
/// reparse attribute and a reparse tag of `0`, and a hydrated file reads as a plain `0x20`. Two
/// things went wrong under that, and both are fixed by this one call:
///
/// - **The Status column could not tell a synced folder from any other** unless something in it
///   happened to be cloud-only or pinned — see [`crate::fs::dir::FLAG_PLACEHOLDER`], which is the
///   tag this makes visible.
/// - **The shell code running in this process read cloud-only files as if they were on the disk.**
///   Properties opened the file's own property handler, and reading it is what downloads it. With
///   placeholders exposed the shell treats them as Explorer's own process does, and leaves them in
///   the cloud.
///
/// Process-wide, and before any other thread exists; a thread left at its default follows it.
#[cfg(windows)]
pub fn expose_placeholders() {
    // Declared here rather than taken from `windows-sys`, which only has it behind the driver kit's
    // `Wdk_Storage_FileSystem` feature — a feature for one function.
    #[link(name = "ntdll", kind = "raw-dylib")]
    extern "system" {
        fn RtlSetProcessPlaceholderCompatibilityMode(mode: i8) -> i8;
    }
    const PHCM_EXPOSE_PLACEHOLDERS: i8 = 2;
    // SAFETY: no arguments beyond the mode; the previous mode it returns is not needed.
    unsafe { RtlSetProcessPlaceholderCompatibilityMode(PHCM_EXPOSE_PLACEHOLDERS) };
}

/// Ask Windows not to put up an "insert a disk" dialog behind our back.
///
/// Any call that touches an empty removable drive — the drive list does, on every
/// refresh — pops a modal from inside the syscall unless this is set. It is
/// per-thread, so the loader's workers each call it once.
#[cfg(windows)]
pub fn silence_device_dialogs() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SetThreadErrorMode, SEM_FAILCRITICALERRORS,
    };
    unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, std::ptr::null_mut()) };
}
