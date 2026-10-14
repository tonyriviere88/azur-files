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
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Storage::FileSystem::{
        WIN32_FIND_DATAW, FILE_ATTRIBUTE_DIRECTORY,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH};

    let wide = crate::shell::nul_terminated(path);
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
    let arguments = crate::shell::nul_terminated(arguments);
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


/// Whether this process may make a symbolic link. See [`super::may_symlink`].
///
/// **Two ways, and either will do.** The privilege is what `CreateSymbolicLinkW` checks, and it is in
/// the token of an elevated administrator and of nobody else by default — an administrator's
/// *unelevated* token has it filtered out. Developer Mode is the other: with it on,
/// `SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE` (which `std` passes on every call) lets a standard
/// token make one. Read from the registry key the Settings page writes, because there is no API.
#[cfg(windows)]
pub(super) fn may_symlink() -> bool {
    holds_symlink_privilege() || developer_mode()
}

#[cfg(windows)]
fn holds_symlink_privilege() -> bool {
    use windows_sys::Win32::Foundation::LUID;
    use windows_sys::Win32::Security::{LookupPrivilegeValueW, TokenPrivileges, TOKEN_PRIVILEGES};

    let name = windows_sys::w!("SeCreateSymbolicLinkPrivilege");
    let mut wanted = LUID { LowPart: 0, HighPart: 0 };
    // SAFETY: a static NUL-terminated name and a local to write the LUID into.
    if unsafe { LookupPrivilegeValueW(std::ptr::null(), name, &mut wanted) } == 0 {
        return false;
    }
    // `u64`s, so the buffer is aligned for the `LUID_AND_ATTRIBUTES` inside it.
    let Some(buffer) = crate::shell::token_information(TokenPrivileges) else {
        return false;
    };
    // SAFETY: the call filled the buffer with a `TOKEN_PRIVILEGES` and its trailing array, whose
    // length is the count it starts with.
    let privileges = unsafe {
        let head = &*(buffer.as_ptr() as *const TOKEN_PRIVILEGES);
        std::slice::from_raw_parts(
            head.Privileges.as_ptr(),
            head.PrivilegeCount as usize,
        )
    };
    // Present in the token is enough: `CreateSymbolicLinkW` enables it itself.
    privileges
        .iter()
        .any(|p| p.Luid.LowPart == wanted.LowPart && p.Luid.HighPart == wanted.HighPart)
}

#[cfg(windows)]
fn developer_mode() -> bool {
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    crate::shell::registry::dword(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock",
        "AllowDevelopmentWithoutDevLicense",
    )
    .is_some_and(|data| data != 0)
}

/// The file system's name for the volume at `root` — `NTFS`, `ReFS`, `FAT32` — or `None`.
#[cfg(windows)]
pub(super) fn file_system(root: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let mut wide = crate::shell::wide(root);
    // `C:` is the current directory on C:, not its root; the call wants the slash.
    if wide.len() >= 2 && wide[wide.len() - 2] != u16::from(b'\\') {
        wide.insert(wide.len() - 1, u16::from(b'\\'));
    }
    let mut name = [0u16; 64];
    // SAFETY: a NUL-terminated root and a buffer as long as said; nothing else is asked for.
    let ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    } != 0;
    let len = name.iter().position(|&unit| unit == 0).unwrap_or(0);
    ok.then(|| String::from_utf16_lossy(&name[..len]))
}

/// Make `at` a junction to the folder `target`: an empty folder carrying a mount-point reparse buffer.
///
/// What `mklink /J` does — there is no API for it. The buffer holds the target twice, as the
/// substitute name the file system follows (`\??\C:\target`) and as the print name a listing shows
/// (`C:\target`). Removed again if the reparse data does not take, because an empty folder where a
/// link should be is worse than nothing.
#[cfg(windows)]
pub(super) fn junction(at: &Path, target: &Path) -> Result<(), String> {
    use crate::shell::FSCTL_SET_REPARSE_POINT;
    use std::ffi::c_void;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{GetLastError, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, RemoveDirectoryW, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    let words = crate::shell::win32_message;

    // The target as a plain absolute path: no `\\?\`, which is the substitute name's job to add.
    let text = target.to_string_lossy();
    let print = text.strip_prefix(r"\\?\").unwrap_or(&text).trim_end_matches('\\');
    if !print.as_bytes().get(1).is_some_and(|&c| c == b':') {
        return Err("a junction can only lead to a folder on a local drive".to_owned());
    }
    let substitute: Vec<u16> = format!(r"\??\{print}").encode_utf16().collect();
    let print: Vec<u16> = print.encode_utf16().collect();

    // REPARSE_DATA_BUFFER: tag, data length, reserved; then the mount point's four offsets and
    // lengths, in bytes; then both names, each NUL-terminated.
    let names = (substitute.len() + 1 + print.len() + 1) * 2;
    let data_length = 8 + names;
    let mut buffer: Vec<u8> = Vec::with_capacity(8 + data_length);
    buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend_from_slice(&(data_length as u16).to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&((substitute.len() * 2) as u16).to_le_bytes());
    buffer.extend_from_slice(&(((substitute.len() + 1) * 2) as u16).to_le_bytes());
    buffer.extend_from_slice(&((print.len() * 2) as u16).to_le_bytes());
    for unit in substitute.iter().chain(&[0]).chain(&print).chain(&[0]) {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }

    let wide = crate::shell::wide(at);
    // SAFETY: a NUL-terminated path and no security attributes.
    if unsafe { CreateDirectoryW(wide.as_ptr(), std::ptr::null()) } == 0 {
        return Err(words(unsafe { GetLastError() }));
    }
    // SAFETY: as above; opened as itself, for writing its reparse data.
    let folder = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    let written = if folder == INVALID_HANDLE_VALUE {
        Err(words(unsafe { GetLastError() }))
    } else {
        // SAFETY: a handle just opened, and owned from here on; closed once, when this goes.
        let folder = unsafe { OwnedHandle::from_raw_handle(folder) };
        // SAFETY: an open handle; the buffer is as long as said and outlives the call.
        unsafe {
            DeviceIoControl(
                windows::Win32::Foundation::HANDLE(folder.as_raw_handle()),
                FSCTL_SET_REPARSE_POINT,
                Some(buffer.as_ptr() as *const c_void),
                buffer.len() as u32,
                None,
                0,
                None,
                None,
            )
        }
        .map_err(|e| e.message())
    };
    if written.is_err() {
        // SAFETY: the folder made above, still empty.
        unsafe { RemoveDirectoryW(wide.as_ptr()) };
    }
    written
}

/// The reparse tag of the reparse point at `path`, which tells a junction from a symbolic link.
///
/// From the find data, whose `dwReserved0` carries the tag for a reparse point — the same field the
/// listing reads it from. One directory read of one name, and nothing is followed.
#[cfg(windows)]
pub(super) fn reparse_tag(path: &Path) -> Option<u32> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindFirstFileW, FILE_ATTRIBUTE_REPARSE_POINT, WIN32_FIND_DATAW,
    };
    let wide = crate::shell::wide(path);
    // SAFETY: a NUL-terminated path, and a local for the call to fill.
    let mut found: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let find = unsafe { FindFirstFileW(wide.as_ptr(), &mut found) };
    if find == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: opened above, closed once.
    unsafe { FindClose(find) };
    (found.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0).then_some(found.dwReserved0)
}
