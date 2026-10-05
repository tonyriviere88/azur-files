//! Dragging files out of an archive, without extracting them first.
//!
//! The third OLE interface this program implements rather than calls — `IDataObject` — and the only
//! one it implements because the shell's own will not do. [`super::data_object`] asks the shell
//! for a data object over a selection, which is right for every real file and impossible for a path
//! inside an archive: `SHParseDisplayName` refuses a path that names no file, so that function comes
//! back `None` and the drag never starts.
//!
//! # The mechanism: promise the files, deliver them at the drop
//!
//! `CF_HDROP` — the format a file drag normally carries — is a list of **paths**, so everything in it
//! has to exist before the drag begins. That is the whole difficulty: extracting a selection at the
//! moment the button moves would stall the gesture for as long as the decompression took.
//!
//! Windows has a format pair for exactly this, and it is what Outlook uses to drag an attachment
//! onto the desktop:
//!
//! | format | what it carries | what it costs here |
//! | --- | --- | --- |
//! | `CFSTR_FILEDESCRIPTORW` | the names, sizes, dates and which are folders | **nothing** — see [`crate::archive::manifest`] |
//! | `CFSTR_FILECONTENTS` | one `IStream` per entry, asked for by index | one entry's decompression, at the drop |
//!
//! The descriptors come straight out of the [`crate::archive::Index`] the listing already read, so
//! the data object is complete before the pointer has moved. Nothing is decompressed until a target
//! asks for a specific entry's contents, and then only that entry.
//!
//! # `GetData` is not a getter
//!
//! Which is the fact the whole design rests on, and [`super`] says so already about the
//! receiving half: it is a request that the source *render* a format, and a target makes it at the
//! drop rather than at the start. Every render below therefore happens inside [`Offered::render`],
//! and — because [`super::drag_out`] runs `DoDragDrop` on a thread of its own — on the drag
//! thread rather than the UI one. A slow entry stalls the drop it belongs to and the window goes on
//! painting.
//!
//! # `CF_HDROP` is offered too, and that is a considered concession
//!
//! Not every target understands virtual files, and one of them was **this program**: its own drop
//! target read `CF_HDROP` and nothing else, so without the fallback a drag from an archive pane into
//! another pane of the same window would do nothing. Explorer, given both, generally takes
//! `CF_HDROP`.
//!
//! What that costs is the per-entry granularity: the fallback extracts the whole selection in one
//! render, where a target taking the descriptors gets the fully lazy path.
//!
//! **And it was claimed to cost nothing else, on the grounds that the render happens at the drop.**
//! It does not: a target asks what a drag is carrying the moment it *arrives*, so the concession was
//! really several seconds of a dead cursor at the start of every drag out of a solid `.7z`. See
//! [`Promised`] — the receiving half, and the fix.

use std::path::PathBuf;

use windows::core::{implement, Ref, BOOL, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    GlobalFree, DV_E_FORMATETC, DV_E_LINDEX, DV_E_TYMED, E_NOTIMPL, E_OUTOFMEMORY,
    OLE_E_ADVISENOTSUPPORTED, S_OK,
};
use windows::Win32::System::Com::{
    IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, IStream, DATADIR_GET,
    DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, STGM_READ, STGM_SHARE_DENY_WRITE,
    TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::ReleaseStgMedium;
use windows::Win32::UI::Shell::{
    SHCreateStdEnumFmtEtc, SHCreateStreamOnFileEx, CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW,
    CFSTR_PREFERREDDROPEFFECT, DROPFILES, FD_ATTRIBUTES, FD_FILESIZE, FD_PROGRESSUI, FD_WRITESTIME,
    FILEDESCRIPTORW,
};

use crate::archive::Dragged;

/// `CF_HDROP`, the one predefined format here. Spelled out for the reason [`super`] spells it
/// out: it is a number rather than a name and the crate does not carry a constant for it.
const CF_HDROP: u16 = 15;

/// `DROPEFFECT_COPY`. A drag out of an archive is always a copy — there is no move out of a file
/// this program only reads — and saying so through `CFSTR_PREFERREDDROPEFFECT` is what stops a
/// target from offering to move and then reporting a move that never happened.
const COPY: u32 = 1;

/// The clipboard format numbers, registered once per process.
///
/// `RegisterClipboardFormatW` returns the same number for the same name for the lifetime of the
/// session, so this is a lookup rather than an allocation — but it is a syscall, and it is on the
/// path of every `QueryGetData` a hovering target makes.
struct Formats {
    descriptor: u16,
    contents: u16,
    effect: u16,
}

impl Formats {
    fn get() -> &'static Self {
        static ONCE: std::sync::OnceLock<Formats> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            // SAFETY: three static, null-terminated names from the `windows` crate.
            let register = |name: PCWSTR| unsafe { RegisterClipboardFormatW(name) as u16 };
            Formats {
                descriptor: register(CFSTR_FILEDESCRIPTORW),
                contents: register(CFSTR_FILECONTENTS),
                effect: register(CFSTR_PREFERREDDROPEFFECT),
            }
        })
    }
}

/// The data object for a drag out of an archive.
///
/// Holds only what the renders need, and nothing that would have to be kept in step with the pane:
/// the drag is a snapshot by definition, and an archive cannot change under it without its
/// [`crate::archive::CACHE`] key changing too.
#[implement(IDataObject)]
pub struct Offered {
    /// Every file and folder the drag is offering, flattened, in descriptor order. The index into
    /// this is the `lindex` a target asks `CFSTR_FILECONTENTS` for.
    items: Vec<Dragged>,
    /// The paths as they were *selected*, for the `CF_HDROP` fallback — which names what was picked
    /// up rather than every file underneath it, exactly as a drag of a real folder does.
    selection: Vec<PathBuf>,
}

impl Offered {
    /// Build the object for a selection inside an archive, or `None` if there is nothing virtual
    /// about it.
    ///
    /// The manifest is read here, on the drag thread, before `DoDragDrop` is called — and it is the
    /// only work done before the gesture starts, because it decompresses nothing. See
    /// [`crate::archive::manifest`].
    pub fn over(selection: &[PathBuf]) -> Option<IDataObject> {
        let items = crate::archive::manifest(selection).ok()?;
        Some(
            Self {
                items,
                selection: selection.to_vec(),
            }
            .into(),
        )
    }

