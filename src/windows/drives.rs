//! Which drives exist, and what to call them.
//!
//! The Windows half of [`crate::fs::drives`]. The expensive call is deliberately not here on the
//! startup path — asking a disconnected share for its volume label can take twenty seconds.

use super::*;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

/// The codes the calls below end on, written out rather than imported: they live behind
/// `windows-sys` features that carry nothing else this program wants, and they are stable numbers —
/// the same reason the `DRIVE_*` values are spelled out in [`list_letters`].
#[cfg(windows)]
const NO_ERROR: u32 = 0;
#[cfg(windows)]
const ERROR_MORE_DATA: u32 = 234;
#[cfg(windows)]
const ERROR_NO_MORE_ITEMS: u32 = 259;
/// What [`connect`] answers when a test asked it not to open a dialog: a prompt nobody was shown is
/// a prompt nobody answered. Not `NO_ERROR`, which as an error code reads "this failed,
/// successfully". Only a test build has anything to say it with.
#[cfg(all(windows, test))]
const ERROR_CANCELLED: u32 = 1223;

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
        let path = PathBuf::from(format!("{letter}:\\"));
        let root = wide_root(&path);
        let kind = match unsafe { GetDriveTypeW(root.as_ptr()) } {
            DRIVE_FIXED => DriveKind::Fixed,
            DRIVE_REMOVABLE => DriveKind::Removable,
            DRIVE_CDROM => DriveKind::Optical,
            DRIVE_REMOTE => DriveKind::Network,
            DRIVE_RAMDISK => DriveKind::RamDisk,
            _ => DriveKind::Unknown,
        };
        drives.push(Drive {
            path,
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

/// Every network connection this machine has *and can currently use*, as `(local name, remote path,
/// resource type)`.
///
/// **The "currently" is a second call and not a detail of this one** — see [`unavailable`], which is
/// what knows the difference and why this enumeration cannot. A stale entry left behind by an old
/// sign-in is indistinguishable from a live connection here, and taking one for the other is what
/// put a disconnected machine in the Network group.
///
/// The local name is `H:` for a mapped drive and **empty for a deviceless connection** — one made
/// to a UNC path without mapping a letter, which is what happens every time a UNC path is opened,
/// typed into an address bar, or reached through a shortcut. That second kind is the gap
/// [`list_letters`] cannot close: `GetLogicalDrives` is a 26-bit mask, and a connection with no
/// letter is not one of those bits. `net use` shows one as a row with an empty *Local* column;
/// this is the same enumeration `net use` does.
///
/// **`RESOURCETYPE_ANY`, and the type comes back with each row rather than filtering the
/// enumeration.** Asking for `RESOURCETYPE_DISK` looks obviously right and quietly loses the most
/// important connection there is: signing in to a *machine* — which is what [`connect`] does when a
/// server will not say what it offers — establishes a session to `\\server\IPC$`, and `IPC$` is an
/// inter-process channel rather than a disk. `net use` listed it and this enumeration did not, so a
/// machine the user had just authenticated to was reported as not connected. The two callers want
/// different halves of the answer, so the filtering is theirs: [`list_shares`] wants disks and
/// [`list_servers`] wants anything that names a machine.
///
/// **`RESOURCE_CONNECTED`, and it does not touch the network.** Which is the whole reason this is
/// allowed to sit next to [`list_letters`] on the startup path — see the note about the 22-second
/// trap on [`crate::fs::drives`], which this would otherwise walk straight into. Measured on a
/// machine with `H:` mapped and `\\fileserver\web` connected but *unreachable*: the enumeration
/// returns both in **0 ms**, dead share included, because the redirector's connection table is
/// local. It is the same reason [`crate::shell::over_network`] can afford `GetDriveTypeW`. The
/// 13 ms this costs the first time is `mpr.dll` being loaded, once per process.
///
/// **`RESOURCE_REMEMBERED` is deliberately not asked for.** A *persistent* connection is by
/// construction one with a letter — that is what is persisted — so the remembered scope returns
/// nothing the Drives group is not already showing. Measured on the same machine: `H:` and nothing
/// else. `RESOURCE_RECENT` is the same list again.
#[cfg(windows)]
fn connections() -> Vec<(String, PathBuf, u32)> {
    use windows_sys::Win32::NetworkManagement::WNet::{
        WNetCloseEnum, WNetEnumResourceW, WNetOpenEnumW, NETRESOURCEW, RESOURCETYPE_ANY,
        RESOURCE_CONNECTED,
    };

    let mut handle = std::ptr::null_mut();
    // `dwUsage` of zero is "no filter", which is the only sensible thing to ask of a scope that
    // is a flat list of connections rather than a tree to be walked.
    let opened = unsafe {
        WNetOpenEnumW(
            RESOURCE_CONNECTED,
            RESOURCETYPE_ANY,
            0,
            std::ptr::null(),
            &mut handle,
        )
    };
    if opened != NO_ERROR {
        // A machine with no network provider at all, which is a machine with no network
        // locations. An empty list is the honest answer and there is nothing to report.
        return Vec::new();
    }

    // A `Vec<NETRESOURCEW>` rather than a byte buffer, for its alignment: the call writes an
    // array of these structs at the front and packs the strings they point at into the space
    // left at the back, so the buffer has to be aligned for the struct even though most of it
    // ends up holding UTF-16.
    //
    // 64 entries is far more connections than a machine has, and the loop below is what makes
    // that a starting size rather than a limit.
    let mut buffer = vec![NETRESOURCEW::default(); 64];
    let mut found: Vec<(String, PathBuf, u32)> = Vec::new();
    // Asked once, before the walk, because it is a table rather than a question per row — and
    // asked here rather than in the two callers so that neither can forget it. See [`unavailable`].
    let down = unavailable();
    loop {
        // `u32::MAX` is "as many as fit", and both counts are in/out — so both are set afresh
        // on every pass round the loop.
        let mut count = u32::MAX;
        let mut bytes = (buffer.len() * std::mem::size_of::<NETRESOURCEW>()) as u32;
        let result = unsafe {
            WNetEnumResourceW(
                handle,
                &mut count,
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if result == ERROR_MORE_DATA {
            // One entry whose strings will not fit in what we offered. `bytes` comes back as
            // the size that would do; grow to it and ask again rather than losing the row.
            let wanted = (bytes as usize).div_ceil(std::mem::size_of::<NETRESOURCEW>()) + 1;
            if wanted <= buffer.len() {
                break;
            }
            buffer.resize(wanted, NETRESOURCEW::default());
            continue;
        }
        // The end of the list, which is how every one of these ends.
        if result == ERROR_NO_MORE_ITEMS {
            break;
        }
        // A provider that has stopped answering. What has been collected so far stands: a panel
        // listing three of four network locations is better than one listing none, and there is
        // nobody here to report the fourth to.
        if result != NO_ERROR {
            break;
        }

        // Clamped, because `count` went in as `u32::MAX` and a provider that returns success
        // without having written it back would turn a slice into a panic. It is the one number
        // here this program did not choose.
        let written = (count as usize).min(buffer.len());
        for entry in &buffer[..written] {
            let remote = from_wide_ptr(entry.lpRemoteName);
            // A provider that hands back something other than a UNC path — there is nothing
            // useful a row could do with it, since navigating there is the row's only purpose.
            if !remote.starts_with("\\\\") {
                continue;
            }
            // One connection can be reported twice — the multiple-provider router and a
            // redirector both answering for it — and the two need not be next to each other in
            // the list, so this is checked against everything found rather than against the row
            // before. Case-insensitively, because a UNC path is. Two identical rows in the panel
            // would read as two places.
            let path = PathBuf::from(remote.trim_end_matches('\\'));
            // A connection Windows says is down is not a place to offer. Dropped here, at the one
            // door both [`list_shares`] and [`list_servers`] come through, so that a share and the
            // machine it is on cannot disagree about whether it is there.
            if down
                .iter()
                .any(|dead| dead.as_os_str().eq_ignore_ascii_case(path.as_os_str()))
            {
                continue;
            }
            if found
                .iter()
                .any(|(_, had, _)| had.as_os_str().eq_ignore_ascii_case(path.as_os_str()))
            {
                continue;
            }
            found.push((from_wide_ptr(entry.lpLocalName), path, entry.dwType));
        }
    }
    unsafe { WNetCloseEnum(handle) };
    found
}

/// The connections Windows knows are not usable right now, as remote paths.
///
/// **[`connections`] cannot answer this, and that is the whole reason this exists.** `NETRESOURCEW`
/// carries no status field, so every row in the `RESOURCE_CONNECTED` scope looks equally live —
/// including the `\\fileserver\IPC$` a sign-in left behind long ago, whose session is gone while the
/// entry is not. [`list_servers`] read one of those as a machine this program was connected to, and
/// put a row in the Network group for a machine nothing had reached in weeks. `net use` prints a
/// status against the same row because it asks a different API, and this is that API.
///
/// **Only the two states that mean "not reachable now"** — `USE_DISCONN` (which the headers also
/// spell `USE_SESSLOST`) and `USE_NETERR`. Paused and reconnecting are left alone: one is suspended
/// and the other is trying, and neither is a machine that has gone away. A positive list of
/// failures rather than a list of successes, so a status nobody here has heard of keeps its row
/// instead of silently losing it.
///
/// **A connection absent from this table keeps its row too**, which is not a detail: `NetUseEnum` is
/// the SMB redirector's own use table and does not account for every network provider. Measured on
/// this machine, a DFS drive mapped through the Windows network provider is in the
/// [`connections`] list and **not** in this one — so a filter that demanded `USE_OK` would have
/// hidden a working mapped drive. Absence is not evidence.
///
/// **0.2 ms, and no network I/O** — the workstation service answers from a local table exactly as
/// the enumeration beside it does. The first call in a process that has not yet loaded
/// `netapi32.dll` is 3.6 ms, once. Both are why this is allowed on the startup path; see the note
/// about the 22-second trap on [`crate::fs::drives`].
///
/// `pub(super)` for the test beside the other startup-path ones, which is the only thing outside
/// this file that names it — the filter itself belongs to [`connections`] and stays there.
#[cfg(windows)]
pub(super) fn unavailable() -> Vec<PathBuf> {
    use windows_sys::Win32::NetworkManagement::NetManagement::{
        NetApiBufferFree, NetUseEnum, USE_DISCONN, USE_INFO_2, USE_NETERR,
    };

    /// "As much as it takes", which is what makes the resume loop below a formality.
    const MAX_PREFERRED_LENGTH: u32 = u32::MAX;

    let mut down: Vec<PathBuf> = Vec::new();
    let mut resume = 0u32;
    loop {
        let mut buffer: *mut u8 = std::ptr::null_mut();
        let (mut read, mut total) = (0u32, 0u32);
        // A null server name is "this machine's own connections", which is the only question here.
        // Level 2 for one field of it, `ui2_status`: level 1 has no status and level 3 wraps this
        // one to add flags nothing here reads. The level also carries `ui2_password`, which Windows
        // blanks and this never touches.
        let result = unsafe {
            NetUseEnum(
                std::ptr::null(),
                2,
                &mut buffer,
                MAX_PREFERRED_LENGTH,
                &mut read,
                &mut total,
                &mut resume,
            )
        };
        // No status table means nothing is *known* to be down, so every connection stands and the
        // panel is what it was before this filter existed. The right way round to fail: a row too
        // many is visible and can be clicked, while a machine wrongly hidden is unreachable.
        if result != NO_ERROR && result != ERROR_MORE_DATA {
            break;
        }
        if buffer.is_null() {
            break;
        }
        // SAFETY: the call has written `read` `USE_INFO_2` records into a buffer it allocated, and
        // it is given back below whatever this loop makes of them.
        let entries =
            unsafe { std::slice::from_raw_parts(buffer.cast::<USE_INFO_2>(), read as usize) };
        for entry in entries {
            if entry.ui2_status == USE_DISCONN || entry.ui2_status == USE_NETERR {
                let remote = from_wide_ptr(entry.ui2_remote);
                if !remote.is_empty() {
                    down.push(PathBuf::from(remote.trim_end_matches('\\')));
                }
            }
        }
        unsafe { NetApiBufferFree(buffer.cast()) };
        if result == NO_ERROR {
            break;
        }
    }
    down
}

/// The network locations that need a row of their own.
///
/// One of the two answers [`connections`] gives, and what is left after two exclusions:
///
/// - **Anything with a drive letter**, because [`list_letters`] already has it. A mapped share is
///   a volume with a letter and belongs beside the local disks, not in a second row under another
///   heading.
/// - **Anything already one click away through its machine.** A connection to `\\fileserver\web`
///   is reached by opening the `fileserver` row that [`list_servers`] puts in the same group, so
///   a row for it as well names one folder twice — and names it in the less durable of the two
///   ways: a machine is there as long as it is there, while a *connection* exists only because
///   something opened that path and is gone after a reboot. A place you want kept is a bookmark,
///   which is the list built for exactly that.
///
/// What survives both is the connection nothing else can show: a DFS path such as
/// `\\domain\namespace\area\share`, whose first component is a domain rather than a machine and
/// which therefore contributes no machine row to be reached through. Without a row here it would
/// be invisible again, which is the whole fault this list exists to fix.
///
/// Asked of [`list_servers`] and [`machine_of`] rather than read off the shape of the path here, so
/// that the two cannot drift: what is hidden is exactly what that function offers, decided by the
/// same rule it decides with. The second enumeration it costs is another 0 ms.
#[cfg(windows)]
pub fn list_shares() -> Vec<Drive> {
    use windows_sys::Win32::NetworkManagement::WNet::RESOURCETYPE_DISK;

    let browsable = list_servers();
    connections()
        .into_iter()
        // A folder, and not the `IPC$` channel a machine sign-in leaves behind — that one is
        // evidence of a *machine* rather than a place, which is what [`list_servers`] takes it as.
        .filter(|(local, _, kind)| local.is_empty() && *kind == RESOURCETYPE_DISK)
        .filter(|(_, path, _)| {
            // Which machine this connection is on is [`machine_of`]'s answer and not a second
            // reading of the path here, so that the row this hides and the row [`list_servers`]
            // offers cannot disagree: both ask the same function. `None` is a connection with no
            // machine row to hide behind — a DFS path — and that is the one that stays.
            let Some(host) = machine_of(path) else {
                return true;
            };
            let machine = PathBuf::from(format!("\\\\{host}"));
            !browsable
                .iter()
                .any(|had| had.as_os_str().eq_ignore_ascii_case(machine.as_os_str()))
        })
        .map(|(_, path, _)| Drive {
            // The share rather than the volume label: see `describe`, which does not overwrite
            // this one.
            label: split_unc(&path)
                .map(|(_, share)| share)
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            path,
            // No letter, which is the whole point of this list and what
            // [`Drive::display_name`] keys the network naming on.
            letter: String::new(),
            kind: DriveKind::Network,
            total: 0,
            free: 0,
            described: false,
        })
        .collect()
}

/// The machines this program knows are there, as `\\server` paths to open.
///
/// The other answer out of [`connections`], and the one that makes a share nobody has connected
/// to *reachable*: `\\fileserver\web` being connected says the machine is there, and asking the
/// machine (see [`shares_on`]) turns up the other thirteen shares on it. Without this, a share is
/// discoverable only once something has already opened it, which is the wrong way round.
///
/// **Only a connection of the form `\\server\share` contributes one**, and that is a measurement
/// rather than a nicety. `H:` here is `\\lgs-net.com\alyo\alyodata\Common` — a DFS namespace, whose
/// first component is a *domain* and not a server, and [`shares_on`] against it takes **22.1
/// seconds** to come back with "the network path was not found". A row that spends twenty-two
/// seconds to say nothing is worse than no row: the share it leads to is reachable by typing its
/// path, which is the one thing a dead row cannot help with either.
#[cfg(windows)]
pub fn list_servers() -> Vec<PathBuf> {
    let mut servers: Vec<PathBuf> = Vec::new();
    // Every type, `IPC$` included: a session to `\\server\IPC$` is exactly what signing in to a
    // machine leaves behind, and it is the strongest evidence there is that the machine is
    // connected. See [`connections`], which is why the type is carried rather than filtered.
    for (_, remote, _) in connections() {
        // Which connections name a browsable machine is [`machine_of`]'s judgement, and it lives
        // out there rather than here so that it can be tested: this function reads the machine's
        // own connection table, and a rule buried in it has nowhere to be checked.
        let Some(host) = machine_of(&remote) else {
            continue;
        };
        let path = PathBuf::from(format!("\\\\{host}"));
        if !servers
            .iter()
            .any(|had| had.as_os_str().eq_ignore_ascii_case(path.as_os_str()))
        {
            servers.push(path);
        }
    }
    servers
}

/// Sign in to a machine or a share, with Windows' own credential dialog.
///
/// **The same thing Explorer does**, and by the same route: `WNetAddConnection3W` hands the whole
/// business to `mpr.dll`, which puts up the system credential prompt — the one that offers the
/// saved accounts, the domain choice and "Remember my credentials", none of which this program
/// could reproduce and none of which it should try to. There is no password anywhere in this
/// process: both string arguments are null, the dialog collects what it collects, and Windows
/// stores it in the credential manager if the user asks it to.
///
/// `CONNECT_INTERACTIVE` and deliberately **not** `CONNECT_PROMPT`. Interactive means "ask if you
/// have to"; `CONNECT_PROMPT` means "ask even if you already know", which would put a dialog in
/// front of somebody whose saved credentials would have worked. Everything reaching here has
/// already failed a read, so trying the stored answer first costs nothing and often succeeds
/// without a dialog at all.
///
/// And **not** `CONNECT_UPDATE_PROFILE`: that is what makes a mapped drive come back at logon, and
/// this is not mapping a drive. The connection lasts as long as the session, exactly like the one
/// Explorer makes by opening a UNC path — see [`connections`], which is what then lists it.
///
/// `owner` is this window, so the dialog is modal to it and cannot appear behind it. The UI thread
/// keeps painting while it is up, because this runs on a worker.
///
/// **Blocks for as long as the dialog is on screen.** Never call it on the UI thread.
#[cfg(windows)]
pub fn connect(target: &Path, owner: crate::shell::Owner) -> Result<(), u32> {
    use windows_sys::Win32::NetworkManagement::WNet::{
        WNetAddConnection3W, NETRESOURCEW, CONNECT_INTERACTIVE, RESOURCETYPE_DISK,
    };

    // **Not from a test, unless a test asked for it.** What is below this line puts a dialog on a
    // real screen and opens a real session to a real server, which is the same class of act as
    // handing a job to `IFileOperation` — so it answers to the same switch rather than a second one
    // of its own. See [`crate::shell::ops::FOR_REAL`] for the bug that switch is named after.
    //
    // A run-time check under `cfg(test)` rather than a `cfg`-ed early return, and that is not a
    // style choice: an unconditional return leaves everything below it unreachable, so a test build
    // stops compiling the very code the guard is protecting.
    #[cfg(test)]
    if !crate::shell::ops::FOR_REAL.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(ERROR_CANCELLED);
    }

    let mut name: Vec<u16> = target.as_os_str().encode_wide().collect();
    name.push(0);
    let resource = NETRESOURCEW {
        dwType: RESOURCETYPE_DISK,
        // The only field the call reads besides the type: no local name, because this is not
        // mapping a letter, and no provider, so the router picks whichever one owns the path.
        lpRemoteName: name.as_mut_ptr(),
        ..Default::default()
    };

    let result = unsafe {
        WNetAddConnection3W(
            owner.raw(),
            &resource,
            std::ptr::null(),
            std::ptr::null(),
            CONNECT_INTERACTIVE,
        )
    };
    if result == NO_ERROR {
        Ok(())
    } else {
        Err(result)
    }
}

/// Every disk share a server offers, asked of the server itself.
///
/// `NetShareEnum`, which is what `net view \\server` calls and the only way to enumerate a
/// machine: `FindFirstFileW` against `\\fileserver\*` fails with `ERROR_BAD_PATHNAME` (161),
/// because a server is not a directory. It is the reason a bare `\\server` path used to come up
/// as "Could not read this folder (error 161)".
///
/// **This one really does go to the network, and it must never run on the UI thread.** Measured
/// here:
///
/// | server | answer | cost |
/// | --- | --- | --- |
/// | `\\fileserver` | 15 shares | **50 ms** cold, 15 ms warm |
/// | a machine that refuses | `ERROR_ACCESS_DENIED` | 544 ms |
/// | a name that does not resolve | `ERROR_BAD_NETPATH` | 1.3 s |
/// | `\\lgs-net.com`, a DFS domain | `ERROR_BAD_NETPATH` | **22.1 s** |
///
/// That last row is the 22-second trap in another guise, which is why this is only ever called
/// from a scanner worker — the same place a slow directory read already costs nothing but its own
/// thread. See [`crate::fs::scan::scan`].
///
/// The `Err` is the Win32 code, so the listing can say *why* rather than coming up empty: an
/// empty folder and a folder that refused to answer look identical, and only one of them is
/// something the user can do anything about.
#[cfg(windows)]
pub fn shares_on(server: &str) -> Result<Vec<String>, u32> {
    // `NetShareEnum` and its `SHARE_INFO_1` are filed under the file system, and the one call that
    // frees what it allocated is filed under network management. Two features for three functions.
    use windows_sys::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
    use windows_sys::Win32::Storage::FileSystem::{
        NetShareEnum, SHARE_INFO_1, STYPE_DISKTREE, STYPE_MASK, STYPE_SPECIAL,
    };

    /// "As much as it takes", which is what makes the resume loop below a formality rather than
    /// a page-by-page walk.
    const MAX_PREFERRED_LENGTH: u32 = u32::MAX;

    let name: Vec<u16> = format!("\\\\{server}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut shares = Vec::new();
    let mut resume = 0u32;
    loop {
        let mut buffer: *mut u8 = std::ptr::null_mut();
        let (mut read, mut total) = (0u32, 0u32);
        let result = unsafe {
            NetShareEnum(
                name.as_ptr(),
                1,
                &mut buffer,
                MAX_PREFERRED_LENGTH,
                &mut read,
                &mut total,
                &mut resume,
            )
        };
        // `ERROR_MORE_DATA` still comes with a buffer full of entries, so it is a reason to go
        // round again rather than to fail.
        if result != NO_ERROR && result != ERROR_MORE_DATA {
            return Err(result);
        }
        if buffer.is_null() {
            break;
        }
        // SAFETY: on success `NetShareEnum` has written `read` `SHARE_INFO_1` records into a
        // buffer it allocated, and it is freed below whatever this loop does with them.
        let entries =
            unsafe { std::slice::from_raw_parts(buffer.cast::<SHARE_INFO_1>(), read as usize) };
        for entry in entries {
            // **`IPC$` and the admin shares are not folders.** The type word carries flags in its
            // top bits and the kind in its bottom byte, so both halves have to be asked: `IPC$`
            // comes back as `STYPE_IPC | STYPE_SPECIAL`, and `C$` as `STYPE_DISKTREE |
            // STYPE_SPECIAL`. Neither is a place anybody navigated here to find.
            if entry.shi1_type & STYPE_MASK != STYPE_DISKTREE
                || entry.shi1_type & STYPE_SPECIAL != 0
            {
                continue;
            }
            let share = from_wide_ptr(entry.shi1_netname);
            if !share.is_empty() {
                shares.push(share);
            }
        }
        unsafe { NetApiBufferFree(buffer.cast()) };
        if result == NO_ERROR {
            break;
        }
    }
    Ok(shares)
}

/// Fill in a volume's label and free space.
///
/// **May block for tens of seconds.** Never call this on the UI thread.
#[cfg(windows)]
pub fn describe(drive: &mut Drive) {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumeInformationW};

    let root = wide_root(&drive.path);

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
        // **A lettered volume is named by its label; a network location is not.** What a server
        // hands back for `\\fileserver\web` is the label of the volume the share happens to
        // live on — `Data`, for a share called `web` — and a row reading *Data on fileserver*
        // names something the user has no way to recognise. Explorer does not use it either: it
        // names a network place after the share. So the name [`list_shares`] gave this row
        // stands, and only the measurement below is filled in.
        if !text.is_empty() && !drive.letter.is_empty() {
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

/// A volume's root as a null-terminated wide string, which every volume call wants.
///
/// Built from the path rather than from a drive letter, because a network location has not got
/// one: `\\server\share` is as much a volume root as `C:\` is, and both `GetVolumeInformationW`
/// and `GetDiskFreeSpaceExW` document **a trailing backslash as required** for a UNC root. Without
/// it the call fails and the row would silently never be measured, which looks exactly like a
/// share that is down.
#[cfg(windows)]
pub(super) fn wide_root(path: &Path) -> Vec<u16> {
    let mut text: Vec<u16> = path.as_os_str().encode_wide().collect();
    if text.last() != Some(&u16::from(b'\\')) {
        text.push(u16::from(b'\\'));
    }
    text.push(0);
    text
}

/// A null-terminated wide buffer as a `String`.
#[cfg(windows)]
pub(super) fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// The same, for a pointer the system filled in — which may be null.
///
/// `NETRESOURCEW` leaves every string it has nothing to say about as a null pointer, and a
/// missing local name is precisely what [`list_shares`] is looking for, so this has to answer
/// rather than refuse.
#[cfg(windows)]
fn from_wide_ptr(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: the system guarantees a null terminator within the buffer it filled, and the
    // buffer outlives this call — it is the caller's, and nothing here holds the pointer.
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf16_lossy(slice)
}
