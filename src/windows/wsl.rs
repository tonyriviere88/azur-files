//! The distributions WSL has registered for this user.
//!
//! The Windows half of [`crate::fs::wsl`]: one `RegOpenKeyExW` on `Lxss`, a `RegEnumKeyExW` over
//! its subkeys, and three `RegGetValueW` per distribution. No process, and nothing that reaches
//! the WSL service.

use super::*;

use crate::shell::nul_terminated;
use crate::shell::registry::{dword, Key};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_SZ,
};

const LXSS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss";

/// `State` of a distribution that has finished installing. Anything else — `3` while it is still
/// being unpacked, `4` while it is being removed — is a distribution with no filesystem to open.
const INSTALLED: u32 = 1;

pub(super) fn registered() -> Vec<Distro> {
    let Ok(key) = Key::open(HKEY_CURRENT_USER, LXSS, KEY_READ) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for index in 0.. {
        // A subkey is a GUID, 38 characters.
        let Some(id) = key.subkey(index) else {
            break;
        };
        let sub = format!("{LXSS}\\{id}");
        // A distribution written before `State` existed has none, and is installed.
        if dword(HKEY_CURRENT_USER, &sub, "State").is_some_and(|state| state != INSTALLED) {
            continue;
        }
        let Some(name) = string(&sub, "DistributionName") else {
            continue;
        };
        let base = string(&sub, "BasePath").map(PathBuf::from);
        found.push(Distro::new(&name, base.as_deref()));
    }
    found
}

/// A `REG_SZ` under `HKEY_CURRENT_USER\<key>`.
fn string(key: &str, value: &str) -> Option<String> {
    let (key, value) = (nul_terminated(key), nul_terminated(value));
    // A path, so `MAX_PATH` doubled for the `\\?\` case. Anything longer is not one.
    let mut buffer = [0u16; 560];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: both names are NUL-terminated; `buffer` and `bytes` describe the same allocation and
    // `bytes` is updated to what was written, terminator included.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if read != 0 {
        return None;
    }
    let units = (bytes as usize / 2).min(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..units]);
    let text = text.trim_end_matches('\0').trim();
    // A `BasePath` from an older install can carry the `\\?\` prefix, which the shell does not
    // take for an icon's path.
    let text = text.strip_prefix("\\\\?\\").unwrap_or(text);
    (!text.is_empty()).then(|| text.to_owned())
}
