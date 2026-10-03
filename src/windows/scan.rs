//! A directory read, with each entry's file ID in it — and `FindFirstFileExW` at `FindExInfoBasic`
//! where there are no IDs worth having.
//!
//! The Windows half of [`crate::fs::scan`]. The name comes out of the syscall's own buffer and
//! goes straight into the arena; nothing here asks a second question about a file it has already
//! been told about. See the module header next door for what that is worth.
//!
//! # Two ways to read one directory
//!
//! **The same kernel call either way.** `FindFirstFileExW` and `GetFileInformationByHandleEx` both
//! end in `NtQueryDirectoryFile`, and what differs is the *information class* asked for: the find
//! API asks for one with no file ID in it, and `WIN32_FIND_DATAW` has no field to put one in.
//! [`scan_by_handle`] opens the directory and asks for `FileIdExtdDirectoryInfo` instead — the same
//! name, size, times, attributes and reparse tag with a 128-bit ID on the end, sixteen more bytes
//! per entry in the same sequential read. The ID is what keywords are kept under; see
//! [`crate::fs::keywords`].
//!
//! It is only used where the ID means something, **NTFS and ReFS**. Everything else goes the old
//! way — [`scan_find`], unchanged — and so does every failure, so the words a failed read comes back
//! with are still the find API's. The one exception is written down at [`asks_twice`].

use super::*;

#[cfg(windows)]
pub(super) fn scan_real(path: &Path, started: Instant) -> Dir {
    match scan_by_handle(path, started) {
        Ok(dir) => dir,
        Err(Some(failed)) => *failed,
        Err(None) => scan_find(path, started),
    }
}

/// A failure that asking again the find API's way would only repeat — slowly.
///
/// The codes a share that is not there, or not letting us in, comes back with. An unreachable
/// server costs twenty-odd seconds to fail, and a second attempt is a second wait for the same
/// sentence; a refused sign-in asked again is a second refusal. Every other failure falls back to
/// [`scan_find`], whose treatment of each code is the tested one — `ERROR_FILE_NOT_FOUND` from a
/// *pattern* is an empty folder and from an *open* it is a folder that has gone, and only the find
/// API can tell which it met.
#[cfg(windows)]
fn asks_twice(code: u32) -> bool {
    matches!(code, 53 | 64 | 67 | 86 | 1219 | 1231 | 1244 | 1265 | 1311 | 1326 | 2202)
}

