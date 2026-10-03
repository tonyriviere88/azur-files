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
//!
//! # Three things this machine can reach, and only one of them has a letter
//!
//! A drive letter is not what makes something a place to go, and treating it as though it were is
//! how a file manager comes to be unable to see `\\fileserver\web`. `GetLogicalDrives` answers
//! with 26 bits, and a UNC path opened without mapping a letter is not one of them. So:
//!
//! | | what it is | cost to list |
//! | --- | --- | --- |
//! | [`list_letters`] | the volumes with a letter | microseconds, no I/O |
//! | [`list_servers`] | the machines to browse | 0 ms — the same local table |
//! | [`list_shares`] | a connection no machine above reaches | 0 ms — likewise |
//!
//! The middle one is what makes a share *discoverable*: `\\fileserver` offers fourteen and one
//! of them happened to be connected, so a panel built from connections alone showed one row and
//! hid thirteen. [`shares_on`] is what asks the machine, it is the only call here that goes to
//! the network, and [`server_dir`] is the listing it becomes — see [`super::scan::scan`], which
//! is the one place either is called from.
//!
//! [`list_shares`] is therefore a short list and usually empty: a connection reached by opening
//! its machine needs no row of its own. What is left is the one nothing else can show — a DFS
//! path, whose first component is a domain and not a server.
//!
//! **Both mean connections that currently work.** The connection table holds entries whose session
//! is long gone — a sign-in leaves a `\\fileserver\IPC$` behind, and it outlives what it was for by
//! weeks — and the enumeration that lists them cannot tell those from live ones. So a second local
//! call supplies the status and the dead ones are dropped before either list is built; see
//! `win::unavailable`, which is where that costs its 0.2 ms and why it is not the letters' problem.
//!
//! **A dropped connection is not the end of that story, though.** It says the *connection* is gone;
//! it says very little about the machine, which may well be answering — what dropped could have been
//! a sleep, a VPN, or Windows pruning an idle deviceless connection out from under a server that
//! never moved. So the machines those dead entries name are collected by [`down_servers`] and asked,
//! one detached thread each, by [`reachable`]: a bounded socket to the SMB port, milliseconds for a
//! machine that is there. The ones that answer come back as *found* rows rather than connected ones
//! — see [`crate::loader::Volumes::found`] — which is the honest ink for a machine that is there
//! and is not mounted. None of this is on the startup path's critical section: the listing is 0 ms
//! and the asking happens on threads nothing waits for.
//!
//! Volumes and connections both come back as a [`Drive`], because the rest of the program has no
//! reason to care: a network location has a name, a capacity, an icon and somewhere it goes,
//! exactly as `C:` does. The one place the difference shows is [`Drive::display_name`], since a
//! row with no letter cannot be named after one. A *machine* is not a `Drive` at all — it has no
//! capacity and nothing to describe, so it is a path and nothing else.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(windows)]
#[path = "../windows/drives.rs"]
mod win;
#[cfg(windows)]
pub use win::{
    connect, describe, down_servers, list_letters, list_servers, list_shares, shares_on,
};

/// Machines the network has announced, which is a different question from what is mounted — and
/// the only one here that is asked on request rather than answered from a local table. See
/// [`discover::machines`].
#[cfg(windows)]
#[path = "../windows/discover.rs"]
pub mod discover;

use super::dir::{display_name, Dir, DirBuilder, FLAG_DIR};

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

/// One mounted volume: a lettered drive, or a network location that has no letter.
#[derive(Clone, Debug)]
pub struct Drive {
    /// `C:\` or `\\fileserver\web` — where clicking it navigates, and the row's
    /// identity: the key descriptions are merged on, and the one thing both kinds of
    /// volume are guaranteed to have.
    pub path: PathBuf,
    /// `C:`, and **empty for a network location with no letter**. See
    /// [`Drive::display_name`], which is where that difference is answered for.
    pub letter: String,
    /// The volume label, or the drive kind's default name until one is known. For a
    /// network location it is the share — see [`win::describe`], which leaves it alone.
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

    /// The name a row shows, in the sidebar and in the "This PC" listing both.
    ///
    /// `Windows (C:)` for a lettered volume: the label and the letter, which is what Explorer
    /// puts on a drive and the reason a row says which machine it is on without being told.
    ///
    /// A network location has no letter to put there, so it is named the way Windows has always
    /// named a network place — **`web on fileserver`**, the share and the machine it is on.
    /// The alternative was the bare UNC path, which puts the least distinguishing part of it
    /// (`\\`) at the front, where the eye scanning a column of names arrives first.
    pub fn display_name(&self) -> String {
        if !self.letter.is_empty() {
            return format!("{} ({})", self.label, self.letter);
        }
        match split_unc(&self.path) {
            Some((host, _)) => format!("{} on {host}", self.label),
            // Not a UNC path and not a letter either. Nothing is known about it beyond where it
            // goes, so that is what the row says.
            None => self.path.to_string_lossy().into_owned(),
        }
    }
}

