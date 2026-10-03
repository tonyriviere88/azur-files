//! OLE drag and drop, as a source and as a target.
//!
//! The Windows half of [`crate::shell::dnd`], and the two interfaces this program *implements*
//! rather than merely calls: `IDropSource` and `IDropTarget`.
//!
//! What the pointer is *told* is decided here too — [`Target_Impl::describe`], which turns the
//! zone under the pointer and the effect it answered into a sentence — but nothing here draws it.
//! The shell has a mechanism of its own for that, `CFSTR_DROPDESCRIPTION` on the data object, and
//! this program deliberately does not use it: see the module header of [`super`].

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
///
/// It holds the block the *target* writes, which is the only state it has. Both halves of a drag
/// inside this window run against the same one — the target on the UI thread, this on the drag's
/// own — and the cursor is the one answer that needs both ends: what the drop would do is the
/// target's, and setting the pointer is the source's. See [`Shared::silent`].
#[implement(IDropSource)]
struct Source {
    shared: Arc<Mutex<Shared>>,
}

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
        // **The drops this window says nothing about** — a folder over its own row where it was
        // picked up, and a move into the folder the items are already in; see [`Shared::silent`].
        // The standard cursors have no way to say nothing. The effect for both is
        // `DROPEFFECT_NONE`, so the set below would put a no-entry sign on the pointer, and that
        // sign is exactly the feedback these must not have: one would mark the first inch of every
        // drag of a folder as a mistake, and the other would make an empty gesture look like a
        // forbidden one. So the plain arrow is set here instead and OLE is told to leave the cursor
        // alone — which is what `S_OK` from this means.
        //
        // Set on every call rather than once, because OLE puts its own cursor back the moment the
        // answer below is the one it gets, and this runs between one `DragOver` and the next.
        if self.shared.lock().is_ok_and(|shared| shared.silent) {
            use windows::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_ARROW};

            // SAFETY: a stock cursor, shared and never freed, set on the thread holding the drag.
            unsafe {
                if let Ok(arrow) = LoadCursorW(None, IDC_ARROW) {
                    SetCursor(Some(arrow));
                }
            }
            return S_OK;
        }
        // Otherwise let OLE show the standard copy, move and no-entry cursors rather than
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
pub fn drag_out(items: &[PathBuf], ui_thread: u32, shared: Arc<Mutex<Shared>>) -> Option<Effect> {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};

    if items.is_empty() {
        return None;
    }
    let data = data_object(items)?;
    let source: IDropSource = Source { shared }.into();

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
    // What the *target* settled on, which is all a source learns. Only the move matters to the
    // caller — it is what takes files out of the folder this drag came from, so that folder has to
    // be re-read; see `App::pump_drag`.
    //
    // A **link** answers `None` deliberately, alongside a drop that was refused. A shortcut is
    // made in the destination and nothing whatever happens where the drag started, so there is no
    // more for this end to report than for a drop that went nowhere.
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
    /// How the frame loop is woken, for the reason set out on [`Shared::hovering`].
    ///
    /// Beside `hwnd` and outside the lock, for the same reason it is: the callbacks reach for this
    /// while the shared block is held.
    wake: egui::Context,
}

impl Target {
    pub fn new(shared: Arc<Mutex<Shared>>, hwnd: isize, wake: egui::Context) -> Self {
        Self {
            shared,
            held: Mutex::new(None),
            hwnd,
            wake,
        }
    }