/// Read `path` with each entry's file ID, or say why not.
///
/// `Err(None)` means *use the find API instead*: the volume is not one whose IDs are stable, or it
/// does not answer this information class, or the open failed in a way [`scan_find`] should word.
/// `Err(Some)` is a failure that is final — see [`asks_twice`].
#[cfg(windows)]
fn scan_by_handle(path: &Path, started: Instant) -> Result<Dir, Option<Box<Dir>>> {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileIdExtdDirectoryInfo, FileIdInfo, GetFileInformationByHandleEx,
        GetVolumeInformationByHandleW, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_EXTD_DIR_INFO,
        FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };

    let name = directory_name(path);
    // What `FindFirstFileExW` opens the directory with, give or take: listing access, sharing
    // everything, and backup semantics because a directory cannot be opened without them.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let code = unsafe { GetLastError() };
        if asks_twice(code) {
            return Err(Some(Box::new(
                Dir::failed(path, error_text(code))
                    .wanting_credentials(super::wants_credentials(code, path)),
            )));
        }
        return Err(None);
    }
    struct Close(HANDLE);
    impl Drop for Close {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    let _close = Close(handle);

    // **Which file system, first**: an "ID" on FAT or exFAT is where the entry sits in its
    // directory, and it moves with the entry. Asked of the handle already open, so no path parse.
    let mut system = [0u16; 16];
    let known = unsafe {
        GetVolumeInformationByHandleW(
            handle,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            system.as_mut_ptr(),
            system.len() as u32,
        )
    } != 0;
    let system = String::from_utf16_lossy(trimmed(&system));
    if !known || !matches!(system.as_str(), "NTFS" | "ReFS") {
        return Err(None);
    }
    // And the volume's serial, all 64 bits of it: `GetVolumeInformation`'s is the low 32.
    let mut volume = FILE_ID_INFO::default();
    let identified = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut volume) as *mut FILE_ID_INFO as *mut c_void,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } != 0;
    if !identified {
        return Err(None);
    }

    // 64 KB, which is what `FIND_FIRST_EX_LARGE_FETCH` has the find API ask for. `u64`s, so the
    // buffer is aligned for records that hold `i64`s.
    let mut buffer = vec![0u64; 64 * 1024 / 8];
    let bytes = (buffer.len() * 8) as u32;
    let mut builder = DirBuilder::new(path);
    builder.on_volume(Some(volume.VolumeSerialNumber));
    let mut first = true;
    loop {
        let read = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdExtdDirectoryInfo,
                buffer.as_mut_ptr() as *mut c_void,
                bytes,
            )
        } != 0;
        if !read {
            const ERROR_NO_MORE_FILES: u32 = 18;
            if unsafe { GetLastError() } == ERROR_NO_MORE_FILES {
                break;
            }
            // A class the volume does not answer shows here, on the first read — and so does
            // anything else odd about this directory, which the find API is then left to word.
            if first {
                return Err(None);
            }
            // Mid-listing, which is where `FindNextFileW` failing leaves the other path too: what
            // was read is what there is.
            break;
        }
        first = false;

        let base = buffer.as_ptr() as *const u8;
        let mut at = 0usize;
        loop {
            // SAFETY: `at` is 0 or a sum of the `NextEntryOffset`s the call wrote, each of which
            // lands on an 8-byte-aligned record wholly inside the buffer; the name is
            // `FileNameLength` bytes from `FileName`, inside the same record.
            let record = unsafe { &*(base.add(at) as *const FILE_ID_EXTD_DIR_INFO) };
            let name = unsafe {
                std::slice::from_raw_parts(
                    std::ptr::addr_of!(record.FileName) as *const u16,
                    record.FileNameLength as usize / 2,
                )
            };
            if !is_dot_entry(name) {
                builder.push_wide(
                    name,
                    record.EndOfFile as u64,
                    record.LastWriteTime as u64,
                    flags_of(record.FileAttributes, record.ReparsePointTag),
                );
                builder.identify(u128::from_le_bytes(record.FileId.Identifier));
            }
            if record.NextEntryOffset == 0 {
                break;
            }
            at += record.NextEntryOffset as usize;
        }
    }
    Ok(builder.finish(elapsed_micros(started)))
}

/// `path` as `\\?\C:\some\dir`, for opening the directory itself: [`search_pattern`] without the
/// `*`, and without the separator in front of it unless that separator is the root's own.
#[cfg(windows)]
fn directory_name(path: &Path) -> Vec<u16> {
    let mut name = search_pattern(path);
    name.pop(); // the terminator
    name.pop(); // `*`
    const SEP: u16 = b'\\' as u16;
    // `\\?\C:\` needs its backslash — without it, it names the volume rather than its root.
    let root = name.len() >= 2 && name[name.len() - 2] == b':' as u16;
    if name.last() == Some(&SEP) && !root {
        name.pop();
    }
    name.push(0);
    name
}

#[cfg(windows)]
fn trimmed(buf: &[u16]) -> &[u16] {
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    &buf[..len]
}

/// The find API's read: no IDs, and every failure worded the way the rest of the program expects.
#[cfg(windows)]
pub(super) fn scan_find(path: &Path, started: Instant) -> Dir {
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
/// `tag` is the reparse tag — the find data's `dwReserved0`, or the ID read's `ReparsePointTag` — which
/// means something when the attributes say there is one and is meaningless otherwise. It is read for one family: `IO_REPARSE_TAG_CLOUD_*` is
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
