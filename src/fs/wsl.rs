//! The Linux distributions installed under WSL, for the sidebar's WSL group.
//!
//! **Read out of the registry, never asked of WSL.** Each distribution registered for this user is
//! a subkey of `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss` carrying its name, its state
//! and the folder its disk lives in — which is everything a row needs, for **0.1 ms** and no
//! process (96 µs with one distribution installed; see `measure_reading_the_installed_distros`).
//! The two alternatives both cost what a sidebar must not:
//!
//! - **`wsl.exe --list`** is a process launch per question, and it talks to the WSL service, which
//!   is not always running.
//! - **Listing `\\wsl.localhost`** goes through the Plan 9 redirector, and touching a distribution's
//!   share is what *boots* it: a VM, a kernel and a few hundred megabytes, to draw a row.
//!
//! So the absence of the key is the whole of "is WSL here": no key, or a key with no distribution
//! in it, and the group is not drawn at all.
//!
//! **A row is never asked about until it is clicked.** Its icon is the `shortcut.ico` WSL writes
//! beside the distribution's disk — a local file — and never the shell's answer for the
//! `\\wsl.localhost` path, which would boot the distribution exactly as listing it would. Opening the
//! row does boot it, which is the same thing Explorer does and the reason the row exists.

use std::path::{Path, PathBuf};

#[cfg(windows)]
#[path = "../windows/wsl.rs"]
mod win;

/// The host every distribution is shared under, on Windows 11 and on 10 from 21H2.
pub const HOST: &str = "wsl.localhost";

/// One installed distribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Distro {
    /// What `wsl -l` calls it, and what its share is named: `Ubuntu-24.04`.
    pub name: String,
    /// `\\wsl.localhost\<name>`, the root of its filesystem.
    pub path: PathBuf,
    /// A local icon for the row, when WSL left one — see the module header for why it is never
    /// the shell's icon for [`Self::path`].
    pub icon: Option<PathBuf>,
}

impl Distro {
    /// A distribution as the registry describes it.
    ///
    /// `base` is its `BasePath`, the folder its `ext4.vhdx` lives in. A distribution installed by a
    /// current WSL keeps a `shortcut.ico` there — the logo its Start menu entry wears — and one
    /// installed from the Store before that has none, so the row falls back to its painted glyph.
    /// Not checked for here: [`crate::shell::icons::Icons::place`] answers nothing for a file that
    /// is not there, and the painted glyph is what stands in for nothing.
    pub fn new(name: &str, base: Option<&Path>) -> Self {
        Self {
            name: name.to_owned(),
            path: PathBuf::from(format!("\\\\{HOST}\\{name}")),
            icon: base.map(|base| base.join("shortcut.ico")),
        }
    }
}

/// The distributions installed for this user, by name, as Explorer's Linux node lists them.
///
/// Empty when WSL is not installed, which is what hides the group.
pub fn distros() -> Vec<Distro> {
    #[cfg(windows)]
    let mut found = win::registered();
    #[cfg(not(windows))]
    let mut found: Vec<Distro> = Vec::new();
    // Case-insensitively, the way the share names compare.
    found.sort_by_key(|distro| distro.name.to_lowercase());
    found
}

/// Whether a path is on a WSL share: `\\wsl.localhost\…`, or the older `\\wsl$\…` that still
/// resolves.
///
/// What keeps these out of the Network group. The Plan 9 provider may report a distribution it has
/// open as a network connection, and a machine row named `wsl.localhost` beside the file servers —
/// or a share row for a distribution — would be the same place twice, under the wrong heading.
pub fn is_wsl(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let Some(inner) = text.strip_prefix("\\\\") else {
        return false;
    };
    let host = inner.split('\\').next().unwrap_or_default();
    host.eq_ignore_ascii_case(HOST) || host.eq_ignore_ascii_case("wsl$")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_distro_is_shared_under_wsl_localhost_with_its_icon_beside_its_disk() {
        let distro = Distro::new("Ubuntu-24.04", Some(Path::new("C:\\wsl\\{dd09}")));
        assert_eq!(distro.path, PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04"));
        assert_eq!(
            distro.icon,
            Some(PathBuf::from("C:\\wsl\\{dd09}\\shortcut.ico"))
        );
        // A Store install with no base path recorded has nothing to point at.
        assert_eq!(Distro::new("Debian", None).icon, None);
    }

    /// What the check costs on this machine, and what it finds. A measurement, so ignored: it
    /// reads the real registry, which is read-only but is still this user's.
    #[test]
    #[ignore]
    fn measure_reading_the_installed_distros() {
        let _ = distros();
        let runs = 100;
        let start = std::time::Instant::now();
        for _ in 0..runs {
            std::hint::black_box(distros());
        }
        println!(
            "{:?} per read, found {:?}",
            start.elapsed() / runs,
            distros()
        );
    }

    #[test]
    fn both_hosts_are_wsl_and_nothing_else_is() {
        for path in [
            "\\\\wsl.localhost\\Ubuntu",
            "\\\\WSL.LOCALHOST\\Ubuntu\\home",
            "\\\\wsl$\\Debian",
            "\\\\wsl.localhost",
        ] {
            assert!(is_wsl(Path::new(path)), "{path}");
        }
        for path in [
            "\\\\fileserver\\web",
            "\\\\wsl.localhost.example.com\\x",
            "C:\\wsl.localhost",
            "",
        ] {
            assert!(!is_wsl(Path::new(path)), "{path}");
        }
    }
}
