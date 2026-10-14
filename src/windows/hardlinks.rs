//! A file's link count, by name relative to its folder, and every name it has.
//!
//! The Windows half of [`crate::hardlinks`].
//!
//! `NtQueryInformationByName` with `FileStatInformation` answers the count without a handle on the
//! file: the name is resolved relative to one handle on the folder, opened once per request. Where
//! that information class is refused — a file system that does not implement it — each file is
//! opened for its attributes instead and asked with `GetFileInformationByHandle`, which every
//! version answers.

use super::*;
use crate::shell::{nul_terminated, wide_text};
use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FindClose, FindFirstFileNameW, FindNextFileNameW, GetFileInformationByHandle,
    GetVolumePathNameW, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// `UNICODE_STRING`, `OBJECT_ATTRIBUTES` and `IO_STATUS_BLOCK`, spelled out rather than taken from
/// `windows-sys`, which only has the call behind the driver kit's features — the same choice
/// [`crate::fs::scan`]'s placeholder call makes.
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: HANDLE,
    name: *const UnicodeString,
    attributes: u32,
    security: *const c_void,
    quality: *const c_void,
}

#[repr(C)]
struct IoStatus {
    status: usize,
    information: usize,
}

/// `FILE_STAT_INFORMATION`.
#[repr(C)]
#[derive(Default)]
struct FileStat {
    file_id: i64,
    creation: i64,
    access: i64,
    write: i64,
    change: i64,
    allocation: i64,
    end_of_file: i64,
    attributes: u32,
    reparse_tag: u32,
    links: u32,
    effective_access: u32,
}

const FILE_STAT_INFORMATION: u32 = 68;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;

#[link(name = "ntdll", kind = "raw-dylib")]
extern "system" {
    fn NtQueryInformationByName(
        attributes: *const ObjectAttributes,
        status: *mut IoStatus,
        information: *mut c_void,
        length: u32,
        class: u32,
    ) -> i32;
}

/// The folder whose files are being counted, closed when this goes.
pub(super) struct Counter {
    folder: OwnedHandle,
    path: PathBuf,
    /// Cleared on the first refusal of the information class, after which every file is opened.
    by_name: std::cell::Cell<bool>,
}

impl Counter {
    pub(super) fn open(dir: &Path) -> Option<Self> {
        let wide = nul_terminated(dir);
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        let folder = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        (folder != INVALID_HANDLE_VALUE).then(|| Self {
            // SAFETY: a handle just opened, and owned from here on.
            folder: unsafe { OwnedHandle::from_raw_handle(folder) },
            path: dir.to_owned(),
            by_name: std::cell::Cell::new(true),
        })
    }

    /// How many names the file `name` has. `None` when it cannot be asked — gone since the listing
    /// was read, or refused.
    pub(super) fn links(&self, name: &str) -> Option<u32> {
        if self.by_name.get() {
            match self.by_name(name) {
                Ok(links) => return links,
                Err(()) => self.by_name.set(false),
            }
        }
        self.by_handle(name)
    }

    /// `Err` when the information class itself is refused, which no later file will answer either.
    fn by_name(&self, name: &str) -> Result<Option<u32>, ()> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let Ok(bytes) = u16::try_from(units.len() * 2) else {
            return Ok(None);
        };
        let text = UnicodeString {
            length: bytes,
            maximum: bytes,
            buffer: units.as_ptr(),
        };
        let attributes = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root: self.folder.as_raw_handle(),
            name: &text,
            attributes: OBJ_CASE_INSENSITIVE,
            security: std::ptr::null(),
            quality: std::ptr::null(),
        };
        let mut io = IoStatus { status: 0, information: 0 };
        let mut stat = FileStat::default();
        // SAFETY: every pointer is to a local that outlives the call, and the length is the
        // buffer's.
        let status = unsafe {
            NtQueryInformationByName(
                &attributes,
                &mut io,
                (&mut stat as *mut FileStat).cast(),
                std::mem::size_of::<FileStat>() as u32,
                FILE_STAT_INFORMATION,
            )
        };
        const STATUS_INVALID_INFO_CLASS: i32 = 0xC000_0003_u32 as i32;
        const STATUS_NOT_IMPLEMENTED: i32 = 0xC000_0002_u32 as i32;
        const STATUS_INVALID_PARAMETER: i32 = 0xC000_000D_u32 as i32;
        const STATUS_NOT_SUPPORTED: i32 = 0xC000_00BB_u32 as i32;
        match status {
            0 => Ok(Some(stat.links)),
            STATUS_INVALID_INFO_CLASS | STATUS_NOT_IMPLEMENTED | STATUS_INVALID_PARAMETER
            | STATUS_NOT_SUPPORTED => Err(()),
            _ => Ok(None),
        }
    }

    fn by_handle(&self, name: &str) -> Option<u32> {
        let wide = nul_terminated(self.path.join(name));
        // SAFETY: as in [`Counter::open`]. Opened for its attributes only and as itself, so nothing
        // is read and nothing is followed.
        let file = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if file == INVALID_HANDLE_VALUE {
            return None;
        }
        // SAFETY: a handle just opened, and owned from here on; closed when this goes.
        let file = unsafe { OwnedHandle::from_raw_handle(file) };
        // SAFETY: an open handle, and a local of the size the call writes.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0;
        ok.then_some(info.nNumberOfLinks)
    }
}

/// Every path the file at `path` is known by, as `C:\a\b.txt`.
///
/// `FindFirstFileNameW` gives each name relative to the volume's root, without the drive — so the
/// root is asked for once and put back in front.
pub(super) fn names(path: &Path) -> Vec<String> {
    let wide = nul_terminated(path);
    let mut root = [0u16; 1024];
    // SAFETY: `wide` is NUL-terminated; the buffer is as long as said.
    if unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return Vec::new();
    }
    let root = wide_text(&root);
    let root = root.trim_end_matches('\\');

    let mut names = Vec::new();
    let mut buffer = vec![0u16; 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: as above; `length` is the buffer's length in characters, updated by the call.
    let find = unsafe { FindFirstFileNameW(wide.as_ptr(), 0, &mut length, buffer.as_mut_ptr()) };
    if find == INVALID_HANDLE_VALUE {
        return names;
    }
    loop {
        names.push(format!("{root}{}", wide_text(&buffer)));
        length = buffer.len() as u32;
        // SAFETY: an open find handle and the same buffer.
        if unsafe { FindNextFileNameW(find, &mut length, buffer.as_mut_ptr()) } == 0 {
            break;
        }
    }
    // SAFETY: opened above, closed once.
    unsafe { FindClose(find) };
    names
}
