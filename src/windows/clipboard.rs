//! `CF_HDROP` and `Preferred DropEffect`: cut, copy and paste, as the shell means them.
//!
//! The Windows half of [`crate::shell::clipboard`].

use super::*;
use windows::core::PCWSTR;
use windows::Win32::System::Com::{IDataObject, FORMATETC, STGMEDIUM, TYMED_HGLOBAL};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{OleGetClipboard, OleSetClipboard};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    SHCreateShellItemArrayFromIDLists, SHParseDisplayName, BHID_DataObject,
};

/// `CF_HDROP`, which is a fixed value rather than a registered one.
const CF_HDROP: u16 = 15;

/// The registered format that says whether a paste should copy or move.
fn preferred_effect_format() -> u16 {
    // SAFETY: registering the same name twice returns the same id, so this is safe
    // to call as often as it is convenient.
    let id = unsafe { RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect")) };
    id as u16
}

/// A PIDL that frees itself.
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by `SHParseDisplayName`, freed exactly once.
            unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
        }
    }
}

fn pidl_of(path: &Path) -> Option<Pidl> {
    let wide = crate::shell::wide(path);
    let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
    // SAFETY: `wide` is null-terminated and outlives the call; `raw` is only read
    // when the call succeeded.
    let ok = unsafe {
        SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut raw, 0, None).is_ok()
    };
    (ok && !raw.is_null()).then_some(Pidl(raw))
}

/// The shell's own data object for these paths, carrying the cut-or-copy flag.
///
/// Separate from [`put`] because it is the part with the decisions in it — the shell's
/// object rather than a hand-rolled `CF_HDROP`, plus the `Preferred DropEffect` block —
/// and because it can then be built and read back in a test without going anywhere near
/// the one clipboard the whole desktop shares.
pub fn data_object(paths: &[PathBuf], effect: Effect) -> Result<IDataObject, String> {
    if paths.is_empty() {
        return Err("Nothing selected".to_owned());
    }

    // Built from the items themselves, so it offers every format Explorer offers —
    // not just the one this program knows about.
    let pidls: Vec<Pidl> = paths.iter().filter_map(|p| pidl_of(p)).collect();
    if pidls.is_empty() {
        return Err("Those items could not be resolved".to_owned());
    }
    let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0 as *const _).collect();

    // SAFETY: the PIDLs outlive the array, which copies what it needs; the data
    // object is reference counted from here on.
    unsafe {
        let array = SHCreateShellItemArrayFromIDLists(&raw)
            .map_err(|e| format!("The shell refused: {}", e.message()))?;
        let data: IDataObject = array
            .BindToHandler(None, &BHID_DataObject)
            .map_err(|e| format!("The shell refused: {}", e.message()))?;

        // Cut or copy. Without this the target has to guess, and guesses copy.
        //
        // `Link` is here for completeness rather than because anything takes this route: nothing
        // in this program puts a link on the clipboard — Ctrl+X and Ctrl+C are the only two callers
        // — and a shortcut is made by [`crate::shell::ops::Job::Link`] rather than announced to
        // somebody else's paste. Spelled out anyway, so that a third caller cannot arrive and have
        // its effect silently read as a copy.
        let value = match effect {
            Effect::Copy => DROPEFFECT_COPY,
            Effect::Move => DROPEFFECT_MOVE,
            Effect::Link => DROPEFFECT_LINK,
        };
        if let Some(medium) = global_dword(value) {
            let format = FORMATETC {
                cfFormat: preferred_effect_format(),
                ptd: std::ptr::null_mut(),
                dwAspect: 1, // DVASPECT_CONTENT
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            };
            // `release: true` hands the block to the data object, which frees it.
            let _ = data.SetData(&format, &medium, true);
        }
        Ok(data)
    }
}

pub fn put(paths: &[PathBuf], effect: Effect) -> Result<(), String> {
    let data = data_object(paths, effect)?;
    // The clipboard is a single global lock, and `OleSetClipboard` fails outright if
    // anything else holds it — a clipboard manager polling it, an Office add-in, the
    // previous owner not having let go yet. Contention is normal and transient, so this
    // retries rather than reporting a failure the user can do nothing about.
    // Microsoft's own guidance for `OpenClipboard` says the same.
    //
    // SAFETY: `data` is a live reference-counted object owned by this scope.
    unsafe {
        retrying(|| OleSetClipboard(&data)).map_err(|_| {
            // Deliberately not the `HRESULT`'s own words: `CLIPBRD_E_CANT_OPEN` renders as
            // "OpenClipboard failed", which tells the user nothing they can act on.
            "Another program is holding the clipboard — try again".to_owned()
        })
    }
}

