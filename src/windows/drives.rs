//! Which drives exist, and what to call them.
//!
//! The Windows half of [`crate::fs::drives`]. The expensive call is deliberately not here on the
//! startup path — asking a disconnected share for its volume label can take twenty seconds.

use super::*;

/// Every mounted volume, by letter and kind only. Microseconds, no I/O.
#[cfg(windows)]
pub fn list_letters() -> Vec<Drive> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    // The DRIVE_* constants live behind a `windows-sys` feature that carries nothing
    // else this program wants, and they are stable numbers.
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    const DRIVE_RAMDISK: u32 = 6;

    let mask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();

    for bit in 0..26u32 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let letter = (b'A' + bit as u8) as char;
        let root = root_of(letter);
        let kind = match unsafe { GetDriveTypeW(root.as_ptr()) } {
            DRIVE_FIXED => DriveKind::Fixed,
            DRIVE_REMOVABLE => DriveKind::Removable,
            DRIVE_CDROM => DriveKind::Optical,
            DRIVE_REMOTE => DriveKind::Network,
            DRIVE_RAMDISK => DriveKind::RamDisk,
            _ => DriveKind::Unknown,
        };
        drives.push(Drive {
            path: PathBuf::from(format!("{letter}:\\")),
            letter: format!("{letter}:"),
            label: kind.default_label().to_owned(),
            kind,
            total: 0,
            free: 0,
            described: false,
        });
    }
    drives
}

/// Fill in a volume's label and free space.
///
/// **May block for tens of seconds.** Never call this on the UI thread.
#[cfg(windows)]
pub fn describe(drive: &mut Drive) {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumeInformationW};

    let letter = drive.letter.chars().next().unwrap_or('C');
    let root = root_of(letter);

    let mut label_buf = [0u16; 128];
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            label_buf.as_mut_ptr(),
            label_buf.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    } != 0;

    if ok {
        let text = wide_to_string(&label_buf);
        if !text.is_empty() {
            drive.label = text;
        }
        let (mut total, mut free, mut available) = (0u64, 0u64, 0u64);
        let measured =
            unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut available, &mut total, &mut free) }
                != 0;
        if measured {
            drive.total = total;
            // The quota-aware figure is the one that matters to whoever is looking,
            // and it is never larger than the volume's own free count.
            drive.free = free.min(available);
        }
    }
    // Failure leaves the kind's default label and no bar, which is the honest
    // rendering of "there is a slot here and nothing in it".
    drive.described = true;
    remember(drive.clone());
}

/// `C:\` as a null-terminated wide string, which every volume call wants.
#[cfg(windows)]
pub(super) fn root_of(letter: char) -> [u16; 4] {
    [letter as u16, b':' as u16, b'\\' as u16, 0]
}

/// A null-terminated wide buffer as a `String`.
#[cfg(windows)]
pub(super) fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}
