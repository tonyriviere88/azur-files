//! OLE drag and drop, as a source and as a target.
//!
//! The Windows half of [`crate::shell::dnd`], and the two interfaces this program *implements*
//! rather than merely calls: `IDropSource` and `IDropTarget`.

use super::*;
use windows::core::{implement, Ref, BOOL, HRESULT};
use windows::Win32::Foundation::{POINTL, S_OK};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::{
    MK_CONTROL, MK_LBUTTON, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
};

/// The three `DRAGDROP_S_*` values a source returns. Success codes, not errors,
/// which is why they are spelled out rather than gone looking for.
const DRAGDROP_S_DROP: HRESULT = HRESULT(0x0004_0100u32 as i32);
const DRAGDROP_S_CANCEL: HRESULT = HRESULT(0x0004_0101u32 as i32);
const DRAGDROP_S_USEDEFAULTCURSORS: HRESULT = HRESULT(0x0004_0102u32 as i32);

// ---- Dragging out --------------------------------------------------

/// The source half of a drag: the two questions OLE asks while one is running.
#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            return DRAGDROP_S_CANCEL;
        }
        // The drag ends when the button that started it comes up. Both are checked
        // because a right-drag is a legitimate gesture — it is what produces
        // Explorer's "copy here / move here / create shortcut" menu on drop.
        if keys.0 & (MK_LBUTTON.0 | MK_RBUTTON.0) == 0 {
            return DRAGDROP_S_DROP;
        }
        S_OK
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        // Let OLE show the standard copy, move and no-entry cursors rather than
        // inventing a set that would not match anything else on the desktop.
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Run the drag to its end. Called on the drag's own thread, never on the UI one.
///
/// `ui_thread` owns the window the gesture started in, and its input queue is the one that
/// knows the button is down. `DoDragDrop` reads that state to decide which button it is
/// following and when to stop following it, and on Windows key state and mouse capture are
/// per *input queue* rather than per process — so from a fresh thread it would see no button
/// held and end the drag before the pointer had moved. `AttachThreadInput` joins the two
/// queues for the length of the drag, which is what makes the capture and the button state
/// reachable from here. It is undone on the way out, including when the drag fails.
pub fn drag_out(items: &[PathBuf], ui_thread: u32) -> Option<Effect> {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};

    if items.is_empty() {
        return None;
    }
    let data = data_object(items)?;
    let source: IDropSource = Source.into();

    /// Undoes the attachment however this function leaves.
    struct Attached(u32, u32);
    impl std::ops::Drop for Attached {
        fn drop(&mut self) {
            // SAFETY: undoes exactly the attachment made below, once.
            let _ = unsafe { AttachThreadInput(self.0, self.1, false) };
        }
    }
    // SAFETY: both threads are alive for the length of the drag — this one by definition,
    // the UI one because it is the one waiting on the outcome.
    let attached = unsafe {
        let mine = GetCurrentThreadId();
        AttachThreadInput(mine, ui_thread, true)
            .as_bool()
            .then(|| Attached(mine, ui_thread))
    };

    let mut effect = DROPEFFECT_NONE;
    // SAFETY: both interfaces outlive the call, and `DoDragDrop` runs its own modal
    // loop on the calling thread — which is the one holding the apartment.
    let hr = unsafe {
        DoDragDrop(
            &data,
            &source,
            // `LINK` as well, because this window has a target of its own that answers with
            // it: the Bookmarks group, which pins a folder rather than copying it. A target
            // returning an effect the source never offered is a target OLE refuses, so
            // leaving it out made dragging a folder onto Bookmarks do nothing at all.
            DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
            &mut effect,
        )
    };
    drop(attached);
    if hr != DRAGDROP_S_DROP {
        return None;
    }
    if effect.0 & DROPEFFECT_MOVE.0 != 0 {
        Some(Effect::Move)
    } else if effect.0 & DROPEFFECT_COPY.0 != 0 {
        Some(Effect::Copy)
    } else {
        None
    }
}

