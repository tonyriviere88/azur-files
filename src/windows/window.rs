//! The window itself, before anything has been drawn into it.
//!
//! Five things this program does to its own window that no cross-platform layer offers, each
//! for a reason the doc comment states: where a restored window opens, how a maximised one
//! opens without being seen on the way up, keeping the compositor from showing it too early,
//! whether a remembered position still has a screen under it, and putting back the Ctrl+V that
//! `egui-winit` swallows.
//!
//! The Windows half of `crate::main`. The `#[cfg(not(windows))]` stubs stay next to their
//! callers, so another platform is a sibling of this folder and nothing else moves.

use super::shell;

/// Put the window back where it was left.
///
/// **From here rather than through eframe, and in physical pixels.** Three ways to place a
/// window were available and two of them are wrong:
///
/// - `ViewportBuilder::with_position` takes *points*, and the platform turns them into pixels
///   with the scale factor of whichever monitor it happened to open the window on — not the
///   one being aimed at. On a desktop where the laptop panel is at 150% and the external
///   monitors are at 100%, restoring a window to an external monitor lands it half again too
///   far out, which is frequently a different screen or none.
/// - `ViewportCommand::OuterPosition` from inside the first frame *is* exact — it multiplies by
///   the same scale factor [`App`] divided by when it saved the position. But eframe reveals the
///   window in `post_rendering`, which runs *before* the frame's viewport commands are applied,
///   so the move lands after the window is already on screen somewhere else.
/// - This runs in the creation closure: the window exists, nothing has been painted into it, and
///   eframe keeps it hidden until something has been. The pixels go straight to the platform in
///   the units the platform works in, so no scale factor is involved on either side of the round
///   trip.
///
/// Nothing is clamped here. Whether the position is still on a monitor is
/// [`reachable`]'s question, asked before the window is built at all — because a window with no
/// screen under its title bar cannot be dragged back, and this one draws its own title bar.
#[cfg(windows)]
pub(super) fn restore_position(cc: &eframe::CreationContext<'_>, position: Option<[f32; 2]>) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    let Some([x, y]) = position else { return };
    let window = shell::Owner::from_handle(cc);
    if window.0 == 0 {
        return;
    }
    // SAFETY: a window this process owns, moved and not resized, restacked or activated.
    let _ = unsafe {
        SetWindowPos(
            window.hwnd(),
            None,
            x as i32,
            y as i32,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// Open maximised without the window being seen on the way up.
///
/// **`ViewportBuilder::with_maximized` cannot be used for this on Windows.** It flashes the
/// window on screen *three times*, unpainted, before eframe reveals it: a white rectangle the
/// size of the screen, then one the size of the last session's window, then a screen-sized one
/// again — measured at 51, 169 and 243 ms into a run whose first frame was not drawn until
/// 1478, so all three are over and gone before the graphics device even exists.
///
/// The cause is one habit of winit's: every state change it makes to a window goes through
/// `ShowWindow`, and `SW_MAXIMIZE` and `SW_RESTORE` *show* a window as well as resize it. It
/// hides the window again immediately afterwards — but the compositor has already been handed a
/// window with nothing drawn in it, and it draws that white. Three of those calls happen before
/// this program is given the handle:
///
/// 1. winit maximises the freshly created window, because the builder asked for it.
/// 2. `egui-winit` then applies the builder's *size* to the same window, and winit un-maximises
///    any window whose size is set — so `SW_RESTORE`, at the size in the settings file.
/// 3. `egui-winit` then applies the builder's maximised flag, so `SW_MAXIMIZE` again.
///
/// None of the three can be stopped from here, so none of them is asked for: the builder is told
/// nothing about maximising, and this puts the window into the state those calls were trying to
/// reach — without `ShowWindow`, which is the only part of them that shows it.
///
/// **What it does instead**, on the window eframe has created and is still keeping hidden:
///
/// - Records where a restore should land, since a maximised window has to have somewhere to go
///   back to. The platform keeps that rectangle itself, and the only call that sets it is
///   `SetWindowPlacement` — which would show the window too, at whatever `showCmd` it is
///   handed, so it is handed `SW_HIDE`: the state the window is already in.
/// - Sets `WS_MAXIMIZE` and moves the window onto the work area of the monitor it was created
///   on — the same monitor the platform would have maximised it onto. With the style bit set
///   first, the move is a *maximise*: `WM_SIZE` arrives as `SIZE_MAXIMIZED`, which is where
///   winit learns the state it no longer set itself, so egui reports a maximised window from
///   the first frame on and the title bar draws the right button. The platform then adjusts the
///   rectangle the way it adjusts any maximised window's, so the client area is the work area
///   exactly rather than approximately.
///
/// The window is still hidden through all of it. eframe reveals it once, after the first frame
/// has been drawn, already maximised — which is the whole point.
#[cfg(windows)]
pub(super) fn open_maximized(cc: &eframe::CreationContext<'_>, position: Option<[f32; 2]>) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, GetWindowPlacement, GetWindowRect, SetWindowLongW, SetWindowPlacement,
        SetWindowPos, GWL_STYLE, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, WINDOWPLACEMENT,
        WS_MAXIMIZE,
    };

    let window = shell::Owner::from_handle(cc);
    if window.0 == 0 {
        return;
    }
    let hwnd = window.hwnd();

    // The monitor first, and nothing is changed until it has answered: half of this — a window
    // wearing `WS_MAXIMIZE` at the size of the last session — would be worse than none of it.
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut screen = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: a pure query about a monitor handle, into a struct whose size is declared.
    if !unsafe { GetMonitorInfoW(monitor, &mut screen) }.as_bool() {
        return;
    }
    let work = screen.rcWork;

    // Where the restore button will put it: the size the window has *right now*, at the place the
    // settings file remembers. Reading the size off the window rather than out of the settings is
    // what keeps a scale factor out of this — eframe has already turned the remembered size into
    // pixels, with the scale factor of the monitor the window actually opened on, which is the
    // one conversion nothing here could do as well.
    let mut rect = RECT::default();
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: a window this process owns, read into two structs, one of whose size is declared.
    // `rcNormalPosition` as it stands is the rectangle the window was *created* at rather than
    // the one it was then sized to, which is why it is replaced rather than moved.
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok()
        && unsafe { GetWindowPlacement(hwnd, &mut placement) }.is_ok()
    {
        let [x, y] = position
            .map(|[x, y]| [x as i32, y as i32])
            .unwrap_or([rect.left, rect.top]);
        placement.showCmd = SW_HIDE.0 as u32;
        placement.rcNormalPosition = RECT {
            left: x,
            top: y,
            right: x + (rect.right - rect.left),
            bottom: y + (rect.bottom - rect.top),
        };
        // SAFETY: as above, with `SW_HIDE` for a window that is hidden — so the only thing this
        // call changes is the rectangle a restore reads.
        let _ = unsafe { SetWindowPlacement(hwnd, &placement) };
    }

    // SAFETY: a window this process owns. `GetWindowLongW` is a read; the style it is given
    // back differs by one bit, the one the platform reads to mean "maximised".
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    unsafe { SetWindowLongW(hwnd, GWL_STYLE, (style | WS_MAXIMIZE.0) as i32) };
    // SAFETY: a window this process owns, resized and not restacked or activated.
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            work.left,
            work.top,
            work.right - work.left,
            work.bottom - work.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// What the window was before it filled the screen, so it can be put back exactly.
///
/// Four numbers and a bit, kept here rather than in [`crate::app::App`] because it is a Win32
/// rectangle in physical pixels and nothing portable has any use for it. One window, so one slot.
#[cfg(windows)]
static SAVED: std::sync::Mutex<Option<([i32; 4], bool)>> = std::sync::Mutex::new(None);

/// Fill the monitor with this window, or put it back exactly as it was.
///
/// # Why this is not `ViewportCommand::Fullscreen`
///
/// Because of what this window *is*. It is maximised by carrying `WS_MAXIMIZE` and sitting on the
/// monitor's **work area** — see [`open_maximized`], whose whole point is that the client area is the
/// work area exactly. The work area is the screen minus the taskbar, and Windows keeps a window
/// wearing that bit clamped to it. So asking such a window to go fullscreen grew the window and left
/// the client area behind, and the program drew into a surface one taskbar shorter than the window it
/// was in: a band along the bottom, exactly one taskbar tall, of pixels nothing ever painted.
///
/// Un-maximising first fixes that and costs an animation each way — the window shrinking to its
/// restored size and then growing to the monitor, twice per transition, both of which the platform
/// animates. What a video filling the screen wants is for the screen to *be* filled, now.
///
/// So the whole transition is one `SetWindowPos`, which does not animate: the style bit comes off, the
/// window goes to the monitor's bounds, and the way back is the same move in reverse — through
/// [`open_maximized`]'s own recipe when the window was maximised, so `WM_SIZE` arrives as
/// `SIZE_MAXIMIZED` and winit learns the state it is in.
///
/// **Topmost while it is filling the screen**, which is the one thing here that is not merely a
/// restatement of the window's own geometry. Windows hides the taskbar for a foreground window that
/// covers a monitor, but that is a heuristic about the shell's own idea of "fullscreen" rather than a
/// promise — and the failure is the taskbar sitting over the controls at the bottom of the video,
/// which is exactly where they are. Given back on the way out.
#[cfg(windows)]
pub(crate) fn fill_screen(owner: shell::Owner, on: bool) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, GetWindowRect, SetWindowLongW, SetWindowPos, GWL_STYLE, HWND_NOTOPMOST,
        HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, WS_MAXIMIZE,
    };

    if owner.0 == 0 {
        return;
    }
    let hwnd = owner.hwnd();
    // SAFETY: every call below is against a window this process owns. The two style calls read and
    // write one bit; the rest are a read of a rectangle and a move.
    unsafe {
        // Which monitor this window is on, and `None` when the platform will not say — every caller
        // of it here answers that the same way, by leaving the window alone rather than moving it to
        // a rectangle that was guessed at. A `MONITORINFO` has to be told its own size before it can
        // be filled in, which is the part worth writing once.
        let monitor = || {
            let mut screen = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut screen)
                .as_bool()
                .then_some(screen)
        };
        // And the move, which is the same move every way through here: `SWP_FRAMECHANGED` because
        // the style bit changed underneath it, and never activating, because the window being moved
        // is the one already in front.
        let place = |z, to: RECT| {
            let _ = SetWindowPos(
                hwnd,
                Some(z),
                to.left,
                to.top,
                to.right - to.left,
                to.bottom - to.top,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        };

        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;

        if on {
            // Both reads first, and nothing is changed until they have both answered — the same
            // order `open_maximized` takes, and for the same reason: half of this would be worse
            // than none of it.
            let Some(screen) = monitor() else { return };
            let mut was = RECT::default();
            if GetWindowRect(hwnd, &mut was).is_err() {
                return;
            }
            if let Ok(mut slot) = SAVED.lock() {
                let maximised = style & WS_MAXIMIZE.0 != 0;
                *slot = Some(([was.left, was.top, was.right, was.bottom], maximised));
            }
            // **The bit comes off before the move**, or the move is clamped to the work area and this
            // is the bug it exists to prevent.
            SetWindowLongW(hwnd, GWL_STYLE, (style & !WS_MAXIMIZE.0) as i32);
            place(HWND_TOPMOST, screen.rcMonitor);
            return;
        }

        // And back. Nothing saved means nothing to go back to, which is a window that was never
        // filling the screen — leave it alone rather than guess at a rectangle for it.
        let Some((rect, was_maximised)) = SAVED.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        if was_maximised {
            // `open_maximized`'s recipe: the bit on, then onto the work area, so the platform adjusts
            // the rectangle the way it adjusts any maximised window's and `WM_SIZE` says
            // `SIZE_MAXIMIZED`.
            if let Some(screen) = monitor() {
                SetWindowLongW(hwnd, GWL_STYLE, (style | WS_MAXIMIZE.0) as i32);
                place(HWND_NOTOPMOST, screen.rcWork);
                return;
            }
        }
        let [left, top, right, bottom] = rect;
        place(
            HWND_NOTOPMOST,
            RECT {
                left,
                top,
                right,
                bottom,
            },
        );
    }
}

/// Keep the compositor from showing this window, or stop keeping it.
///
/// **The last white frame, and why hiding the window was not enough to stop it.** eframe keeps a
/// new window hidden until it has painted into it, and then shows it — which is the right idea and
/// a frame short of working. Showing the window is not the same event as the compositor having
/// something of this program's to put on the screen, and in between it puts up white: for a
/// maximised window that is the whole screen, which is why it is far more obvious than the same
/// frame in a window covering a fifth of it.
///
/// Nothing about the *window* can fix that, because by the time this program is given the handle
/// the window is already the one eframe will reveal. What can be fixed is what the compositor does
/// with it: `DWMWA_CLOAK` is the compositor's own "this window exists but is not to be drawn",
/// which — unlike hiding — leaves a window that paints, presents and is composed as usual. So the
/// window is cloaked before it is ever shown, eframe reveals it into the cloak, and this program
/// decides when it can be looked at.
///
/// **Which is on the third frame, and one is measurably not enough.** `examples/flash.rs` reads
/// nine points of the composited desktop thousands of times a second while the window opens, and
/// counts the runs that put white on any of them. Over five launches of a maximised window, each:
///
/// | | white frames |
/// | --- | --- |
/// | as eframe leaves it, no cloak | 10 |
/// | cloaked, revealed after one frame | 5 |
/// | **cloaked, revealed after two** | **0** |
///
/// Two frames rather than one is not a guess about the compositor's internals — from inside the
/// process the first frame is presented and the screen is white anyway, and this program cannot
/// see why. It is the smallest number that measured clean, and the cost of it is one frame of a
/// window nobody can see yet. A `DwmFlush` before the uncloak was tried on the same instrument and
/// made no difference at all, so it is not here: waiting on the compositor without a reason to is
/// still a blocked frame.
#[cfg(windows)]
pub(super) fn cloak(handle: &impl raw_window_handle::HasWindowHandle, hidden: bool) {
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};

    let window = shell::Owner::from_handle(handle);
    if window.0 == 0 {
        return;
    }
    let flag = windows::core::BOOL::from(hidden);
    // SAFETY: a window this process owns, and one `BOOL` of the size declared.
    let _ = unsafe {
        DwmSetWindowAttribute(
            window.hwnd(),
            DWMWA_CLOAK,
            (&raw const flag).cast(),
            std::mem::size_of::<windows::core::BOOL>() as u32,
        )
    };
}

