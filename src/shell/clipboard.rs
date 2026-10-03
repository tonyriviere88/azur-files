//! Cut, copy and paste — on the same clipboard Explorer uses.
//!
//! Interoperability is the whole point: files cut here paste into Explorer, files
//! copied in Explorer paste here, and both work with 7-Zip, Notepad++, every Office
//! application and anything else that speaks the shell's clipboard. That means using
//! the shell's own formats rather than inventing one.
//!
//! Two formats carry it:
//!
//! - **`CF_HDROP`** — a `DROPFILES` header followed by the paths as wide strings, one
//!   after another, with an extra terminator at the end. This is the format that has
//!   meant "these files" since Windows 3.1 and every target understands it.
//! - **`"Preferred DropEffect"`** — a registered format holding a single `DWORD`.
//!   `DROPEFFECT_COPY` means copy; `DROPEFFECT_MOVE` means **cut**. There is no
//!   separate "cut" clipboard: a cut is a copy that asks the *target* to move. Which
//!   is also why a cut with no paste leaves the files exactly where they were.
//!
//! The data object is built by the shell itself, from the items' PIDLs, so it carries
//! `CFSTR_SHELLIDLIST` and the rest of the formats Explorer offers alongside
//! `CF_HDROP` — a target that prefers one of those gets it.
//!
//! # Reading one back is not reading `CF_HDROP`
//!
//! `CF_HDROP` can only carry paths in the file system, and not everything Explorer lets you
//! copy is one. Copy a file out of a `.zip` and the data object it offers is
//! `Shell IDList Array`, `FileGroupDescriptorW` and `FileContents` — measured, and with no
//! `CF_HDROP` at all — so a paste that only knows `CF_HDROP` reads nothing and does nothing.
//!
//! So a paste asks the shell instead: `SHCreateShellItemArrayFromDataObject` turns any of
//! those into items, and each item's `SIGDN_DESKTOPABSOLUTEPARSING` name —
//! `C:\bundle.zip\inner.txt` for the archive case — goes straight back into
//! `SHCreateItemFromParsingName` on the other side. `IFileOperation` then extracts it exactly
//! as Explorer does. `CF_HDROP` stays as the fallback, for a source that offers only that.
//!
//! # The end of a cut
//!
//! A paste that *moved* has to say so. `CFSTR_PASTESUCCEEDED` is how the source finds out:
//! it is what makes Explorer stop showing the items faded, and what lets a source whose
//! items exist only while it holds them let go of them. Then the clipboard is emptied,
//! because a cut that has been pasted must not be pastable twice — the files are no longer
//! where it says they are. See [`cut_pasted`], and note that it happens when the operation
//! *finishes*: doing it when the paste starts loses the cut for anyone who answers the
//! conflict dialog with Cancel.

use std::path::{Path, PathBuf};

/// What the clipboard is asking a paste to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    Copy,
    Move,
}

/// What is on the clipboard, if it is files.
pub struct Pasteable {
    pub items: Vec<PathBuf>,
    pub effect: Effect,
}

/// `DROPEFFECT_COPY` and `DROPEFFECT_MOVE`, which are the values in the preferred effect and
/// are also what a drag reports.
///
/// Windows numbers them `NONE = 0, COPY = 1, MOVE = 2, LINK = 4` — copy first. These were
/// written as `1 << 1` and `1 << 0`, which is the same pair of bits the other way round, and the
/// consequence was not subtle: a copy taken here announced itself to every other program as a
/// **cut**, a cut announced itself as a copy, and reading somebody else’s copy came back as a
/// move. Copy a file in Explorer, paste it here, and the original was *gone*.
///
/// Taken from the Win32 headers now rather than written out, so they cannot be transposed again.
#[cfg(windows)]
const DROPEFFECT_COPY: u32 = windows::Win32::System::Ole::DROPEFFECT_COPY.0;
#[cfg(windows)]
const DROPEFFECT_MOVE: u32 = windows::Win32::System::Ole::DROPEFFECT_MOVE.0;