/// Read a data object the way a paste does — which is all [`get`] is, once the
/// clipboard has handed the object over.
pub fn read(data: &IDataObject) -> Option<Pasteable> {
    // SAFETY: every medium taken below is released by the readers.
    unsafe {
        // The shell first, so a copy taken from inside an archive is not invisible; see
        // the note at the top of this file.
        let items = read_shell_items(data).or_else(|| read_hdrop(data))?;
        if items.is_empty() {
            return None;
        }
        let effect = read_effect(data).unwrap_or(Effect::Copy);
        Some(Pasteable { items, effect })
    }
}

/// Whatever the data object is offering, as parsing names.
///
/// Works for anything the shell can name, which includes things that are not files:
/// an entry inside a `.zip` comes back as `C:\bundle.zip\inner.txt`, and
/// `SHCreateItemFromParsingName` takes that straight back.
unsafe fn read_shell_items(data: &IDataObject) -> Option<Vec<PathBuf>> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        IShellItemArray, SHCreateShellItemArrayFromDataObject, SIGDN_DESKTOPABSOLUTEPARSING,
    };

    let array: IShellItemArray = SHCreateShellItemArrayFromDataObject(data).ok()?;
    let count = array.GetCount().ok()?;
    let mut items = Vec::with_capacity(count as usize);
    for index in 0..count {
        let Ok(shell_item) = array.GetItemAt(index) else {
            continue;
        };
        let Ok(name) = shell_item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING) else {
            continue;
        };
        if !name.is_null() {
            if let Ok(text) = name.to_string() {
                items.push(PathBuf::from(text));
            }
            // The shell allocated it; this frees it.
            CoTaskMemFree(Some(name.0 as *const std::ffi::c_void));
        }
    }
    (!items.is_empty()).then_some(items)
}

/// `CFSTR_PASTESUCCEEDED`, registered the same way the preferred effect is.
fn paste_succeeded_format() -> u16 {
    // SAFETY: registering the same name twice returns the same id.
    unsafe { RegisterClipboardFormatW(windows::core::w!("Paste Succeeded")) as u16 }
}

pub fn cut_pasted(was: u32) {
    // Still the same clipboard? If the user copied something else while the shell was
    // working, that is theirs and this leaves it alone.
    if super::sequence() != was {
        return;
    }
    // SAFETY: the data object is reference counted, and the block handed to `SetData`
    // is released by it.
    unsafe {
        let Ok(data) = retrying(|| OleGetClipboard()) else {
            return;
        };
        if let Some(medium) = global_dword(DROPEFFECT_MOVE) {
            let format = FORMATETC {
                cfFormat: paste_succeeded_format(),
                ptd: std::ptr::null_mut(),
                dwAspect: 1,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            };
            let _ = data.SetData(&format, &medium, true);
        }
        clear();
    }
}

/// Let go of whatever this process has on the clipboard, and answer the calls that come
/// of having had it there.
///
/// For tests only, and it earns its place. Each of them takes over the desktop's one
/// clipboard, and a test that leaves a live data object on it leaves the *next* test in the
/// same process answering calls about it — which is how four clipboard tests that each pass
/// alone managed to fail two at a time when run together. Production never has this
/// problem: one copy per keystroke, and a window pumping between them.
#[cfg(test)]
pub fn settle() {
    clear();
    // SAFETY: answering calls, which is all this does.
    unsafe {
        answering_calls(250);
    }
}

/// Empty the clipboard. A null data object is the documented way.
pub fn clear() {
    // SAFETY: as patient as every other call here, and for the same reason.
    unsafe {
        let _ = retrying(|| OleSetClipboard(None));
    }
}

/// Retry a clipboard call while it is only losing a race.
///
/// The clipboard is one lock shared by the whole desktop, and on a normal Windows install
/// several things take it the instant its contents change — clipboard history first among
/// them. Losing that race is ordinary and transient, and the only sensible answer is to
/// wait and ask again; Microsoft's own guidance for `OpenClipboard` says the same.
///
/// The window was ten attempts over 150 ms, which measurement showed to be too narrow:
/// `probe_consecutive_puts` hammering the clipboard produced `CLIPBRD_E_CANT_OPEN` on the
/// second put and then on every put after it, pumping or not. That matters more than it
/// looks — a *failed* copy leaves the previous contents in place, so the next paste
/// quietly pastes the wrong files, which is the shape of a bug that loses somebody's work.
/// Thirty attempts over about a second outlasts it, and a second is only ever spent when
/// something is genuinely holding on.
unsafe fn retrying<T>(
    mut attempt: impl FnMut() -> windows::core::Result<T>,
) -> windows::core::Result<T> {
    const TRIES: u32 = 30;
    let mut last = attempt();
    for step in 1..TRIES {
        if last.is_ok() {
            return last;
        }
        // Backing off, capped: quick at first, because most contention is over in a
        // millisecond or two, and then patient.
        let wait = (2 + step as u64 * 3).min(60);
        answering_calls(wait);
        last = attempt();
    }
    last
}

