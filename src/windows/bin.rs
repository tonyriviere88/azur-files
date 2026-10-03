//! The Recycle Bin: taking something back out of it.
//!
//! The Windows half of [`crate::shell::ops::Job::Restore`], and the one operation in this program
//! that goes through `IContextMenu` rather than `IFileOperation`.
//!
//! # Why a verb and not a move
//!
//! A deleted file is still a file. This is what the bin was holding while the round-trip test was
//! running, read off the items themselves:
//!
//! ```text
//! DeletedFrom = D:\…\target\sandbox\recycle
//! SIGDN_FILESYSPATH = D:\$RECYCLE.BIN\S-1-12-1-3285167865-…\$RKOFDIE.txt
//! SIGDN_NORMALDISPLAY = D:\…\target\sandbox\recycle\recycled.txt
//! ```
//!
//! So `IFileOperation` could move that `$RKOFDIE.txt` back and it would appear to work. It would
//! also be wrong: each deleted item is *two* files, the `$R…` with the contents and an `$I…`
//! beside it holding the original path and the deletion time. The `$I…` is what the bin displays
//! and what Restore reads. Move the first and the second is left behind, and the bin then lists an
//! item whose contents have gone.
//!
//! `undelete` is the shell's own Restore — the entry on the context menu of an item in the bin. It
//! puts the file where the `$I…` says it came from, removes both, and asks the user about a name
//! that has since been taken. There is no flat API for it: neither `SHFileOperation` nor
//! `IFileOperation` has an undelete, and the documented way to reach a namespace verb is the
//! item's context menu.
//!
//! # Finding the item again, and the two ways it went wrong first
//!
//! The item has to be one of the **bin's own**, which is the part that is easy to get wrong twice:
//!
//! - `PostDeleteItem` hands over a `psiNewlyCreated`, and it is tempting to treat that as the bin
//!   item. It is not: its ID list is a **file system** one for the `$R…` file. Invoking against it
//!   produced a menu reading `["open", "edit", "OpenWithCode", …, "delete", "properties"]` — a
//!   file's menu, with no `undelete` on it — and the restore came back `ERROR_NO_ASSOCIATION`.
//! - Composing the original path out of `DeletedFrom` and the item's *parsing* name does not work
//!   either, because the parsing name of a bin item is the `$R…` path and not the old name. The
//!   old name is on `SIGDN_NORMALDISPLAY` — and it is the whole path, not the leaf.
//!
//! So the bin is enumerated, and the item taken from the enumeration — where it is a namespace
//! item by construction, and `SHBindToParent` on it gives the bin itself. Which item, in order:
//!
//! | | |
//! | --- | --- |
//! | [`Recycled::bin`] is set | the item whose `SIGDN_FILESYSPATH` is that `$R…` file. Exact, and it tells two deletions of the same path apart |
//! | it is not | the most recently deleted item that came from [`Recycled::from`], by `DeletedFrom` and display name |

use std::path::{Path, PathBuf};

use super::win::friendly;
use super::{Owner, Recycled};
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, PROPERTYKEY};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{IContextMenu, IShellFolder, IShellItem, IShellItem2};

/// `System.Recycle.DeletedFrom` — the folder an item in the bin came out of.
///
/// The property set is `FMTID_Displaced`, which the `windows` crate names; its two members are
/// numbered rather than named anywhere, so they are written out here. This is the pair behind the
/// Recycle Bin's own **Original Location** and **Date Deleted** columns.
const DELETED_FROM: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::Win32::UI::Shell::FMTID_Displaced,
    pid: 2,
};

/// `System.Recycle.DateDeleted` — when it went there. Breaks the tie between two items that came
/// from the same path, which is what a file deleted, remade and deleted again leaves behind.
const DATE_DELETED: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::Win32::UI::Shell::FMTID_Displaced,
    pid: 3,
};

/// The verb behind Explorer's **Restore** on an item in the Recycle Bin.
///
/// Both encodings of the same eight characters. Which one the shell reads is its own choice —
/// `CMIC_MASK_UNICODE` asks for the wide one, and a handler that ignores the mask reads the other
/// — so the rule from [`super::menu`] applies here as well: fill in both.
const UNDELETE: &[u8] = b"undelete\0";
const UNDELETE_WIDE: &[u16] = &[
    b'u' as u16, b'n' as u16, b'd' as u16, b'e' as u16, b'l' as u16, b'e' as u16, b't' as u16,
    b'e' as u16, 0,
];

