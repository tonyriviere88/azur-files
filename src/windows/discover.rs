//! Finding machines on the network that nothing here is connected to.
//!
//! # Why this is the shell and not `WNetOpenEnum`
//!
//! The obvious call is `WNetOpenEnum(RESOURCE_GLOBALNET)`, walking providers down to servers.
//! It does not work on a current Windows and it fails expensively. Measured on this machine:
//!
//! | step | answer |
//! | --- | --- |
//! | the root | 4 providers, 0 ms |
//! | `Microsoft Windows Network` | **`ERROR_EXTENDED_ERROR` after 9.9 s** |
//! | `net view` | `error 6118`, the list of servers is not available |
//!
//! Both are the same fact: that enumeration is the SMB1 *Computer Browser* service, which is not
//! installed on Windows 10 or later. Ten seconds to find nothing, twice over, is what it costs.
//!
//! Explorer's Network node does not use it either. It shows what **WSD and SSDP** have announced,
//! which reaches this program the same way it reaches Explorer: the shell's Network folder, a
//! namespace whose children are those announcements.
//!
//! # What is a machine, out of what that folder holds
//!
//! Two kinds of child, and only the first is a file server:
//!
//! | child | parsing name | what it is |
//! | --- | --- | --- |
//! | `DESKTOP-3A9VGG7` | `\\DESKTOP-3A9VGG7` | an SMB machine — what Explorer lets you open |
//! | `FileServer` | `…Microsoft.Networking.SSDP//uuid:0011322e-…` | a UPnP device announcement |
//! | `Archer BE3600` | `…SSDP//uuid:1d66f945-…` | likewise, and a router |
//!
//! A child whose parsing name is a UNC path **is** a machine and is taken as one. The others are
//! devices: double-clicking one in Explorer opens its web page, not a share.
//!
//! But a NAS announces itself over SSDP and not as an SMB computer, so the machine most worth
//! finding is in the second row of that table — which is why a device whose name is shaped like a
//! host name is offered as a **candidate**. `FileServer` becomes `\\FileServer`; `Archer
//! BE3600` does not, because a host name has no spaces in it. Whether a candidate is really a file
//! server is not asked here — that answer costs up to 22 seconds (see
//! [`crate::fs::drives::shares_on`]) and it is what clicking the row is for.

use std::path::PathBuf;

/// Every machine the network has announced, as `\\name` paths.
///
/// **Blocks, and only ever runs when asked.** Nothing calls this on a timer, at startup or on F5:
/// it reaches the network, and the answer is a list of other people's computers rather than
/// anything about this one. See `Volumes::discover`, which puts it on a worker.
#[cfg(windows)]
pub fn machines() -> Vec<PathBuf> {
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, IEnumShellItems, IShellItem, SHGetKnownFolderItem,
        FOLDERID_NetworkFolder, KF_FLAG_DEFAULT, SIGDN_DESKTOPABSOLUTEPARSING,
        SIGDN_NORMALDISPLAY,
    };

    // This thread's own apartment, entered and left around the walk. Not the long-lived one in
    // [`super::com`]: nothing here hands an interface out, so there is no promise to keep past the
    // last line — and a browse that takes seconds has no business occupying a thread the icons and
    // the context menus share.
    //
    // SAFETY: paired with the `CoUninitialize` below, on a thread this function owns.
    let apartment = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let mut found: Vec<PathBuf> = Vec::new();

    // SAFETY: every interface below is checked before use, and every `PWSTR` the shell allocates
    // is freed by `Name` on the way out of its scope.
    unsafe {
        if let Ok(network) = SHGetKnownFolderItem::<IShellItem>(
            &FOLDERID_NetworkFolder,
            KF_FLAG_DEFAULT,
            None,
        ) {
            // `None` is "no bind context", and the second parameter is what the handler is to be
            // returned as. Both have to be spelled out: the first is generic over anything that can
            // stand in for an `IBindCtx`.
            if let Ok(items) =
                network.BindToHandler::<Option<&windows::Win32::System::Com::IBindCtx>, IEnumShellItems>(
                    None,
                    &BHID_EnumItems,
                )
            {
                loop {
                    let mut child = [None];
                    let mut got = 0u32;
                    // `Next` returns `S_FALSE` at the end, which is not an error — so the count is
                    // what says whether anything came back.
                    if items.Next(&mut child, Some(&mut got)).is_err() || got == 0 {
                        break;
                    }
                    let Some(child) = child[0].as_ref() else { break };

                    // A UNC parsing name is a machine, full stop. This is the row Explorer lets
                    // you open, and it needs no guessing.
                    let parsing = Name::of(child, SIGDN_DESKTOPABSOLUTEPARSING);
                    if let Some(text) = parsing.text().filter(|t| t.starts_with("\\\\")) {
                        push(&mut found, text.trim_end_matches('\\'));
                        continue;
                    }
                    // Otherwise a device announcement. Its *name* is a machine name often enough
                    // to be worth offering — a NAS is announced this way and nowhere else — and
                    // the shape of a host name is what decides: one token, no spaces.
                    let shown = Name::of(child, SIGDN_NORMALDISPLAY);
                    if let Some(name) = shown.text().filter(|name| host_shaped(name)) {
                        push(&mut found, &format!("\\\\{name}"));
                    }
                }
            }
        }
    }

    if apartment.is_ok() {
        // SAFETY: this thread entered the apartment above and hands nothing out of it.
        unsafe { CoUninitialize() };
    }
    found
}

