//! The known folders the sidebar offers, resolved once at startup.
//!
//! Asked of the shell rather than assembled from `%USERPROFILE%`: Downloads,
//! Documents and the rest can be redirected anywhere — to another volume, or to
//! OneDrive, which is the common case on a managed machine — and a hard-coded
//! `~/Documents` would quietly point at an empty folder beside the real one.

use std::path::PathBuf;

#[cfg(windows)]
#[path = "../windows/places.rs"]
mod win;
#[cfg(windows)]
use win::{known_folder, known_folders};
#[cfg(windows)]
pub use win::shell_place;

use crate::fs::fmt::Kind;

/// Which glyph a place gets. Distinct from [`Kind`] because these are *places*,
/// not file types: Downloads is not "a folder", it is Downloads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlaceIcon {
    ThisPc,
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
    Trash,
}

/// One sidebar entry under Places.
#[derive(Clone, Debug)]
pub struct Place {
    pub label: String,
    /// Where it goes. Empty means the synthetic "This PC".
    pub path: PathBuf,
    pub icon: PlaceIcon,
}

/// The standard places, in the order the sidebar shows them.
///
/// Anything the shell will not resolve — a machine with no Videos folder — is
/// dropped rather than shown as a dead row.
pub fn standard() -> Vec<Place> {
    let mut places = vec![Place {
        label: "This PC".to_owned(),
        path: PathBuf::new(),
        icon: PlaceIcon::ThisPc,
    }];

    for (folder, label, icon) in known_folders() {
        if let Some(path) = folder {
            places.push(Place {
                label: label.to_owned(),
                path,
                icon,
            });
        }
    }

    // Listed here like any other place — see [`crate::fs::recycle`] for how, and for why it used to
    // open Explorer instead.
    places.push(Place {
        label: "Recycle Bin".to_owned(),
        path: crate::fs::recycle::location(),
        icon: PlaceIcon::Trash,
    });
    places
}

/// The place a name on its own means, the way Explorer's address bar reads one: `Downloads`,
/// `desktop`, `Recycle Bin`.
///
/// Only a bare name, never anything with a separator or a colon in it — those are paths, and a
/// path is the disk's to answer. Case-insensitive, and against the labels the sidebar shows, so
/// what can be typed is what can be read off the left of the window.
pub fn named(text: &str) -> Option<PathBuf> {
    if text.contains(['\\', '/', ':', '%', '~']) {
        return None;
    }
    // Resolved once, as [`crate::app`] resolves its own copy: the known folders do not move while
    // the program runs, and this is asked on every Enter in the path bar.
    static PLACES: std::sync::OnceLock<Vec<Place>> = std::sync::OnceLock::new();
    PLACES
        .get_or_init(standard)
        .iter()
        .find(|place| place.label.eq_ignore_ascii_case(text))
        .map(|place| place.path.clone())
}

/// What one of the shell's own names for a place turns out to be.
///
/// The names in question are the ones the Run box and Explorer's bar take — `shell:Downloads`,
/// `shell:AppData`, `shell:Startup`, `shell:SendTo`, `::{20D04FE0-…}` — and they come in three
/// kinds, which is why this is not simply a path:
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellPlace {
    /// A folder on disk, which is nearly all of them.
    Folder(PathBuf),
    /// This PC, which this program shows as the empty path.
    ThisPc,
    /// The Recycle Bin, which this program shows at [`crate::fs::recycle::LOCATION`].
    RecycleBin,
    /// Somewhere that is only the shell's to show — Control Panel, Network, a library. Handed to
    /// Explorer, since there is nothing here that could draw it.
    Elsewhere,
}

#[cfg(not(windows))]
pub fn shell_place(_name: &str) -> Option<ShellPlace> {
    None
}

#[cfg(not(windows))]
fn known_folders() -> Vec<(Option<PathBuf>, &'static str, PlaceIcon)> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let under = |name: &str| home.as_ref().map(|h| h.join(name));
    vec![
        (home.clone(), "Home", PlaceIcon::Home),
        (under("Desktop"), "Desktop", PlaceIcon::Desktop),
        (under("Documents"), "Documents", PlaceIcon::Documents),
        (under("Downloads"), "Downloads", PlaceIcon::Downloads),
        (under("Pictures"), "Pictures", PlaceIcon::Pictures),
        (under("Music"), "Music", PlaceIcon::Music),
        (under("Videos"), "Videos", PlaceIcon::Videos),
    ]
}

/// Where a new window opens if nothing was remembered.
pub fn default_start() -> PathBuf {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::FOLDERID_Profile;
        if let Some(home) = known_folder(&FOLDERID_Profile) {
            return home;
        }
        PathBuf::from("C:\\")
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }
}

/// The [`Kind`] a place's icon stands in for, when a listing row needs one.
pub fn kind_of(icon: PlaceIcon) -> Kind {
    match icon {
        PlaceIcon::Trash => Kind::Other,
        _ => Kind::Folder,
    }
}
