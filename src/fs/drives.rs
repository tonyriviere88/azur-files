//! Volumes: what is mounted, how full it is, and the synthetic "This PC" listing.
//!
//! # The 22-second trap
//!
//! Listing drive letters is free. *Describing* one is not, and the difference is
//! not small: on this machine, the first `GetVolumeInformationW` against a mapped
//! network drive that is not currently reachable takes **22 seconds** while SMB
//! tries to reconnect underneath it. A file manager that asks for volume labels on
//! its startup path is a file manager whose window does not appear for 22 seconds.
//!
//! So the two halves are separate:
//!
//! - [`list_letters`] is `GetLogicalDrives` + `GetDriveTypeW`. Both answer from the
//!   local mount table with no I/O, in microseconds. This is what the sidebar shows
//!   immediately.
//! - [`describe`] is the part that can block — the label and the free space. It runs
//!   on a worker (see [`crate::loader::Volumes`]), one thread per volume so a
//!   stalled share delays only its own row, and the answer is merged in when it
//!   arrives.
//!
//! Every thread that calls [`describe`] must have called
//! [`super::scan::silence_device_dialogs`] first, or an empty card reader raises
//! "Please insert a disk into drive E:" from inside the syscall.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use super::dir::{Dir, DirBuilder, FLAG_DIR};

/// What kind of thing a volume is, which decides its icon.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DriveKind {
    Fixed,
    Removable,
    Optical,
    Network,
    RamDisk,
    Unknown,
}

impl DriveKind {
    /// What Explorer calls a volume with no label of its own.
    pub fn default_label(self) -> &'static str {
        match self {
            Self::Fixed => "Local Disk",
            Self::Removable => "Removable Disk",
            Self::Optical => "DVD Drive",
            Self::Network => "Network Drive",
            Self::RamDisk => "RAM Disk",
            Self::Unknown => "Disk",
        }
    }
}

/// One mounted volume.
#[derive(Clone, Debug)]
pub struct Drive {
    /// `C:\` — where clicking it navigates.
    pub path: PathBuf,
    /// `C:` — the row's identity, and the key descriptions are merged on.
    pub letter: String,
    /// The volume label, or the drive kind's default name until one is known.
    pub label: String,
    pub kind: DriveKind,
    /// Capacity and free space in bytes. Both zero until described.
    pub total: u64,
    pub free: u64,
    /// Whether [`describe`] has run against this volume yet.
    pub described: bool,
}

impl Drive {
    /// How full, in `0..=1`. `None` when the volume has no measurement — either
    /// because nothing is in it, or because it has not been described yet.
    pub fn used_fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| {
            let used = self.total.saturating_sub(self.free);
            (used as f64 / self.total as f64) as f32
        })
    }
}

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
fn root_of(letter: char) -> [u16; 4] {
    [letter as u16, b':' as u16, b'\\' as u16, 0]
}

/// Mount points, for the platforms that have no drive letters.
#[cfg(not(windows))]
pub fn list_letters() -> Vec<Drive> {
    vec![Drive {
        path: PathBuf::from("/"),
        letter: "/".to_owned(),
        label: "Filesystem".to_owned(),
        kind: DriveKind::Fixed,
        total: 0,
        free: 0,
        described: false,
    }]
}

#[cfg(not(windows))]
pub fn describe(drive: &mut Drive) {
    drive.described = true;
    remember(drive.clone());
}

// ---------------------------------------------------------------------------
// What has been learned so far
// ---------------------------------------------------------------------------

/// Descriptions already paid for, keyed by letter.
///
/// A process-wide cache rather than state threaded through the application, because
/// the set of volumes on the machine *is* process-wide — and because [`this_pc`] runs
/// on a scanner worker that has no way to reach the sidebar's copy. Nothing here
/// blocks: it is only ever read, or written by a probe that has already paid the
/// cost.
static KNOWN: Mutex<Vec<Drive>> = Mutex::new(Vec::new());