/// Whether a remembered position still has a screen under it.
///
/// The one failure worth guarding against: the window was last closed on a monitor that is no
/// longer connected, so the saved position names a place that no longer exists. Windows does not
/// clamp a window into view, and this one has no caption of its own for the platform's
/// Move command to work on — so it would open as a taskbar button with nothing on screen, which
/// is indistinguishable from a program that failed to start.
///
/// The point tested is a little way into the title bar rather than the corner, and in physical
/// pixels, so it is on the bar itself at every scale factor: 16px down is inside a 32-point bar
/// at 100% and a 48-pixel one at 150%. If *that* has a monitor under it the window can be picked
/// up and dragged, which is the whole of what has to be true.
#[cfg(windows)]
pub(super) fn reachable(position: &[f32; 2]) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONULL};

    let [x, y] = *position;
    let grab = POINT {
        x: x as i32 + 60,
        y: y as i32 + 16,
    };
    // SAFETY: a pure query about a point on the desktop.
    !unsafe { MonitorFromPoint(grab, MONITOR_DEFAULTTONULL) }.is_invalid()
}

/// Put back the Ctrl+V that never arrives.
///
/// `egui-winit` does not deliver Ctrl+C, Ctrl+X or Ctrl+V as key presses. It recognises them
/// itself and queues `Event::Copy`, `Event::Cut` or `Event::Paste` *instead*, returning before the
/// `Event::Key` is added at all — so `i.key_pressed(Key::C)` is never true for a copy, however
/// reasonable that looks to write. Worse for paste: the `Event::Paste` it substitutes carries the
/// clipboard's **text**, and it is only queued when there is some. Files are not text, so pressing
/// Ctrl+V over a listing produced no event of any kind and the shortcut simply did not exist.
///
/// Copy and cut need nothing here — `Event::Copy` and `Event::Cut` always arrive, and
/// [`App::keyboard`] reads them. Paste has nothing to arrive, so the keystroke is put back from
/// the only place that still knows about it: the keyboard.
///
/// # Why the latch is cleared by an event and not by the key going up
///
/// The swallowed press still asks for a repaint, so there *is* a frame in which the keys read as
/// down, and one paste per press needs a latch to stop the following frames repeating it. The
/// obvious way to clear that latch is to notice the key is no longer down — and it does not work,
/// because after a paste the window has nothing to draw and stops running frames entirely. The
/// release goes unobserved, the latch stays set, and Ctrl+V works exactly once per session.
/// Measured, and it is what a paste that "worked once and then no longer" turns out to be.
///
/// `Key { key: V, pressed: false }` is not swallowed and always arrives, and delivering it *is* a
/// frame. So the release clears the latch through the event queue rather than through a sample
/// nobody was awake to take.
#[cfg(windows)]
pub(super) fn paste_keystroke(raw_input: &mut egui::RawInput) {
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_V};

    /// Whether this press has already been acted on.
    static ACTED: AtomicBool = AtomicBool::new(false);

    // The release, whenever it comes, re-arms.
    let released = raw_input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: egui::Key::V,
                pressed: false,
                ..
            }
        )
    });
    if released {
        ACTED.store(false, Relaxed);
    }

    // SAFETY: a pure query of the keyboard state.
    let down = unsafe {
        GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0
            && GetAsyncKeyState(VK_V.0 as i32) as u16 & 0x8000 != 0
    };
    if !down {
        ACTED.store(false, Relaxed);
        return;
    }
    // `focused` because `GetAsyncKeyState` answers for the whole desktop, and a Ctrl+V meant for
    // another window is not this program's to act on.
    if !raw_input.focused || ACTED.swap(true, Relaxed) {
        return;
    }
    // Unless egui-winit found text to paste, in which case it has already queued one.
    if !raw_input
        .events
        .iter()
        .any(|event| matches!(event, egui::Event::Paste(_)))
    {
        raw_input.events.push(egui::Event::Paste(String::new()));
    }
}