/// Put every one of these back where it came from.
///
/// `None` when it worked, which includes a user who answered the shell's own name-clash dialog
/// with Cancel — the same convention as the rest of [`crate::shell::ops`].
///
/// One `InvokeCommand` for all of them rather than one each: every item in the bin has the bin as
/// its parent, so they go into a single context menu, and the shell shows one progress dialog and
/// asks about a clash once. An item that cannot be found is left out rather than failing the batch
/// — that is a file the user has already restored or emptied by hand, and the others are still
/// worth putting back.
#[cfg(windows)]
pub(crate) fn restore(items: &[Recycled], owner: Owner) -> Option<String> {
    use windows::Win32::UI::Shell::{SHBindToParent, SHGetIDListFromObject};

    // SAFETY: every ID list taken from the shell here is owned by an `Ids`, which frees it; each
    // child list points into one of those and is used only while it is alive. Nothing is retained
    // past the return.
    unsafe {
        let found = find(items);
        if found.is_empty() {
            return Some("Those items are no longer in the Recycle Bin".to_owned());
        }

        // Each item's own absolute ID list, kept alive for as long as the children point into it.
        let lists: Vec<Ids> = found
            .iter()
            .filter_map(|item| SHGetIDListFromObject(item).ok().map(Ids))
            .filter(|list| !list.0.is_null())
            .collect();
        let Some(first) = lists.first() else {
            return Some("Could not name those items to the shell".to_owned());
        };

        // The bin as an `IShellFolder`, taken from an item's own parent. Which is the point:
        // these came out of the bin's enumeration, so their parent *is* the bin — where the same
        // call on the `$R…` file's list gave the file system folder, and a menu with no
        // `undelete` on it.
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let Ok(bin) = SHBindToParent::<IShellFolder>(first.0, Some(&mut child)) else {
            return Some("Could not open the Recycle Bin".to_owned());
        };
        let mut children: Vec<*const ITEMIDLIST> = vec![child as *const ITEMIDLIST];
        for list in lists.iter().skip(1) {
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHBindToParent::<IShellFolder>(list.0, Some(&mut child)).is_ok()
                && !child.is_null()
            {
                children.push(child as *const ITEMIDLIST);
            }
        }

        let menu: IContextMenu = match bin.GetUIObjectOf(HWND::default(), &children, None) {
            Ok(menu) => menu,
            Err(e) => return friendly(&e),
        };
        // **Queried before invoking, and the menu kept alive across the invoke.** Both halves are
        // load-bearing, and the second was learned the hard way: this destroyed the `HMENU` first
        // and `InvokeCommand("undelete")` came back `ERROR_NO_ASSOCIATION`, which is what the
        // shell says when a verb names nothing. A verb is filled in while the menu is
        // *populated*, and it lives as long as the menu does. `super::menu`'s `invoke` has the
        // same ordering, and its note on `CMF_OPTIMIZEFORINVOKE` is the same lesson.
        let Ok(hmenu) = windows::Win32::UI::WindowsAndMessaging::CreatePopupMenu() else {
            return Some("Could not ask the Recycle Bin what it can do".to_owned());
        };
        let _ = menu.QueryContextMenu(hmenu, 0, 1, 0x7FFF, windows::Win32::UI::Shell::CMF_NORMAL);

        let mut info = windows::Win32::UI::Shell::CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<windows::Win32::UI::Shell::CMINVOKECOMMANDINFOEX>() as u32,
            // `CMIC_MASK_UNICODE`, which the `windows` crate names only in its `SEE_MASK_`
            // spelling — the two families share their numbering. See [`super::menu`], which says
            // this at greater length.
            fMask: windows::Win32::UI::Shell::SEE_MASK_UNICODE,
            hwnd: if owner.0 != 0 {
                owner.hwnd()
            } else {
                HWND::default()
            },
            // No `lpDirectory` on either side: these items are in a namespace rather than in a
            // folder, and there is no working directory that would mean anything to them.
            lpVerb: windows::core::PCSTR(UNDELETE.as_ptr()),
            lpVerbW: windows::core::PCWSTR(UNDELETE_WIDE.as_ptr()),
            nShow: windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let invoked = menu.InvokeCommand(&mut info as *mut _ as *const _);
        // Only a debug build has a console to say it on, and a verb that has gone missing is the
        // kind of thing that is unreadable in a log and priceless in a session.
        #[cfg(debug_assertions)]
        if invoked.is_err() {
            eprintln!(
                "recycle bin: undelete was refused ({invoked:?}); the menu offers {:?}",
                verbs_of(&menu, hmenu)
            );
        }
        let _ = windows::Win32::UI::WindowsAndMessaging::DestroyMenu(hmenu);
        match invoked {
            Ok(()) => None,
            Err(e) => friendly(&e),
        }
    }
}

