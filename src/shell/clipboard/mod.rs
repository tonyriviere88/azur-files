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

/// What a paste or a drop is asking for.
///
/// The three `DROPEFFECT` values that mean something, which is why this is here rather than in
/// [`crate::shell::dnd`]: the clipboard and a drag are the same question asked twice, and Windows
/// answers both with the same word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    Copy,
    Move,
    /// Make a shortcut to each item rather than a copy of it — the Alt-drag, and Explorer's
    /// *Create shortcuts here*. See [`crate::shell::ops::Job::Link`].
    ///
    /// **Never comes off the clipboard.** `Preferred DropEffect` can hold `DROPEFFECT_LINK` in
    /// principle and nothing puts it there in practice, so [`get`] answers `Copy` for it — see
    /// `win::read_effect`. This variant exists for the drag half, where it is the whole point.
    Link,
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
#[cfg(windows)]
const DROPEFFECT_LINK: u32 = windows::Win32::System::Ole::DROPEFFECT_LINK.0;

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
#[path = "../../windows/clipboard.rs"]
mod win;

#[cfg(all(test, windows))]
mod tests;