/// The shell's own data object for a selection, so a target gets every format
/// Explorer would have offered rather than only the one this program knows about.
fn data_object(items: &[PathBuf]) -> Option<IDataObject> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        SHCreateShellItemArrayFromIDLists, SHParseDisplayName, BHID_DataObject,
    };

    struct Pidl(*mut ITEMIDLIST);
    impl std::ops::Drop for Pidl {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: allocated by `SHParseDisplayName`, freed once.
                unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
            }
        }
    }

    let pidls: Vec<Pidl> = items
        .iter()
        .filter_map(|path| {
            let wide = crate::shell::wide(path);
            let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
            // SAFETY: `wide` is null-terminated and outlives the call.
            let ok = unsafe {
                SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut raw, 0, None).is_ok()
            };
            (ok && !raw.is_null()).then_some(Pidl(raw))
        })
        .collect();
    if pidls.is_empty() {
        return None;
    }
    let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0 as *const _).collect();

    // SAFETY: the PIDLs outlive the array, which copies what it needs.
    unsafe {
        let array = SHCreateShellItemArrayFromIDLists(&raw).ok()?;
        array.BindToHandler(None, &BHID_DataObject).ok()
    }
}

// ---- Dropping in ---------------------------------------------------

/// The receiving half. Every method runs on the UI thread, inside the window's
/// message pump, where the application is not reachable — so all it does is read and
/// write the shared block.
#[implement(IDropTarget)]
pub struct Target {
    shared: Arc<Mutex<Shared>>,
    /// What the drag in flight is carrying, read between `DragEnter` and `Drop` so the
    /// effect rules can see where it came from.
    held: Mutex<Option<Incoming>>,
    /// The window this is registered on, for turning a screen point into a client one.
    /// Held here rather than in `shared` because the callbacks convert points while
    /// holding that lock, and a `Mutex` is not reentrant.
    hwnd: isize,
}

impl Target {
    pub fn new(shared: Arc<Mutex<Shared>>, hwnd: isize) -> Self {
        Self {
            shared,
            held: Mutex::new(None),
            hwnd,
        }
    }
}

/// What a drag is carrying, as far as it can be known *while it is still moving* — read
/// once, when it arrives, to answer `DragOver` with.
///
/// `GetData` is not a getter. It is a request that the source *render* what it is
/// offering, and a source may do arbitrary work to answer one — an archiver renders
/// `CF_HDROP` by extracting files to a temporary folder, because until it has it has no
/// paths to put in one. This was being called from `effect_at`, which runs on every
/// `DragOver`, so a drag crossing the window asked the source to render its data dozens of
/// times a second.
///
/// Which cuts the other way too, and is why this is not what the drop then acts on: a
/// source is entitled to have nothing to give until the drop is real, and answering a
/// speculative request during the drag is the part it is allowed to skip. `Drop` asks
/// again for that reason and falls back to this only if the second answer is empty.
struct Incoming {
    /// Every path the data object offered, via `CF_HDROP`. Empty is not an error — see
    /// above.
    items: Vec<PathBuf>,
    /// Whether those paths are a temporary the source is going to take back — see
    /// [`super::under_temp`]. Decided from the first path: a data object carrying files
    /// from two places at once is not a thing any source produces.
    temporary: bool,
}

impl Incoming {
    fn read(data: Option<&IDataObject>) -> Self {
        let items = data.and_then(paths_of).unwrap_or_default();
        let temporary = items.first().is_some_and(|first| super::under_temp(first));
        Self { items, temporary }
    }
}

/// The nearest thing to `wanted` that the source is willing to allow.
///
/// `pdwEffect` is in/out on all three callbacks: on the way in it holds the effects the
/// source passed to `DoDragDrop`. Answering with one that is not in that set was how
/// dragging the contents of an archive into a folder on the same volume told the archiver
/// to delete its extraction — the drag had only ever been offered as a copy.
///
/// A move degrades to a copy and never the other way round. The fallback for an effect the
/// source will not allow has to be the one that destroys nothing, which is also why
/// pinning — which copies nothing at all — may report a copy but must never report a move.
fn permitted(wanted: DROPEFFECT, allowed: DROPEFFECT) -> DROPEFFECT {
    // A source that fills this in as nothing has told us nothing, rather than that it
    // refuses every drop. Taken as no restriction, which is what ignoring the field
    // altogether amounted to.
    if allowed == DROPEFFECT_NONE {
        return wanted;
    }
    let offers = |effect: DROPEFFECT| allowed.0 & effect.0 != 0;
    match wanted {
        asked if offers(asked) => asked,
        DROPEFFECT_MOVE | DROPEFFECT_LINK if offers(DROPEFFECT_COPY) => DROPEFFECT_COPY,
        _ => DROPEFFECT_NONE,
    }
}

