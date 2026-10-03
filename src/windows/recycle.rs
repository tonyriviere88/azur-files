//! Where this user's Recycle Bin is on disk.
//!
//! The Windows half of [`crate::fs::recycle`]: the account's SID, which names its folder inside
//! each volume's `$Recycle.Bin`, and the volumes that can have one.

use super::*;

/// `X:\$Recycle.Bin\<SID>` for every local lettered volume.
///
/// Local only. A mapped network drive has no bin — a file deleted from a share is gone — and
/// asking a share for a folder it does not have is a round trip to a server that may not answer.
/// Nothing here touches a disk: the letters come from the mount table, and whether each folder
/// exists is [`super::listing`]'s question.
pub(super) fn bins() -> Vec<PathBuf> {
    use crate::fs::drives::DriveKind;

    let Some(sid) = user_sid() else {
        return Vec::new();
    };
    crate::fs::drives::list_letters()
        .into_iter()
        .filter(|drive| {
            matches!(
                drive.kind,
                DriveKind::Fixed | DriveKind::Removable | DriveKind::RamDisk
            )
        })
        // Spelled as NTFS stores it and as the shell reports it — `D:\$RECYCLE.BIN\…`, measured in
        // `crate::shell::ops::bin` — so a row's target is the same string the shell hands back for
        // the same item, and not merely the same file.
        .map(|drive| drive.path.join("$RECYCLE.BIN").join(sid))
        .collect()
}

/// This account's SID as a string, `S-1-5-21-…`, asked once.
///
/// Formatted here rather than by `ConvertSidToStringSidW`, which lives behind a crate feature this
/// program would otherwise not need. The format is the documented one and short: revision, the
/// identifier authority — in decimal when it fits in 32 bits, which it always does for an account
/// — and each subauthority.
fn user_sid() -> Option<&'static str> {
    static SID: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    SID.get_or_init(read_sid).as_deref()
}

fn read_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetSidIdentifierAuthority, GetSidSubAuthority, GetSidSubAuthorityCount,
        GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: the token is closed on every path out; the buffer is sized by the first call and
    // aligned for `TOKEN_USER` by being `u64`s; the SID pointer points into that buffer and is only
    // read while it is alive.
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut needed = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        let read = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut needed,
        );
        CloseHandle(token);
        if read == 0 {
            return None;
        }
        let sid = (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid;
        if sid.is_null() {
            return None;
        }
        // The revision is the SID's first byte; there is no accessor for it.
        let revision = *(sid as *const u8);
        let authority = (*GetSidIdentifierAuthority(sid)).Value;
        let mut text = format!("S-{revision}-");
        if authority[0] == 0 && authority[1] == 0 {
            let value = u32::from_be_bytes([authority[2], authority[3], authority[4], authority[5]]);
            text.push_str(&value.to_string());
        } else {
            text.push_str("0x");
            for byte in authority {
                text.push_str(&format!("{byte:02X}"));
            }
        }
        for at in 0..*GetSidSubAuthorityCount(sid) as u32 {
            text.push_str(&format!("-{}", *GetSidSubAuthority(sid, at)));
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    /// The SID formatted here is the one Windows itself prints for this account.
    #[test]
    fn the_sid_is_the_one_windows_prints() {
        let sid = super::user_sid().expect("a process has a user");
        let mut command = std::process::Command::new("whoami");
        command.args(["/user", "/fo", "csv", "/nh"]);
        crate::shell::no_window(&mut command);
        let said = command.output().expect("whoami runs");
        let said = String::from_utf8_lossy(&said.stdout);
        assert!(said.contains(sid), "whoami says {said:?}, this says {sid:?}");
    }
}
