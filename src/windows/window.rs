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
///
/// **`Some` is also what "this window is filling the screen" means**, and that is not merely a
/// convenience: it is what makes [`fill_screen`] safe to call twice. The first version of this
/// overwrote the slot on every way in, so a second `fill_screen(true)` recorded the *monitor's*
/// rectangle as the shape to go back to — after which leaving fullscreen put the window back to
/// exactly the size it already was, which reads as the restore having done nothing at all.
#[cfg(windows)]
static SAVED: std::sync::Mutex<Option<([i32; 4], bool)>> = std::sync::Mutex::new(None);

/// Enumerate the monitors: each one's whole rectangle, and its work area.
///
/// Both, because the two together are what a taskbar is: the band of a monitor that is in
/// `rcMonitor` and not in `rcWork`. See [`span_screens`], the only caller.
#[cfg(windows)]
fn monitors() -> Vec<(windows::Win32::Foundation::RECT, windows::Win32::Foundation::RECT)> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };

    // SAFETY: called by `EnumDisplayMonitors` below, once per monitor, with the `LPARAM` this
    // program handed it — a pointer to the `Vec` on that call's own stack, which cannot have gone
    // away because the enumeration has not returned yet. Nothing else is dereferenced.
    unsafe extern "system" fn each(
        monitor: HMONITOR,
        _dc: HDC,
        _clip: *mut RECT,
        out: LPARAM,
    ) -> BOOL {
        let mut screen = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut screen) }.as_bool() {
            let found = out.0 as *mut Vec<(RECT, RECT)>;
            if let Some(found) = unsafe { found.as_mut() } {
                found.push((screen.rcMonitor, screen.rcWork));
            }
        }
        // Keep going: every monitor is wanted, and one that would not answer is one to leave out
        // rather than a reason to stop asking about the rest.
        true.into()
    }

    let mut found: Vec<(RECT, RECT)> = Vec::new();
    // SAFETY: a pure query of the desktop. The callback above is the only thing handed the pointer,
    // and it is only called while this frame is live.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(each),
            LPARAM((&raw mut found) as isize),
        )
    };
    found
}

/// The rectangle that covers every monitor and leaves the taskbar showing.
///
/// # Why it is not simply the union of the work areas
///
/// A window is one rectangle and a desktop of several monitors is not, so covering all of them means
/// a bounding box either way — and the two obvious boxes are both wrong. The union of the *monitors*
/// covers the taskbar, which is the one thing this was asked not to do. The union of the *work areas*
/// covers it too whenever the taskbar is not on the outermost monitor: a taskbar along the bottom of
/// the primary screen shortens that monitor's work area, but the second monitor's reaches its own
/// bottom edge, and the union takes the larger of the two.
///
/// So the box is the union of the monitors, **pulled in at each edge by whatever the monitors on that
/// edge reserve**: for every monitor that reaches the box's bottom, the box's bottom comes back to
/// that monitor's work area. The taskbar stays visible, and what it costs is a band of desktop left
/// showing along the bottom of the monitors that do not have one — which is the price of a window
/// being a rectangle, and cheaper than a window with a strip of itself underneath the taskbar.
///
/// `None` when the platform will not enumerate its monitors, which every caller answers by leaving
/// the window alone rather than by moving it to a rectangle that was guessed at.
#[cfg(windows)]
fn spanning_rect() -> Option<windows::Win32::Foundation::RECT> {
    across(&monitors())
}

/// The arithmetic of [`spanning_rect`], over monitors somebody has already listed.
///
/// Its own function so that the rule can be checked against desktops this machine does not have —
/// two screens with the taskbar on the left-hand one, screens of different heights, a taskbar down
/// the side. See the tests at the foot of this file; a monitor layout is not something a test can
/// arrange for real.
#[cfg(windows)]
fn across(
    found: &[(windows::Win32::Foundation::RECT, windows::Win32::Foundation::RECT)],
) -> Option<windows::Win32::Foundation::RECT> {
    use windows::Win32::Foundation::RECT;

    if found.is_empty() {
        return None;
    }

    let mut box_ = RECT {
        left: i32::MAX,
        top: i32::MAX,
        right: i32::MIN,
        bottom: i32::MIN,
    };
    for (screen, _) in found {
        box_.left = box_.left.min(screen.left);
        box_.top = box_.top.min(screen.top);
        box_.right = box_.right.max(screen.right);
        box_.bottom = box_.bottom.max(screen.bottom);
    }

    // Each edge comes in as far as the monitors sitting on it ask for. Measured against the box the
    // monitors made rather than against the one being narrowed, so the four edges are decided
    // independently — a taskbar on the left of one screen has nothing to say about the bottom.
    let whole = box_;
    let mut work = box_;
    for (screen, usable) in found {
        if screen.left == whole.left {
            work.left = work.left.max(usable.left);
        }
        if screen.top == whole.top {
            work.top = work.top.max(usable.top);
        }
        if screen.right == whole.right {
            work.right = work.right.min(usable.right);
        }
        if screen.bottom == whole.bottom {
            work.bottom = work.bottom.min(usable.bottom);
        }
    }
    (work.right > work.left && work.bottom > work.top).then_some(work)
}