/// The machine a path names, when it names a machine and nothing on it: `\\fileserver`.
///
/// The level between This PC and a share, and a real one — [`shares_on`] lists what is on it, and
/// [`super::parent_of`] walks through it. `None` for a share, a folder inside one, or a drive
/// letter.
///
/// **Backslashes only**, unlike [`split_unc`]. Two reasons, and the second is the important one:
/// every path that comes in from outside has been through [`super::normalize`], which rewrites the
/// slashes at the door; and on a platform where `//foo` is an ordinary absolute path, a
/// forward-slash form here would quietly turn a directory into a machine.
pub fn unc_server(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let inner = text.strip_prefix("\\\\")?.trim_end_matches('\\');
    (!inner.is_empty() && !inner.contains('\\')).then(|| inner.to_owned())
}

/// The machine a connection points at, if it is one this program can offer to browse.
///
/// The whole of [`list_servers`]' judgement, out here where it can be tested: it reads the
/// machine's real connection table, so the rule it applies to each entry had nowhere to be
/// checked — and it was wrong, in the one case a sign-in produces.
///
/// | connection | machine | why |
/// | --- | --- | --- |
/// | `\\fileserver\web` | `fileserver` | a share on it, which is what opening a UNC path makes |
/// | `\\fileserver` | `fileserver` | the machine itself, which is what signing in to one makes |
/// | `\\lgs-net.com\alyo\alyodata\Common` | none | a DFS namespace: the first component is a domain |
/// | `C:\` | none | not a network path |
///
/// The second row is the one that was missing. `split_unc` answers `None` for a bare machine by
/// design — a machine is not a place on itself — so a rule built on it alone dropped the very
/// machine somebody had just signed in to, and the row did not come back.
///
/// The DFS exclusion is a measurement, not tidiness: [`shares_on`] against a domain name takes
/// **22.1 seconds** to answer "the network path was not found", and a row that spends that long to
/// say nothing is worse than no row.
pub fn machine_of(remote: &Path) -> Option<String> {
    if let Some(host) = unc_server(remote) {
        return Some(host);
    }
    match split_unc(remote) {
        Some((host, _)) if !host.contains(['\\', '/']) => Some(host),
        _ => None,
    }
}

/// What to sign in to, for a UNC path that would not open.
///
/// A credential prompt is about a **connection**, and the thing a connection is made to is a
/// machine or a share on it — never a folder inside one. So `\\fileserver\web\owncloud\apps`
/// reduces to `\\fileserver\web`, which is the tree the server authenticates, and after signing
/// in to that the folder deep inside it opens with no further asking.
///
/// A bare `\\server` stays as it is: a session can be established to a machine without naming a
/// share, which is exactly what has to happen before it will say what shares it has.
///
/// `None` for anything that is not a UNC path, because there is nobody to sign in to on `C:`.
pub fn connect_target(path: &Path) -> Option<PathBuf> {
    let text = path.to_string_lossy();
    let inner = text.strip_prefix("\\\\")?.trim_end_matches('\\');
    if inner.is_empty() {
        return None;
    }
    let mut parts = inner.split('\\').filter(|part| !part.is_empty());
    let server = parts.next()?;
    Some(match parts.next() {
        Some(share) => PathBuf::from(format!("\\\\{server}\\{share}")),
        None => PathBuf::from(format!("\\\\{server}")),
    })
}

/// A UNC path split into the machine it names and the share on it: `\\fileserver\web` is
/// `("fileserver", "web")`.
///
/// The machine part keeps **everything** between the leading `\\` and the last component, so a
/// DFS path like `\\lgs-net.com\alyo\alyodata\Common` comes back as
/// `("lgs-net.com\alyo\alyodata", "Common")` rather than quietly losing its middle and claiming
/// to be a share on `lgs-net.com`.
///
/// `None` for anything that is not a UNC path with something after the machine — a drive letter,
/// a relative path, or `\\server` on its own, which names a machine and not a place on it.
pub fn split_unc(path: &Path) -> Option<(String, String)> {
    let text = path.to_string_lossy();
    let inner = text.strip_prefix("\\\\").or_else(|| text.strip_prefix("//"))?;
    let (host, share) = inner.trim_end_matches(['\\', '/']).rsplit_once(['\\', '/'])?;
    (!host.is_empty() && !share.is_empty()).then(|| (host.to_owned(), share.to_owned()))
}

/// Whether `list` already names `path`, compared the way a UNC path has to be.
///
/// **Case-insensitively**, because `\\FileServer` and `\\fileserver` are one machine — and a panel
/// showing both would be naming one place twice, which is the fault this guards against everywhere
/// it is used. Spelled out here once because half a dozen places were each spelling it out for
/// themselves, and a comparison rule that is copied is a comparison rule that drifts.
///
/// Whole paths, never [`Path::starts_with`]: that matches by component, and `\\fileserver` is not a
/// component of `\\fileserver\web` — the server and the share arrive welded into one UNC prefix.
pub fn holds(list: &[PathBuf], path: &Path) -> bool {
    list.iter()
        .any(|had| had.as_os_str().eq_ignore_ascii_case(path.as_os_str()))
}

