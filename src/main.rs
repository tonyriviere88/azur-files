//! Azur Files — a file manager that stays out of the way.
//!
//! Tabs in the title bar, above the pane they belong to, a breadcrumb that behaves like
//! Explorer's, a details view that does not care how big the folder is, and panes you
//! build by dragging a tab to the edge of one.
//!
//! # The shape of it
//!
//! | module | responsibility |
//! | --- | --- |
//! | [`fs`] | everything that touches the disk, and nothing that touches the screen |
//! | [`loader`] | scans on worker threads, with a cache in front of them |
//! | [`sizes`] | what is inside each folder, counted on worker threads too |
//! | [`pane`] | tabs: where they point, how they are sorted, what is selected |
//! | [`dock`] | the tree that arranges panes on screen |
//! | [`ui`] | painting, at explicit rects |
//! | [`app`] | state, and the single place anything changes |
//! | [`theme`] | Azur's roles, plus the file-kind hues |
//! | [`brand`] | the name, and the design system's mark in its three forms |
//!
//! # Where the platform lives
//!
//! Every line that names a Win32 function is under **`src/windows/`**, and nothing in that
//! folder is reached by name from outside it. Each file is pulled in as the `win` submodule of
//! the portable module it serves:
//!
//! ```ignore
//! #[cfg(windows)]
//! #[path = "../windows/menu.rs"]
//! mod win;
//! ```
//!
//! So the module *path* is `shell::menu::win` wherever the file sits, and the portable half
//! keeps its `#[cfg(not(windows))]` arm next to its callers. Another operating system is then a
//! sibling folder — `src/linux/menu.rs` behind `#[cfg(target_os = "linux")]` — and not a single
//! call site moves. The declaration is the only place a platform is named.
//!
//! What is deliberately *not* in there: the short `#[cfg(windows)] { … }` blocks inside
//! otherwise-portable functions, where the whole platform-specific part is a handful of lines
//! (see [`shell::init`], [`shell::over_network`]). Hoisting those would cost a file each and
//! hide the fallback from the function it belongs to.
//!
//! # Why it is fast
//!
//! Almost none of it is clever code. It is four per-file costs that this program
//! declines to pay, and the two that matter are enormous — measured over 60,000
//! files by [`fs::scan`]'s own benchmark, on the machine this was written on:
//!
//! 1. **Nothing is asked about twice.** A directory read already hands over each
//!    name, size, timestamp and attribute word. Reaching for `Path::is_dir` or
//!    `fs::metadata` on top of that turns one sequential read into a round trip per
//!    file: **202× slower**. The choice of enumeration call, by contrast, is worth
//!    about 5% — `std::fs::read_dir` is nearly as good, and this uses
//!    `FindFirstFileExW` mostly to get the UTF-16 name into its arena directly.
//! 2. **The shell is never asked what a file is.** `SHGetFileInfo` for one type
//!    name measures at over a millisecond here — 67 *seconds* for that folder.
//!    A static table gives the same answer in 45 ns.
//! 3. **The listing is two allocations**, not one per entry: every name in one
//!    `String`, every record in one 32-byte-per-entry `Vec`. A natural-order sort of
//!    60,000 of them takes 2.8 ms, because it sorts 4-byte indices.
//! 4. **Only visible rows are drawn**, through one widget rather than one per row,
//!    formatting into a buffer that is reused.
//!
//! The other half is that no scan ever blocks a frame: [`loader`] reads on worker
//! threads with a cache in front, and the one call that genuinely *can* take twenty
//! seconds — asking a disconnected network drive for its volume label — is off the
//! startup path entirely (see [`fs::drives`]).
//!
//! The status line shows how long the current folder took to read. That is not
//! decoration: a program that claims to be fast should be checkable.

// No console behind the window in a release build. Without this, Windows gives a
// console-subsystem executable a console of its own, so launching this from Explorer or a
// shortcut flashes up a black window and then leaves it sitting in the taskbar beside the
// real one.
//
// Debug builds keep it, deliberately: that is where `log`, a panic message and the output of
// the developer flags below go, and a build you are debugging is a build you are running
// from a terminal anyway.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
/// Reading an archive as though it were a folder.
mod archive;
mod brand;
/// Little-endian integers out of a byte slice.
mod bytes;
mod config;
mod console;
mod diff;
/// The developer flags, which a shipped build does not carry.
#[cfg(any(test, feature = "devtools"))]
mod devtools;
mod dock;
mod fs;
mod git;
mod hardlinks;
mod history;
mod icons;
mod inflight;
mod loader;
mod markdown;
mod pane;
mod pe;
/// The worker queue behind the loader, the size counter and git.
mod pool;
mod preview;
/// Where a test may touch the disk, and the guard that holds it there.
#[cfg(test)]
mod sandbox;
/// The worker, live-view set and answer channel a per-view background service is built from.
mod service;
mod shell;
mod sizes;
mod syntax;
mod theme;
mod ui;
mod update;
mod watch;

