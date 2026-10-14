//! Reading the registry: an open key that closes itself, and the three questions asked of one.
//!
//! The Windows half of [`crate::shell`], beside `windows/com.rs` and for the same reason that is
//! there: five files read keys — the thumbnail-provider probe, the `Win+E` setting, the context
//! menu's handler names, the WSL distributions, Developer Mode — and each had written the open, the
//! read and the close out by hand, with a `RegCloseKey` to remember at every way out.
//!
//! What stays with each caller is everything they actually disagree about: which value kinds count
//! as text, what an empty value means, and how big a buffer the value deserves. Those are the
//! decisions; this is only the plumbing.

use windows_sys::Win32::System::Registry::{RegCloseKey, HKEY};

/// An open registry key, closed when this goes.
pub(crate) struct Key(pub(crate) HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: opened or created by this program, owned by this guard alone, closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    /// Open `path` under `root` with `access`. `Err` is the Win32 code, for the callers that tell a
    /// key that is not there from one that refused.
    pub(crate) fn open(root: HKEY, path: &str, access: u32) -> Result<Key, u32> {
        use windows_sys::Win32::System::Registry::RegOpenKeyExW;

        let wide = crate::shell::nul_terminated(path);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and outlives the call, and `key` is only kept when the
        // call reports success.
        match unsafe { RegOpenKeyExW(root, wide.as_ptr(), 0, access, &mut key) } {
            0 => Ok(Key(key)),
            code => Err(code),
        }
    }

    /// A value read as text — the key's own default when `name` is `None` — with the kind it was
    /// stored as, so the caller decides which kinds it will take. Into a buffer of `N` units, which
    /// is the caller's measure of what a real value of this kind can be.
    ///
    /// The text is whatever was written, less the terminator and surrounding whitespace, and may be
    /// empty: an empty value is two bytes of terminator, or occasionally none at all, and whether
    /// that means anything is again the caller's to say.
    pub(crate) fn string<const N: usize>(&self, name: Option<&str>) -> Option<(u32, String)> {
        use windows_sys::Win32::System::Registry::RegQueryValueExW;

        let wide_name = name.map(crate::shell::nul_terminated);
        let mut buffer = [0u16; N];
        let mut bytes = std::mem::size_of_val(&buffer) as u32;
        let mut kind = 0u32;
        // SAFETY: the key is open, `buffer` and `bytes` describe the same allocation, `bytes` is
        // updated by the call to what was actually written, and the name pointer is null exactly
        // when there is no name — which is how the API is told to read the key's default value.
        let read = unsafe {
            RegQueryValueExW(
                self.0,
                wide_name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                std::ptr::null_mut(),
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if read != 0 {
            return None;
        }
        // `bytes` counts bytes and includes the terminator, which is not part of the string.
        let units = (bytes as usize / 2).min(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..units]);
        Some((kind, text.trim_end_matches('\0').trim().to_owned()))
    }

    /// The name of the subkey at `index`, or `None` past the last one.
    pub(crate) fn subkey(&self, index: u32) -> Option<String> {
        use windows_sys::Win32::System::Registry::RegEnumKeyExW;

        // 256 is the documented maximum for a key name.
        let mut buffer = [0u16; 256];
        let mut units = buffer.len() as u32;
        // SAFETY: `buffer` and `units` describe the same allocation, and `units` is updated by the
        // call to the length written, not counting the terminator.
        let read = unsafe {
            RegEnumKeyExW(
                self.0,
                index,
                buffer.as_mut_ptr(),
                &mut units,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (read == 0).then(|| String::from_utf16_lossy(&buffer[..(units as usize).min(buffer.len())]))
    }
}

/// A `REG_DWORD` at `root\path`, asked in one call without opening anything to close.
pub(crate) fn dword(root: HKEY, path: &str, name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_DWORD};

    let (path, name) = (crate::shell::nul_terminated(path), crate::shell::nul_terminated(name));
    let mut data = 0u32;
    let mut bytes = std::mem::size_of::<u32>() as u32;
    // SAFETY: both names are NUL-terminated; `data` is a `u32` and `bytes` says so.
    let read = unsafe {
        RegGetValueW(
            root,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut bytes,
        )
    };
    (read == 0).then_some(data)
}
