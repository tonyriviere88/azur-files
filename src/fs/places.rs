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
    /// Hand this to the shell instead of listing it ourselves.
    ///
    /// The Recycle Bin is not a directory — it is a shell namespace extension
    /// stitched together from a per-volume `$Recycle.Bin\<SID>` plus an index of
    /// original paths. Enumerating it with `FindFirstFile` shows the mangled
    /// `$R…` names and no way to restore anything, so this row opens the real
    /// thing rather than lying about it.
    pub shell_only: bool,
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
        shell_only: false,
    }];

    for (folder, label, icon) in known_folders() {
        if let Some(path) = folder {
            places.push(Place {
                label: label.to_owned(),
                path,
                icon,
                shell_only: false,
            });
        }
    }

    places.push(Place {
        label: "Recycle Bin".to_owned(),
        path: PathBuf::from("shell:RecycleBinFolder"),
        icon: PlaceIcon::Trash,
        shell_only: true,
    });
    places
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