/// Waiting that answers what this apartment owes; the reasoning lives on
/// [`crate::shell::answering_calls`], because more than the clipboard depends on it.
///
/// Measured here, by `one_copy_after_another_keeps_working`: sleeping between retries
/// refused eight to eleven of twelve copies made 200 ms apart -- about how long Windows'
/// clipboard history takes to come asking -- and refused nothing at all when they came back
/// to back, before it had started. Answering during the wait: nought out of forty-eight.
unsafe fn answering_calls(ms: u64) {
    crate::shell::answering_calls(ms);
}

/// A `DWORD` in a moveable global block, which is what `SetData` wants.
unsafe fn global_dword(value: u32) -> Option<STGMEDIUM> {
    let handle = GlobalAlloc(GMEM_MOVEABLE, 4).ok()?;
    let locked = GlobalLock(handle);
    if locked.is_null() {
        return None;
    }
    std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), locked.cast(), 4);
    let _ = GlobalUnlock(handle);
    Some(STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: windows::Win32::System::Com::STGMEDIUM_0 {
            hGlobal: handle,
        },
        pUnkForRelease: std::mem::ManuallyDrop::new(None),
    })
}

pub fn has_files() -> bool {
    use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
    // SAFETY: a pure query; it opens nothing and allocates nothing.
    unsafe { IsClipboardFormatAvailable(CF_HDROP as u32).is_ok() }
}

pub fn get() -> Option<Pasteable> {
    // The whole read is retried, not just getting hold of the object. Getting the object
    // is a lock; *reading* it is a call into whoever owns it, and that can fail on its own
    // — measured, when Windows' clipboard history happened to be rendering the same object
    // at the same moment. Retrying only the first half left a paste reporting "there are
    // no files on the clipboard" for files that were plainly on it.
    //
    // SAFETY: the data object is reference counted, and `read` releases every medium it
    // takes.
    unsafe {
        for step in 0..READ_TRIES {
            if let Ok(data) = OleGetClipboard() {
                if let Some(pasteable) = read(&data) {
                    return Some(pasteable);
                }
            }
            // An empty clipboard is an answer, not a race, and waiting a second to say so
            // would make Ctrl+V feel broken. `IsClipboardFormatAvailable` takes no lock.
            if !has_files() {
                return None;
            }
            answering_calls((2 + step * 3).min(60));
        }
        None
    }
}

/// How many times a read is worth trying. Fewer than a write: a paste that cannot read the
/// clipboard has a sensible thing to say, whereas a copy that cannot write it has left the
/// *previous* contents in place and the next paste would use them.
const READ_TRIES: u64 = 12;

unsafe fn read_hdrop(data: &IDataObject) -> Option<Vec<PathBuf>> {
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    let format = FORMATETC {
        cfFormat: CF_HDROP,
        ptd: std::ptr::null_mut(),
        dwAspect: 1,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let medium = data.GetData(&format).ok()?;
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
            items.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
        }
    }

    // `ReleaseStgMedium` is what frees the block the clipboard handed over.
    let mut medium = medium;
    windows::Win32::System::Ole::ReleaseStgMedium(&mut medium);
    Some(items)
}

unsafe fn read_effect(data: &IDataObject) -> Option<Effect> {
    let format = FORMATETC {
        cfFormat: preferred_effect_format(),
        ptd: std::ptr::null_mut(),
        dwAspect: 1,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let medium = data.GetData(&format).ok()?;
    let handle = medium.u.hGlobal;
    let mut effect = None;
    if !handle.is_invalid() {
        let locked = GlobalLock(handle);
        if !locked.is_null() {
            let mut bytes = [0u8; 4];
            std::ptr::copy_nonoverlapping(locked.cast::<u8>(), bytes.as_mut_ptr(), 4);
            let value = u32::from_le_bytes(bytes);
            // A source may offer both; a move is the more specific intent, so it
            // wins. This is what Explorer does with the same pair of bits.
            effect = Some(if value & DROPEFFECT_MOVE != 0 {
                Effect::Move
            } else {
                Effect::Copy
            });
            let _ = GlobalUnlock(handle);
        }
    }
    let mut medium = medium;
    windows::Win32::System::Ole::ReleaseStgMedium(&mut medium);
    effect
}