// ---------------------------------------------------------------------------
// Is that machine still there?
// ---------------------------------------------------------------------------

/// Which machines are worth asking about, out of the connections that are down and the ones that
/// are up.
///
/// The whole of [`down_servers`]' judgement, out here where synthetic input can reach it: that
/// function reads this machine's two connection tables, and the state this rule most needs checking
/// against — a dead connection to a machine that is nevertheless answering — is one a machine is
/// only sometimes in. Same reason [`machine_of`] is not buried in [`list_servers`].
///
/// | a connection that is down | asked about | why |
/// | --- | --- | --- |
/// | `\\fileserver\web` | `fileserver` | the machine may be there even though the connection is not |
/// | `\\fileserver` | `fileserver` | a sign-in that lapsed, which is the same question |
/// | `\\fileserver\web`, with `\\fileserver` **up** | nothing | one down and one up is a machine that is *connected* |
/// | `\\lgs-net.com\alyo\alyodata\Common` | nothing | a DFS namespace: the first component is a domain |
/// | three dead shares on one machine | that machine, once | one probe, not three |
pub fn candidates(dead: &[PathBuf], live: &[PathBuf]) -> Vec<PathBuf> {
    let mut asking: Vec<PathBuf> = Vec::new();
    for connection in dead {
        // The same rule [`list_servers`] applies, so the two cannot disagree about what a machine
        // is — and `None` here is a DFS path, whose first component is a domain. Probing that would
        // report a domain controller answering on 445 and say nothing whatever about whether the
        // namespace resolves.
        let Some(host) = machine_of(connection) else {
            continue;
        };
        let path = PathBuf::from(format!("\\\\{host}"));
        if !holds(live, &path) && !holds(&asking, &path) {
            asking.push(path);
        }
    }
    asking
}

/// How long [`reachable`] gets, across every address one name resolves to.
///
/// **A second, where 300 ms looks generous and is not.** The budget is not there to catch the fast
/// case — a live machine on this LAN answers in **3 ms** warm and 53 ms cold — it is there to
/// cover a *slow name*. A machine with no DNS record is found by LLMNR or NBNS instead, which is a
/// second or two, and a 300 ms budget calls that machine unreachable while it is sitting there
/// answering. Nothing waits on this, so a second costs one detached thread and no frames.
const REACH_BUDGET: Duration = Duration::from_millis(1000);

/// The port an SMB server listens on, which is the whole of the question [`reachable`] asks.
///
/// **445 only.** Every Windows server since 2000 and every NAS worth naming listens there. The
/// NetBIOS-era 139 would add a second to every machine that has genuinely gone away, to catch one
/// old enough to have neither.
const SMB_PORT: u16 = 445;

/// Whether a machine is answering on the SMB port right now.
///
/// # Why this is cheap where a browse is not
///
/// Finding machines costs **14.3 seconds** (see [`discover::machines`]) and this costs
/// milliseconds, and the difference is not the network — it is the shape of the question. A browse
/// asks *who is out there*, into a multicast group where no reply means "that was everyone", so the
/// only way to finish is to wait out a fixed window: measured here, 14 334 ms of waiting around
/// 12 ms of work. Asking whether a **named** machine is there is closed, so it costs one round trip
/// on a deadline of our choosing. Measured on this machine:
///
/// | target | resolve | answer |
/// | --- | --- | --- |
/// | a live machine on this LAN | 23 ms | **53 ms** cold, **3 ms** warm |
/// | `lgs-net.com`, off the VPN | 0 ms | closed, at the budget |
/// | a machine that is switched off | 2.7 s, fails | not there |
/// | a name with no record | 1.3 s, fails | not there |
/// | an unrouted address, **no deadline** | — | **21.1 s** |
///
/// That last row is the whole reason the budget exists: 21 seconds is the stack's SYN retry
/// schedule, not a floor, and every failure above returns within 15 ms of whatever deadline it is
/// given.
///
/// # What it costs nothing to ask
///
/// **This never touches the redirector.** A socket to 445 does not go through `mup.sys` or
/// `mrxsmb`, so it cannot trip the 22-second reconnect that a volume query walks into — see the
/// note at the top of this module. It also sends no SMB negotiate, so there is no session, no
/// authentication, and nothing that could put a failed logon against a domain account or count
/// towards locking one out. It is a handshake and a close.
///
/// # What the answer means
///
/// That something is listening on the SMB port at that name, and no more: not that a particular
/// share exists, not that these credentials open it, and not that a DFS referral resolves. That is
/// the right confidence for putting a row back — clicking it is what does the real work, and the
/// credential path is already there. See [`crate::app::connect`].
pub fn reachable(host: &str) -> bool {
    reachable_on(host, SMB_PORT)
}