/// The bin's own item for each of these, skipping any that is no longer there.
///
/// The bin is enumerated once however many are wanted. It holds everything the user has deleted
/// and not cleared, which can be thousands of items, and each is asked for two properties — so
/// this runs on the operation's thread like the rest of [`super::ops`], never on the UI thread,
/// and only when something is actually being put back.
#[cfg(windows)]
unsafe fn find(wanted: &[Recycled]) -> Vec<IShellItem> {
    use windows::Win32::UI::Shell::{
        IEnumShellItems, SHGetKnownFolderItem, BHID_EnumItems, FOLDERID_RecycleBinFolder,
        KF_FLAG_DEFAULT,
    };

    /// The best candidate for one wanted item so far.
    struct Best {
        item: IShellItem,
        /// When it was deleted, which is what picks between two items claiming the same original
        /// path. Only the search route sets it — an exact `$R…` match leaves it `0`, because
        /// nothing is ever compared against it: a `$R…` file names one item in the bin, so there
        /// is no second candidate to choose over.
        when: u64,
    }

    let Ok(bin) =
        SHGetKnownFolderItem::<IShellItem>(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None)
    else {
        return Vec::new();
    };
    let Ok(items) = bin.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else {
        return Vec::new();
    };

    // Whether anything here has to be identified by where it came from. Usually nothing does —
    // `PostDeleteItem` names the `$R…` file for every item it recycles — and the difference is two
    // property reads per item in a bin that can hold thousands of them.
    let searching = wanted.iter().any(|want| want.bin.is_none());

    let mut best: Vec<Option<Best>> = wanted.iter().map(|_| None).collect();
    loop {
        let mut batch: [Option<IShellItem>; 16] = Default::default();
        let mut fetched = 0u32;
        if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
            break;
        }
        for item in batch.iter().flatten().take(fetched as usize) {
            let Ok(described) = item.cast::<IShellItem2>() else {
                continue;
            };
            // The `$R…` file this item is held as, and — only if something is being searched for —
            // where it came from and what it was called there.
            let held = name_of(item, windows::Win32::UI::Shell::SIGDN_FILESYSPATH);
            let folder = searching
                .then(|| read_string(&described, &DELETED_FROM).map(PathBuf::from));
            let was = searching
                .then(|| name_of(item, windows::Win32::UI::Shell::SIGDN_NORMALDISPLAY));

            for (index, want) in wanted.iter().enumerate() {
                // An exact match, and the only thing consulted when there is one to consult: a
                // `$R…` file that has gone means the item has been restored or emptied since,
                // and falling back to the path would then take back a *different* deletion of
                // the same name.
                if let Some(recorded) = &want.bin {
                    if held.as_deref().is_some_and(|held| same(held, recorded)) {
                        best[index] = Some(Best {
                            item: item.clone(),
                            when: 0,
                        });
                    }
                    continue;
                }
                // `searching` is true whenever any wanted item has no `$R…` path, so both of these
                // are read by the time one is reached through this arm.
                let (Some(Some(was)), Some(Some(folder))) = (&was, &folder) else {
                    continue;
                };
                if !came_from(was, folder, &want.from) {
                    continue;
                }
                let when = described.GetFileTime(&DATE_DELETED).map_or(0, |time| {
                    (time.dwHighDateTime as u64) << 32 | time.dwLowDateTime as u64
                });
                if best[index].as_ref().is_none_or(|held| held.when < when) {
                    best[index] = Some(Best {
                        item: item.clone(),
                        when,
                    });
                }
            }
        }
    }
    best.into_iter().flatten().map(|found| found.item).collect()
}

