1//! Azur File Explorer — a file manager that stays out of the way.
2//!
3//! Tabs in the title bar, above the pane they belong to, a breadcrumb that behaves like
4//! Explorer's, a details view that does not care how big the folder is, and panes you
5//! build by dragging a tab to the edge of one.
6//!
7//! # The shape of it
8//!
9//! | module | responsibility |
10//! | --- | --- |
11//! | [`fs`] | everything that touches the disk, and nothing that touches the screen |
12//! | [`loader`] | scans on worker threads, with a cache in front of them |
13//! | [`pane`] | tabs: where they point, how they are sorted, what is selected |
14//! | [`dock`] | the tree that arranges panes on screen |
15//! | [`ui`] | painting, at explicit rects |
16//! | [`app`] | state, and the single place anything changes |
17//! | [`theme`] | Azur's roles, plus the file-kind hues |
18//! | [`brand`] | the name, and the design system's mark in its three forms |
19//!
20//! # Why it is fast
21//!
22//! Almost none of it is clever code. It is four per-file costs that this program
23//! declines to pay, and the two that matter are enormous — measured over 60,000
24//! files by [`fs::scan`]'s own benchmark, on the machine this was written on:
25//!
26//! 1. **Nothing is asked about twice.** A directory read already hands over each
27//!    name, size, timestamp and attribute word. Reaching for `Path::is_dir` or
28//!    `fs::metadata` on top of that turns one sequential read into a round trip per
29//!    file: **202× slower**. The choice of enumeration call, by contrast, is worth
30//!    about 5% — `std::fs::read_dir` is nearly as good, and this uses
31//!    `FindFirstFileExW` mostly to get the UTF-16 name into its arena directly.
32//! 2. **The shell is never asked what a file is.** `SHGetFileInfo` for one type
33//!    name measures at over a millisecond here — 67 *seconds* for that folder.
34//!    A static table gives the same answer in 45 ns.
35//! 3. **The listing is two allocations**, not one per entry: every name in one
36//!    `String`, every record in one 32-byte-per-entry `Vec`. A natural-order sort of
37//!    60,000 of them takes 2.8 ms, because it sorts 4-byte indices.
38//! 4. **Only visible rows are drawn**, through one widget rather than one per row,
39//!    formatting into a buffer that is reused.
40//!
41//! The other half is that no scan ever blocks a frame: [`loader`] reads on worker
42//! threads with a cache in front, and the one call that genuinely *can* take twenty
43//! seconds — asking a disconnected network drive for its volume label — is off the
44//! startup path entirely (see [`fs::drives`]).
45//!
46//! The status line shows how long the current folder took to read. That is not
47//! decoration: a program that claims to be fast should be checkable.
48
49// No console behind the window in a release build. Without this, Windows gives a
50// console-subsystem executable a console of its own, so launching this from Explorer or a
51// shortcut flashes up a black window and then leaves it sitting in the taskbar beside the
52// real one.
53//
54// Debug builds keep it, deliberately: that is where `log`, a panic message and the output of
55// the developer flags below go, and a build you are debugging is a build you are running
56// from a terminal anyway.
57#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
58
59mod app;
60mod brand;
61mod config;
62mod dock;
63mod fs;
64mod icons;
65mod loader;
66mod pane;
67mod pe;
68mod preview;
69mod shell;
70mod theme;
71mod ui;
72mod watch;
73
74use app::App;
75use config::Config;
76
77/// Counts live heap bytes, so a test can tell a leak from a cache.
78///
79/// Only in a test build. "Memory grows as I browse" has two very different causes — an LRU
80/// cache filling up to a budget and then holding steady, and something that never lets go —
81/// and from outside the process they look identical. This makes the difference a number.
82/// See `app::leak_tests`.
83#[cfg(test)]
84mod counting {
85    use std::alloc::{GlobalAlloc, Layout, System};
86    use std::sync::atomic::{AtomicIsize, Ordering};
87
88    /// Live bytes: every allocation, minus every free.
89    pub static LIVE: AtomicIsize = AtomicIsize::new(0);
90
91    pub struct Counting;
92
93    // SAFETY: every method forwards to `System` unchanged and only adds an atomic counter
94    // around it, so the allocator's own contract is whatever `System`'s is.
95    unsafe impl GlobalAlloc for Counting {
96        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
97            let p = unsafe { System.alloc(layout) };
98            if !p.is_null() {
99                LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
100            }
101            p
102        }
103
104        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
105            LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
106            unsafe { System.dealloc(p, layout) }
107        }
108
109        unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
110            let q = unsafe { System.realloc(p, layout, new_size) };
111            if !q.is_null() {
112                LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
113            }
114            q
115        }
116    }
117}
118
119#[cfg(test)]
120#[global_allocator]
121static ALLOCATOR: counting::Counting = counting::Counting;
122
123fn main() -> eframe::Result {
124    // Set on the main thread as well as on each worker: any call that touches an
125    // empty removable drive can otherwise raise a modal from inside the syscall.
126    fs::scan::silence_device_dialogs();
127    // COM, on the thread that will own every shell call: the context menu and the file
128    // operations both put up windows, and both need an apartment with a message pump.
129    shell::init();
130    // Anything a previous run was killed in the middle of claiming from a drop. Cheap, and the
131    // only thing standing between a crash mid-copy and an extracted archive sitting in `%TEMP%`
132    // for good.
133    shell::dnd::sweep();
134
135    let mut config = Config::load();
136    let mut shot: Option<Shot> = None;
137    let mut open: Vec<std::path::PathBuf> = Vec::new();
138    let mut reveal: Option<String> = None;
139    let mut menu = false;
140    let mut rename = false;
141    let mut stack = false;
142    let mut preview = false;
143    let mut compare = false;
144    let mut trace = false;
145    let mut walk: Option<std::path::PathBuf> = None;
146    let mut scroll = false;
147    let mut flat = false;
148
149    // `--open=<path>`, repeatable: one pane per path, so `--open=A --open=B` comes up
150    // side by side. `--reveal=<name>` selects and scrolls to an entry once the listing
151    // lands, which is what a shell "show this file" integration needs. `--shot=<file>`,
152    // `--size=WxH` and `--menu` are for checking the interface and regenerating its
153    // screenshots rather than capturing them by hand.
154    for arg in std::env::args().skip(1) {
155        if let Some(path) = arg.strip_prefix("--open=") {
156            open.push(fs::normalize(std::path::Path::new(path)));
157        } else if let Some(path) = arg.strip_prefix("--shot=") {
158            shot = Some(Shot {
159                path: std::path::PathBuf::from(path),
160                frame: 0,
161            });
162        } else if arg == "--light" {
163            config.dark = false;
164        } else if arg == "--dark" {
165            config.dark = true;
166        } else if let Some(name) = arg.strip_prefix("--reveal=") {
167            reveal = Some(name.to_owned());
168        } else if arg == "--menu" {
169            menu = true;
170        } else if arg == "--rename" {
171            rename = true;
172        } else if arg == "--preview" {
173            preview = true;
174        } else if arg == "--compare" {
175            preview = true;
176            compare = true;
177        } else if arg == "--stack" {
178            stack = true;
179        } else if arg == "--trace" {
180            trace = true;
181        } else if arg == "--scroll" {
182            scroll = true;
183        } else if arg == "--flat" {
184            flat = true;
185        } else if let Some(dir) = arg.strip_prefix("--walk=") {
186            walk = Some(fs::normalize(std::path::Path::new(dir)));
187        } else if let Some(size) = arg.strip_prefix("--size=") {
188            if let Some((w, h)) = size.split_once('x') {
189                if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
190                    config.window = Some([w, h]);
191                    config.maximized = false;
192                    // A capture is a window of a stated size and nothing else about the last
193                    // session: where it was left is not part of what is being photographed,
194                    // and a screenshot run that moves the window about is a nuisance.
195                    config.position = None;
196                }
197            }
198        }
199    }
200
201    let size = config.window.unwrap_or(config::WINDOW_SIZE);
202    // Only for a restored window. A maximised one is described by the flag — the platform
203    // maximises it onto the monitor it opens it on — and asking for a position as well would
204    // un-maximise it on the way, which is exactly what `Reset window size` relies on.
205    let position = config.position.filter(|_| !config.maximized).filter(reachable);
206
207    let options = eframe::NativeOptions {
208        viewport: {
209            let viewport = egui::ViewportBuilder::default()
210                .with_inner_size(size)
211                .with_min_inner_size([720.0, 420.0])
212                .with_maximized(config.maximized)
213                // The window's own taskbar button and its Alt-Tab entry, which is a different
214                // slot from the `.ico` in the executable's resources — `build.rs` fills that
215                // one. Without this egui supplies a white `e` — "for egui or eframe", says its
216                // own documentation — and the mark would change the moment the window opened.
217                .with_icon(brand::window_icon())
218                // The title bar is drawn by this program — the tabs live in it, which no
219                // platform caption can do — so the platform is asked not to draw a
220                // second one. `ui::chrome::resize_borders` puts the edge grips back.
221                .with_decorations(false)
222                .with_title(brand::NAME);
223            // Where the window *goes* is not set here — see `restore_position`. The builder
224            // takes points, and points are not a coordinate space a desktop of mixed scale
225            // factors has one of. Except on a platform where the handle this program would
226            // reach for is not an `HWND`, where approximately is better than not at all.
227            #[cfg(not(windows))]
228            let viewport = match position {
229                Some([x, y]) => viewport.with_position(egui::pos2(x, y)),
230                None => viewport,
231            };
232            viewport
233        },
234        // A file listing is text and rectangles; there is nothing to gain from
235        // multisampling and a frame per pixel-row to lose. egui antialiases edges by
236        // feathering them one pixel, which `azur_egui_theme::style` pins.
237        multisampling: 0,
238        // eframe dithers by default, which adds noise to anything it samples from a
239        // texture — and the glyph atlas is a texture. Dithering earns its keep on wide
240        // gradients; this window has none, and 14px text is the wrong thing to add noise
241        // to. Off, as `azur_egui_theme::render` documents for every Azur window.
242        dithering: false,
243        // `wgpu`, on the backend the design system pins. See `reporting_wgpu`.
244        renderer: if std::env::var_os("AZUR_GLOW").is_some() {
245            eframe::Renderer::Glow
246        } else {
247            eframe::Renderer::Wgpu
248        },
249        wgpu_options: reporting_wgpu(),
250        ..Default::default()
251    };
252
253    eframe::run_native(
254        brand::NAME,
255        options,
256        Box::new(move |cc| {
257            azur_egui_theme::fonts::install(&cc.egui_ctx);
258            restore_position(cc, position);
259            Ok(Box::new(Window {
260                app: App::opening(
261                    &cc.egui_ctx,
262                    config,
263                    open,
264                    if stack {
265                        pane::Side::Bottom
266                    } else {
267                        pane::Side::Right
268                    },
269                )
270                .revealing(reveal)
271                .tracing(trace)
272                .walking(walk)
273                .scrolling(scroll)
274                .flattened(flat),
275                shot,
276                menu,
277                rename,
278                preview,
279                compare,
280                waited: 0,
281                owned: false,
282                // trace,
283            }))
284        }),
285    )
286}
287
288/// Put the window back where it was left.
289///
290/// **From here rather than through eframe, and in physical pixels.** Three ways to place a
291/// window were available and two of them are wrong:
292///
293/// - `ViewportBuilder::with_position` takes *points*, and the platform turns them into pixels
294///   with the scale factor of whichever monitor it happened to open the window on — not the
295///   one being aimed at. On a desktop where the laptop panel is at 150% and the external
296///   monitors are at 100%, restoring a window to an external monitor lands it half again too
297///   far out, which is frequently a different screen or none.
298/// - `ViewportCommand::OuterPosition` from inside the first frame *is* exact — it multiplies by
299///   the same scale factor [`App`] divided by when it saved the position. But eframe reveals the
300///   window in `post_rendering`, which runs *before* the frame's viewport commands are applied,
301///   so the move lands after the window is already on screen somewhere else.
302/// - This runs in the creation closure: the window exists, nothing has been painted into it, and
303///   eframe keeps it hidden until something has been. The pixels go straight to the platform in
304///   the units the platform works in, so no scale factor is involved on either side of the round
305///   trip.
306///
307/// Nothing is clamped here. Whether the position is still on a monitor is
308/// [`reachable`]'s question, asked before the window is built at all — because a window with no
309/// screen under its title bar cannot be dragged back, and this one draws its own title bar.
310#[cfg(windows)]
311fn restore_position(cc: &eframe::CreationContext<'_>, position: Option<[f32; 2]>) {
312    use windows::Win32::UI::WindowsAndMessaging::{
313        SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
314    };
315
316    let Some([x, y]) = position else { return };
317    let window = shell::Owner::from_handle(cc);
318    if window.0 == 0 {
319        return;
320    }
321    // SAFETY: a window this process owns, moved and not resized, restacked or activated.
322    let _ = unsafe {
323        SetWindowPos(
324            window.hwnd(),
325            None,
326            x as i32,
327            y as i32,
328            0,
329            0,
330            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
331        )
332    };
333}
334
335#[cfg(not(windows))]
336fn restore_position(_cc: &eframe::CreationContext<'_>, _position: Option<[f32; 2]>) {}
337
338/// Whether a remembered position still has a screen under it.
339///
340/// The one failure worth guarding against: the window was last closed on a monitor that is no
341/// longer connected, so the saved position names a place that no longer exists. Windows does not
342/// clamp a window into view, and this one has no caption of its own for the platform's
343/// Move command to work on — so it would open as a taskbar button with nothing on screen, which
344/// is indistinguishable from a program that failed to start.
345///
346/// The point tested is a little way into the title bar rather than the corner, and in physical
347/// pixels, so it is on the bar itself at every scale factor: 16px down is inside a 32-point bar
348/// at 100% and a 48-pixel one at 150%. If *that* has a monitor under it the window can be picked
349/// up and dragged, which is the whole of what has to be true.
350#[cfg(windows)]
351fn reachable(position: &[f32; 2]) -> bool {
352    use windows::Win32::Foundation::POINT;
353    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONULL};
354
355    let [x, y] = *position;
356    let grab = POINT {
357        x: x as i32 + 60,
358        y: y as i32 + 16,
359    };
360    // SAFETY: a pure query about a point on the desktop.
361    !unsafe { MonitorFromPoint(grab, MONITOR_DEFAULTTONULL) }.is_invalid()
362}
363
364#[cfg(not(windows))]
365fn reachable(_position: &[f32; 2]) -> bool {
366    true
367}
368
369/// Which graphics backend this window uses, and why it is not the obvious one.
370///
371/// **`wgpu`, not `glow`.** `egui_glow`'s painter leaks a couple of kilobytes for every draw
372/// call, every frame: scrolling one folder grew this process by 7 MB a second and never gave
373/// any of it back. It is not this program's bug — `examples/spin.rs` is forty lines of eframe
374/// that reproduces it, and the same forty lines are flat under `wgpu`. Batching the icons and
375/// putting them in one atlas cut it from 7 MB/s to 1.16, because it cut the draw calls; only
376/// changing backend removes it.
377///
378/// **On Vulkan, and that one is not this program's call** — it is
379/// [`azur_egui_theme::render`], because a design system built on one-pixel strokes has a stake
380/// in whether a pixel survives the trip to the screen. It does not, on OpenGL or on D3D12: the
381/// compositor hands those windows to the screen through a vertical resample that costs a
382/// hairline a quarter of its contrast and erases the faintest lines altogether. The library has
383/// the measurement.
384///
385/// This window used to run on OpenGL, chosen on the numbers below, and read as blurry a few
386/// seconds after it stopped being touched — which is when a window that paints on demand stops
387/// presenting and the compositor takes it back. So the table is still true and no longer
388/// decisive:
389///
390/// | | private bytes at rest | scrolling one folder | survives composition |
391/// | --- | --- | --- | --- |
392/// | `glow` | 180 MB | +1.16 MB/s | — |
393/// | `wgpu`, D3D12 | 443 MB | flat | **no** |
394/// | **`wgpu`, Vulkan** | **426 MB** | **flat** | **yes** |
395/// | `wgpu`, OpenGL | 251 MB | flat | **no** |
396///
397/// Vulkan costs about 200 MB over OpenGL on this window — 181 MB against 398 in one run and 216
398/// against 413 in another, the spread being what the window had been doing rather than the
399/// backend. A file listing that is soft whenever nobody is touching it is a file listing that is
400/// soft nearly all the time, so the memory goes.
401///
402/// Three escape hatches, all named for the design system rather than for this program, because
403/// all three Azur applications now read the same ones. `AZUR_GLOW=1` goes back to the leaking
404/// painter, for a machine where `wgpu` will not start at all. `AZUR_BACKEND=gl` or `=d3d12`
405/// changes backend without leaving `wgpu`. `AZUR_ADAPTER=low` renders on the integrated GPU,
406/// which is around 100 MB lighter and whose correctness depends on how the machine is wired —
407/// see [`azur_egui_theme::render`] before taking it.
408fn reporting_wgpu() -> eframe::egui_wgpu::WgpuConfiguration {
409    use eframe::egui_wgpu::{wgpu, SurfaceErrorAction};
410
411    // The backend, the adapter and the allocator: all three from the design system rather than
412    // from here, because all three decide whether what this window draws reaches the screen
413    // intact. This function adds exactly one thing of its own, below.
414    let mut options = azur_egui_theme::render::wgpu_options();
415
416    // Why the rest of this exists: a window that cannot present its frames goes on taking input and
417    // showing whatever it last drew, which from the outside is indistinguishable from a hang.
418    // The default handler takes the right action for each of these and says nothing about any of
419    // them; this one takes the same actions and *reports*, so the next time it happens there is
420    // an answer rather than a guess.
421    options.on_surface_status = std::sync::Arc::new(|status| match status {
422        wgpu::CurrentSurfaceTexture::Outdated => SurfaceErrorAction::Reconfigure,
423        wgpu::CurrentSurfaceTexture::Lost => {
424            gpu_trouble("the drawing surface was lost; making a new one".to_owned());
425            SurfaceErrorAction::RecreateSurface
426        }
427        wgpu::CurrentSurfaceTexture::Occluded => SurfaceErrorAction::SkipFrame,
428        other => {
429            gpu_trouble(format!("a frame could not be presented: {other:?}"));
430            SurfaceErrorAction::SkipFrame
431        }
432    });
433    options
434}
435
436/// The last thing the graphics device said went wrong, for the status line and the console.
437///
438/// A `static` because the callbacks that write it belong to wgpu and outlive any borrow this
439/// program could lend them.
440static GPU_TROUBLE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
441
442/// Report a graphics failure to whoever is looking: the console, and the window itself.
443///
444/// Both, deliberately. A debug build has a console and a release build does not, and "the
445/// window stopped drawing after the machine woke up" is a report that needs to survive being
446/// made by somebody who was not running it from a terminal.
447fn gpu_trouble(what: String) {
448    eprintln!("graphics: {what}");
449    if let Ok(mut slot) = GPU_TROUBLE.lock() {
450        *slot = Some(what);
451    }
452}
453
454/// A pending `--shot=` capture: where to write it, and how many frames have gone
455/// past. Fonts, the first directory scan and the scroll area all settle over the
456/// first few, so the capture waits for them.
457struct Shot {
458    path: std::path::PathBuf,
459    frame: u32,
460}
461
462/// The eframe shell around [`App`].
463struct Window {
464    app: App,
465    shot: Option<Shot>,
466    /// `--menu`: raise the folder's context menu, so a capture can show one.
467    menu: bool,
468    /// `--rename`: open the selected row's name for editing. The one part of this interface a
469    /// screenshot cannot otherwise reach, since it needs a keystroke on a focused row — and
470    /// where the caret lands is the whole thing worth looking at.
471    rename: bool,
472    /// `--preview`: put the keyboard on the first previewable file in the listing and open the
473    /// preview panel on it, for the same reason — it is behind `Ctrl+P` and a focused row.
474    preview: bool,
475    /// `--compare`: select the first *two* pictures instead, so a capture can show the comparison.
476    compare: bool,
477    /// Frames a `--menu` capture has spent waiting for the shell's half of that menu.
478    waited: u32,
479    /// Whether the window handle has been handed to the shell layer yet.
480    owned: bool,
481    // `--trace`: name the adapter and backend the window actually got, once.
482    //
483    // Which GPU a window ends up on cannot be told reliably from outside the process. The
484    // loaded driver DLLs do not say — the Vulkan loader opens every installed ICD just to
485    // enumerate devices — and neither does `nvidia-smi`, which lists this process whether it
486    // renders on the discrete GPU or merely presents through it. The only component that knows
487    // is the one that asked for the adapter, so it says.
488    // trace: bool,
489}
490
491impl eframe::App for Window {
492    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
493        self.app.clear_color()
494    }
495
496    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
497        if !self.owned {
498            // eframe is the only thing that knows the window handle, and only once the
499            // platform has actually made a window.
500            self.app.set_owner(shell::Owner::from_handle(frame));
501            self.owned = true;
502            // And the only thing that knows the graphics device. A device is *lost* when the
503            // driver resets, when a hybrid machine switches GPU, or when the machine sleeps and
504            // the context does not survive it -- after which nothing this program draws can
505            // reach the screen and there is no way to tell from inside the frame. wgpu will say
506            // so exactly once, if anybody has asked to be told.
507            if let Some(state) = frame.wgpu_render_state() {
508                state.device.set_device_lost_callback(|reason, message| {
509                    gpu_trouble(format!("the graphics device was lost ({reason:?}): {message}"));
510                });
511                // if self.trace {
512                //     let it = state.adapter.get_info();
513                //     println!(
514                //         "adapter: {} — {:?} on {:?}, driver {} {}",
515                //         it.name, it.device_type, it.backend, it.driver, it.driver_info
516                //     );
517                // }
518            }
519        }
520        if let Some(what) = GPU_TROUBLE.lock().ok().and_then(|mut slot| slot.take()) {
521            self.app.report(what);
522        }
523        // Before the frame, and only once: the listing has landed by now, and the menu
524        // then has the four frames before `SETTLE` to lay itself out.
525        if self.menu && self.shot.as_ref().is_some_and(|s| s.frame == 8) {
526            self.menu = false;
527            self.app.open_folder_menu(ui.ctx());
528        }
529        // Once the listing has landed and not before: a scan arriving afterwards resets the
530        // cursor and takes the rename with it, which is what happened to the first version of
531        // this and produced a screenshot of an ordinary listing.
532        if self.rename && self.shot.as_ref().is_some_and(|s| s.frame >= 8) && self.app.has_rows() {
533            self.rename = false;
534            self.app.begin_rename_here();
535        }
536        // The same window, for the same reason: the panel follows the keyboard, and a scan
537        // landing afterwards would move the keyboard off the row that was chosen for it.
538        if self.preview && self.shot.as_ref().is_some_and(|s| s.frame >= 8) && self.app.has_rows() {
539            self.preview = false;
540            if self.compare {
541                self.app.compare_here();
542            }
543            self.app.open_preview_here();
544        }
545        self.app.frame(ui);
546        // A menu waiting on the shell is a menu with `Loading...` in it, and the four frames
547        // before `SETTLE` are nowhere near long enough for `QueryContextMenu` — so the
548        // capture waits, or it would photograph the placeholder. Bounded, because the thing
549        // being waited for is other people's code: ten seconds of not answering is an
550        // answer, and a screenshot of the placeholder beats a run that never ends.
551        const PATIENCE: u32 = 600;
552        if (self.app.menu_pending() || self.app.preview_pending()) && self.waited < PATIENCE {
553            self.waited += 1;
554            ui.ctx().request_repaint();
555        } else {
556            self.capture(&ui.ctx().clone());
557        }
558    }
559
560    /// Called before each pass, with the input as the platform handed it over.
561    ///
562    /// The one place raw input can be added to, which two things here need: the Ctrl+V that
563    /// egui-winit swallows, and the button release that an OLE drag consumes.
564    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
565        paste_keystroke(raw_input);
566        raw_input.events.extend(self.app.take_injected());
567    }
568
569    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
570        // Anything copied here is still only a pointer into this process until it is rendered,
571        // so a copy taken a moment ago has to be made real before the process that owns it goes
572        // away. See `shell::flush`.
573        shell::flush();
574        // The window size, the sidebar, the bookmarks and every open tab, so the
575        // next launch opens where this one left off. Skipped for a capture run,
576        // which should not rewrite the user's settings.
577        if self.shot.is_none() {
578            self.app.settings().save();
579        }
580    }
581}
582
583impl Window {
584    /// Ask egui for the frame once things have settled, write it out, and quit.
585    ///
586    /// A PNG, so the file in `docs/` is the file this wrote. It used to be raw RGBA with the
587    /// dimensions in the name, on the grounds that a PNG encoder was too much to carry for a
588    /// developer-only flag — and then every screenshot needed converting by hand. The
589    /// encoder arrived anyway, for the application icon.
590    fn capture(&mut self, ctx: &egui::Context) {
591        const SETTLE: u32 = 12;
592        let Some(shot) = &mut self.shot else { return };
593
594        shot.frame += 1;
595        ctx.request_repaint();
596        if shot.frame < SETTLE {
597            return;
598        }
599        if shot.frame == SETTLE {
600            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
601            return;
602        }
603
604        let image = ctx.input(|i| {
605            i.events.iter().find_map(|e| match e {
606                egui::Event::Screenshot { image, .. } => Some(image.clone()),
607                _ => None,
608            })
609        });
610        let Some(image) = image else { return };
611
612        let [w, h] = image.size;
613        let bytes: Vec<u8> = image
614            .pixels
615            .iter()
616            .flat_map(|p| p.to_srgba_unmultiplied())
617            .collect();
618        let path = shot.path.with_extension("png");
619        match image::save_buffer(
620            &path,
621            &bytes,
622            w as u32,
623            h as u32,
624            image::ExtendedColorType::Rgba8,
625        ) {
626            Ok(()) => println!("wrote {} ({w}x{h})", path.display()),
627            Err(e) => eprintln!("could not write {}: {e}", path.display()),
628        }
629        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
630    }
631}
632
633/// Put back the Ctrl+V that never arrives.
634///
635/// `egui-winit` does not deliver Ctrl+C, Ctrl+X or Ctrl+V as key presses. It recognises them
636/// itself and queues `Event::Copy`, `Event::Cut` or `Event::Paste` *instead*, returning before the
637/// `Event::Key` is added at all — so `i.key_pressed(Key::C)` is never true for a copy, however
638/// reasonable that looks to write. Worse for paste: the `Event::Paste` it substitutes carries the
639/// clipboard's **text**, and it is only queued when there is some. Files are not text, so pressing
640/// Ctrl+V over a listing produced no event of any kind and the shortcut simply did not exist.
641///
642/// Copy and cut need nothing here — `Event::Copy` and `Event::Cut` always arrive, and
643/// [`App::keyboard`] reads them. Paste has nothing to arrive, so the keystroke is put back from
644/// the only place that still knows about it: the keyboard.
645///
646/// # Why the latch is cleared by an event and not by the key going up
647///
648/// The swallowed press still asks for a repaint, so there *is* a frame in which the keys read as
649/// down, and one paste per press needs a latch to stop the following frames repeating it. The
650/// obvious way to clear that latch is to notice the key is no longer down — and it does not work,
651/// because after a paste the window has nothing to draw and stops running frames entirely. The
652/// release goes unobserved, the latch stays set, and Ctrl+V works exactly once per session.
653/// Measured, and it is what a paste that "worked once and then no longer" turns out to be.
654///
655/// `Key { key: V, pressed: false }` is not swallowed and always arrives, and delivering it *is* a
656/// frame. So the release clears the latch through the event queue rather than through a sample
657/// nobody was awake to take.
658#[cfg(windows)]
659fn paste_keystroke(raw_input: &mut egui::RawInput) {
660    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
661    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_V};
662
663    /// Whether this press has already been acted on.
664    static ACTED: AtomicBool = AtomicBool::new(false);
665
666    // The release, whenever it comes, re-arms.
667    let released = raw_input.events.iter().any(|event| {
668        matches!(
669            event,
670            egui::Event::Key {
671                key: egui::Key::V,
672                pressed: false,
673                ..
674            }
675        )
676    });
677    if released {
678        ACTED.store(false, Relaxed);
679    }
680
681    // SAFETY: a pure query of the keyboard state.
682    let down = unsafe {
683        GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0
684            && GetAsyncKeyState(VK_V.0 as i32) as u16 & 0x8000 != 0
685    };
686    if !down {
687        ACTED.store(false, Relaxed);
688        return;
689    }
690    // `focused` because `GetAsyncKeyState` answers for the whole desktop, and a Ctrl+V meant for
691    // another window is not this program's to act on.
692    if !raw_input.focused || ACTED.swap(true, Relaxed) {
693        return;
694    }
695    // Unless egui-winit found text to paste, in which case it has already queued one.
696    if !raw_input
697        .events
698        .iter()
699        .any(|event| matches!(event, egui::Event::Paste(_)))
700    {
701        raw_input.events.push(egui::Event::Paste(String::new()));
702    }
703}
704
705#[cfg(not(windows))]
706fn paste_keystroke(_raw_input: &mut egui::RawInput) {}
