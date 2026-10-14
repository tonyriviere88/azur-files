//! The two COM primitives every call in [`crate::shell`] is built on: waiting without
//! breaking the apartment's promise, and turning a path into something the shell will parse.
//!
//! And the small pieces of Win32 plumbing that more than one file under `src/windows/` needs, each
//! written once here instead of once per caller: a string or an ID list the shell allocated, given
//! back; a `CF_HDROP` read; a moveable global block for a medium; a short-lived apartment; text in
//! and out of NUL-terminated UTF-16; the process token. Every one of those was a near-copy in two to
//! eight files, and the copies had started to differ — one of them in the way that crashes.
//!
//! The Windows half of [`crate::shell`].

/// Wait for a while, answering the cross-apartment calls this thread owes, and nothing else.
///
/// A single-threaded apartment is a promise: anything this thread hands out can be called back
/// into, and the caller blocks until it is answered. A thread that sleeps, or blocks on a
/// channel, breaks that promise — and on Windows the things it breaks are not obvious:
///
/// - **The clipboard stops working.** Whatever put data on the clipboard owns it, and reading
///   it from anywhere else is a call back into the owning apartment. A shell Copy run on the
///   modal thread put the files there correctly and *nothing could read them*, this program
///   included, because that thread was parked in `recv`.
/// - **A copy refuses the next copy.** The clipboard holds a reference rather than the bytes,
///   so a reader takes the clipboard lock and then calls in for the data; unanswered, the lock
///   stays taken. See the note on `retrying` in [`clipboard`].
///
/// `CoWaitForMultipleHandles` on an event nobody will ever signal, so the wait *is* the sleep.
/// Without `COWAIT_DISPATCH_WINDOW_MESSAGES` it services cross-apartment calls and leaves the
/// window queue alone — which matters on the UI thread, where dispatching a `WM_PAINT` from
/// inside an egui pass would have egui begin a pass while already inside one.
#[cfg(windows)]
pub(crate) fn answering_calls(ms: u64) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Com::{CoWaitForMultipleHandles, COWAIT_DISPATCH_CALLS};
    use windows::Win32::System::Threading::CreateEventW;

    thread_local! {
        /// One per thread, made once. The modal thread waits like this twenty times a second
        /// for as long as the program runs, and creating and closing a handle each time would
        /// be a needless million of them over an afternoon. Never signalled, so it is only
        /// ever a thing to wait on; leaked at thread exit, which is where the process is
        /// going anyway.
        static NEVER: Option<HANDLE> = {
            // SAFETY: a plain manual-reset event, unnamed and unsignalled.
            unsafe { CreateEventW(None, true, false, None).ok() }
        };
    }

    let waited = NEVER.with(|event| match event {
        Some(event) => {
            // SAFETY: the handle belongs to this thread and outlives the wait.
            unsafe {
                let _ = CoWaitForMultipleHandles(
                    COWAIT_DISPATCH_CALLS.0 as u32,
                    ms as u32,
                    &[*event],
                );
            }
            true
        }
        None => false,
    });
    // No event to wait on is no reason to spin.
    if !waited {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// A path as a null-terminated wide string, which every call here wants.
///
/// Forward slashes become backslashes on the way through, and that is not cosmetic. The
/// kernel accepts either — `std::fs` opens `C:/Windows/notepad.exe` quite happily — but the
/// shell *parses* paths, and `SHCreateItemFromParsingName` and `SHParseDisplayName` both
/// refuse one with a slash in it. So a path that arrived with slashes, which is exactly what
/// `--open=C:/Windows` gives, used to list its folder perfectly and then fail every single
/// shell call against it: no icons, no context menu, and copy, paste and delete all reporting
/// that the items could not be found. One conversion here fixes all of them, because every
/// call in this module comes through this function.
#[cfg(windows)]
pub(crate) fn wide(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    // Unit by unit is safe: `/` is U+002F, which never appears as either half of a surrogate
    // pair, so no character outside the BMP can be damaged by this.
    const SLASH: u16 = b'/' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    path.as_os_str()
        .encode_wide()
        .map(|unit| if unit == SLASH { BACKSLASH } else { unit })
        .chain(std::iter::once(0))
        .collect()
}

/// Text as a NUL-terminated wide string, exactly as it is — **without** [`wide`]'s slash rewrite.
///
/// For everything that is not a path on its way to the shell's parser: a registry key, whose parts
/// are separated by the very character [`wide`] rewrites; a value name, a verb, a moniker, a command
/// line; a path handed to a call that takes it as written. The two are kept apart on purpose —
/// routing one of these through [`wide`] would quietly change what it names.
pub(crate) fn nul_terminated(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    text.as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// The part of a fixed-size wide buffer before its terminator, or all of it when there is none.
///
/// `#[inline]` because [`crate::fs::scan`] cuts every directory entry's name with it.
#[inline]
pub(crate) fn until_nul(units: &[u16]) -> &[u16] {
    let len = units.iter().position(|&unit| unit == 0).unwrap_or(units.len());
    &units[..len]
}

/// A fixed-size wide buffer the system filled in, as text: [`until_nul`], decoded lossily.
pub(crate) fn wide_text(units: &[u16]) -> String {
    String::from_utf16_lossy(until_nul(units))
}

/// A string the COM task allocator handed over, as text — and given back.
///
/// `None` for a null pointer, and that check is why this is one function rather than six copies.
/// An `S_OK` with nothing at the pointer is the one shape of answer that takes the process with it:
/// `PWSTR::to_string` is `wcslen` on whatever it is handed and there is no check inside it. Not a
/// theory — `landed` in `windows/ops.rs` carries the stack from a real crash of exactly this,
/// faulting in `ucrtbase!wcslen` on a replace over a network share. Of the copies this replaced,
/// one had no check.
///
/// Freed whatever the decoding made of it, so no caller's `?` can leak the string.
///
/// # Safety
///
/// `text` is null, or a NUL-terminated string out of `CoTaskMemAlloc` that nothing else will free.
pub(crate) unsafe fn take_co_string(text: windows::core::PWSTR) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let decoded = text.to_string().ok();
    windows::Win32::System::Com::CoTaskMemFree(Some(text.0 as *const std::ffi::c_void));
    decoded
}

/// One of an item's names: `GetDisplayName`, through [`take_co_string`]. `None` when the shell has
/// no such name for the item, or answers with nothing.
///
/// # Safety
///
/// `item` is live, on a thread COM is usable from.
pub(crate) unsafe fn display_name(
    item: &windows::Win32::UI::Shell::IShellItem,
    sigdn: windows::Win32::UI::Shell::SIGDN,
) -> Option<String> {
    take_co_string(item.GetDisplayName(sigdn).ok()?)
}

/// An ID list the shell allocated, freed when this goes.
///
/// A guard rather than a pair of statements because what is handed to the shell — an array of
/// these, the child lists `SHBindToParent` gives — points *into* them, so their lifetime is the
/// call's, and every path out between taking them and the call has to free them.
pub(crate) struct Pidl(pub(crate) *mut windows::Win32::UI::Shell::Common::ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by the shell and owned by this guard alone, so freed exactly once.
            unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
        }
    }
}