/// Spread this window across every monitor, taskbar excepted. `Ctrl+Win+Up`.
///
/// **The same move `fill_screen` makes and for the same reasons**, which is why they are neighbours:
/// the style bit comes off first, or the platform clamps the window to the work area of the *one*
/// monitor it thinks it is maximised on and the span stops at that monitor's edge. One `SetWindowPos`
/// after that, which does not animate — a window crossing three monitors in the platform's own
/// maximise animation is a second of nothing happening.
///
/// **Not topmost**, unlike the fullscreen video: the taskbar is deliberately still showing here, and
/// a window that covered it after all the arithmetic above went to the trouble of leaving room would
/// be the arithmetic wasted.
///
/// What it is *not* is a maximised window. The platform's idea of maximised is one monitor, so the
/// style bit stays off and the window is an ordinary restored window that happens to be the size of
/// the desktop — which is also what makes `Ctrl+Win+Down` able to put it back with an ordinary
/// resize. The window is left rememberable: it is a size like any other, and reopening at it is what
/// the settings file does with every size.
#[cfg(windows)]
pub(crate) fn span_screens(owner: shell::Owner) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_STYLE, HWND_NOTOPMOST, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, WS_MAXIMIZE,
    };

    if owner.0 == 0 {
        return false;
    }
    let Some(to) = spanning_rect() else {
        return false;
    };
    let hwnd = owner.hwnd();
    // SAFETY: a window this process owns. The style calls read and write one bit; the move is a
    // move, and never activates — the window being moved is the one already in front.
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        SetWindowLongW(hwnd, GWL_STYLE, (style & !WS_MAXIMIZE.0) as i32);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_NOTOPMOST),
            to.left,
            to.top,
            to.right - to.left,
            to.bottom - to.top,
            SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    true
}

/// One pixel out past every edge of `screen`, which is where a window filling it goes.
///
/// **The hairline along the top edge of a fullscreen video, and why the frame is the wrong place to
/// look for it.** Nothing this program draws could have made it: while a video fills the screen the
/// frame paints the picture over every point of the window and returns before the one-pixel border at
/// the foot of [`crate::app::App::frame`] is reached. Nor is it the compositor's border — [`borderless`]
/// takes that off, and the line survived it.
///
/// It is the *non-client frame* of a resizable window with no caption. winit keeps `WS_THICKFRAME` on
/// an undecorated window, because that is what makes it resizable at all, and Windows draws a
/// one-pixel sizing edge for that style however little else of the frame is left. Windows' own
/// borderless windows do not show it because a **maximised** window's frame is pushed off the monitor
/// on every side; a window merely *placed* at the monitor's rectangle keeps its frame inside the
/// screen, and the top edge is the one that is drawn light enough to see against a film.
///
/// So the window is put one pixel further out in each direction and the frame lands where the monitor
/// is not. The cost is a pixel of picture off each edge of a video that is already letterboxed inside
/// the black — invisible, and measured against the alternative of taking `WS_THICKFRAME` off and
/// putting it back, which is a style change winit recomputes from its own bookkeeping on the next
/// call it is given.
///
/// Physical pixels, like everything else here, so it is one *device* pixel at any scale factor.
#[cfg(windows)]
fn over(screen: windows::Win32::Foundation::RECT) -> windows::Win32::Foundation::RECT {
    windows::Win32::Foundation::RECT {
        left: screen.left - 1,
        top: screen.top - 1,
        right: screen.right + 1,
        bottom: screen.bottom + 1,
    }
}

