//! `System.StorageProviderState`, read off a child of a synced folder.
//!
//! The Windows half of [`crate::shell::cloud`]. One shell item for the folder, then one relative
//! item per name — the folder is parsed once rather than once per row.

use super::*;
use windows::core::PCWSTR;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::UI::Shell::{IShellItem, IShellItem2, SHCreateItemFromRelativeName};

/// `PKEY_StorageProviderState`, spelled out rather than taken from the `windows` crate, which only
/// has it behind `Win32_Storage_EnhancedStorage` — a feature for one constant.
const STORAGE_PROVIDER_STATE: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0xe77e90df_6271_4f5b_834f_2dd1f245dda4),
    pid: 3,
};

/// The folder whose children are being asked about.
pub(super) struct Parent(IShellItem);

impl Parent {
    pub(super) fn open(dir: &Path) -> Option<Self> {
        // SAFETY: nothing beyond the parse itself; the item releases itself on drop.
        unsafe { crate::shell::parse_item::<IShellItem>(dir) }.ok().map(Self)
    }

    /// What the provider says about one child, by name. `None` for a name it has no state for —
    /// excluded from sync, not a placeholder at all, or gone since the listing was read.
    pub(super) fn state(&self, name: &str) -> Option<Sync> {
        let wide = crate::shell::nul_terminated(name);
        // SAFETY: `wide` is NUL-terminated and outlives the call; the property key is a constant that outlives the call.
        let value = unsafe {
            let item = SHCreateItemFromRelativeName::<_, _, _, IShellItem2>(
                &self.0,
                PCWSTR(wide.as_ptr()),
                None,
            )
            .ok()?;
            item.GetUInt32(&STORAGE_PROVIDER_STATE).ok()?
        };
        // `STORAGEPROVIDERSTATE_*`, as `propkey.h` numbers them.
        match value {
            1 => Some(Sync::Online),
            2 => Some(Sync::Local),
            3 => Some(Sync::Pinned),
            // Pending upload, pending download, transferring, pending for some other reason.
            4 | 5 | 6 | 10 => Some(Sync::Syncing),
            7 => Some(Sync::Error),
            8 => Some(Sync::Warning),
            // None, and excluded — which Explorer leaves blank too.
            _ => None,
        }
    }
}