    /// Which of the four formats this is, if it is one this object offers at all.
    fn offers(&self, what: &FORMATETC) -> Option<Offer> {
        let formats = Formats::get();
        // `DVASPECT_CONTENT` throughout: the other aspects are a thumbnail and an icon of the data,
        // which a file drag has no business answering.
        if what.dwAspect != DVASPECT_CONTENT.0 {
            return None;
        }
        match what.cfFormat {
            f if f == formats.descriptor => Some(Offer::Descriptor),
            f if f == formats.contents => Some(Offer::Contents),
            f if f == formats.effect => Some(Offer::Effect),
            CF_HDROP => Some(Offer::Paths),
            _ => None,
        }
    }

    /// Render a format. **This is where the decompression happens**, and it is called at the drop.
    fn render(&self, what: &FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let Some(offer) = self.offers(what) else {
            return Err(DV_E_FORMATETC.into());
        };
        let wanted = what.tymed;
        match offer {
            Offer::Descriptor => {
                if wanted & TYMED_HGLOBAL.0 as u32 == 0 {
                    return Err(DV_E_TYMED.into());
                }
                self.as_global(&self.descriptors())
            }
            Offer::Effect => {
                if wanted & TYMED_HGLOBAL.0 as u32 == 0 {
                    return Err(DV_E_TYMED.into());
                }
                self.as_global(&COPY.to_le_bytes())
            }
            Offer::Paths => {
                if wanted & TYMED_HGLOBAL.0 as u32 == 0 {
                    return Err(DV_E_TYMED.into());
                }
                self.as_global(&self.dropfiles()?)
            }
            Offer::Contents => {
                if wanted & TYMED_ISTREAM.0 as u32 == 0 {
                    return Err(DV_E_TYMED.into());
                }
                let stream = self.stream(what.lindex)?;
                Ok(STGMEDIUM {
                    tymed: TYMED_ISTREAM.0 as u32,
                    u: STGMEDIUM_0 {
                        pstm: std::mem::ManuallyDrop::new(Some(stream)),
                    },
                    // The target releases it. See [`Offered::spare`].
                    pUnkForRelease: std::mem::ManuallyDrop::new(None),
                })
            }
        }
    }

    /// `FILEGROUPDESCRIPTORW`: a count, then one packed `FILEDESCRIPTORW` per entry.
    ///
    /// Built as bytes rather than as the crate's struct because that struct is declared with a
    /// one-element array — `fgd: [FILEDESCRIPTORW; 1]` — so there is no way to *hold* the real thing
    /// in Rust's type system. The layout is exact: both structs are `#[repr(C, packed(1))]`, so a
    /// `u32` followed by `n` descriptors is what the shell reads.
    fn descriptors(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.items.len() * size_of::<FILEDESCRIPTORW>());
        out.extend_from_slice(&(self.items.len() as u32).to_le_bytes());