/// Take the compositor's own border and rounded corners off this window, or give them back.
///
/// **Half of the "the window still has a border in fullscreen" bug.** Windows 11 draws a one-pixel
/// line outside the client area of every top-level window: `DWMWA_BORDER_COLOR` is the compositor's
/// own, and `DWMWA_COLOR_NONE` is how a window says it does not want one. The rounded corners are the
/// same story an attribute along — four notches of desktop showing through the corners of a film.
///
/// The other half is the sizing edge of `WS_THICKFRAME`, which this does not touch and which survived
/// this being added: see [`over`], where that one is dealt with by geometry instead. Two causes, two
/// fixes, and knowing which is which is what stops the next person taking one of them back out.
///
/// Both are given back on the way out, because both are right for a window that is a window. Neither
/// call is checked: they are Windows 11 attributes, an older build refuses them, and a border this
/// program could not remove is a border it draws over anyway everywhere except here.
#[cfg(windows)]
fn borderless(owner: shell::Owner, on: bool) {
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_DEFAULT, DWMWCP_DONOTROUND, DWM_WINDOW_CORNER_PREFERENCE,
    };

    if owner.0 == 0 {
        return;
    }
    /// What `dwmapi.h` calls `DWMWA_COLOR_NONE`: no border at all, as against a colour.
    const NO_BORDER: u32 = 0xFFFF_FFFE;
    /// And `DWMWA_COLOR_DEFAULT`: whatever the system would have chosen.
    const SYSTEM_BORDER: u32 = 0xFFFF_FFFF;

    let colour: u32 = if on { NO_BORDER } else { SYSTEM_BORDER };
    let corners: DWM_WINDOW_CORNER_PREFERENCE = if on {
        DWMWCP_DONOTROUND
    } else {
        DWMWCP_DEFAULT
    };
    let hwnd = owner.hwnd();
    // SAFETY: a window this process owns, and one value of the size declared for each attribute.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&raw const colour).cast(),
            std::mem::size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const corners).cast(),
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}

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
///
/// **And the compositor's border comes off**, which is a different thing from the window's geometry
/// and the one part of the frame this program does not draw. See [`borderless`].
///
/// # Called twice, either way round
///
/// Both directions are guarded by [`SAVED`], which is what says whether the window is filling the
/// screen at all: a second way in does not overwrite the shape to go back to, and a way out with
/// nothing saved leaves the window alone. Without the first of those, entering fullscreen twice —
/// which two panes each showing a video can ask for — recorded the monitor as the window's own
/// rectangle and the way out became a move to where the window already was.
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
            // Already filling it, so there is nothing to do and — much more to the point — nothing
            // to record. See [`SAVED`], where what overwriting it costs is written down.
            if SAVED.lock().is_ok_and(|slot| slot.is_some()) {
                return;
            }
            // Both reads first, and nothing is changed until they have both answered — the same
            // order `open_maximized` takes, and for the same reason: half of this would be worse
            // than none of it.
            let Some(screen) = monitor() else { return };
            let mut was = RECT::default();
            if GetWindowRect(hwnd, &mut was).is_err() {
                return;
            }
            let Ok(mut slot) = SAVED.lock() else { return };
            let maximised = style & WS_MAXIMIZE.0 != 0;
            *slot = Some(([was.left, was.top, was.right, was.bottom], maximised));
            drop(slot);
            // **The bit comes off before the move**, or the move is clamped to the work area and this
            // is the bug it exists to prevent.
            SetWindowLongW(hwnd, GWL_STYLE, (style & !WS_MAXIMIZE.0) as i32);
            borderless(owner, true);
            place(HWND_TOPMOST, over(screen.rcMonitor));
            return;
        }

        // And back. Nothing saved means nothing to go back to, which is a window that was never
        // filling the screen — leave it alone rather than guess at a rectangle for it.
        let Some((rect, was_maximised)) = SAVED.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        borderless(owner, false);
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

/// Whether a Windows key is held.
///
/// **Asked of the keyboard rather than of the frame, because egui does not carry it.** `egui::Modifiers`
/// has four fields — Alt, Ctrl, Shift and the Mac command key — and winit's `SUPER` is dropped on the
/// way in, so a shortcut with Win in it cannot be recognised from the input at all. `GetAsyncKeyState`
/// is the same instrument [`paste_keystroke`] below reaches for, for the same kind of reason.
///
/// Read on the frame the arrow key arrives in, which is what makes the sample safe: it is a question
/// about a key that is being held *now*, alongside one this program was told about properly.
///
/// # What Windows will not let through
///
/// The shell claims a good deal of the Win key for itself and an application never sees those
/// presses: `Win+Up` and `Win+Down` are Snap, and **`Ctrl+Win+Left` and `Ctrl+Win+Right` switch
/// virtual desktop**. A combination the shell has taken is not a combination this can rescue — the
/// keystroke does not arrive, so there is nothing to sample the modifier against.
///
/// A `WH_KEYBOARD_LL` hook was written to take that chord out of the chain before the shell resolved
/// it, which is the only place it can be reached from a window. **It was tried and it did not work**,
/// and it is not here: a desktop-wide keyboard hook is a real thing to have installed in a file
/// manager — every keystroke on the machine handed to this process — and one that does not even
/// deliver the shortcut is all cost. So `Ctrl+Win+Left` is a binding for a machine whose shell is not
/// holding it, and `Ctrl+B` is the panel's shortcut.
///
/// `Ctrl+Win+Up` and `Ctrl+Win+Down` are not on the shell's list and do arrive. See
/// [`crate::app::App::window_keys`], where all three are bound.
#[cfg(windows)]
pub(crate) fn super_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LWIN, VK_RWIN};

    // SAFETY: a pure query of the keyboard state.
    unsafe {
        [VK_LWIN, VK_RWIN]
            .into_iter()
            .any(|key| GetAsyncKeyState(key.0 as i32) as u16 & 0x8000 != 0)
    }
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