/// Put files on the clipboard.
///
/// `Effect::Move` is a cut: nothing is moved here, and nothing will be unless
/// something pastes.
pub fn put(paths: &[PathBuf], effect: Effect) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::put(paths, effect)
    }
    #[cfg(not(windows))]
    {
        let _ = (paths, effect);
        Err("The clipboard is implemented against the Windows shell only".to_owned())
    }
}

/// Whether there is anything a paste could act on.
///
/// Cheap enough to ask once a frame, which is what greys out the menu entry.
pub fn has_files() -> bool {
    #[cfg(windows)]
    {
        win::has_files()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Read the files off the clipboard, and what to do with them.
pub fn get() -> Option<Pasteable> {
    #[cfg(windows)]
    {
        win::get()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// [`win::settle`], for the tests outside this module.
#[cfg(test)]
pub fn settle_for_tests() {
    #[cfg(windows)]
    win::settle();
}

/// How many times the clipboard has changed, which is how to tell whether it is still the
/// one you were looking at.
///
/// Cheap, and it takes no lock at all.
pub fn sequence() -> u32 {
    #[cfg(windows)]
    {
        use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
        // SAFETY: a pure query.
        unsafe { GetClipboardSequenceNumber() }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Finish a cut whose paste has just succeeded: tell the source, then empty the clipboard.
///
/// `was` is the sequence number from when the paste started. If the clipboard has changed
/// since — the user copied something else while the shell was still working — nothing happens,
/// because emptying *that* would be this program throwing away data it was never given.
///
/// Identified by sequence number rather than by comparing the items, which is what this did
/// first and what did not work: a *successful* move leaves the clipboard naming files that are
/// no longer there, the data object can no longer render them, and the comparison that was
/// supposed to prove "still the same cut" instead read nothing and concluded "something else".
/// So the cut stayed on the clipboard after being pasted, ready to move files that had already
/// moved. The sequence number is a fact about the clipboard rather than about the files.
pub fn cut_pasted(was: u32) {
    #[cfg(windows)]
    {
        win::cut_pasted(was);
    }
    #[cfg(not(windows))]
    let _ = was;
}

/// Empty the clipboard, retrying like every other call here.
///
/// It was `let _ = OleSetClipboard(None)`, and the swallowed error was not academic: emptying
/// the clipboard at the end of a cut lost the same race everything else here loses, silently,
/// so a cut that had been pasted stayed on the clipboard and Ctrl+V would move files that had
/// already moved. Found by the end-to-end test, which asserted the clipboard was empty and
/// found it was not.
///
/// Only the tests reach for this directly; the program empties the clipboard as the last step
/// of finishing a cut, inside [`cut_pasted`].
#[cfg(test)]
pub fn clear() {
    #[cfg(windows)]
    {
        win::clear();
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Com::{
        IDataObject, DATADIR_GET, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
    };
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
            let value = match effect {
                Effect::Copy => DROPEFFECT_COPY,
                Effect::Move => DROPEFFECT_MOVE,
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
        let _ = DATADIR_GET;
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
        let _ = HANDLE::default();
        effect
    }

    /// The interface id lookup the array bind needs, kept honest.
    #[allow(dead_code)]
    fn data_object_iid() -> windows::core::GUID {
        IDataObject::IID
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A round trip through the real clipboard, which is the only test worth having
    /// here: it is interoperability that matters, and interoperability with a mock is
    /// not a fact about anything.
    /// The data object a cut or a copy hands over: the shell's own, with `CF_HDROP` and
    /// the `Preferred DropEffect` block that tells the target which of the two it was.
    ///
    /// Built and read back directly rather than through the clipboard. The clipboard is one
    /// object shared by every process on the desktop — Windows' own history service reads
    /// each new item the moment it lands — and a test that goes through it is a test that
    /// fails one run in four for reasons that have nothing to do with this program. What is
    /// worth checking is the object, and that needs no lock at all.
    #[test]
    #[cfg(windows)]
    fn the_data_object_carries_the_files_and_the_effect() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();

        let here = crate::sandbox::dir("clip");
        std::fs::create_dir_all(&here).expect("temp dir");
        let one = here.join("one.txt");
        let two = here.join("two.txt");
        std::fs::write(&one, b"1").expect("write");
        std::fs::write(&two, b"2").expect("write");

        for effect in [Effect::Copy, Effect::Move] {
            let data = win::data_object(&[one.clone(), two.clone()], effect)
                .expect("the shell should build a data object for two real files");
            let read = win::read(&data).expect("and it should read back as files");
            assert_eq!(
                read.effect, effect,
                "a cut has to come back as a cut, or a paste would copy when it should move"
            );
            assert_eq!(read.items.len(), 2);
            // The shell normalises the case of what it hands back, so compare that way.
            let names: Vec<String> = read
                .items
                .iter()
                .map(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default()
                })
                .collect();
            assert!(names.contains(&"one.txt".to_owned()), "{names:?}");
            assert!(names.contains(&"two.txt".to_owned()), "{names:?}");
        }

        assert!(
            win::data_object(&[], Effect::Copy).is_err(),
            "and nothing is not something to put on a clipboard"
        );
        crate::sandbox::remove(&here);
    }

    /// The same, but through the real clipboard, which is what a paste into Explorer
    /// actually uses.
    ///
    /// Ignored by default: it needs the desktop's one clipboard to stay still for a moment,
    /// and nothing on a working machine promises that. Run it on purpose:
    ///
    /// ```text
    /// cargo test -- --ignored the_real_clipboard
    /// ```
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn the_real_clipboard_round_trips() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        win::settle();

        let here = crate::sandbox::dir("clip-real");
        std::fs::create_dir_all(&here).expect("temp dir");
        let one = here.join("one.txt");
        std::fs::write(&one, b"1").expect("write");

        put(std::slice::from_ref(&one), Effect::Move).expect("put on the clipboard");
        assert!(has_files(), "the clipboard should be offering files");
        let read = get().expect("read back off the clipboard");
        assert_eq!(read.effect, Effect::Move);
        assert_eq!(read.items.len(), 1);

        clear();
        crate::sandbox::remove(&here);
    }

    /// A zip holding one stored `inner.txt`, so the test that needs a non-file shell item
    /// need no archiver.
    ///
    /// Made by `zipfile` and pasted in: a hand-written one had a bad central directory, which
    /// Windows treats as a plain file rather than a folder -- and the probe that found this
    /// duly reported that the shell could not resolve a path inside a zip.
    #[cfg(all(test, windows))]
    const ZIP_WITH_ONE_ENTRY: &[u8] = &[
        0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0xb2, 0x8e, 0x03, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x69, 0x6e, 0x6e, 0x65, 0x72, 0x2e, 0x74, 0x78, 0x74, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0xb2, 0x8e, 0x03, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00, 0x69, 0x6e, 0x6e, 0x65, 0x72, 0x2e, 0x74, 0x78, 0x74, 0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x37, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// Copying something that is not a file: an entry inside a `.zip`.
    ///
    /// This is the case that made a paste read the shell rather than `CF_HDROP`. Explorer
    /// offers `Shell IDList Array`, `FileGroupDescriptorW` and `FileContents` for an archive
    /// entry and no `CF_HDROP` at all, so a paste that only knew `CF_HDROP` read nothing back
    /// and did nothing at all -- copy a file out of a zip in Explorer, press Ctrl+V here, and
    /// the answer was silence.
    ///
    /// What is asserted is the whole way through: the data object reads back as the item, and
    /// the name it reads back as is one the shell can resolve again, which is what lets
    /// `IFileOperation` extract it.
    #[test]
    #[cfg(windows)]
    fn an_entry_inside_a_zip_reads_back_as_something_pasteable() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("zip");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        let zip = root.join("bundle.zip");
        std::fs::write(&zip, ZIP_WITH_ONE_ENTRY).expect("write the zip");
        let inside = zip.join("inner.txt");

        let data = win::data_object(std::slice::from_ref(&inside), Effect::Copy)
            .expect("the shell should build a data object for an item inside an archive");
        let read = win::read(&data).expect(
            "and a paste should read it back -- if this is None, the read has gone back to \
             `CF_HDROP`, which an archive entry does not offer",
        );
        assert_eq!(read.items.len(), 1, "{:?}", read.items);

        // The name has to be one the shell can resolve, since that is what the copy engine
        // is handed on the other side.
        let name = &read.items[0];
        assert!(
            name.to_string_lossy().to_lowercase().contains("bundle.zip"),
            "expected a path through the archive, got {}",
            name.display()
        );
        // SAFETY: a pure lookup; nothing is retained.
        unsafe {
            assert!(
                crate::shell::ops::item(name).is_ok(),
                "the shell cannot resolve {} back, so a paste of it would fail",
                name.display()
            );
        }

        crate::sandbox::remove(&root);
    }

    /// Interoperability, across a process boundary, in both directions.
    ///
    /// The claim worth checking is not that this program can read its own clipboard -- it is
    /// that *another* process sees what it puts there, and that it sees what another process
    /// puts. PowerShell stands in for Explorer: `Set-Clipboard -Path` writes `CF_HDROP` the
    /// same way a copy in a folder window does, and `Get-Clipboard -Format FileDropList`
    /// reads it back the same way a paste does.
    ///
    /// Ignored by default, because it takes over the desktop's one clipboard. Run it on
    /// purpose:
    ///
    /// ```text
    /// cargo test -- --ignored --test-threads=1 explorer_and_this_program
    /// ```
    #[test]
    #[ignore = "takes over the real clipboard; run explicitly"]
    #[cfg(windows)]
    fn explorer_and_this_program_read_each_other_s_clipboard() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        win::settle();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("interop");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        let one = root.join("one.txt");
        let two = root.join("two.txt");
        std::fs::write(&one, b"1").expect("write");
        std::fs::write(&two, b"2").expect("write");

        /// Run a snippet of PowerShell in its own STA, pumping messages while it runs.
        ///
        /// The pumping is not incidental. `OleSetClipboard` does not copy anything: it leaves
        /// the clipboard holding a reference to the data object *in this process*, and another
        /// process asking for the bytes is a marshalled call back into this apartment, which
        /// arrives as a window message. A thread that is not dispatching messages therefore
        /// hands out nothing at all -- which is exactly what the first version of this test
        /// measured, and it would have been wrong to conclude from it that interoperability
        /// was broken. The real window pumps continuously.
        fn powershell(script: &str) -> String {
            use windows::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
            };

            let mut child = std::process::Command::new("powershell")
                .args(["-NoProfile", "-STA", "-Command", script])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .expect("powershell should be on the path");

            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while std::time::Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                // SAFETY: a plain pump over this thread's own queue.
                unsafe {
                    let mut message = MSG::default();
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            let out = child.wait_with_output().expect("powershell finished");
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }

        // ---- this program copies, another process pastes ----
        put(&[one.clone(), two.clone()], Effect::Copy).expect("put on the clipboard");
        let seen = powershell(
            "(Get-Clipboard -Format FileDropList | ForEach-Object { $_.Name }) -join ','",
        );
        assert!(
            seen.to_lowercase().contains("one.txt") && seen.to_lowercase().contains("two.txt"),
            "another process read `{seen}` off the clipboard, not the two files put there"
        );

        // ---- another process copies, this program pastes ----
        let script = format!(
            "Set-Clipboard -Path '{}','{}'",
            one.display(),
            two.display()
        );
        powershell(&script);
        let read = get().expect("this program should read a clipboard another process wrote");
        assert_eq!(read.effect, Effect::Copy, "no preferred effect means copy");
        let mut names: Vec<String> = read
            .items
            .iter()
            .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_lowercase())
            .collect();
        names.sort();
        assert_eq!(names, ["one.txt", "two.txt"], "{:?}", read.items);

        clear();
        crate::sandbox::remove(&root);
    }

    /// The end of a cut: the clipboard is emptied, and only when it is still the same cut.
    #[test]
    #[ignore = "takes over the real clipboard; run explicitly"]
    #[cfg(windows)]
    fn a_cut_that_has_been_pasted_empties_the_clipboard() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        win::settle();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("cut");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        let one = root.join("one.txt");
        let other = root.join("other.txt");
        std::fs::write(&one, b"1").expect("write");
        std::fs::write(&other, b"2").expect("write");

        // A clipboard that has changed since the paste began is not this program's to throw
        // away, however much it looks like the one it was given.
        put(std::slice::from_ref(&one), Effect::Move).expect("put");
        let stale = sequence();
        put(std::slice::from_ref(&other), Effect::Move).expect("put something else");
        cut_pasted(stale);
        assert!(
            has_files(),
            "the clipboard moved on between the paste and its finish, and this emptied it \
             anyway -- that is somebody else's data"
        );

        // The real thing: the same clipboard the paste was given.
        put(std::slice::from_ref(&one), Effect::Move).expect("put");
        let ours = sequence();
        assert!(has_files());
        cut_pasted(ours);
        assert!(
            !has_files(),
            "a cut that has been pasted has to leave the clipboard empty, or Ctrl+V would \
             move files that are no longer there"
        );

        clear();
        crate::sandbox::remove(&root);
    }

    /// Puts two files on the clipboard and returns, so the process exits with them on it.
    ///
    /// Half of a test: the other half is another process reading the clipboard afterwards.
    /// Driven from the shell, because what is being checked is what survives *this* program
    /// closing, and that cannot be checked from inside it:
    ///
    /// ```text
    /// cargo test --release -- --ignored --exact \
    ///   shell::clipboard::tests::leaves_a_copy_behind_and_exits
    /// powershell -NoProfile -STA -Command "Get-Clipboard -Format FileDropList"
    /// ```
    #[test]
    #[ignore = "half of a cross-process check; see the note"]
    #[cfg(windows)]
    fn leaves_a_copy_behind_and_exits() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("survives");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        let one = root.join("survivor.txt");
        std::fs::write(&one, b"1").expect("write");

        put(std::slice::from_ref(&one), Effect::Copy).expect("put on the clipboard");
        if std::env::var_os("YAFE_NO_FLUSH").is_none() {
            crate::shell::flush();
        }
        // Deliberately leaves the file: the reader on the other side names it.
    }

    /// One copy after another has to keep working, at every cadence.
    ///
    /// This is the regression test for the least obvious bug in this file. A copy leaves the
    /// clipboard holding a reference to a data object *here*, so Windows' clipboard history
    /// comes asking for the bytes a couple of hundred milliseconds later -- and it asks while
    /// holding the clipboard. A thread that does not answer that call leaves the lock taken and
    /// the next copy refused, and the `HRESULT` for it says `OpenClipboard failed`, which reads
    /// like somebody else's fault.
    ///
    /// The shape is what gives it away, and it is the shape asserted here: back-to-back copies
    /// were fine, because the history service had not started yet, and copies 200 ms apart
    /// failed eight to eleven times in twelve. So a version of this test that only hammered
    /// would pass against the bug. `answering_calls` is the fix; see the note on it.
    ///
    /// Ignored because it takes over the desktop's one clipboard.
    #[test]
    #[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
    #[cfg(windows)]
    fn one_copy_after_another_keeps_working() {
        let _serialised = crate::shell::serialised();
        crate::shell::init();
        win::settle();

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sandbox")
            .join("consecutive");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("sandbox");
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        std::fs::write(&a, b"a").expect("write");
        std::fs::write(&b, b"b").expect("write");

        // 200 ms is the one that mattered, so it is in the list; the others are there because
        // a fix that only worked at one cadence would not be a fix.
        for gap in [0u64, 20, 50, 200] {
            const TIMES: usize = 8;
            for step in 0..TIMES {
                let which = if step % 2 == 0 { &a } else { &b };
                put(std::slice::from_ref(which), Effect::Copy).unwrap_or_else(|why| {
                    panic!("copy {step} of {TIMES}, {gap} ms apart, was refused: {why}")
                });
                let read = get().unwrap_or_else(|| {
                    panic!("copy {step} of {TIMES}, {gap} ms apart, read back as nothing")
                });
                assert_eq!(read.items.len(), 1);
                std::thread::sleep(std::time::Duration::from_millis(gap));
            }
        }

        clear();
        crate::sandbox::remove(&root);
    }

    #[test]
    fn putting_nothing_is_refused_rather_than_clearing() {
        let _serialised = crate::shell::serialised();
        assert!(put(&[], Effect::Copy).is_err());
    }
}