use app::App;
use config::Config;

#[cfg(windows)]
#[path = "windows/window.rs"]
mod win;
#[cfg(windows)]
use win::{cloak, open_maximized, paste_keystroke, reachable, restore_position};

/// Counts live heap bytes, so a test can tell a leak from a cache.
///
/// Only in a test build. "Memory grows as I browse" has two very different causes — an LRU
/// cache filling up to a budget and then holding steady, and something that never lets go —
/// and from outside the process they look identical. This makes the difference a number.
/// See `app::leak_tests`.
#[cfg(test)]
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicIsize, Ordering};

    /// Live bytes: every allocation, minus every free.
    pub static LIVE: AtomicIsize = AtomicIsize::new(0);

    pub struct Counting;

    // SAFETY: every method forwards to `System` unchanged and only adds an atomic counter
    // around it, so the allocator's own contract is whatever `System`'s is.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(layout) };
            if !p.is_null() {
                LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
            }
            p
        }

        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
            unsafe { System.dealloc(p, layout) }
        }

        unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let q = unsafe { System.realloc(p, layout, new_size) };
            if !q.is_null() {
                LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
            }
            q
        }
    }
}

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

fn main() -> eframe::Result {
    // Before anything reads a file or starts a thread: what the scan and the shell are told about
    // a OneDrive file depends on it. See the function.
    fs::scan::expose_placeholders();
    // Set on the main thread as well as on each worker: any call that touches an
    // empty removable drive can otherwise raise a modal from inside the syscall.
    fs::scan::silence_device_dialogs();
    // COM, on the thread that will own every shell call: the context menu and the file
    // operations both put up windows, and both need an apartment with a message pump.
    shell::init();
    // Anything a previous run was killed in the middle of claiming from a drop. Cheap, and the
    // only thing standing between a crash mid-copy and an extracted archive sitting in `%TEMP%`
    // for good.
    shell::dnd::sweep();

    let config = Config::load();
    // Every other word on the command line is a developer flag, and only a `devtools` build reads
    // them: see `devtools`.
    #[cfg(any(test, feature = "devtools"))]
    let (config, mut devtools) = devtools::Devtools::from_args(config);
    let mut open: Vec<std::path::PathBuf> = Vec::new();

    // `--open=<path>`, repeatable: one pane per path, so `--open=A --open=B` comes up
    // side by side. The one flag a shipped build reads, for a shortcut that opens a folder.
    for arg in std::env::args().skip(1) {
        if let Some(path) = arg.strip_prefix("--open=") {
            // Resolved as the path bar resolves one — `%AppData%`, `shell:Downloads`, the Recycle
            // Bin — since a shortcut or a Git Bash prompt hands the text over unexpanded.
            open.push(fs::from_outside(path));
        }
    }

    let size = config.window.unwrap_or(config::WINDOW_SIZE);
    // Where the window last sat at a size of its own. That setting is only ever written while
    // the window is restored, which makes it the answer to two questions: where a restored
    // window opens, and where a maximised one goes when the restore button is pressed.
    let placed = config.position.filter(reachable);
    // The first of the two, and only for a restored window. A maximised one is described by the
    // flag — the platform maximises it onto the monitor it opens it on — and asking for a
    // position as well would un-maximise it on the way, which is exactly what `Reset window
    // size` relies on. `open_maximized` is handed `placed` for the second.
    let position = placed.filter(|_| !config.maximized);

    let options = eframe::NativeOptions {
        viewport: {
            let viewport = egui::ViewportBuilder::default()
                .with_inner_size(size)
                .with_min_inner_size([720.0, 420.0])
                // The window's own taskbar button and its Alt-Tab entry, which is a different
                // slot from the `.ico` in the executable's resources — `build.rs` fills that
                // one. Without this egui supplies a white `e` — "for egui or eframe", says its
                // own documentation — and the mark would change the moment the window opened.
                .with_icon(brand::window_icon())
                // The title bar is drawn by this program — the tabs live in it, which no
                // platform caption can do — so the platform is asked not to draw a
                // second one. `ui::chrome::resize_borders` puts the edge grips back.
                .with_decorations(false)
                .with_title(brand::NAME);
            // Neither where the window *goes* nor whether it opens maximised is set here — see
            // `restore_position` and `open_maximized`. The builder takes points, and points are
            // not a coordinate space a desktop of mixed scale factors has one of; and
            // `with_maximized` on this platform shows the window three times before anything has
            // been drawn into it. Except where the handle this program would reach for is not an
            // `HWND`, where the builder is all there is.
            #[cfg(not(windows))]
            let viewport = match position {
                Some([x, y]) => viewport.with_position(egui::pos2(x, y)),
                None => viewport,
            };
            #[cfg(not(windows))]
            let viewport = viewport.with_maximized(config.maximized);
            viewport
        },
        // A file listing is text and rectangles; there is nothing to gain from
        // multisampling and a frame per pixel-row to lose. egui antialiases edges by
        // feathering them one pixel, which `azur_egui_theme::style` pins.
        multisampling: 0,
        // eframe dithers by default, which adds noise to anything it samples from a
        // texture — and the glyph atlas is a texture. Dithering earns its keep on wide
        // gradients; this window has none, and 14px text is the wrong thing to add noise
        // to. Off, as `azur_egui_theme::render` documents for every Azur window.
        dithering: false,
        // `wgpu`, on the backend the design system pins. See `reporting_wgpu`.
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: reporting_wgpu(),
        ..Default::default()
    };

    eframe::run_native(
        brand::NAME,
        options,
        Box::new(move |cc| {
            // Before anything else, and before the window has been shown even once: nothing
            // this program draws is on screen yet, and the compositor has a white surface
            // ready to show in its place. See `cloak`.
            cloak(cc, true);
            azur_egui_theme::fonts::install(&cc.egui_ctx);
            // One wheel notch is one "line", which egui makes 40 points: a row and two thirds,
            // which felt slow. 72 is exactly three rows of `ROW_HEIGHT`.
            cc.egui_ctx.options_mut(|o| o.input_options.line_scroll_speed = 72.0);
            // The zoom the last session ended at. The window's size above is in the monitor's
            // points and so does not change with it — see `Config::zoom`.
            cc.egui_ctx.set_zoom_factor(config.zoom);
            restore_position(cc, position);
            if config.maximized {
                open_maximized(cc, placed);
            }
            Ok(Box::new(Window {
                app: {
                    #[cfg(any(test, feature = "devtools"))]
                    let side = devtools.side();
                    #[cfg(not(any(test, feature = "devtools")))]
                    let side = pane::Side::Right;
                    let app = App::opening(&cc.egui_ctx, config, open, side);
                    #[cfg(any(test, feature = "devtools"))]
                    let app = devtools.configure(app);
                    app
                },
                #[cfg(any(test, feature = "devtools"))]
                devtools,
                frames: 0,
                owned: false,
                cloaked: true,
            }))
        }),
    )
}