    /// The same target with a drag already in flight, for a test that needs one.
    ///
    /// [`Incoming`] is read by `DragEnter` out of a real `IDataObject`, and a test has none to
    /// offer — the shell builds them. So the paths go straight in, and everything the callbacks
    /// then do with them is the code under test: which effect comes out, which drops are refused,
    /// and what the pointer is told. Without this, every callback test is a drag carrying nothing,
    /// which is the one case that refuses nothing.
    #[cfg(test)]
    pub fn holding(
        shared: Arc<Mutex<Shared>>,
        wake: egui::Context,
        items: Vec<PathBuf>,
        all_folders: bool,
    ) -> Self {
        Self {
            shared,
            held: Mutex::new(Some(Incoming {
                items,
                temporary: false,
                all_folders,
            })),
            hwnd: 0,
            wake,
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
    /// [`super::under_temp`].
    ///
    /// True if *any* of them is. A data object carrying files from two places at once is not
    /// something an archiver produces, but Explorer does: a drag out of a search result spans
    /// every folder it matched. Deciding this from the first path alone would offer such a drag
    /// as a move, and a move reported to a source is an instruction to delete what it handed
    /// over. `any` errs towards a copy, which is the answer that destroys nothing.
    temporary: bool,
    /// Whether *every* one of them is a folder, which is what decides whether the sidebar will
    /// take this drag at all — see [`super::refuses`].
    ///
    /// Asked here, once, for the reason everything else on this struct is: `is_dir` is a syscall,
    /// and the question is asked again on every mouse move while the drag is over the sidebar.
    all_folders: bool,
}

impl Incoming {
    fn read(data: Option<&IDataObject>) -> Self {
        let items = data.and_then(paths_of).unwrap_or_default();
        let temporary = items.iter().any(|item| super::under_temp(item));
        let all_folders = items.iter().all(|item| item.is_dir());
        Self {
            items,
            temporary,
            all_folders,
        }
    }
}

/// Whether Alt is down, which is how Explorer is asked for a shortcut.
///
/// # Two ways of asking, because the first one is not certain to answer
///
/// `MK_ALT` is documented on `IDropTarget`'s key state and it is **bit `0x20`** — which the
/// `windows` crate names twice, as `Ole::MK_ALT` and as `SystemServices::MK_XBUTTON1`. They are the
/// same bit with two meanings and the meaning is the caller's: in an OLE drag it is Alt, and the
/// `MODIFIERKEYS_FLAGS` set next door in this same function is the mouse-message family where it is
/// the first thumb button. `Ole::MK_ALT` is the one named here so that nobody reconciles the two.
///
/// What is *not* certain is that the drag loop fills it in — it is optional in the interface, and
/// this could not be driven from a test to find out: an Alt-drag is a real pointer under a real
/// modifier, and no synthesised event reaches `DoDragDrop`'s own modal loop. So the physical key is
/// read as well. `GetAsyncKeyState` and not `GetKeyState`, deliberately: the latter answers from the
/// calling thread's message queue, which for a drag coming out of *another* program has never seen
/// the key.
///
/// The cost of asking both is one call per `DragOver` that reads a byte the input system already
/// has, against a modifier that would otherwise silently do nothing.
#[cfg(windows)]
fn alt_held(keys: MODIFIERKEYS_FLAGS) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU};

    if keys.0 & windows::Win32::System::Ole::MK_ALT != 0 {
        return true;
    }
    // SAFETY: a read of the input system's own state; it takes nothing and retains nothing.
    // The high bit is "down now", which is the documented shape of the answer.
    unsafe { GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000 != 0 }
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