/// Whether a bin item that displays as `was`, out of `folder`, is the one that used to be at
/// `wanted`.
///
/// `SIGDN_NORMALDISPLAY` on a bin item is the **whole original path**, measured above, so the
/// first line is the whole answer nearly always. The second is the case that makes a name
/// comparison alone unsafe: on a machine with known extensions hidden — which is Windows' own
/// default — a file that was `one.txt` displays as `one`, and only the folder and the stem match.
#[cfg(windows)]
fn came_from(was: &Path, folder: &Path, wanted: &Path) -> bool {
    if same(was, wanted) {
        return true;
    }
    wanted.parent().is_some_and(|parent| same(folder, parent))
        && was.file_name().is_some_and(|shown| Some(shown) == wanted.file_stem())
}

/// One of an item's names, freed on the way out.
#[cfg(windows)]
unsafe fn name_of(
    item: &IShellItem,
    which: windows::Win32::UI::Shell::SIGDN,
) -> Option<PathBuf> {
    let wide = item.GetDisplayName(which).ok()?;
    let text = wide.to_string().ok();
    windows::Win32::System::Com::CoTaskMemFree(Some(wide.0 as *const std::ffi::c_void));
    text.map(PathBuf::from)
}

/// One string property, freed on the way out.
#[cfg(windows)]
unsafe fn read_string(item: &IShellItem2, key: &PROPERTYKEY) -> Option<String> {
    let wide = item.GetString(key).ok()?;
    if wide.is_null() {
        return None;
    }
    let text = wide.to_string().ok();
    windows::Win32::System::Com::CoTaskMemFree(Some(wide.0 as *const std::ffi::c_void));
    text
}

/// Whether two paths name the same thing, the way Windows means it.
///
/// Case-insensitively and with the separators normalised, for the reason
/// [`crate::shell::ops::all_already_in`] does the same: `C:\Temp\a.txt` and `c:/temp/a.txt` are one
/// file, and a comparison that says otherwise leaves the file in the bin and the user looking at a
/// Ctrl+Z that did nothing.
#[cfg(windows)]
fn same(a: &Path, b: &Path) -> bool {
    let fold = |path: &Path| {
        path.as_os_str()
            .to_string_lossy()
            .replace('/', "\\")
            .to_lowercase()
    };
    fold(a) == fold(b)
}

/// An absolute ID list from the shell, freed when this goes.
///
/// A guard rather than a pair of statements because the child lists handed to `GetUIObjectOf`
/// point *into* these, so their lifetime is the invoke's — and every path out of [`restore`]
/// between taking them and returning has to free them.
#[cfg(windows)]
struct Ids(*mut ITEMIDLIST);

#[cfg(windows)]
impl Drop for Ids {
    fn drop(&mut self) {
        // SAFETY: allocated by `SHGetIDListFromObject`, freed exactly once.
        unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
    }
}

/// Every verb on a menu that has been queried, for the one message worth printing when a verb
/// this program depends on is not among them.
///
/// A debug build only: it costs a `GetCommandString` per entry, and the only reader is somebody
/// looking at why an `undelete` came back refused. It is how the two mistakes in this module's
/// header were found, and both were invisible without it.
#[cfg(all(windows, debug_assertions))]
unsafe fn verbs_of(
    menu: &IContextMenu,
    hmenu: windows::Win32::UI::WindowsAndMessaging::HMENU,
) -> Vec<String> {
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuItemID};

    let mut found = Vec::new();
    for position in 0..GetMenuItemCount(Some(hmenu)).max(0) {
        let id = GetMenuItemID(hmenu, position);
        if id == u32::MAX || id == 0 {
            continue;
        }
        let mut wide = [0u16; 128];
        if menu
            .GetCommandString(
                id as usize - 1,
                windows::Win32::UI::Shell::GCS_VERBW,
                None,
                windows::core::PSTR(wide.as_mut_ptr() as *mut u8),
                wide.len() as u32,
            )
            .is_ok()
        {
            let end = wide.iter().position(|unit| *unit == 0).unwrap_or(wide.len());
            found.push(String::from_utf16_lossy(&wide[..end]));
        }
    }
    found
}