#[cfg(not(windows))]
fn restore_position(_cc: &eframe::CreationContext<'_>, _position: Option<[f32; 2]>) {}

#[cfg(not(windows))]
fn open_maximized(_cc: &eframe::CreationContext<'_>, _position: Option<[f32; 2]>) {}

#[cfg(not(windows))]
fn cloak(_handle: &impl raw_window_handle::HasWindowHandle, _hidden: bool) {}

#[cfg(not(windows))]
fn reachable(_position: &[f32; 2]) -> bool {
    true
}

/// Which graphics backend this window uses, and why it is not the obvious one.
///
/// **`wgpu`, not `glow`.** `egui_glow`'s painter leaks a couple of kilobytes for every draw
/// call, every frame: scrolling one folder grew this process by 7 MB a second and never gave
/// any of it back. It is not this program's bug — `examples/spin.rs` was forty lines of eframe
/// that reproduced it, and the same forty lines were flat under `wgpu`. Batching the icons and
/// putting them in one atlas cut it from 7 MB/s to 1.16, because it cut the draw calls; only
/// changing backend removes it. `glow` is no longer compiled in at all, so that example now
/// measures the backend this program actually ships.
///
/// **On Vulkan, and that one is not this program's call** — it is
/// [`azur_egui_theme::render`], because a design system built on one-pixel strokes has a stake
/// in whether a pixel survives the trip to the screen. It does not, on OpenGL or on D3D12: the
/// compositor hands those windows to the screen through a vertical resample that costs a
/// hairline a quarter of its contrast and erases the faintest lines altogether. The library has
/// the measurement.
///
/// This window used to run on OpenGL, chosen on the numbers below, and read as blurry a few
/// seconds after it stopped being touched — which is when a window that paints on demand stops
/// presenting and the compositor takes it back. So the table is still true and no longer
/// decisive:
///
/// | | private bytes at rest | scrolling one folder | survives composition |
/// | --- | --- | --- | --- |
/// | `glow` | 180 MB | +1.16 MB/s | — |
/// | `wgpu`, D3D12 | 443 MB | flat | **no** |
/// | **`wgpu`, Vulkan** | **426 MB** | **flat** | **yes** |
/// | `wgpu`, OpenGL | 251 MB | flat | **no** |
///
/// Vulkan costs about 200 MB over OpenGL on this window — 181 MB against 398 in one run and 216
/// against 413 in another, the spread being what the window had been doing rather than the
/// backend. A file listing that is soft whenever nobody is touching it is a file listing that is
/// soft nearly all the time, so the memory goes.
///
/// One escape hatch left, named for the design system rather than for this program because all
/// three Azur applications read the same one. `AZUR_ADAPTER=low` renders on the integrated GPU,
/// which is around 100 MB lighter and whose correctness depends on how the machine is wired — see
/// [`azur_egui_theme::render`] before taking it.
///
/// There used to be three, and what happened to the other two is a single decision applied twice.
/// `AZUR_GLOW=1` went back to the leaking painter for a machine where `wgpu` would not start at
/// all; `AZUR_BACKEND=gl` and `=d3d12` changed backend without leaving `wgpu`. Between them they
/// were 181,476 bytes of `egui_glow`, `glutin` and `glow`, 531,456 for wgpu's own GL backend, and
/// 407,396 for `wgpu-hal::dx12` and the HLSL writer it needs — 1.1 MB of every copy of this
/// program, standing by for a machine nobody has reported. All three are gone, and the table above
/// is why: there was only ever one row worth presenting on.
///
/// **What that costs is worth knowing before it happens.** A machine with no working Vulkan driver
/// no longer has anything to fall back to, and this window will not open on it. See
/// [`azur_egui_theme::render`], which also explains why a name wgpu cannot serve is worse than no
/// name at all.
fn reporting_wgpu() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{wgpu, SurfaceErrorAction};

    // The backend, the adapter and the allocator: all three from the design system rather than
    // from here, because all three decide whether what this window draws reaches the screen
    // intact. This function adds exactly one thing of its own, below.
    let mut options = azur_egui_theme::render::wgpu_options();

    // Why the rest of this exists: a window that cannot present its frames goes on taking input and
    // showing whatever it last drew, which from the outside is indistinguishable from a hang.
    // The default handler takes the right action for each of these and says nothing about any of
    // them; this one takes the same actions and *reports*, so the next time it happens there is
    // an answer rather than a guess.
    options.on_surface_status = std::sync::Arc::new(|status| match status {
        wgpu::CurrentSurfaceTexture::Outdated => SurfaceErrorAction::Reconfigure,
        wgpu::CurrentSurfaceTexture::Lost => {
            gpu_trouble("the drawing surface was lost; making a new one".to_owned());
            SurfaceErrorAction::RecreateSurface
        }
        wgpu::CurrentSurfaceTexture::Occluded => SurfaceErrorAction::SkipFrame,
        other => {
            gpu_trouble(format!("a frame could not be presented: {other:?}"));
            SurfaceErrorAction::SkipFrame
        }
    });
    options
}

/// The last thing the graphics device said went wrong, for the status line and the console.
///
/// A `static` because the callbacks that write it belong to wgpu and outlive any borrow this
/// program could lend them.
static GPU_TROUBLE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Report a graphics failure to whoever is looking: the console, and the window itself.
///
/// Both, deliberately. A debug build has a console and a release build does not, and "the
/// window stopped drawing after the machine woke up" is a report that needs to survive being
/// made by somebody who was not running it from a terminal.
fn gpu_trouble(what: String) {
    eprintln!("graphics: {what}");
    if let Ok(mut slot) = GPU_TROUBLE.lock() {
        *slot = Some(what);
    }
}

/// The eframe shell around [`App`].
struct Window {
    app: App,
    /// The developer flags this run was given; see `devtools`.
    #[cfg(any(test, feature = "devtools"))]
    devtools: devtools::Devtools,
    /// Frames drawn: the cloak comes off after two, and the developer flags wait on it for the
    /// window to settle.
    ///
    /// Its own counter rather than the capture's, which only advances when there is a capture
    /// pending — so a flag gated on that one silently does nothing without `--shot`. Which is how
    /// `--console` came to open no console at all when it was driven by hand.
    frames: u32,
    /// Whether the window handle has been handed to the shell layer yet.
    owned: bool,
    /// Whether the compositor is still being kept from showing this window.
    cloaked: bool,
}

impl eframe::App for Window {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.app.clear_color()
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Two frames have been drawn and presented, so the window can be looked at — see `cloak`,
        // which has been holding the compositor off until this point, and has the measurements for
        // why it is two and not one. The repaint is asked for because nothing else has: a window
        // this program forgets to uncloak is a window that never appears at all.
        if self.cloaked {
            if self.frames >= 2 {
                self.cloaked = false;
                cloak(frame, false);
            } else {
                ui.ctx().request_repaint();
            }
        }
        if !self.owned {
            // eframe is the only thing that knows the window handle, and only once the
            // platform has actually made a window.
            self.app.set_owner(shell::Owner::from_handle(frame));
            self.owned = true;
            // And the only thing that knows the graphics device. A device is *lost* when the
            // driver resets, when a hybrid machine switches GPU, or when the machine sleeps and
            // the context does not survive it -- after which nothing this program draws can
            // reach the screen and there is no way to tell from inside the frame. wgpu will say
            // so exactly once, if anybody has asked to be told.
            if let Some(state) = frame.wgpu_render_state() {
                state.device.set_device_lost_callback(|reason, message| {
                    gpu_trouble(format!("the graphics device was lost ({reason:?}): {message}"));
                });
            }
        }
        if let Some(what) = GPU_TROUBLE.lock().ok().and_then(|mut slot| slot.take()) {
            self.app.report(what);
        }
        self.frames += 1;
        #[cfg(any(test, feature = "devtools"))]
        self.devtools.before_frame(&mut self.app, ui, self.frames);
        self.app.frame(ui);
        #[cfg(any(test, feature = "devtools"))]
        self.devtools.after_frame(&mut self.app, ui, self.frames);
    }

    /// Called before each pass, with the input as the platform handed it over.
    ///
    /// The one place raw input can be added to, which two things here need: the Ctrl+V that
    /// egui-winit swallows, and the button release that an OLE drag consumes.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        paste_keystroke(raw_input);
        raw_input.events.extend(self.app.take_injected());
    }

    fn on_exit(&mut self) {
        // Anything copied here is still only a pointer into this process until it is rendered,
        // so a copy taken a moment ago has to be made real before the process that owns it goes
        // away. See `shell::flush`.
        shell::flush();
        // The window size, the sidebar, the bookmarks and every open tab, so the
        // next launch opens where this one left off. Skipped for a capture run,
        // which should not rewrite the user's settings.
        //
        // And skipped when this window has not changed any of them, which is what
        // `App::save_settings` decides: an unconditional save here is how a window left open all
        // day overwrote a bookmark added in another one.
        #[cfg(any(test, feature = "devtools"))]
        let capturing = self.devtools.capturing();
        #[cfg(not(any(test, feature = "devtools")))]
        let capturing = false;
        if !capturing {
            self.app.save_settings();
            self.app.save_visits();
        }
        // Whatever was pulled out of an archive to be previewed or opened. After `shell::flush`
        // above, deliberately: a copy taken out of an archive and left on the clipboard names these
        // files, and rendering the clipboard is what makes the paste that follows this window's
        // closing still work.
        //
        // Best-effort, and one directory per process — see [`archive::extract::temp_root`], which is
        // why this cannot remove another window's files and why a file still open in the application
        // that opened it is simply left for `%TEMP%` to deal with.
        archive::extract::cleanup();
    }
}

#[cfg(not(windows))]
fn paste_keystroke(_raw_input: &mut egui::RawInput) {}