    /// What this drag would do at a point, given the keys held and what the source allows —
    /// and, on the way, the sentence the pointer is given saying so.
    fn effect_at(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        allowed: DROPEFFECT,
    ) -> DROPEFFECT {
        let at = self.in_client(pt);
        // The highlight is drawn from what is written just below, and a drag from another program
        // has nothing else asking for frames — see [`Shared::hovering`]. Asked for on every
        // `DragOver` rather than only when the point changes, because the frame that reads it is
        // the same frame that decides whether it has moved.
        //
        // **This is also what gets a drop acted on.** Every callback that carries a point comes
        // through here, `Drop` included — `DragLeave` is the one that does not, and asks for itself
        // — so the frame that reads `Shared::dropped` and starts the copy is this same request.
        // Made before the drop is pushed, which is in time, because
        // the frame it asks for cannot run until this callback has returned. Without it the copy
        // waited on the next unrelated event, a mouse twitch over the window usually, which is
        // why it looked prompt rather than broken.
        self.wake.request_repaint();
        let (region, started_in) = {
            let Ok(mut shared) = self.shared.lock() else {
                return DROPEFFECT_NONE;
            };
            shared.hovering = Some(at);
            // Remembered here because `Drop` is called with the button already released.
            if keys.0 & MK_RBUTTON.0 != 0 {
                shared.right_button = true;
            }
            // Both out of the same turn of the lock: the region says what is under the pointer and
            // this says whether the pointer has left where it started, and the two are answered
            // together below.
            (
                shared.targets.at(at).cloned(),
                shared.targets.started_in(at),
            )
        };
        // What it *would* do, and then whether it may. The two are worked out separately because
        // the sentence needs both: a refusal is drawn as the gesture it is refusing — *Cannot move
        // src into main* — so the verb has to survive the answer coming out as nothing.
        let would = self.wanted(keys, region.as_ref(), allowed);
        let refused = self.refuses(region.as_ref());
        // And whether there is anything here to do at all: a move into the folder the items are
        // already in is not a drop that fails, it is a drop with nothing in it.
        let nothing = self.does_nothing(region.as_ref(), keys, would);
        // The two the pointer keeps to itself — see [`Shared::silent`]. The refusal stands in both;
        // only the saying of it goes.
        let silent = nothing || (refused == Some(Refused::Itself) && started_in);
        // Then the words, in a turn of the lock of its own — the effect is worked out from the
        // region and from `held`, and holding the shared block across that would stall the frame
        // loop for no reason.
        //
        // A drop with nothing in it is described as *nowhere*: handing the sentence no region is
        // what says there is nothing to promise, and it is also what stands the destination's
        // highlight down, since the frame loop draws that from what it was told.
        self.describe(
            if nothing { None } else { region.as_ref() },
            would,
            refused,
            silent,
        );
        if refused.is_some() || nothing {
            DROPEFFECT_NONE
        } else {
            would
        }
    }

    /// Whether the drop under the pointer would do nothing whatever — see [`super::does_nothing`],
    /// which is the rule and is portable and tested on its own.
    ///
    /// The two halves that are not portable: which effect this gesture is asking for, which is what
    /// makes the difference between a move that has nowhere to go and a copy that has an answer,
    /// and whether the *right* button is carrying it — a right drag has not decided which it is
    /// yet, and the menu it opens on drop is where it says so.
    fn does_nothing(
        &self,
        region: Option<&Region>,
        keys: MODIFIERKEYS_FLAGS,
        would: DROPEFFECT,
    ) -> bool {
        let Some(region) = region else {
            return false;
        };
        let moving = would.0 & DROPEFFECT_MOVE.0 != 0;
        let asked = keys.0 & MK_RBUTTON.0 != 0;
        self.held.lock().is_ok_and(|held| {
            held.as_ref().is_some_and(|incoming| {
                super::does_nothing(&region.onto, &incoming.items, moving, asked)
            })
        })
    }

    /// Why the drop under the pointer is one this program will not make, if it is.
    ///
    /// The rule is [`super::refuses`], which is portable and tested on its own; all this adds is
    /// the drag's own list of paths, read when it arrived. A drag that has said nothing about what
    /// it holds refuses nothing — see [`Incoming`].
    fn refuses(&self, region: Option<&Region>) -> Option<Refused> {
        let region = region?;
        self.held.lock().ok().and_then(|held| {
            held.as_ref().and_then(|incoming| {
                super::refuses(&region.onto, &incoming.items, incoming.all_folders)
            })
        })
    }