/// Whether a name could be a machine's.
///
/// One token and nothing else: a host name has no spaces, and the parenthetical in
/// `FileServer (DS414j)` is a model number rather than part of a name. Which also dedups that
/// device against the plain `FileServer` beside it in the same folder, since only the plain one
/// gets through.
#[cfg(windows)]
fn host_shaped(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.contains(['\\', '/', ' ', '(', ')', ':', '"'])
}

/// Add a machine, unless it is already there. Case-insensitively, because a UNC path is.
#[cfg(windows)]
fn push(found: &mut Vec<PathBuf>, unc: &str) {
    if unc.len() <= 2 {
        return;
    }
    if !found
        .iter()
        .any(|had| had.as_os_str().eq_ignore_ascii_case(unc))
    {
        found.push(PathBuf::from(unc));
    }
}

/// A display name the shell allocated, freed when this goes out of scope.
///
/// `IShellItem::GetDisplayName` hands back a `PWSTR` out of the COM allocator, and the leak if it
/// is not given back is per item per browse. A guard rather than a call at each exit, because there
/// are three ways out of the loop above.
#[cfg(windows)]
struct Name(windows::core::PWSTR);

#[cfg(windows)]
impl Name {
    /// SAFETY: `item` must be live, which it is — it is borrowed from the enumerator's slot.
    unsafe fn of(
        item: &windows::Win32::UI::Shell::IShellItem,
        kind: windows::Win32::UI::Shell::SIGDN,
    ) -> Self {
        Self(item.GetDisplayName(kind).unwrap_or_default())
    }

    fn text(&self) -> Option<String> {
        if self.0.is_null() {
            return None;
        }
        // SAFETY: a non-null `PWSTR` from the shell is a null-terminated wide string.
        let text = unsafe { self.0.to_string() }.ok()?;
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(windows)]
impl Drop for Name {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by the shell with the COM allocator, freed once, here.
            unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(self.0 .0 as *const _)) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule that turns a device announcement into a machine to try.
    ///
    /// A NAS announces itself over SSDP rather than as an SMB computer, so the name is all there
    /// is to go on — and the shape of a host name is what separates `FileServer` from a router
    /// called `Archer BE3600`.
    #[test]
    #[cfg(windows)]
    fn a_device_name_is_a_machine_only_if_it_is_shaped_like_one() {
        assert!(host_shaped("FileServer"));
        assert!(host_shaped("DESKTOP-3A9VGG7"));
        assert!(host_shaped("nas.local"));
        // A model number in brackets is not part of a name — and dropping this one dedups it
        // against the plain `FileServer` in the same folder.
        assert!(!host_shaped("FileServer (DS414j)"));
        assert!(!host_shaped("Archer BE3600"));
        assert!(!host_shaped(""));
        assert!(!host_shaped("\\\\already-a-path"));
    }

    /// The same machine announced twice is one row.
    #[test]
    #[cfg(windows)]
    fn a_machine_is_added_once_whatever_its_case() {
        let mut found = Vec::new();
        push(&mut found, "\\\\FileServer");
        push(&mut found, "\\\\fileserver");
        push(&mut found, "\\\\DESKTOP-3A9VGG7");
        // And nothing that is not a name at all.
        push(&mut found, "\\\\");
        assert_eq!(
            found,
            [
                PathBuf::from("\\\\FileServer"),
                PathBuf::from("\\\\DESKTOP-3A9VGG7")
            ]
        );
    }
}