/// A path as an ID list, through [`wide`] — so a path written with forward slashes still parses.
/// `None` for a path naming nothing the shell can find.
pub(crate) fn pidl_of(path: &std::path::Path) -> Option<Pidl> {
    parsed_pidl(&wide(path))
}

/// Any parsing name as an ID list, given NUL-terminated and taken exactly as written.
///
/// The half of [`pidl_of`] for a name that is not a path — a `shell:` or `::{…}` moniker — which
/// [`wide`]'s slash rewrite has no business touching.
pub(crate) fn parsed_pidl(name: &[u16]) -> Option<Pidl> {
    use windows::Win32::UI::Shell::{Common::ITEMIDLIST, SHParseDisplayName};
    let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated and outlives the call; `raw` is only read when the call
    // succeeded, and is then the guard's.
    let ok = unsafe {
        SHParseDisplayName(windows::core::PCWSTR(name.as_ptr()), None, &mut raw, 0, None).is_ok()
    };
    (ok && !raw.is_null()).then_some(Pidl(raw))
}

/// A path as a shell item, in whichever of its interfaces the caller asks for — `IShellItem`,
/// `IShellItem2`, `IShellItemImageFactory` — through [`wide`], so a path with forward slashes
/// parses.
///
/// # Safety
///
/// The shell's own rules for the calling thread apply; nothing here adds to them.
pub(crate) unsafe fn parse_item<T: windows::core::Interface>(
    path: &std::path::Path,
) -> windows::core::Result<T> {
    let wide = wide(path);
    windows::Win32::UI::Shell::SHCreateItemFromParsingName(
        windows::core::PCWSTR(wide.as_ptr()),
        None,
    )
}

/// `CF_HDROP`, which is a fixed value rather than a registered one, and a number the `windows`
/// crate carries no constant for.
pub(crate) const CF_HDROP: u16 = 15;

/// The `FORMATETC` for format `cf` in an `HGLOBAL`: the content aspect, no target device, every
/// item. The shape every clipboard and drag format in this program is asked for and offered in,
/// bar the per-item contents of a virtual file.
pub(crate) fn hglobal_format(cf: u16) -> windows::Win32::System::Com::FORMATETC {
    windows::Win32::System::Com::FORMATETC {
        cfFormat: cf,
        ptd: std::ptr::null_mut(),
        dwAspect: 1, // DVASPECT_CONTENT
        lindex: -1,
        tymed: windows::Win32::System::Com::TYMED_HGLOBAL.0 as u32,
    }
}