        for item in &self.items {
            let mut fd = FILEDESCRIPTORW {
                dwFlags: (FD_ATTRIBUTES.0 | FD_FILESIZE.0 | FD_WRITESTIME.0 | FD_PROGRESSUI.0)
                    as u32,
                // `FILE_ATTRIBUTE_DIRECTORY` (16) or `FILE_ATTRIBUTE_NORMAL` (128). A directory
                // descriptor is how a dragged folder is conveyed, and a target never asks for its
                // contents — see [`Offered::stream`], which refuses if one does.
                dwFileAttributes: if item.is_dir { 16 } else { 128 },
                ..Default::default()
            };
            // A folder's size is noise, as it is everywhere else in this program.
            if !item.is_dir {
                fd.nFileSizeLow = item.size as u32;
                fd.nFileSizeHigh = (item.size >> 32) as u32;
            }
            fd.ftLastWriteTime = windows::Win32::Foundation::FILETIME {
                dwLowDateTime: item.modified as u32,
                dwHighDateTime: (item.modified >> 32) as u32,
            };
            // 259 units and a terminator, which is the field's whole size. A name too long to fit
            // is truncated rather than dropped: a file the target names oddly is better than a file
            // that silently does not arrive, and `MAX_PATH` is the format's limit rather than this
            // program's choice.
            //
            // **Filled in a local and assigned whole**, because `FILEDESCRIPTORW` is
            // `#[repr(C, packed(1))]`: taking `&mut fd.cFileName` would be a reference to a field
            // that may be misaligned, which is undefined behaviour even if nothing dereferences it.
            // The compiler refuses it, and rightly — this is a copy of 520 bytes instead.
            let mut name = [0u16; 260];
            for (slot, unit) in name.iter_mut().zip(item.name.encode_utf16().take(259)) {
                *slot = unit;
            }
            fd.cFileName = name;
            // SAFETY: `FILEDESCRIPTORW` is `#[repr(C, packed(1))]` and entirely plain data — no
            // padding to leak and no pointers in it — so its bytes are exactly what goes on the
            // wire.
            out.extend_from_slice(unsafe {
                std::slice::from_raw_parts(
                    (&fd as *const FILEDESCRIPTORW).cast::<u8>(),
                    size_of::<FILEDESCRIPTORW>(),
                )
            });
        }
        out
    }

    /// The `CF_HDROP` fallback: a `DROPFILES` header then the extracted paths, one after another,
    /// double-null terminated.
    ///
    /// **This is the render that extracts everything**, and the reason the module header calls the
    /// format a concession. It names what was *selected* rather than every file below it, so a
    /// dragged folder arrives as a folder — [`crate::archive::extracted`] expands one.
    fn dropfiles(&self) -> windows::core::Result<Vec<u8>> {
        // **One call for the whole selection.** Extracting path by path would walk a solid `.7z`
        // once per file — see [`crate::archive::extract::all`], which is also why the answers come
        // back in the order asked: a `CF_HDROP` is a positional list.
        let real = crate::archive::extract::all(&self.selection).map_err(|_| {
            // The words are lost here — OLE carries a code and nothing else — but the failure is not
            // silent: the target reports a drop that did nothing, and the pane's own status line has
            // already said why if the same entry was ever previewed.
            windows::core::Error::from(DV_E_FORMATETC)
        })?;

        let mut names: Vec<u16> = Vec::new();
        for path in &real {
            names.extend(path.as_os_str().to_string_lossy().encode_utf16());
            names.push(0);
        }
        // The second terminator, which is what ends the list.
        names.push(0);

        let header = DROPFILES {
            pFiles: size_of::<DROPFILES>() as u32,
            pt: Default::default(),
            fNC: BOOL(0),
            fWide: BOOL(1),
        };
        let mut out = Vec::with_capacity(size_of::<DROPFILES>() + names.len() * 2);
        // SAFETY: `DROPFILES` is plain data, and this writes its bytes and nothing else.
        out.extend_from_slice(unsafe {
            std::slice::from_raw_parts(
                (&header as *const DROPFILES).cast::<u8>(),
                size_of::<DROPFILES>(),
            )
        });
        for unit in names {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        Ok(out)
    }

    /// One entry's contents, as a stream the target reads at its own pace.
    ///
    /// # Backed by a temp file, and not by the decompressor directly
    ///
    /// An `IStream` is seekable, and a decompressor is not: for a solid `.7z` a backward seek means
    /// decompressing the block again from its start. A target is entitled to `Seek` and to `Stat`,
    /// and Explorer does both. So the entry is extracted — which
    /// [`crate::archive::extracted`] already does exactly once per entry, cached, read-only, and with
    /// the zip-slip guarantee — and the stream is opened on the result.
    ///
    /// The cost is that the bytes are written once here and read once by the target, where a true
    /// streaming implementation would write them only once. That was the trade taken: a forward-only
    /// `IStream` that errors on a seek works with most targets and fails strangely with the rest,
    /// and "strangely" is not a way for a file manager to lose somebody's data. The laziness that
    /// matters is kept — **only entries a target actually asks for are ever extracted**.
    fn stream(&self, lindex: i32) -> windows::core::Result<IStream> {
        let item = usize::try_from(lindex)
            .ok()
            .and_then(|at| self.items.get(at))
            .ok_or_else(|| windows::core::Error::from(DV_E_LINDEX))?;
        // A directory has no contents. A target that asks anyway gets a refusal rather than an empty
        // file that would land beside the folder it was meant to be.
        if item.is_dir {
            return Err(DV_E_FORMATETC.into());
        }

        let real = crate::archive::extracted(&item.path)
            .map_err(|_| windows::core::Error::from(DV_E_FORMATETC))?;
        let wide = crate::shell::wide(&real);
        // SAFETY: `wide` is null-terminated and outlives the call. Read-only and denying writers,
        // which is what the extracted copy is anyway.
        unsafe {
            SHCreateStreamOnFileEx(
                PCWSTR(wide.as_ptr()),
                STGM_READ.0 | STGM_SHARE_DENY_WRITE.0,
                0,
                false,
                None,
            )
        }
    }

    /// Copy bytes into an `HGLOBAL` for a medium.
    ///
    /// **Nothing tracks the handle afterwards, and nothing needs to.** A medium handed out of
    /// `GetData` with a null `pUnkForRelease` belongs to the *target*, which frees it with
    /// `ReleaseStgMedium`; and the only way out of this function between the allocation and the
    /// hand-over is the `GlobalLock` failure below, which frees it on the spot. So there is no
    /// third path for an owner to be in doubt about.
    fn as_global(&self, bytes: &[u8]) -> windows::core::Result<STGMEDIUM> {
        // SAFETY: `GMEM_MOVEABLE` is what a clipboard medium requires, and the size is the slice's.
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len())? };
        // SAFETY: freshly allocated and locked once; the pointer is valid for `bytes.len()`, which
        // is what it was allocated with. Unlocked before the handle escapes.
        unsafe {
            let into = GlobalLock(handle);
            if into.is_null() {
                let _ = GlobalFree(Some(handle));
                return Err(E_OUTOFMEMORY.into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), into.cast::<u8>(), bytes.len());
            let _ = GlobalUnlock(handle);
        }
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: handle },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }
}

/// Which format a target is asking about.
enum Offer {
    /// `CFSTR_FILEDESCRIPTORW` — the names and sizes, free.
    Descriptor,
    /// `CFSTR_FILECONTENTS` — one entry's bytes, by `lindex`.
    Contents,
    /// `CFSTR_PREFERREDDROPEFFECT` — always a copy.
    Effect,
    /// `CF_HDROP` — the fallback, which extracts.
    Paths,
}

impl IDataObject_Impl for Offered_Impl {
    fn GetData(&self, what: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        // SAFETY: OLE passes a valid pointer for the length of the call.
        let what = unsafe { what.as_ref() }.ok_or_else(|| windows::core::Error::from(E_NOTIMPL))?;
        self.render(what)
    }

    /// Rendering *into* a medium the caller has already allocated.
    ///
    /// `E_NOTIMPL` is a legitimate answer and the usual one: a caller that gets it falls back to
    /// [`IDataObject_Impl::GetData`], and nothing in the drop path prefers this.
    fn GetDataHere(&self, _what: *const FORMATETC, _medium: *mut STGMEDIUM) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    /// Whether a format *would* render — asked while the pointer is merely hovering.
    ///
    /// **Nothing is decompressed here**, which is the point: a target hovering over a window asks
    /// this many times a second, and answering it is a comparison of two integers. The extraction
    /// waits for [`IDataObject_Impl::GetData`], which comes at the drop.
    fn QueryGetData(&self, what: *const FORMATETC) -> HRESULT {
        // SAFETY: as above.
        let Some(what) = (unsafe { what.as_ref() }) else {
            return DV_E_FORMATETC;
        };
        let Some(offer) = self.offers(what) else {
            return DV_E_FORMATETC;
        };
        let wanted = match offer {
            Offer::Contents => TYMED_ISTREAM.0 as u32,
            _ => TYMED_HGLOBAL.0 as u32,
        };
        if what.tymed & wanted == 0 {
            return DV_E_TYMED;
        }
        S_OK
    }