impl Target_Impl {
    /// A screen point in the window's own coordinates, which is what the zones are in.
    ///
    /// `IDropTarget` is handed **screen** coordinates and [`Targets`] is published in the
    /// window's own. They were compared directly, and the only reason anything worked at all
    /// is that the two overlap when a window sits near the top left of the screen: a drop
    /// resolved to whichever zone the *screen* point happened to fall in, which was almost
    /// always the whole pane rather than the folder row under the pointer. So a file dropped
    /// on a folder went into the folder already being shown, where it was filtered out as a
    /// no-op — and dragging appeared to do nothing whatsoever.
    ///
    /// The handle is the target's own and not the shared block's, deliberately: the callbacks
    /// below call this *while holding* that lock, and a `Mutex` is not reentrant.
    fn in_client(&self, pt: &POINTL) -> (i32, i32) {
        use windows::Win32::Foundation::{HWND, POINT};
        use windows::Win32::Graphics::Gdi::ScreenToClient;

        if self.hwnd == 0 {
            return (pt.x, pt.y);
        }
        let hwnd = self.hwnd;
        let mut point = POINT { x: pt.x, y: pt.y };
        // SAFETY: a coordinate conversion against a live window handle.
        unsafe {
            let _ = ScreenToClient(HWND(hwnd as *mut std::ffi::c_void), &mut point);
        }
        (point.x, point.y)
    }

    /// What this drag would do at a point, given the keys held and what the source allows.
    fn effect_at(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        allowed: DROPEFFECT,
    ) -> DROPEFFECT {
        let at = self.in_client(pt);
        let onto = {
            let Ok(mut shared) = self.shared.lock() else {
                return DROPEFFECT_NONE;
            };
            shared.hovering = Some(at);
            // Remembered here because `Drop` is called with the button already released.
            if keys.0 & MK_RBUTTON.0 != 0 {
                shared.right_button = true;
            }
            shared.targets.at(at).cloned()
        };
        let target = match onto {
            Some(Onto::Folder(path)) => path,
            // Pinning moves nothing, so it answers `LINK` whatever is held down. It is
            // also the only honest answer: a copy cursor over the sidebar would be
            // promising a copy that is not going to happen.
            Some(Onto::Bookmarks) => return permitted(DROPEFFECT_LINK, allowed),
            None => return DROPEFFECT_NONE,
        };

        if keys.0 & MK_CONTROL.0 != 0 {
            return permitted(DROPEFFECT_COPY, allowed);
        }
        if keys.0 & MK_SHIFT.0 != 0 {
            return permitted(DROPEFFECT_MOVE, allowed);
        }
        // Where it came from, read from the data object when the drag arrived rather than
        // guessed — a drag can come from anywhere, including from nowhere with a path.
        let (source, temporary) = self
            .held
            .lock()
            .ok()
            .and_then(|held| {
                let incoming = held.as_ref()?;
                Some((incoming.items.first().cloned(), incoming.temporary))
            })
            .unwrap_or((None, false));
        // Nothing the source is about to delete out from under the copy is a move.
        if temporary {
            return permitted(DROPEFFECT_COPY, allowed);
        }
        let wanted = match super::default_effect(source.as_deref(), &target) {
            Effect::Move => DROPEFFECT_MOVE,
            Effect::Copy => DROPEFFECT_COPY,
        };
        permitted(wanted, allowed)
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // Once, here — see [`Incoming`]. The lock is taken and let go before `effect_at`,
        // which takes it again and would deadlock on a `Mutex` that is not reentrant.
        if let Ok(mut held) = self.held.lock() {
            *held = Some(Incoming::read(data.as_ref()));
        }
        // SAFETY: OLE always passes a valid out-pointer here. It is read before it is
        // written because on the way in it holds the effects the source allows.
        unsafe {
            let allowed = *effect;
            *effect = self.effect_at(keys, pt, allowed);
        }
        Ok(())
    }