/// The port is a parameter so the true path can be tested against a listener on this machine,
/// which is a test that needs no network and no other computer. Nothing else passes anything but
/// [`SMB_PORT`].
fn reachable_on(host: &str, port: u16) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};

    // **Unbounded, and deliberately.** `ToSocketAddrs` takes no deadline and there is no resolver
    // in `std` that does — but the system one has its own, measured at 1.3 s for a name with no
    // record and 2.7 s for one whose lookup fails outright. A name that will not resolve is a
    // machine that is not there, so reaching that answer slowly is still reaching the right one.
    let Ok(addresses) = (host, port).to_socket_addrs() else {
        return false;
    };

    // The clock starts *after* the name, so a slow lookup cannot eat the connect's budget and turn
    // a machine that is there into one that is not.
    let deadline = Instant::now() + REACH_BUDGET;
    for address in addresses {
        // **Every address the name gave, not the first of them.** The NAS here resolves to a
        // link-local IPv6 *before* its IPv4, so a probe that tried only the head of that list
        // would report a machine down while it was answering. One deadline across all of them,
        // rather than one each, so a name with six addresses cannot cost six budgets.
        let left = deadline.saturating_duration_since(Instant::now());
        // `connect_timeout` rejects a zero duration rather than reading it as "already too late",
        // so the budget running out has to end the loop rather than fall through to a call that
        // would fail for the wrong reason.
        if left.is_zero() {
            break;
        }
        if TcpStream::connect_timeout(&address, left).is_ok() {
            return true;
        }
    }
    false
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

/// Network locations with no mount point of their own, of which a POSIX machine has none: a
/// remote filesystem is mounted into the tree there, so it is already somewhere you can go and
/// needs no row of its own to be reachable.
#[cfg(not(windows))]
pub fn list_shares() -> Vec<Drive> {
    Vec::new()
}

/// For the same reason, no machines to browse: there is no level between the filesystem root and
/// a directory.
#[cfg(not(windows))]
pub fn list_servers() -> Vec<PathBuf> {
    Vec::new()
}

/// And nothing whose connection could have dropped, since there are no connections to enumerate.
#[cfg(not(windows))]
pub fn down_servers() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn shares_on(_server: &str) -> Result<Vec<String>, u32> {
    Ok(Vec::new())
}

/// And nothing to sign in to. A POSIX machine mounts a remote filesystem before a file manager
/// ever sees it, so credentials were settled by whoever mounted it.
#[cfg(not(windows))]
pub fn connect(_target: &Path, _owner: crate::shell::Owner) -> Result<(), u32> {
    Err(0)
}

#[cfg(not(windows))]
pub fn describe(drive: &mut Drive) {
    drive.described = true;
    remember(drive.clone());
}

// ---------------------------------------------------------------------------
// What has been learned so far
// ---------------------------------------------------------------------------

/// Descriptions already paid for, keyed by path.
///
/// A process-wide cache rather than state threaded through the application, because
/// the set of volumes on the machine *is* process-wide — and because [`this_pc`] runs
/// on a scanner worker that has no way to reach the sidebar's copy. Nothing here
/// blocks: it is only ever read, or written by a probe that has already paid the
/// cost.
///
/// By path and not by letter, which is the one key both kinds of volume have: every network
/// location [`list_shares`] finds has an empty letter, so a letter-keyed cache would merge all
/// of them onto each other and describe the first one twenty times.
static KNOWN: Mutex<Vec<Drive>> = Mutex::new(Vec::new());