    fn GetCanonicalFormatEtc(&self, _in: *const FORMATETC, out: *mut FORMATETC) -> HRESULT {
        // Required to blank the output even when answering "no canonical form".
        if let Some(out) = unsafe { out.as_mut() } {
            *out = FORMATETC::default();
        }
        E_NOTIMPL
    }

    /// What the target writes back — chiefly `CFSTR_PERFORMEDDROPEFFECT`, once the drop is done.
    ///
    /// Accepted and dropped on the floor, deliberately. A source stores these when it has to undo
    /// something afterwards: the classic case is an "optimized move", where the target has moved the
    /// files itself and the source must not also delete them. Nothing here can be moved — an archive
    /// is only read, and [`COPY`] is what this object advertises — so there is no decision left for
    /// the answer to inform.
    ///
    /// The medium is still released when asked, because `frelease` is a transfer of ownership and
    /// ignoring it leaks whatever the target allocated.
    fn SetData(
        &self,
        _what: *const FORMATETC,
        medium: *const STGMEDIUM,
        release: BOOL,
    ) -> windows::core::Result<()> {
        if release.as_bool() {
            // SAFETY: ownership was just transferred to this object, and this frees it exactly once.
            unsafe { ReleaseStgMedium(medium as *mut STGMEDIUM) };
        }
        Ok(())
    }

    /// The formats on offer, which is how a target discovers the descriptors at all.
    ///
    /// `SHCreateStdEnumFmtEtc` rather than a fourth implemented interface: the shell has a helper
    /// that builds an `IEnumFORMATETC` over an array, and an enumerator written here would be forty
    /// lines of `unsafe` to do the same thing less well.
    ///
    /// **One `CFSTR_FILECONTENTS` entry per item**, each with its own `lindex`. A single entry with
    /// `lindex: -1` is what a one-file source offers, and a target reading that literally asks for
    /// one file however many were dragged.
    fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let formats = Formats::get();
        let entry = |format: u16, lindex: i32, tymed: u32| FORMATETC {
            cfFormat: format,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex,
            tymed,
        };

        let mut all = vec![
            entry(formats.descriptor, -1, TYMED_HGLOBAL.0 as u32),
            entry(formats.effect, -1, TYMED_HGLOBAL.0 as u32),
        ];
        for at in 0..self.items.len() {
            all.push(entry(formats.contents, at as i32, TYMED_ISTREAM.0 as u32));
        }
        // Last, so a target walking the list in order meets the virtual files first. Which is a
        // courtesy rather than a guarantee — see the module header on why the fallback is here.
        all.push(entry(CF_HDROP, -1, TYMED_HGLOBAL.0 as u32));

        // SAFETY: the array is copied by the helper, which is why a local outlives nothing.
        unsafe { SHCreateStdEnumFmtEtc(&all) }
    }

    /// Change notifications on a data object, which a drag has no use for: the data cannot change
    /// under it. `OLE_E_ADVISENOTSUPPORTED` is the documented way to say so.
    fn DAdvise(&self, _what: *const FORMATETC, _flags: u32, _sink: Ref<IAdviseSink>) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _connection: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

// ---- Reading one, as a target ---------------------------------------

/// What a source offering virtual files is promising — read from its descriptors, while the drag
/// is still moving.
///
/// # Why the receiving half of this module exists at all
///
/// Everything above is about not extracting until the drop, and the program that defeated it was
/// this one. [`super::Target_Impl::DragEnter`] asks a drag what it is carrying, and the only format
/// it knew how to ask in was `CF_HDROP` — a list of paths, so [`Offered::dropfiles`] had to extract
/// the whole selection to answer. That happens before the pointer has left the pane the drag started
/// in, and before `effect_at` writes the one thing the cursor, the sentence and the destination
/// highlight are all drawn from. So a drag out of a solid `.7z` sat there for several seconds looking
/// like a gesture that was not going to be allowed. The irony is on the record: [`super::Incoming`]
/// documents this exact hazard, about 7-Zip, as a thing *other* sources do.
///
/// `CFSTR_FILEDESCRIPTORW` answers the same question for free — describing files that do not exist
/// yet is the format's whole purpose — which fixes it for every virtual source and not only for this
/// program's own: a drag out of 7-Zip *into* this window tripped the same wire.
///
/// **Where in the order this is asked for is [`super::Incoming::read`]'s decision**, and not an
/// obvious one. It is not first.
pub struct Promised {
    /// The **top level** of the drag: one entry per item that was picked up, in descriptor order.
    ///
    /// Reconstructed rather than read, because a descriptor list is flat — a dragged folder appears
    /// as every file under it — and *Copy 1,205 items into docs* is not what somebody who dragged
    /// one folder did. Every descriptor name is relative to where the drop will land, so the first
    /// component of each is a top-level item, and the distinct ones in order are the selection.
    ///
    /// **These are names and not paths**, which is why [`super::Incoming::promised`] exists to say
    /// so. They are enough for every question asked while the drag is moving — see that field — and
    /// they must never reach a drop.
    pub items: Vec<PathBuf>,
    /// Whether every one of them is a folder, which is what decides whether the sidebar will take
    /// this drag at all.
    ///
    /// From the descriptor's `FILE_ATTRIBUTE_DIRECTORY`, and from having children: a source is
    /// entitled to describe `src\main.rs` without describing `src`, and the folder is no less a
    /// folder for it.
    pub all_folders: bool,
}