    fn DragOver(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // SAFETY: as above.
        unsafe {
            let allowed = *effect;
            *effect = self.effect_at(keys, pt, allowed);
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if let Ok(mut held) = self.held.lock() {
            *held = None;
        }
        if let Ok(mut shared) = self.shared.lock() {
            shared.hovering = None;
            shared.right_button = false;
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // What `DragEnter` read, unless it somehow did not run — in which case read it now
        // rather than leave the drop with nothing. Filled before `effect_at`, which takes
        // this same lock, and taken back out after.
        if let Ok(mut held) = self.held.lock() {
            if held.is_none() {
                *held = Some(Incoming::read(data.as_ref()));
            }
        }
        // SAFETY: OLE always passes a valid out-pointer here, holding on the way in the
        // effects the source allows.
        let chosen = unsafe {
            let allowed = *effect;
            let chosen = self.effect_at(keys, pt, allowed);
            *effect = chosen;
            chosen
        };

        // Asked for again here rather than reused from `DragEnter`, because for a source
        // that renders on demand *this* is the call that matters: 7-Zip extracts the
        // archive to answer it, and has nothing to give until the drop is real. Reusing
        // the drag-time read left the drop with an empty list, so nothing was extracted
        // and nothing was copied. The drag-time read stays as the fallback, for a source
        // that renders once and not again.
        let cached = self
            .held
            .lock()
            .ok()
            .and_then(|mut held| held.take())
            .map(|incoming| incoming.items)
            .unwrap_or_default();
        let items = match data.as_ref().and_then(paths_of) {
            Some(fresh) if !fresh.is_empty() => fresh,
            _ => cached,
        };
        // Converted before the lock is taken, not inside it.
        let at = self.in_client(pt);
        // Where it landed, and the drag forgotten, in a turn of the lock of its own — the
        // claim below touches the filesystem, and doing that while the frame loop waits on
        // this lock would stall the window for exactly as long as the claim takes.
        let landing = self.shared.lock().ok().map(|mut shared| {
            shared.hovering = None;
            let onto = shared.targets.at(at).cloned();
            (onto, std::mem::take(&mut shared.right_button))
        });
        let Some((Some(onto), asked)) = landing else {
            return Ok(());
        };
        if items.is_empty() || chosen == DROPEFFECT_NONE {
            return Ok(());
        }
        // The one thing that has to happen before this returns: see [`super::claim`]. Not
        // for a pin, which copies nothing and would otherwise bookmark a scratch folder.
        let items = match onto {
            Onto::Folder(_) => super::claim(items),
            Onto::Bookmarks => items,
        };
        if let Ok(mut shared) = self.shared.lock() {
            shared.dropped.push(Dropped {
                items,
                effect: if chosen.0 & DROPEFFECT_MOVE.0 != 0 {
                    Effect::Move
                } else {
                    Effect::Copy
                },
                at,
                onto,
                asked,
            });
        }
        Ok(())
    }
}

/// Every path a data object is offering, via `CF_HDROP`.
fn paths_of(data: &IDataObject) -> Option<Vec<PathBuf>> {
    use windows::Win32::System::Com::{FORMATETC, TYMED_HGLOBAL};
    use windows::Win32::System::Ole::ReleaseStgMedium;
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    const CF_HDROP: u16 = 15;
    let format = FORMATETC {
        cfFormat: CF_HDROP,
        ptd: std::ptr::null_mut(),
        dwAspect: 1,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: the medium is released on every path out, and the handle is only read
    // while it is held.
    unsafe {
        let medium = data.GetData(&format).ok()?;
        let handle = medium.u.hGlobal;
        if handle.is_invalid() {
            return None;
        }
        let drop = HDROP(handle.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut items = Vec::with_capacity(count as usize);
        for index in 0..count {
            // The length first: a path can be longer than `MAX_PATH`, and a fixed
            // buffer would silently truncate one.
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
        let mut medium = medium;
        ReleaseStgMedium(&mut medium);
        Some(items)
    }
}