/// Record what a probe found.
pub fn remember(drive: Drive) {
    let Ok(mut known) = KNOWN.lock() else { return };
    match known.iter_mut().find(|d| d.path == drive.path) {
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

/// Every volume on the machine, with whatever has already been described merged in.
///
/// The list the sidebar and [`this_pc`] both show, in the order they show it: the lettered
/// volumes, then the network locations that have no letter. Never blocks — both halves answer
/// from the local mount table, and the descriptions are whatever the probes have finished so far.
fn merged() -> Vec<Drive> {
    let described = known();
    list_letters()
        .into_iter()
        .chain(list_shares())
        .map(|drive| {
            described
                .iter()
                .find(|d| d.path == drive.path)
                .cloned()
                .unwrap_or(drive)
        })
        .collect()
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
///
/// **The network locations are in here as well as in the sidebar**, and that is not a second
/// placement of the same rows for the sake of it: [`super::parent_of`] sends `\\fileserver\web`
/// to This PC, because a UNC root's parent is the machine exactly as `C:\`'s is. So This PC is
/// what Up from a network location arrives at, and a listing that did not hold the row you came
/// out of would be a listing that had lost it. Same reason the breadcrumb's leading chevron —
/// which lists This PC's children — can offer them.
pub fn this_pc(started: Instant) -> Dir {
    let drives = merged();
    let servers = list_servers();
    let mut builder = DirBuilder::new(PathBuf::new());
    builder.reserve(drives.len() + servers.len());
    for drive in &drives {
        builder.push_link(
            &drive.display_name(),
            drive.path.clone(),
            drive.total,
            FLAG_DIR,
        );
    }
    // The machines, after the volumes. Here for the same reason the network locations are — Up
    // from `\\fileserver` arrives at This PC, so This PC has to hold the row it came out of.
    for server in servers {
        builder.push_link(&display_name(&server), server, 0, FLAG_DIR);
    }
    builder.finish(started.elapsed().as_micros() as u64)
}

/// What is on a machine, as a [`Dir`] — so `\\fileserver` is a listing like any other.
///
/// The answer to the question a bare server path used to fail with `ERROR_BAD_PATHNAME`: a server
/// is not a directory, so [`super::scan`] cannot read one, and the shares on it are reachable only
/// by asking the machine. See [`shares_on`], which is where the cost is and why this is only ever
/// called on a worker.
///
/// A server that refuses or does not answer comes back as a [`Dir::failed`] carrying the reason,
/// not as an empty folder: those two look identical on screen and only one of them is something
/// the user can act on.
pub fn server_dir(server: &str, path: &Path, started: Instant) -> Dir {
    match shares_on(server) {
        Ok(shares) => {
            let mut builder = DirBuilder::new(path);
            builder.reserve(shares.len());
            for share in &shares {
                // The target is built by joining, which for `\\fileserver` and `web` gives
                // `\\fileserver\web` — the share root, and a path every other part of this
                // program already understands.
                builder.push_link(share, path.join(share), 0, FLAG_DIR);
            }
            builder.finish(started.elapsed().as_micros() as u64)
        }
        // A machine that will not say what it offers until it is told who is asking is the
        // commonest failure here, and the one a sign-in fixes. See
        // [`super::scan::wants_credentials`].
        Err(code) => Dir::failed(path, super::scan::error_text(code))
            .wanting_credentials(super::scan::wants_credentials(code, path)),
    }
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

    /// The other half of the same claim, and the reason [`list_shares`] is allowed next to
    /// [`list_letters`] on the startup path at all.
    ///
    /// Measured on a machine where one of the connections it returns is to an unreachable
    /// server: `WNetOpenEnumW` reads the redirector's connection table and does not go looking,
    /// so a dead share costs nothing here. If that ever stops being true this is the test that
    /// says so, because the symptom in the window would be a startup that hangs.
    #[test]
    fn listing_shares_does_no_io_either() {
        let started = Instant::now();
        let shares = list_shares();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "list_shares took {elapsed:?} -- it is on the startup path beside list_letters, \
             and something in it has started waiting on the network"
        );
        for share in &shares {
            assert!(
                share.letter.is_empty(),
                "a volume with a letter belongs to list_letters, not here: {share:?}"
            );
            assert_eq!(share.kind, DriveKind::Network);
            assert!(!share.described);
            assert_eq!(share.total, 0, "an undescribed volume has no measurement");
            assert!(
                split_unc(&share.path).is_some(),
                "a network location has to be a UNC path with a share on it: {share:?}"
            );
        }
    }

    /// The status table is on the startup path too, because [`list_shares`] and [`list_servers`]
    /// both wait for it — so it gets the same bound they do, and for the same reason.
    ///
    /// Measured at 0.2 ms warm and 3.6 ms for the first call in a process, which is `netapi32.dll`
    /// being loaded. If this ever fails, the workstation service has started going to the network to
    /// answer and the symptom in the window would be a startup that hangs.
    #[test]
    #[cfg(windows)]
    fn the_status_table_does_no_io_either() {
        let started = Instant::now();
        let down = win::unavailable();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "unavailable() took {elapsed:?} -- it runs before every network row in the panel, \
             and something in it has started waiting on the network"
        );
        for dead in &down {
            assert!(
                dead.to_string_lossy().starts_with("\\\\"),
                "a connection is a UNC path or it is nothing: {dead:?}"
            );
        }
    }

    /// A connection Windows says is down gets no row.
    ///
    /// The whole point of the filter, checked against this machine's real connection table — so
    /// what it finds depends on the machine, and on one with nothing dead in it this passes with
    /// nothing to say. That is honest rather than weak: the commonest dead entry there is is the
    /// `\\server\IPC$` an old sign-in left behind, and it is what used to put a machine in the
    /// Network group that nothing had reached in weeks.
    ///
    /// **The connection and not its host**, for the machine rows: a machine can have one connection
    /// down and another up — a dropped share and a live one — and then the machine really is
    /// connected and its row is right. What must never survive is the dead connection itself.
    #[test]
    #[cfg(windows)]
    fn a_disconnected_connection_gets_no_row() {
        let down = win::unavailable();
        let shares = list_shares();
        let servers = list_servers();
        for dead in &down {
            assert!(
                !shares.iter().any(|share| share.path == *dead),
                "{dead:?} is disconnected and still has a share row"
            );
            // A connection to a bare `\\server` *is* the machine, so there is no live sibling it
            // could be standing in for — this one is unconditional.
            if unc_server(dead).is_some() {
                assert!(
                    !servers.iter().any(|server| server == dead),
                    "{dead:?} is disconnected and still has a machine row"
                );
            }
        }
    }

    /// The candidate list is built from two local tables and belongs on the startup path with them.
    ///
    /// It is [`win::unavailable`] and [`list_servers`] and nothing else, so it gets the same bound
    /// they do. If this ever fails, something has started asking the *network* which machines to ask
    /// — which is the probe's job, on a thread, and the symptom in the window would be a startup
    /// that hangs.
    #[test]
    #[cfg(windows)]
    fn listing_probe_candidates_does_no_io() {
        let started = Instant::now();
        let candidates = down_servers();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "down_servers took {elapsed:?} -- it runs on the startup path and something in it \
             has started waiting on the network"
        );
        for machine in &candidates {
            assert!(
                unc_server(machine).is_some(),
                "a candidate is a bare `\\\\machine` and nothing else, because that is what a \
                 socket and a found row both want: {machine:?}"
            );
        }
    }

    /// Which dead connections are worth a probe — against input, because this machine is usually
    /// not in the state that matters.
    ///
    /// The measured reason this test exists rather than leaning on the two above it: the only
    /// disconnected connection here is `H:` → `\\lgs-net.com\alyo\alyodata\Common`, and it is
    /// invisible twice over. It is a DFS namespace, so the rule excludes it by design; and it is
    /// mapped through the Windows network provider rather than the SMB redirector, so `NetUseEnum`
    /// does not carry it at all — see [`win::unavailable`], where that was already measured. So
    /// `down_servers()` is empty here whatever the rule says, and only synthetic input can show that
    /// the rule is right.
    #[test]
    fn which_dead_connections_are_worth_asking_about() {
        let paths = |list: &[&str]| list.iter().map(PathBuf::from).collect::<Vec<_>>();

        // A share on a machine, and a lapsed sign-in to the machine itself. Both name a machine that
        // may well still be there.
        assert_eq!(
            candidates(&paths(&["\\\\fileserver\\web", "\\\\other"]), &[]),
            paths(&["\\\\fileserver", "\\\\other"])
        );

        // **One connection down and another up is a machine that is connected.** Its row is already
        // in the panel in full ink, and a probe could only add a second row saying the same thing
        // more weakly.
        assert!(candidates(
            &paths(&["\\\\fileserver\\web"]),
            &paths(&["\\\\fileserver"])
        )
        .is_empty());

        // A DFS namespace contributes nothing: `lgs-net.com` is a domain, so 445 answering there
        // would be a domain controller and would say nothing about the namespace. This is the case
        // this machine is actually in.
        assert!(candidates(
            &paths(&["\\\\lgs-net.com\\alyo\\alyodata\\Common"]),
            &[]
        )
        .is_empty());

        // Three dead shares on one machine is one probe, and the case of the name is not a second
        // machine.
        assert_eq!(
            candidates(
                &paths(&[
                    "\\\\fileserver\\web",
                    "\\\\FILESERVER\\photos",
                    "\\\\fileserver"
                ]),
                &[]
            ),
            paths(&["\\\\fileserver"])
        );
        // Nor is it a second machine when the live list is the one spelling it differently.
        assert!(candidates(
            &paths(&["\\\\fileserver\\web"]),
            &paths(&["\\\\FILESERVER"])
        )
        .is_empty());

        // And nothing at all out of nothing, which is the state of a machine with no network on it.
        assert!(candidates(&[], &[]).is_empty());
    }

    /// A machine that is already connected is never probed.
    ///
    /// One connection down and another up is a machine that *is* connected: its row is already
    /// there in full ink, and a probe could only add a second row saying the same thing more
    /// weakly. Checked against this machine's real connection table, so on one with nothing dead
    /// in it this passes with nothing to say — honest rather than weak, for the reason on
    /// [`a_disconnected_connection_gets_no_row`].
    #[test]
    #[cfg(windows)]
    fn a_connected_machine_is_not_a_candidate() {
        let servers = list_servers();
        for machine in down_servers() {
            assert!(
                !holds(&servers, &machine),
                "{machine:?} is connected and is being probed as though it were not"
            );
            // A DFS namespace contributes no candidate: probing the domain would report a domain
            // controller and say nothing about whether the namespace resolves.
            assert!(
                machine_of(&machine).is_some(),
                "{machine:?} is not a machine anything should be asked about"
            );
        }
    }

    /// The true path, against a listener on this machine — so it needs no network and no second
    /// computer, and it cannot go stale when a NAS is switched off.
    #[test]
    fn a_listening_port_is_reachable() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("loopback is always bindable");
        let port = listener.local_addr().expect("a bound listener has an address").port();
        let started = Instant::now();
        assert!(
            reachable_on("127.0.0.1", port),
            "a socket that is listening right here did not answer"
        );
        // The point of the whole design: a machine that is there answers in milliseconds, and the
        // budget is never spent on the case that works.
        assert!(
            started.elapsed() < std::time::Duration::from_millis(150),
            "a live host took {:?} -- the fast path is what makes this affordable at startup",
            started.elapsed()
        );
    }

    /// A closed port on a host that is definitely up is not reachable, and says so at once.
    ///
    /// The other half of the true path: the probe is asking about *SMB*, not about whether the
    /// address exists. A machine answering `RST` is a machine with nothing listening, and the stack
    /// reports that immediately rather than waiting out the budget.
    #[test]
    fn a_closed_port_is_not_reachable() {
        // Bound and dropped, so the port was real a moment ago and is refusing now — which is more
        // reliably closed than any number picked out of the air.
        let port = {
            let listener =
                std::net::TcpListener::bind("127.0.0.1:0").expect("loopback is always bindable");
            listener.local_addr().expect("a bound listener has an address").port()
        };
        assert!(!reachable_on("127.0.0.1", port));
    }

    /// **The 21-second hang, which is what the budget is for.**
    ///
    /// `192.0.2.0/24` is reserved for documentation (RFC 5737) and is routed to the gateway and
    /// dropped, which is the address class that takes the stack's full SYN retry schedule — measured
    /// at **21.1 s** with no deadline. This is the regression test for that: five seconds is far
    /// above the one-second budget and far below the hang, so it fails loudly if the deadline is
    /// ever lost and does not flake on a loaded machine.
    ///
    /// One SYN to a black hole, and nothing else leaves this machine.
    #[test]
    fn an_unroutable_address_gives_up_on_the_budget() {
        let started = Instant::now();
        assert!(!reachable_on("192.0.2.1", SMB_PORT));
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "an unrouted address took {elapsed:?} -- the connect deadline is gone and the panel \
             is back to waiting 21 seconds per machine"
        );
    }

    /// A name that cannot resolve is not reachable, and the resolver's own timeout is the bound.
    ///
    /// `.invalid` is reserved by RFC 2606 and never resolves. A DNS server that answers `NXDOMAIN`
    /// with a search page instead would give an address whose port 445 is shut, so the verdict is
    /// the same either way — which is why this asserts the answer and not the mechanism.
    #[test]
    fn a_name_that_does_not_resolve_is_not_reachable() {
        let started = Instant::now();
        assert!(!reachable_on("yafe-no-such-machine.invalid", SMB_PORT));
        // Measured at 1.3-2.7 s for a name the resolver gives up on. Resolution is the one part
        // with no deadline of ours, so this bound is loose on purpose: it is here to catch a hang,
        // not to hold the resolver to a number.
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "an unresolvable name took {elapsed:?} -- something is waiting far past the \
             resolver's own timeout"
        );
    }

    /// What the probe actually costs on this machine, and against what.
    ///
    /// `cargo test probe_reachability -- --ignored --nocapture`. Ignored because it reaches the
    /// network and its answers are this machine's rather than anything a suite can assert — and
    /// because the state it is most interesting about, a dropped connection to a machine that is
    /// still there, is one a machine is only sometimes in.
    ///
    /// Prints the dead connections, what [`down_servers`] makes of them, and a timed [`reachable`]
    /// for every machine involved — connected ones included, since those are the only place a
    /// *true* answer against a real server can be seen on a machine with nothing dropped.
    #[test]
    #[ignore = "reaches the network; run it deliberately"]
    #[cfg(windows)]
    fn probe_reachability() {
        let timed = |host: &str| {
            let started = Instant::now();
            let answer = reachable(host);
            println!(
                "  {host:<40} {:>8.1} ms  {}",
                started.elapsed().as_secs_f64() * 1000.0,
                if answer { "ANSWERS" } else { "no" }
            );
        };

        let candidates = down_servers();
        println!("connections Windows reports as down:");
        for dead in win::unavailable() {
            println!("  {}", dead.display());
        }
        println!("machines worth asking about (down_servers):");
        for machine in &candidates {
            println!("  {}", machine.display());
        }

        println!("probed:");
        // The candidates first, which is what the stage in `Volumes::confirm` asks.
        for machine in candidates.iter().chain(list_servers().iter()) {
            if let Some(host) = unc_server(machine) {
                timed(&host);
            }
        }
        // And the two references that say whether the numbers above mean anything: a name that
        // cannot resolve, and an address routed into a black hole. Both must come back inside the
        // budget rather than in twenty-one seconds.
        timed("yafe-no-such-machine.invalid");
        timed("192.0.2.9");
    }

    #[test]
    fn this_pc_never_blocks_either() {
        let started = Instant::now();
        let dir = this_pc(Instant::now());
        assert!(
            started.elapsed() < std::time::Duration::from_millis(150),
            "This PC is a listing like any other and cannot wait on a share"
        );
        // The network locations and the machines as well as the letters: Up from
        // `\\server\share` arrives at the machine and Up again arrives here, so both rows have to
        // be in the listing. See [`this_pc`].
        assert_eq!(
            dir.len(),
            list_letters().len() + list_shares().len() + list_servers().len()
        );
        for i in 0..dir.len() {
            assert!(dir.entries[i].is_dir());
            assert!(
                !dir.target(i).as_os_str().is_empty(),
                "every drive row has to lead somewhere"
            );
        }
    }

    #[test]
    fn descriptions_merge_by_path() {
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
        assert_eq!(z.len(), 1, "a volume is described once, not appended to");
        assert_eq!(z[0].label, "Second");
        assert_eq!(z[0].used_fraction(), Some(0.75));
    }

    /// **By path and not by letter**, which a letter-keyed cache gets catastrophically wrong:
    /// every network location has an empty letter, so all of them would merge onto each other
    /// and the panel would show one share's measurement under every share's name.
    #[test]
    fn two_letterless_locations_are_two_entries() {
        let sample = |unc: &str, total| Drive {
            path: PathBuf::from(unc),
            letter: String::new(),
            label: unc.rsplit('\\').next().unwrap_or(unc).to_owned(),
            kind: DriveKind::Network,
            total,
            free: total / 4,
            described: true,
        };
        let (one, two) = ("\\\\nowhere-a\\one", "\\\\nowhere-b\\two");
        remember(sample(one, 100));
        remember(sample(two, 200));

        // Compared as whole paths, not with `Path::starts_with`: that matches by component, and
        // `\\nowhere-a` is not a component of `\\nowhere-a\one` — the server and the share
        // together are the one UNC prefix.
        let known = known();
        let found: Vec<&Drive> = known
            .iter()
            .filter(|d| d.path == Path::new(one) || d.path == Path::new(two))
            .collect();
        assert_eq!(found.len(), 2, "one entry each, keyed on the path: {known:?}");
        assert_eq!(found[0].total, 100, "and each keeps its own measurement");
        assert_eq!(found[1].total, 200);
    }

    /// What the rows are called, which is the only place the two kinds of volume differ.
    #[test]
    fn a_row_is_named_after_its_letter_or_its_machine() {
        let drive = Drive {
            path: PathBuf::from("C:\\"),
            letter: "C:".to_owned(),
            label: "Windows".to_owned(),
            kind: DriveKind::Fixed,
            total: 0,
            free: 0,
            described: false,
        };
        assert_eq!(drive.display_name(), "Windows (C:)");

        let share = Drive {
            path: PathBuf::from("\\\\fileserver\\web"),
            letter: String::new(),
            label: "web".to_owned(),
            kind: DriveKind::Network,
            total: 0,
            free: 0,
            described: false,
        };
        assert_eq!(share.display_name(), "web on fileserver");
    }

    /// Which connections put a machine in the Network group — including the shape a *sign-in*
    /// leaves behind, which is the one this got wrong.
    ///
    /// Signing in to a machine makes a connection whose remote name is the bare `\\server`, and a
    /// rule built on [`split_unc`] alone answers `None` for that: a machine is not a place on
    /// itself. So the machine somebody had just authenticated to was dropped, its row did not come
    /// back, and the sign-in looked as though it had failed.
    #[test]
    fn a_machine_is_recognised_however_the_connection_names_it() {
        let machine = |text: &str| machine_of(Path::new(text));
        assert_eq!(
            machine("\\\\fileserver\\web"),
            Some("fileserver".to_owned()),
            "a share on it"
        );
        assert_eq!(
            machine("\\\\fileserver"),
            Some("fileserver".to_owned()),
            "and the machine itself, which is what a sign-in leaves behind"
        );
        // A DFS namespace: the first component is a domain, and asking one for its shares takes
        // 22 seconds to fail. See `list_servers`.
        assert_eq!(machine("\\\\lgs-net.com\\alyo\\alyodata\\Common"), None);
        // Not a network path at all.
        assert_eq!(machine("C:\\"), None);
        assert_eq!(machine("\\\\"), None);
    }

    /// A UNC path is split at its *last* component, so a DFS path keeps its middle.
    #[test]
    fn splitting_a_unc_path_keeps_everything_before_the_share() {
        let split = |text: &str| split_unc(Path::new(text));
        assert_eq!(
            split("\\\\fileserver\\web"),
            Some(("fileserver".to_owned(), "web".to_owned()))
        );
        // The case that says why the machine part is not just the first component: the row would
        // otherwise claim to be `Common` on `lgs-net.com`, which is a different place.
        assert_eq!(
            split("\\\\lgs-net.com\\alyo\\alyodata\\Common"),
            Some(("lgs-net.com\\alyo\\alyodata".to_owned(), "Common".to_owned()))
        );
        // A trailing separator is not a component.
        assert_eq!(
            split("\\\\fileserver\\web\\"),
            Some(("fileserver".to_owned(), "web".to_owned()))
        );
        // Either slash, since a path typed into the bar may arrive with the wrong one — see
        // [`super::normalize`], which is what rewrites it, and this is what does not depend on
        // having been through it.
        assert_eq!(
            split("//fileserver/web"),
            Some(("fileserver".to_owned(), "web".to_owned()))
        );
        // And the things that are not a share on a machine.
        assert_eq!(split("C:\\Users"), None);
        assert_eq!(split("\\\\fileserver"), None, "a machine is not a place on it");
        assert_eq!(split("\\\\"), None);
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