/// Every path a data object offers as `CF_HDROP`. `None` when it offers no such format.
///
/// The clipboard's paste and a drop onto the window read the same format, and both through this.
///
/// # Safety
///
/// `data` is live. The medium is released on the way out, and its handle only read while held.
pub(crate) unsafe fn hdrop_paths(
    data: &windows::Win32::System::Com::IDataObject,
) -> Option<Vec<std::path::PathBuf>> {
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    let medium = data.GetData(&hglobal_format(CF_HDROP)).ok()?;
    let handle = medium.u.hGlobal;
    if handle.is_invalid() {
        return None;
    }

    let drop = HDROP(handle.0);
    // Index `0xFFFF_FFFF` asks how many there are rather than for one of them.
    let count = DragQueryFileW(drop, u32::MAX, None);
    let mut items = Vec::with_capacity(count as usize);
    for index in 0..count {
        // Ask for the length first: a path can be longer than `MAX_PATH`, and a
        // fixed buffer would silently truncate one.
        let len = DragQueryFileW(drop, index, None);
        if len == 0 {
            continue;
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let written = DragQueryFileW(drop, index, Some(&mut buffer));
        if written > 0 {
            buffer.truncate(written as usize);
            items.push(std::path::PathBuf::from(String::from_utf16_lossy(&buffer)));
        }
    }

    // `ReleaseStgMedium` is what frees the block the data object handed over.
    let mut medium = medium;
    windows::Win32::System::Ole::ReleaseStgMedium(&mut medium);
    Some(items)
}

/// Bytes in a moveable global block, as the medium `SetData` and `GetData` trade in.
///
/// **Nothing tracks the handle afterwards, and nothing needs to.** A medium with a null
/// `pUnkForRelease` belongs to whoever it is handed to — `SetData` with `release: true`, or the
/// target of a `GetData` — which frees it with `ReleaseStgMedium`; and the only way out of this
/// function between the allocation and the hand-over is the `GlobalLock` failure below, which frees
/// it on the spot. So there is no third path for an owner to be in doubt about.
pub(crate) fn global_medium(
    bytes: &[u8],
) -> windows::core::Result<windows::Win32::System::Com::STGMEDIUM> {
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    // SAFETY: `GMEM_MOVEABLE` is what a clipboard medium requires, and the size is the slice's.
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len())? };
    // SAFETY: freshly allocated and locked once; the pointer is valid for `bytes.len()`, which
    // is what it was allocated with. Unlocked before the handle escapes.
    unsafe {
        let into = GlobalLock(handle);
        if into.is_null() {
            let _ = windows::Win32::Foundation::GlobalFree(Some(handle));
            return Err(windows::Win32::Foundation::E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), into.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(handle);
    }
    Ok(windows::Win32::System::Com::STGMEDIUM {
        tymed: windows::Win32::System::Com::TYMED_HGLOBAL.0 as u32,
        u: windows::Win32::System::Com::STGMEDIUM_0 { hGlobal: handle },
        pUnkForRelease: std::mem::ManuallyDrop::new(None),
    })
}

/// A single-threaded apartment this thread joined, left again when this goes.
///
/// For the calls that need one only while they run and hand nothing out of it afterwards: a browse
/// of the network, one shell name parsed, one file operation on a thread of its own. Not the UI
/// thread's apartment, which [`crate::shell::init`] enters once and never leaves.
///
/// `S_FALSE` — a thread already in an apartment of this kind — is a join as well and is balanced
/// like one, which is why joining is `is_ok` and leaving is unconditional. Not `Send`, because the
/// leaving has to happen on the thread that joined.
pub(crate) struct Apartment(std::marker::PhantomData<*const ()>);

impl Apartment {
    /// Join, or `None` when this thread cannot — one already in the multithreaded apartment, say.
    pub(crate) fn enter_sta() -> Option<Self> {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        // SAFETY: balanced by the `Drop` below, on this same thread.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .is_ok()
            .then_some(Self(std::marker::PhantomData))
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: this thread joined in `enter_sta`, and this is the one leaving for that join.
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}

/// One class of information about this process's own token — `TokenUser`, `TokenPrivileges` — in a
/// buffer sized by asking first, and aligned for the structure inside it by being `u64`s. `None`
/// when the token will not open or will not say.
pub(crate) fn token_information(
    class: windows_sys::Win32::Security::TOKEN_INFORMATION_CLASS,
) -> Option<Vec<u64>> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: the pseudo-handle needs no closing, and the token is closed on every path out once
    // it is open. The first call asks for the size with no buffer; the second is given at least
    // that many bytes.
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut needed = 0u32;
        GetTokenInformation(token, class, std::ptr::null_mut(), 0, &mut needed);
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        let read = GetTokenInformation(
            token,
            class,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut needed,
        );
        CloseHandle(token);
        (read != 0).then_some(buffer)
    }
}

/// `FSCTL_SET_REPARSE_POINT`, which writes a junction's or a directory link's reparse data onto an
/// open folder. Spelled out because the `windows` crate keeps it behind `Win32_System_Ioctl`, a
/// feature for one number, and two files write reparse points.
pub(crate) const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;

/// A Win32 error as a sentence, without the `(os error 5)` the standard library adds.
///
/// The number is for whoever reads a log, and every message this program shows a person is a
/// sentence of its own; a copy that failed and a junction that could not be made say so the same way.
pub(crate) fn win32_message(code: u32) -> String {
    let text = std::io::Error::from_raw_os_error(code as i32).to_string();
    match text.rfind(" (os error") {
        Some(at) => text[..at].trim_end_matches('.').to_owned(),
        None => text,
    }
}
