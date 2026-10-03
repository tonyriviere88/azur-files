//! The OEM code page, which is the *other* encoding on a console pipe.
//!
//! The Windows half of [`crate::console`]'s `Decoder`, where the reasoning is: the shell writes
//! UTF-8 and every Win32 console tool writes this, into the same pipe, and the bytes carry no
//! marking to say which is which.

/// The system's OEM code page — 850 on a French machine, 437 on an American one, 932 on a Japanese
/// one.
///
/// Asked once. It is a property of the installed system rather than of a thread's locale, so it
/// cannot change under a running process, and it is wanted for every byte that is not UTF-8.
fn code_page() -> u32 {
    static CP: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    // SAFETY: no arguments and no out parameters; it reads a system constant.
    *CP.get_or_init(|| unsafe { windows::Win32::Globalization::GetOEMCP() })
}

/// Whether a byte begins a two-byte character.
///
/// False for every byte of a single-byte code page, which is all of the Latin ones — so on most
/// machines this is asked and answered no. It matters for 932, 936, 949 and 950, where a character's
/// *trail* byte can be in the ASCII range: taking the lead byte on its own there would leave the
/// trail behind as a stray letter, which is half a character silently becoming a whole wrong one.
pub fn double_byte(first: u8) -> bool {
    // SAFETY: two integers by value.
    unsafe { windows::Win32::Globalization::IsDBCSLeadByteEx(code_page(), first).is_ok() }
}

/// Bytes in the OEM code page, as text.
///
/// **No `MB_ERR_INVALID_CHARS`.** The code page is a *guess* at what the tool meant — nothing on the
/// pipe says so — and a byte that does not map wants to come out as the code page's own substitute
/// rather than fail the call and take the line it was in with it.
pub fn decode(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};

    /// Substitute rather than refuse; see above.
    const SUBSTITUTE: MULTI_BYTE_TO_WIDE_CHAR_FLAGS = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);

    if bytes.is_empty() {
        return String::new();
    }
    // SAFETY: measured first, then written into a buffer of exactly the measured length. Both calls
    // are handed the slice, whose length goes with it, so neither can read past its end.
    let wide = unsafe {
        let wanted = MultiByteToWideChar(code_page(), SUBSTITUTE, bytes, None);
        if wanted <= 0 {
            // A code page that cannot read its own bytes. One replacement character says so, and
            // says it in one character rather than one per byte.
            return "\u{fffd}".to_owned();
        }
        let mut wide = vec![0u16; wanted as usize];
        let written = MultiByteToWideChar(code_page(), SUBSTITUTE, bytes, Some(&mut wide));
        wide.truncate(written.max(0) as usize);
        wide
    };
    String::from_utf16_lossy(&wide)
}