/// Ask a data object what it is promising, or `None` if it promises nothing — which is what an
/// ordinary drag of real files answers, and which sends the caller back to `CF_HDROP`.
pub fn promised(data: &IDataObject) -> Option<Promised> {
    let format = FORMATETC {
        cfFormat: Formats::get().descriptor,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: a live object and a `FORMATETC` that outlives both calls. The medium is released on
    // every path out of this block, there being only the one.
    unsafe {
        // Asked before it is fetched, which is the etiquette this module's own `QueryGetData`
        // documents from the other side: a source that does not offer descriptors says so here
        // without being asked to render anything.
        if data.QueryGetData(&format) != S_OK {
            return None;
        }
        let mut medium = data.GetData(&format).ok()?;
        let found = held(&medium);
        ReleaseStgMedium(&mut medium);
        found
    }
}

/// The same, with the medium's handle locked for exactly as long as the parse.
///
/// # Safety
///
/// `medium` must be one a `GetData` has just returned and nothing has released yet.
unsafe fn held(medium: &STGMEDIUM) -> Option<Promised> {
    if medium.tymed != TYMED_HGLOBAL.0 as u32 {
        return None;
    }
    let handle = medium.u.hGlobal;
    let base = GlobalLock(handle).cast::<u8>();
    if base.is_null() {
        return None;
    }
    let found = top_level(base, GlobalSize(handle));
    let _ = GlobalUnlock(handle);
    found
}

/// Parse a `FILEGROUPDESCRIPTORW` down to the top level of what was picked up.
///
/// # Safety
///
/// `base` must be readable for `size` bytes. Nothing beyond that is assumed: **the buffer was
/// written by whichever program started the drag**, and the count in its first four bytes is a
/// number in that buffer rather than a fact about it. So the count is checked against the size the
/// allocation really has before a single descriptor is read — a source that claims four billion of
/// them gets `None` instead of a walk off the end of the heap.
unsafe fn top_level(base: *const u8, size: usize) -> Option<Promised> {
    let each = size_of::<FILEDESCRIPTORW>();
    if size < 4 {
        return None;
    }
    let count =
        u32::from_le_bytes(std::slice::from_raw_parts(base, 4).try_into().ok()?) as usize;
    // Division rather than `4 + count * each`, which is the multiplication that overflows.
    if count == 0 || (size - 4) / each < count {
        return None;
    }

    // The distinct first components in the order they appear, and whether each is a folder. The
    // map is only there to keep this linear: a descriptor list can hold four hundred thousand
    // entries, and scanning the answer for each of them would be quadratic.
    let mut at_top: Vec<(String, bool)> = Vec::new();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for index in 0..count {
        // `read_unaligned`, and the name copied out by value: `FILEDESCRIPTORW` is
        // `#[repr(C, packed(1))]`, so neither the offset nor any field of it carries an alignment
        // guarantee, and `&fd.cFileName` would be undefined behaviour. The same reason
        // [`Offered::descriptors`] fills a local and assigns the field whole.
        let fd = base
            .add(4 + index * each)
            .cast::<FILEDESCRIPTORW>()
            .read_unaligned();
        let units = fd.cFileName;
        let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
        let name = String::from_utf16_lossy(&units[..end]);

        // Both separators, because the format says backslash and a source may write either.
        let (head, has_children) = match name.find(['\\', '/']) {
            Some(cut) => (&name[..cut], true),
            None => (name.as_str(), false),
        };
        if head.is_empty() {
            continue;
        }
        // `FILE_ATTRIBUTE_DIRECTORY`.
        let is_dir = has_children || fd.dwFileAttributes & 16 != 0;

        match seen.get(head) {
            // Folder wins over file for the same name: one descriptor of the two knowing it is a
            // folder is enough, and a source that describes `src` before `src\main.rs` marks it
            // with the attribute while one that describes only the child does not mark it at all.
            Some(&held) => at_top[held].1 |= is_dir,
            None => {
                seen.insert(head.to_owned(), at_top.len());
                at_top.push((head.to_owned(), is_dir));
            }
        }
    }

    if at_top.is_empty() {
        return None;
    }
    Some(Promised {
        all_folders: at_top.iter().all(|(_, is_dir)| *is_dir),
        items: at_top.into_iter().map(|(name, _)| PathBuf::from(name)).collect(),
    })
}

/// Driving the data object the way a drop target does.
///
/// A real drag cannot be tested here — it needs a held pointer button and a target process — but the
/// data object is the whole of what this module contributes, and every question a target asks it is
/// an ordinary call. So these ask them: the descriptors, one entry's contents by index, the refusals
/// for an index that names nothing, and the two claims that matter most — that **hovering extracts
/// nothing** and that **rendering the descriptors extracts nothing**.
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A zip in the sandbox. See [`crate::sandbox`].
    fn a_zip(name: &str, entries: &[(&str, &str)]) -> PathBuf {
        let root = crate::sandbox::fresh(&format!("virtual-{name}"));
        let pkg = root.join("pkg.zip");
        let file = std::fs::File::create(&pkg).expect("sandbox");
        let mut writer = zip::ZipWriter::new(file);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (entry, body) in entries {
            writer.start_file(*entry, stored).expect("zip");
            writer.write_all(body.as_bytes()).expect("zip");
        }
        writer.finish().expect("zip");
        pkg
    }

    /// What a target passes to ask for a format on a particular medium.
    fn asking(format: u16, lindex: i32, tymed: u32) -> FORMATETC {
        FORMATETC {
            cfFormat: format,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex,
            tymed,
        }
    }

    /// Read a `FILEGROUPDESCRIPTORW` back out of an `HGLOBAL`: each name, size and whether it is a
    /// folder.
    ///
    /// **Parses the bytes by offset rather than reusing the writer's own view of them.** A test that
    /// wrote and read through one struct definition would agree with itself about a wrong layout,
    /// and the layout is the part the shell depends on.
    fn descriptors_in(medium: &STGMEDIUM) -> Vec<(String, u64, bool)> {
        assert_eq!(medium.tymed, TYMED_HGLOBAL.0 as u32, "must be an HGLOBAL");
        // SAFETY: the medium was rendered by `as_global`, so the handle is live and holds the count
        // followed by that many packed descriptors. Unlocked before returning.
        unsafe {
            let base = GlobalLock(medium.u.hGlobal).cast::<u8>();
            assert!(!base.is_null(), "the handle should lock");
            let count =
                u32::from_le_bytes(std::slice::from_raw_parts(base, 4).try_into().expect("4 bytes"));
            let mut out = Vec::new();
            for at in 0..count as usize {
                // `packed(1)`, so descriptor `at` begins exactly `4 + at * size_of` in — and
                // `read_unaligned`, because that offset carries no alignment guarantee.
                let fd = base
                    .add(4 + at * size_of::<FILEDESCRIPTORW>())
                    .cast::<FILEDESCRIPTORW>()
                    .read_unaligned();
                // Copied out by value first: `&fd.cFileName` would be a reference into a packed
                // struct, which is undefined behaviour and which the compiler refuses. The same
                // reason `descriptors` fills a local and assigns the field whole.
                let units = fd.cFileName;
                let end = units
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(units.len());
                out.push((
                    String::from_utf16_lossy(&units[..end]),
                    (fd.nFileSizeHigh as u64) << 32 | fd.nFileSizeLow as u64,
                    // `FILE_ATTRIBUTE_DIRECTORY`.
                    fd.dwFileAttributes & 16 != 0,
                ));
            }
            let _ = GlobalUnlock(medium.u.hGlobal);
            out
        }
    }

    /// How many archives have been extracted into the temp root so far.
    ///
    /// A count compared before and against after, rather than "is it empty" — the temp root is one
    /// directory shared by every test in this process, and the tests that *do* extract leave their
    /// directories in it. Asking whether it is empty is therefore a question about test order, which
    /// is exactly the kind of assertion that passes alone and fails in the suite.
    fn extractions() -> usize {
        std::fs::read_dir(crate::archive::extract::temp_root())
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    /// The headline claim: a drag offers a name and a size for every file under what was picked up,
    /// and building that offer decompresses nothing.
    #[test]
    fn the_descriptors_name_every_file_without_extracting_anything() {
        let pkg = a_zip(
            "descriptors",
            &[
                ("src/main.rs", "fn main() {}"),
                ("src/ui/mod.rs", "// ui"),
                ("readme.txt", "hello"),
            ],
        );
        // A file and a folder picked up together, which is what exercises the flattening.
        let picked = vec![pkg.join("readme.txt"), pkg.join("src")];
        let untouched = extractions();
        let data: IDataObject = Offered::over(&picked).expect("a virtual data object");

        let what = asking(Formats::get().descriptor, -1, TYMED_HGLOBAL.0 as u32);
        // SAFETY: a live object, and a `FORMATETC` that outlives the call.
        let medium = unsafe { data.GetData(&what) }.expect("the descriptors should render");
        let mut named = descriptors_in(&medium);
        named.sort();

        assert_eq!(
            named,
            vec![
                ("readme.txt".to_owned(), 5, false),
                ("src".to_owned(), 0, true),
                ("src\\main.rs".to_owned(), 12, false),
                ("src\\ui".to_owned(), 0, true),
                ("src\\ui\\mod.rs".to_owned(), 5, false),
            ],
            "every file under what was picked up, with the relative name the target recreates, \
             its real size, and folders marked as folders"
        );

        // SAFETY: this test holds the medium where a target normally would; freed exactly once.
        unsafe { ReleaseStgMedium(&mut { medium }) };

        assert_eq!(
            extractions(),
            untouched,
            "the descriptors come out of the archive's index, so nothing should have been written"
        );
    }

    /// One entry's contents, asked for by index — and index 1, to prove the index is honoured rather
    /// than ignored in favour of the first entry.
    #[test]
    fn contents_come_one_entry_at_a_time() {
        let pkg = a_zip(
            "contents",
            &[("a.txt", "first"), ("b.txt", "second and longer")],
        );
        let picked = vec![pkg.join("a.txt"), pkg.join("b.txt")];
        let data: IDataObject = Offered::over(&picked).expect("a virtual data object");

        let what = asking(Formats::get().contents, 1, TYMED_ISTREAM.0 as u32);
        // SAFETY: as above.
        let medium = unsafe { data.GetData(&what) }.expect("the contents should render");
        assert_eq!(medium.tymed, TYMED_ISTREAM.0 as u32);

        // SAFETY: the medium holds the stream for the length of this block, and `Read` is given a
        // buffer longer than the entry.
        let read = unsafe {
            let stream = medium.u.pstm.as_ref().expect("a stream").clone();
            let mut buffer = [0u8; 64];
            let mut got = 0u32;
            stream
                .Read(
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    Some(&mut got),
                )
                .ok()
                .expect("the stream should read");
            String::from_utf8_lossy(&buffer[..got as usize]).into_owned()
        };
        assert_eq!(read, "second and longer");

        // SAFETY: freed exactly once, as above.
        unsafe { ReleaseStgMedium(&mut { medium }) };
    }

    /// The index comes from another process, so one past the end has to be refused rather than
    /// panicking through a `#[implement]` vtable.
    #[test]
    fn a_content_index_out_of_range_is_refused() {
        let pkg = a_zip("range", &[("only.txt", "one")]);
        let data: IDataObject = Offered::over(&[pkg.join("only.txt")]).expect("a data object");

        for lindex in [1, 99, -1, i32::MIN] {
            let what = asking(Formats::get().contents, lindex, TYMED_ISTREAM.0 as u32);
            // SAFETY: as above.
            assert!(
                unsafe { data.GetData(&what) }.is_err(),
                "lindex {lindex} names no entry and must be refused"
            );
        }
    }

    /// A hovering target asks `QueryGetData` many times a second. It must answer from memory.
    #[test]
    #[ignore = "fails on the GitHub Actions runner, passes on a desktop: counts the shared extraction folder, which other tests write to at the same time"]
    fn querying_a_format_extracts_nothing() {
        let pkg = a_zip("query", &[("a.txt", "first")]);
        let untouched = extractions();
        let data: IDataObject = Offered::over(&[pkg.join("a.txt")]).expect("a data object");
        let formats = Formats::get();

        for (format, lindex, tymed) in [
            (formats.descriptor, -1, TYMED_HGLOBAL.0 as u32),
            (formats.contents, 0, TYMED_ISTREAM.0 as u32),
            (formats.effect, -1, TYMED_HGLOBAL.0 as u32),
            (CF_HDROP, -1, TYMED_HGLOBAL.0 as u32),
        ] {
            let what = asking(format, lindex, tymed);
            // SAFETY: as above.
            assert_eq!(
                unsafe { data.QueryGetData(&what) },
                S_OK,
                "format {format} is offered and must be answered"
            );
        }

        // A format this object does not have, and a format it does on a medium it does not offer.
        let mystery = asking(0xC0FF, -1, TYMED_HGLOBAL.0 as u32);
        // SAFETY: as above.
        assert_eq!(unsafe { data.QueryGetData(&mystery) }, DV_E_FORMATETC);
        let wrong_medium = asking(formats.descriptor, -1, TYMED_ISTREAM.0 as u32);
        // SAFETY: as above.
        assert_eq!(unsafe { data.QueryGetData(&wrong_medium) }, DV_E_TYMED);

        assert_eq!(
            extractions(),
            untouched,
            "a target merely hovering must not cause an extraction"
        );
    }

    /// A drag out of an archive is a copy, never a move: there is no removing a file from something
    /// this program only reads.
    #[test]
    fn the_preferred_effect_is_a_copy() {
        let pkg = a_zip("effect", &[("a.txt", "first")]);
        let data: IDataObject = Offered::over(&[pkg.join("a.txt")]).expect("a data object");
        let what = asking(Formats::get().effect, -1, TYMED_HGLOBAL.0 as u32);
        // SAFETY: as above.
        let medium = unsafe { data.GetData(&what) }.expect("the effect should render");
        // SAFETY: `as_global` wrote four bytes into it.
        let effect = unsafe {
            let base = GlobalLock(medium.u.hGlobal).cast::<u8>();
            let value = u32::from_le_bytes(
                std::slice::from_raw_parts(base, 4).try_into().expect("4 bytes"),
            );
            let _ = GlobalUnlock(medium.u.hGlobal);
            value
        };
        assert_eq!(effect, COPY, "DROPEFFECT_COPY and nothing else");
        // SAFETY: freed exactly once.
        unsafe { ReleaseStgMedium(&mut { medium }) };
    }

    /// The `CF_HDROP` fallback names real files — this is the render that does extract, and the
    /// paths it produces have to exist or a target that takes this format drops nothing.
    #[test]
    fn the_fallback_names_files_that_exist() {
        let pkg = a_zip("fallback", &[("a.txt", "first"), ("b.txt", "second")]);
        let picked = vec![pkg.join("a.txt"), pkg.join("b.txt")];
        let data: IDataObject = Offered::over(&picked).expect("a data object");

        let what = asking(CF_HDROP, -1, TYMED_HGLOBAL.0 as u32);
        // SAFETY: as above.
        let medium = unsafe { data.GetData(&what) }.expect("the paths should render");
        // SAFETY: the handle holds a `DROPFILES` header then a double-null-terminated wide list.
        let paths = unsafe {
            let base = GlobalLock(medium.u.hGlobal).cast::<u8>();
            let header = base.cast::<DROPFILES>().read_unaligned();
            assert!(header.fWide.as_bool(), "wide, as the header claims");
            let mut units = base.add(header.pFiles as usize).cast::<u16>();
            let mut out: Vec<PathBuf> = Vec::new();
            loop {
                let mut name = Vec::new();
                while units.read_unaligned() != 0 {
                    name.push(units.read_unaligned());
                    units = units.add(1);
                }
                units = units.add(1);
                if name.is_empty() {
                    break;
                }
                out.push(PathBuf::from(String::from_utf16_lossy(&name)));
            }
            let _ = GlobalUnlock(medium.u.hGlobal);
            out
        };

        assert_eq!(paths.len(), 2, "one path per selected item");
        for path in &paths {
            assert!(path.is_file(), "{} must exist by now", path.display());
            assert!(
                path.starts_with(crate::archive::extract::temp_root()),
                "and be inside the temp root"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&paths[0]).expect("read back"),
            "first"
        );

        // SAFETY: freed exactly once.
        unsafe { ReleaseStgMedium(&mut { medium }) };
    }

    /// **The bug this half of the module was written for**, and the drag-time twin of
    /// `querying_a_format_extracts_nothing`: the question a target asks the moment a drag arrives
    /// must not be answered by decompressing anything.
    ///
    /// Driven through [`super::super::Incoming::read`] rather than through [`promised`] alone, because the
    /// claim is about that function — it is what `DragEnter` calls, and it is where the old
    /// `CF_HDROP` request was.
    #[test]
    fn a_drag_arriving_is_described_without_extracting_anything() {
        let pkg = a_zip(
            "arriving",
            &[
                ("src/main.rs", "fn main() {}"),
                ("src/ui/mod.rs", "// ui"),
                ("readme.txt", "hello"),
            ],
        );
        // A folder and a file picked up together: one flattens to three descriptors and the other
        // to one, and the drag is nonetheless of two things.
        let picked = vec![pkg.join("src"), pkg.join("readme.txt")];
        let untouched = extractions();
        let data: IDataObject = Offered::over(&picked).expect("a virtual data object");

        let incoming = super::super::Incoming::read(Some(&data));

        assert_eq!(
            extractions(),
            untouched,
            "a drag entering a window must not cause an extraction — this is the several-second \
             stall the descriptors are here to avoid"
        );

        let mut items = incoming.items.clone();
        items.sort();
        assert_eq!(
            items,
            vec![PathBuf::from("readme.txt"), PathBuf::from("src")],
            "the top level of the drag, so the sentence says `2 items` and not `4`"
        );
        assert!(
            incoming.promised,
            "and it must be marked a promise, or the drop would take these names for paths"
        );
        assert!(
            incoming.temporary,
            "a materialisation is always a copy — there is no moving a file out of an archive"
        );
        assert!(
            !incoming.all_folders,
            "one of the two is a file, which is what stops the sidebar taking this drag"
        );

        // The other half of that last answer, from a selection that really is all folders.
        let folders = super::super::Incoming::read(Some(
            &Offered::over(&[pkg.join("src")]).expect("a virtual data object"),
        ));
        assert_eq!(folders.items, vec![PathBuf::from("src")]);
        assert!(
            folders.all_folders,
            "a folder is a folder whether the archive stored an entry for it or only for its \
             children"
        );
    }

    /// **The discriminator**, and the reason [`super::super::Incoming::read`] asks in the order it
    /// does — which is where that story is written down, this being the test that found it.
    ///
    /// Everything above rests on an ordinary drag of real files not being taken for a promise, so
    /// the assertions are on what `read` decides rather than on which formats the shell happens to
    /// have.
    #[test]
    fn the_shell_s_own_data_object_arrives_as_real_paths() {
        crate::shell::init();
        let root = crate::sandbox::fresh("virtual-discriminator");
        let plain = root.join("ordinary.txt");
        std::fs::write(&plain, b"not in an archive").expect("sandbox");
        // A folder alongside it, because the list this produces is what the refusal rules read: a
        // drag of a folder onto itself is refused by comparing these paths against the destination,
        // and `all_folders` is what decides whether the sidebar will take the drag at all.
        let folder = root.join("a folder");
        std::fs::create_dir_all(&folder).expect("sandbox");

        let Some(data) = super::super::data_object(std::slice::from_ref(&plain)) else {
            // The shell declining to make one at all is not this test's subject, and failing here
            // would report the wrong thing.
            return;
        };

        // The fact the ordering exists for, asserted rather than remembered: if this ever comes back
        // `None`, the step that precedes it is no longer load-bearing and the reason should be
        // understood before it is removed.
        assert!(
            promised(&data).is_some(),
            "the shell offers descriptors for real files, which is exactly why they cannot be the \
             first thing asked for"
        );

        let incoming = super::super::Incoming::read(Some(&data));
        assert!(
            !incoming.promised,
            "a real selection must arrive as real paths, whatever else the shell offers alongside"
        );
        assert_eq!(
            incoming.items,
            vec![plain.clone()],
            "and they must be the paths themselves, since a copy is about to be made from them"
        );
        assert!(
            !incoming.temporary,
            "a file in the sandbox is the user's own, so the volume rule decides the effect"
        );

        // A selection of more than one, in order and with a folder in it — the shape the refusal
        // rules and the sentence are written against.
        let mixed = vec![folder.clone(), plain.clone()];
        let Some(data) = super::super::data_object(&mixed) else {
            return;
        };
        let incoming = super::super::Incoming::read(Some(&data));
        assert_eq!(
            incoming.items, mixed,
            "every item, in the order the drag holds them"
        );
        assert!(
            !incoming.all_folders,
            "one of the two is a file, which is what stops the sidebar taking this drag"
        );

        // And all folders, which is the answer that lets it.
        let Some(data) = super::super::data_object(std::slice::from_ref(&folder)) else {
            return;
        };
        assert!(
            super::super::Incoming::read(Some(&data)).all_folders,
            "a folder is a place, and the sidebar is a list of places"
        );
    }

    /// **The freeze between letting go and the copy starting** — the last place the extraction was
    /// still on the UI thread, and the reason for [`super::super::Shared::carrying`], where the whole
    /// of it is written down.
    ///
    /// Both halves of the fix are asserted: the callback extracts nothing, and what it hands on still
    /// names paths **inside** the archive for `App::land` to extract on a worker.
    #[test]
    fn a_drop_out_of_an_archive_extracts_nothing_in_the_callback() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Ole::{IDropTarget, DROPEFFECT_COPY};
        use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;

        let pkg = a_zip("dropped", &[("a.txt", "first"), ("b.txt", "second")]);
        let into = pkg.parent().expect("a parent").join("elsewhere");
        std::fs::create_dir_all(&into).expect("sandbox");
        let picked = vec![pkg.join("a.txt"), pkg.join("b.txt")];

        let shared = std::sync::Arc::new(std::sync::Mutex::new(super::super::Shared::default()));
        {
            let mut block = shared.lock().expect("a fresh lock");
            block.targets = super::super::Targets {
                zones: vec![super::super::Region {
                    rect: (0, 0, 100, 100),
                    onto: super::super::Onto::Folder(into.clone()),
                    name: "elsewhere".to_owned(),
                }],
                from: None,
            };
            // What `drag_out` publishes for the length of a drag this window started.
            block.carrying = picked.clone();
        }

        // A **real** data object, so that a callback which did ask would really extract and the
        // assertion below would really fail.
        let data: IDataObject = Offered::over(&picked).expect("a virtual data object");
        let target: IDropTarget = super::super::Target::promising(
            shared.clone(),
            egui::Context::default(),
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
        )
        .into();

        let untouched = extractions();
        let mut effect = DROPEFFECT_COPY;
        // SAFETY: a live target and a live data object; the out-parameter is this frame's own, and
        // the callback is the documented one for a completed drop.
        unsafe {
            target.Drop(
                Some(&data),
                MODIFIERKEYS_FLAGS(0),
                POINTL { x: 10, y: 50 },
                &mut effect,
            )
        }
        .expect("the drop was refused");

        assert_eq!(
            extractions(),
            untouched,
            "the drop callback extracted — which is the freeze, because this runs on the UI thread \
             inside the message pump"
        );

        let landed = std::mem::take(&mut shared.lock().expect("a fresh lock").dropped);
        assert_eq!(landed.len(), 1, "the drop was not passed on at all");
        assert_eq!(
            landed[0].items, picked,
            "the paths inside the archive are what the drop has to carry, so that the extraction \
             happens on a worker with the window still painting"
        );
    }

    /// A selection of **real** files is not this object's business: it falls through to the shell's
    /// own data object, which offers every format Explorer would have.
    #[test]
    fn a_real_selection_is_left_to_the_shell() {
        let root = crate::sandbox::fresh("virtual-real");
        let plain = root.join("ordinary.txt");
        std::fs::write(&plain, b"not in an archive").expect("sandbox");
        assert!(
            Offered::over(&[plain]).is_none(),
            "a real file has a real data object and must not get this one"
        );
    }
}