/// Record what a probe found.
pub fn remember(drive: Drive) {
    let Ok(mut known) = KNOWN.lock() else { return };
    match known.iter_mut().find(|d| d.letter == drive.letter) {
        Some(existing) => *existing = drive,
        None => known.push(drive),
    }
}

/// The described volumes, if any probe has finished.
pub fn known() -> Vec<Drive> {
    KNOWN.lock().map(|k| k.clone()).unwrap_or_default()
}

/// Drop everything learned, so a refresh actually re-reads.
pub fn forget_all() {
    if let Ok(mut known) = KNOWN.lock() {
        known.clear();
    }
}

/// The drive list as a [`Dir`], so "This PC" is a listing like any other and the
/// details view needs no special case for it.
///
/// Letters come from the mount table and labels from whatever has already been
/// described, so this never blocks — a volume nobody has probed yet appears under
/// its kind's name and gains its real one once the probe lands.
///
/// The rows carry explicit targets, because a drive's display name (`Windows (C:)`)
/// is not a child of the empty path the way a file name is a child of its folder.
pub fn this_pc(started: Instant) -> Dir {
    let described = known();
    let drives: Vec<Drive> = list_letters()
        .into_iter()
        .map(|drive| {
            described
                .iter()
                .find(|d| d.letter == drive.letter)
                .cloned()
                .unwrap_or(drive)
        })
        .collect();

    let mut builder = DirBuilder::new(PathBuf::new());
    builder.reserve(drives.len());
    for drive in &drives {
        builder.push_link(
            &format!("{} ({})", drive.label, drive.letter),
            drive.path.clone(),
            drive.total,
            FLAG_DIR,
        );
    }
    builder.finish(started.elapsed().as_micros() as u64)
}

/// A null-terminated wide buffer as a `String`.
#[cfg(windows)]
fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the split: this is on the startup path and must not do
    /// I/O. A generous bound, since a loaded machine is still nowhere near a network
    /// timeout.
    #[test]
    fn listing_letters_does_no_io() {
        let started = Instant::now();
        let drives = list_letters();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "list_letters took {elapsed:?} -- it is on the startup path and a \
             blocking volume query has crept back into it"
        );
        for drive in &drives {
            assert!(!drive.described);
            assert_eq!(drive.total, 0, "an undescribed volume has no measurement");
        }
    }

    #[test]
    fn this_pc_never_blocks_either() {
        let started = Instant::now();
        let dir = this_pc(Instant::now());
        assert!(
            started.elapsed() < std::time::Duration::from_millis(150),
            "This PC is a listing like any other and cannot wait on a share"
        );
        assert_eq!(dir.len(), list_letters().len());
        for i in 0..dir.len() {
            assert!(dir.entries[i].is_dir());
            assert!(
                !dir.target(i).as_os_str().is_empty(),
                "every drive row has to lead somewhere"
            );
        }
    }

    #[test]
    fn descriptions_merge_by_letter() {
        let sample = |label: &str, total, free| Drive {
            path: PathBuf::from("Z:\\"),
            letter: "Z:".to_owned(),
            label: label.to_owned(),
            kind: DriveKind::Fixed,
            total,
            free,
            described: true,
        };
        remember(sample("First", 100, 40));
        remember(sample("Second", 200, 50));

        let known = known();
        let z: Vec<&Drive> = known.iter().filter(|d| d.letter == "Z:").collect();
        assert_eq!(z.len(), 1, "a letter is described once, not appended to");
        assert_eq!(z[0].label, "Second");
        assert_eq!(z[0].used_fraction(), Some(0.75));
    }

    #[test]
    fn an_undescribed_volume_has_no_gauge() {
        let drives = list_letters();
        if let Some(drive) = drives.first() {
            assert_eq!(
                drive.used_fraction(),
                None,
                "a bar drawn before the measurement lands would be a made-up number"
            );
        }
    }
}