    /// Which effect this drag asks for over a region, given the keys held and what the source
    /// allows.
    fn wanted(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        region: Option<&Region>,
        allowed: DROPEFFECT,
    ) -> DROPEFFECT {
        let target = match region.map(|region| &region.onto) {
            Some(Onto::Folder(path)) => path,
            // Pinning moves nothing, so it answers `LINK` whatever is held down. It is
            // also the only honest answer: a copy cursor over the sidebar would be
            // promising a copy that is not going to happen.
            Some(Onto::Bookmarks | Onto::BookmarkGroup(_)) => {
                return permitted(DROPEFFECT_LINK, allowed)
            }
            None => return DROPEFFECT_NONE,
        };

        // **Explorer's four, in Explorer's order of precedence.** Ctrl+Shift and Alt both mean
        // "make a shortcut", and the pair has to be tested *before* either key alone or a thumb
        // resting on Shift turns a deliberate Ctrl+Shift into a plain copy.
        let ctrl = keys.0 & MK_CONTROL.0 != 0;
        let shift = keys.0 & MK_SHIFT.0 != 0;
        if alt_held(keys) || (ctrl && shift) {
            return permitted(DROPEFFECT_LINK, allowed);
        }
        if ctrl {
            return permitted(DROPEFFECT_COPY, allowed);
        }
        if shift {
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
        // A drag with nothing held is never a shortcut — Explorer's unmodified rule is the
        // volume one and no more, so [`super::default_effect`] has only two answers. The third arm
        // is here because the type has three, and mapping it to anything but its own effect would
        // be a lie waiting for somebody to widen that function.
        let wanted = match super::default_effect(source.as_deref(), target) {
            Effect::Move => DROPEFFECT_MOVE,
            Effect::Copy => DROPEFFECT_COPY,
            Effect::Link => DROPEFFECT_LINK,
        };
        permitted(wanted, allowed)
    }

    /// Work out what to say this drop will do, and publish it for the frame loop to draw.
    ///
    /// The **destination decides the verb, not the effect**: a drop onto the sidebar pins whatever
    /// it is handed, and a source that offers no `DROPEFFECT_LINK` has that answer degraded to a
    /// copy on the way out — see [`permitted`] — so reading the verb back off the effect would
    /// have the pointer promise a *copy into Bookmarks* for a gesture that copies nothing. Over a
    /// folder the effect is exactly what decides it, because there the two really are the same
    /// question.
    ///
    /// Over anything that would not take the drop there is nothing to say, which is the same thing
    /// the effect coming out as `DROPEFFECT_NONE` already says. **A refusal is not that**: it is a
    /// place that would have taken the drop and will not take *this* one, so `would` is the effect
    /// it would have had and the sentence is drawn from that with `refused` on it.
    ///
    /// `silent` rides along because it is written into the same block in the same turn of the lock:
    /// the sentence and whether to say it out loud are one decision as far as anything reading them
    /// is concerned, and a frame that saw one without the other would draw a refusal for a gesture
    /// the cursor was keeping quiet about. See [`Shared::silent`].
    fn describe(
        &self,
        region: Option<&Region>,
        would: DROPEFFECT,
        refused: Option<Refused>,
        silent: bool,
    ) {
        let doing = match region {
            Some(region) if would != DROPEFFECT_NONE => match region.onto {
                Onto::Bookmarks | Onto::BookmarkGroup(_) => Some(Doing::Pin),
                Onto::Folder(_) if would.0 & DROPEFFECT_MOVE.0 != 0 => Some(Doing::Move),
                // Before the copy, because `permitted` degrades a link the source will not allow
                // *to* a copy — so by the time this is reached, `LINK` means the source agreed to
                // it and a shortcut really is what the drop will make.
                Onto::Folder(_) if would.0 & DROPEFFECT_LINK.0 != 0 => Some(Doing::Link),
                Onto::Folder(_) => Some(Doing::Copy),
            },
            _ => None,
        };
        let told = match (doing, region) {
            (Some(doing), Some(region)) => {
                // What the drag is holding, from the paths read when it arrived — see [`Incoming`].
                // The *drag's* list rather than the pane's selection, so this is right for a drag
                // out of another window as well as for one out of this one; described inside the
                // lock so the list is never copied to be counted.
                //
                // **A refusal names the one item it is about instead**, which for a single-file
                // drag is the same string and for a mixed selection is the difference between an
                // explanation and a sentence about nothing — see [`super::culprit`].
                let source = self.held.lock().ok().and_then(|held| {
                    let items = held.as_ref().map_or(&[][..], |incoming| &incoming.items);
                    match (refused, &region.onto) {
                        (Some(Refused::Itself | Refused::Inside), Onto::Folder(into)) => {
                            super::culprit(items, into).map(crate::fs::display_name)
                        }
                        _ => super::carrying(items),
                    }
                });
                Some(Told {
                    doing,
                    refused,
                    source,
                    target: region.name.clone(),
                })
            }
            _ => None,
        };
        if let Ok(mut shared) = self.shared.lock() {
            shared.telling = told;
            shared.silent = silent;
        }
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
            shared.telling = None;
            shared.silent = false;
        }
        // A highlight that is never taken down is worse than one that never appears: the drag has
        // left the window and the row it was over would go on claiming the drop.
        self.wake.request_repaint();
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
            shared.telling = None;
            shared.silent = false;
            let region = shared.targets.at(at).cloned();
            (region, std::mem::take(&mut shared.right_button))
        });
        let Some((Some(region), asked)) = landing else {
            return Ok(());
        };
        let onto = region.onto;
        if items.is_empty() || chosen == DROPEFFECT_NONE {
            return Ok(());
        }
        // The one thing that has to happen before this returns: see [`super::claim`]. Not
        // for a pin, which copies nothing and would otherwise bookmark a scratch folder.
        let items = match onto {
            Onto::Folder(_) => super::claim(items),
            Onto::Bookmarks | Onto::BookmarkGroup(_) => items,
        };
        if let Ok(mut shared) = self.shared.lock() {
            shared.dropped.push(Dropped {
                items,
                // Read off the effect that was *answered*, which is the one the source agreed
                // to: `permitted` has already degraded anything it would not allow, so a `LINK`
                // reaching here is a shortcut the drag really is willing to have made.
                effect: match chosen {
                    _ if chosen.0 & DROPEFFECT_MOVE.0 != 0 => Effect::Move,
                    _ if chosen.0 & DROPEFFECT_LINK.0 != 0 => Effect::Link,
                    _ => Effect::Copy,
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

/// Hand `into`'s own `IDropTarget` a `DROPEFFECT_LINK` drop of `items` — the call Explorer makes
/// for an Alt-drag — and say whether the shell took it.
///
/// **The measuring stick, and it is only ever used as one.** Production makes its shortcuts through
/// [`crate::shell::links::shortcuts_into`], for the reason set out there: this route reports nothing
/// back, so nothing it does can be undone. What it is good for is establishing what the names ought
/// to be, which is `the_names_match_the_shell_s_own_link_drop`.
#[cfg(all(test, windows))]
pub(crate) fn link_drop_through_the_shell(items: &[PathBuf], into: &std::path::Path) -> bool {
    use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, BHID_SFUIObject};

    // SAFETY: the data object and the drop target are the shell's own, released when they go out
    // of scope here; the three callbacks are the documented sequence and nothing is retained.
    unsafe {
        let Some(data) = data_object(items) else {
            return false;
        };
        let wide = crate::shell::wide(into);
        let folder: IShellItem =
            match SHCreateItemFromParsingName(windows::core::PCWSTR(wide.as_ptr()), None) {
                Ok(folder) => folder,
                Err(_) => return false,
            };
        let Ok(target) = folder.BindToHandler::<_, IDropTarget>(None, &BHID_SFUIObject) else {
            return false;
        };
        let pt = POINTL { x: 0, y: 0 };
        let keys = MODIFIERKEYS_FLAGS(0);
        let mut effect = DROPEFFECT_LINK;
        if target.DragEnter(&data, keys, pt, &mut effect).is_err() {
            return false;
        }
        // Answered as a link, or the folder is one that will not take one and the comparison
        // would be against nothing.
        if effect.0 & DROPEFFECT_LINK.0 == 0 {
            let _ = target.DragLeave();
            return false;
        }
        effect = DROPEFFECT_LINK;
        target.Drop(Some(&data), keys, pt, &mut effect).is_ok()
    }
}