#[cfg(all(windows, test))]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;

    /// A monitor and the band its taskbar reserves, written the way a screen layout reads.
    fn screen(at: [i32; 4], reserved: [i32; 4]) -> (RECT, RECT) {
        let [left, top, right, bottom] = at;
        let whole = RECT {
            left,
            top,
            right,
            bottom,
        };
        let [l, t, r, b] = reserved;
        (
            whole,
            RECT {
                left: left + l,
                top: top + t,
                right: right - r,
                bottom: bottom - b,
            },
        )
    }

    /// One screen with a taskbar along the bottom: the work area, which is what a maximise gives.
    #[test]
    fn one_screen_is_its_own_work_area() {
        let across = across(&[screen([0, 0, 1920, 1080], [0, 0, 0, 48])])
            .expect("a desktop with a screen on it has a rectangle");
        assert_eq!(
            (across.left, across.top, across.right, across.bottom),
            (0, 0, 1920, 1032)
        );
    }

    /// **The bug this arithmetic exists for.**
    ///
    /// Two screens side by side with the taskbar on the left-hand one. The union of the *work areas*
    /// reaches 1080 at the bottom, because the right-hand screen has no taskbar and its work area
    /// runs to its own edge — so the obvious answer covers the taskbar on the left-hand screen,
    /// which is the one thing this was asked not to do.
    #[test]
    fn a_taskbar_on_one_screen_shortens_the_span_across_both() {
        let across = across(&[
            screen([0, 0, 1920, 1080], [0, 0, 0, 48]),
            screen([1920, 0, 3840, 1080], [0, 0, 0, 0]),
        ])
        .expect("two screens have a rectangle");
        assert_eq!(
            (across.left, across.top, across.right, across.bottom),
            (0, 0, 3840, 1032),
            "the span covers both screens and stops above the taskbar"
        );
    }

    /// A taskbar down the side comes off that side and leaves the height alone.
    ///
    /// Which is what makes the four edges four separate questions rather than one.
    #[test]
    fn a_taskbar_down_the_side_only_narrows_that_side() {
        let across = across(&[
            screen([0, 0, 1920, 1080], [72, 0, 0, 0]),
            screen([1920, 0, 3840, 1080], [0, 0, 0, 0]),
        ])
        .expect("two screens have a rectangle");
        assert_eq!(
            (across.left, across.top, across.right, across.bottom),
            (72, 0, 3840, 1080)
        );
    }

    /// A screen to the left of the primary, which Windows numbers with negative coordinates.
    #[test]
    fn a_screen_at_a_negative_origin_is_part_of_the_span() {
        let across = across(&[
            screen([0, 0, 1920, 1080], [0, 0, 0, 48]),
            screen([-1920, 0, 0, 1080], [0, 0, 0, 0]),
        ])
        .expect("two screens have a rectangle");
        assert_eq!(
            (across.left, across.top, across.right, across.bottom),
            (-1920, 0, 1920, 1032)
        );
    }

    /// A monitor that does not reach an edge of the box has no say over that edge.
    ///
    /// A laptop panel below and to the left of a larger monitor: its taskbar is at the bottom of the
    /// desktop and shortens it, but the taller screen's top is the top and the laptop's own top has
    /// nothing to say about it.
    #[test]
    fn only_the_screens_on_an_edge_pull_that_edge_in() {
        let across = across(&[
            screen([0, 0, 2560, 1440], [0, 0, 0, 0]),
            screen([2560, 360, 4480, 1440], [0, 96, 0, 48]),
        ])
        .expect("two screens have a rectangle");
        assert_eq!(
            (across.left, across.top, across.right, across.bottom),
            (0, 0, 4480, 1392),
            "the lower screen's taskbar comes off the bottom and its own top edge is ignored"
        );
    }

    /// No monitors is no rectangle, rather than a rectangle of nothing.
    #[test]
    fn nothing_to_span_is_no_answer() {
        assert!(across(&[]).is_none());
    }
}
