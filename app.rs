1//! The application: state, layout, keyboard, and the one place anything changes.
2//!
3//! # Why everything goes through [`Action`]
4//!
5//! A pane's breadcrumb can close a tab in another pane; the sidebar navigates
6//! whichever pane has focus; dropping a tab restructures the tree the drawing loop
7//! is currently walking. None of that can be done with a `&mut` in hand — so the
8//! drawing code never mutates structure. It pushes an [`Action`], and
9//! [`App::apply`] performs it after the frame is drawn, when nothing is borrowed.
10//!
11//! That is not ceremony: it is what makes "drag this tab out of the pane it is
12//! being drawn from, into a split that does not exist yet" expressible at all.
13
14use std::path::{Path, PathBuf};
15
16use azur_egui_theme as azur;
17use egui::{pos2, vec2, Rect, Ui};
18
19use crate::config::Config;
20use crate::dock::{self, Splitter};
21use crate::fs::places::Place;
22use crate::fs::time::LocalZone;
23use crate::fs::{self, Column};
24use crate::loader::{Loader, Volumes};
25use crate::pane::{Pane, PaneId, Side, Tab};
26use crate::theme::Theme;
27use crate::ui::breadcrumb::{self, CrumbMenu};
28use crate::ui::chrome::{self, TabDrag};
29use crate::ui::sidebar::{self, Sections};
30use crate::ui::{filelist, GUTTER};
31
32/// What the title bar asks of the platform.
33#[derive(Clone, Copy, PartialEq, Eq, Debug)]
34pub enum WindowAction {
35    Minimize,
36    ToggleMaximize,
37    /// Back to [`crate::config::WINDOW_SIZE`] — the way out of a window dragged to a shape you
38    /// did not mean, and the counterpart of double-clicking the sidebar splitter.
39    ResetSize,
40    Close,
41    Drag,
42}
43
44/// Everything the interface can ask for. Performed after the frame, never during.
45pub enum Action {
46    Focus(PaneId),
47
48    // Tabs
49    ActivateTab { pane: PaneId, tab: usize },
50    NewTab { pane: PaneId },
51    /// The same, for the global menu, which has no pane in hand.
52    NewTabFocused,
53    /// Split whichever pane has focus, showing the folder it is already on.
54    SplitFocused { side: Side },
55    CloseTab { pane: PaneId, tab: usize },
56    /// Put the last closed tab back, in whichever pane has focus. `Ctrl+Shift+T`.
57    ReopenTab,
58    NextTab { pane: PaneId, delta: isize },
59    BeginTabDrag { pane: PaneId, tab: usize, grab_dx: f32 },
60    /// Move a tab into another pane's strip. `index == usize::MAX` means append.
61    MoveTab { from: PaneId, tab: usize, to: PaneId, index: usize },
62    /// Pull a tab out into a new pane beside `target`.
63    SplitTab { from: PaneId, tab: usize, target: PaneId, side: Side },
64    /// Open a path in a new pane beside `pane`.
65    OpenInSplit { pane: PaneId, path: PathBuf, side: Side },
66
67    // Navigation
68    Navigate { pane: PaneId, path: PathBuf },
69    NavigateNewTab { pane: PaneId, path: PathBuf },
70    Back(PaneId),
71    Forward(PaneId),
72    Up(PaneId),
73    Refresh(PaneId),
74    EditPath(PaneId),
75
76    // The view
77    Sort { pane: PaneId, column: Column },
78    SelectAll(PaneId),
79    ToggleHidden(PaneId),
80    /// Show this folder's whole tree as one flat listing, or stop.
81    ToggleFlat(PaneId),
82    /// Open this folder's preview panel on whatever the keyboard is on, or shut it.
83    TogglePreview(PaneId),
84    /// Shut it — the panel's own close button.
85    ClosePreview(PaneId),
86    /// The preview panel's position or size changed, so the settings file is out of date.
87    RememberLayout,
88
89    // Files
90    /// Put the selection on the clipboard as a move.
91    Cut(PaneId),
92    /// Put the selection on the clipboard as a copy.
93    Copy(PaneId),
94    /// Act on whatever is on the clipboard, into this pane's folder.
95    Paste(PaneId),
96    /// `permanent` is Shift+Delete; otherwise it goes to the Recycle Bin.
97    Delete { pane: PaneId, permanent: bool },
98    /// Start renaming the row under the cursor.
99    BeginRename(PaneId),
100    /// Finish a rename, or do nothing if the name is unchanged or empty.
101    CommitRename { pane: PaneId, name: String },
102    CancelRename(PaneId),
103    NewFolder(PaneId),
104    /// Pick the selection up and hand it to OLE.
105    DragOut { pane: PaneId, items: Vec<PathBuf> },
106    /// What a right-button drag was asked about, once it has been answered.
107    DropHere {
108        pane: PaneId,
109        items: Vec<PathBuf>,
110        into: PathBuf,
111        moving: bool,
112    },
113    /// Show the shell context menu for these items at this screen position.
114    ///
115    /// `items` empty means the folder's own menu — the one you get by right-clicking the
116    /// background, which is where New and Paste live.
117    ShellMenu {
118        pane: PaneId,
119        items: Vec<PathBuf>,
120        at: (i32, i32),
121    },
122
123    // The outside world
124    Open(PathBuf),
125    /// Open a shortcut in a tab of its own, if it leads somewhere a tab can go.
126    OpenNewTab(PathBuf),
127    Reveal(PathBuf),
128    OpenTerminal(PathBuf),
129    CopyPaths(Vec<PathBuf>),
130    AddBookmark(PathBuf),
131    /// Reorder the bookmarks: take the one at `from` and put it before `to`.
132    MoveBookmark { from: usize, to: usize },
133    RemoveBookmark(PathBuf),
134    ToggleBookmark(PathBuf),
135    SetTheme { dark: bool },
136    Window(WindowAction),
137}
138
139impl Action {
140    /// A stable name, for the journal the click tests read.
141    pub fn name(&self) -> &'static str {
142        match self {
143            Self::Focus(_) => "Focus",
144            Self::ActivateTab { .. } => "ActivateTab",
145            Self::NewTab { .. } => "NewTab",
146            Self::NewTabFocused => "NewTabFocused",
147            Self::SplitFocused { .. } => "SplitFocused",
148            Self::CloseTab { .. } => "CloseTab",
149            Self::ReopenTab => "ReopenTab",
150            Self::NextTab { .. } => "NextTab",
151            Self::BeginTabDrag { .. } => "BeginTabDrag",
152            Self::MoveTab { .. } => "MoveTab",
153            Self::SplitTab { .. } => "SplitTab",
154            Self::OpenInSplit { .. } => "OpenInSplit",
155            Self::Navigate { .. } => "Navigate",
156            Self::NavigateNewTab { .. } => "NavigateNewTab",
157            Self::Back(_) => "Back",
158            Self::Forward(_) => "Forward",
159            Self::Up(_) => "Up",
160            Self::Refresh(_) => "Refresh",
161            Self::EditPath(_) => "EditPath",
162            Self::Sort { .. } => "Sort",
163            Self::SelectAll(_) => "SelectAll",
164            Self::ToggleHidden(_) => "ToggleHidden",
165            Self::ToggleFlat(_) => "ToggleFlat",
166            Self::TogglePreview(_) => "TogglePreview",
167            Self::ClosePreview(_) => "ClosePreview",
168            Self::RememberLayout => "RememberLayout",
169            Self::Cut(_) => "Cut",
170            Self::Copy(_) => "Copy",
171            Self::Paste(_) => "Paste",
172            Self::Delete { .. } => "Delete",
173            Self::BeginRename(_) => "BeginRename",
174            Self::CommitRename { .. } => "CommitRename",
175            Self::CancelRename(_) => "CancelRename",
176            Self::NewFolder(_) => "NewFolder",
177            Self::ShellMenu { .. } => "ShellMenu",
178            Self::DragOut { .. } => "DragOut",
179            Self::DropHere { .. } => "DropHere",
180            Self::Open(_) => "Open",
181            Self::OpenNewTab(_) => "OpenNewTab",
182            Self::Reveal(_) => "Reveal",
183            Self::OpenTerminal(_) => "OpenTerminal",
184            Self::CopyPaths(_) => "CopyPaths",
185            Self::AddBookmark(_) => "AddBookmark",
186            Self::MoveBookmark { .. } => "MoveBookmark",
187            Self::RemoveBookmark(_) => "RemoveBookmark",
188            Self::ToggleBookmark(_) => "ToggleBookmark",
189            Self::SetTheme { .. } => "SetTheme",
190            Self::Window(_) => "Window",
191        }
192    }
193}
194
195/// A context menu the shell is still being asked about.
196    ///
197/// Everything needed to open it, held for the tenth-of-a-second-or-so the shell takes, so
198/// that the menu can appear complete rather than grow while somebody is reading it. The
199/// pointer may well have moved by the time it arrives; `at` is where the click was, which is
200/// where a menu belongs.
201struct Asking {
202    token: u64,
203    pane: PaneId,
204    at: egui::Pos2,
205    items: Vec<PathBuf>,
206    folder: PathBuf,
207    /// How much of a menu was asked for, so that a slow one can be asked for again with less.
208    depth: crate::shell::menu::Depth,
209    /// The pass this was asked for in, so the click that asked for it is not also the click
210    /// that cancels it — see [`App::pump_asking`].
211    since: u64,
212    /// When it was asked for, for the deadline in [`App::pump_asking`].
213    asked: std::time::Instant,
214}
215
216/// How many closed tabs are remembered, for `Ctrl+Shift+T`.
217///
218/// Ten, because the gesture is for undoing a click that went wrong — one or two deep, in the
219/// half-minute after it happened. Nobody reaches for it to go archaeology.
220const CLOSED_TABS: usize = 10;
221
222pub struct App {
223    theme: Theme,
224    /// Set once per theme change; installing a style every frame would throw away
225    /// egui's own caches.
226    installed: bool,
227    zone: LocalZone,
228    loader: Loader,
229
230    layout: dock::Node,
231    panes: Vec<Pane>,
232    next_pane: PaneId,
233    focused: PaneId,
234    /// Where the tabs that have been closed were pointing, oldest first, capped at
235    /// [`CLOSED_TABS`].
236    ///
237    /// The path and nothing else. A closed tab's own Back and Forward trail is deliberately not
238    /// kept: reopening is "put that folder back", not "restore that tab as it was", and ten
239    /// tabs' worth of navigation history is a great deal of state to carry around for a gesture
240    /// that exists to undo a misplaced click. A reopened tab therefore starts with the one
241    /// place in its history, exactly as a tab opened any other way does.
242    ///
243    /// Lives for the session. Closing the window closes it with everything in it, and the last
244    /// tab of the last pane is never recorded — that close *is* the window closing.
245    closed: Vec<PathBuf>,
246
247    drag: Option<TabDrag>,
248    /// Events for egui that no platform event will bring — see [`App::release_buttons`].
249    injected: Vec<egui::Event>,
250    /// Tells us when a folder on screen changes underneath us.
251    watch: crate::watch::Watch,
252    crumbs: CrumbMenu,
253    /// The shell icons, resolved once per file type and held for the session.
254    icons: crate::shell::icons::Icons,
255    /// What each shortcut row points at, asked once per row per view.
256    links: crate::shell::links::Links,
257    /// Where every pane's preview panel goes and how much room it takes: the window's
258    /// preference, one of it. Whether one is *showing* is the tab's — see `Tab::preview`.
259    preview: crate::ui::preview::Layout,
260    /// Reads whatever the preview panels are pointed at, off the UI thread.
261    previews: crate::preview::Previews,
262    /// The window the shell parents its own dialogs to.
263    owner: crate::shell::Owner,
264    /// Shell file operations in flight.
265    ops: crate::shell::ops::Operations,
266    /// Items cut but not yet pasted, shown ghosted the way Explorer shows them.
267    cut: Vec<PathBuf>,
268    /// The last thing that went wrong, for the status line.
269    notice: Option<String>,
270    /// Where a drag from outside is hovering, in points, for the pane highlight.
271    drop_hover: Option<(i32, i32)>,
272    /// Whether the clipboard is offering files. Sampled once a frame rather than once
273    /// per menu entry, since it is a syscall.
274    clipboard_has_files: bool,
275    /// Files being dragged in from elsewhere, and the drops that land.
276    drops: crate::shell::dnd::Zone,
277    /// The two shell calls that take over the pointer, run off the UI thread.
278    modal: crate::shell::Modal,
279    /// The context menu, while one is open.
280    menu: Option<crate::ui::menu::Open>,
281    /// A context menu that has been asked for and is not on screen yet.
282    asking: Option<Asking>,
283    /// Builds the shell's half of the context menu, off this thread — half a second of
284    /// `QueryContextMenu` on a file, which used to be half a second of frozen window.
285    menu_builder: crate::shell::menu::Builder,
286    /// Where the sidebar's Bookmarks group was drawn, so a drag can be dropped on it.
287    /// Read a frame later than it is written, which is a frame the sidebar has not moved in.
288    bookmarks_rect: Option<Rect>,
289    /// A bookmark being dragged to a new position in the list.
290    bookmark_drag: Option<usize>,
291    /// The OLE drag in flight, if there is one, and the pane the files were picked up in so
292    /// a move can re-read the folder they left. One at a time: the pointer is only holding
293    /// one thing, and a second drag would be following the same button as the first.
294    file_drag: Option<(PaneId, crate::shell::dnd::Drag)>,
295
296    volumes: Volumes,
297    places: Vec<Place>,
298    bookmarks: Vec<PathBuf>,
299    sections: Sections,
300    sidebar_width: f32,
301
302    maximized: bool,
303    /// The window's inner size, tracked so it can be restored next launch.
304    window_size: Option<[f32; 2]>,
305    /// And where it is: the outer top-left corner in *physical pixels*. See
306    /// [`crate::config::Config::position`] for why that unit and not points.
307    window_position: Option<[f32; 2]>,
308    /// Collected during drawing, drained after.
309    actions: Vec<Action>,
310    /// One buffer every formatted cell in the window is written through.
311    scratch: String,
312    /// Where each pane was drawn, for the docking gesture and for keyboard focus.
313    pane_rects: Vec<(PaneId, Rect)>,
314    /// Every tab drawn this frame, wherever its strip was. The drag resolution needs all
315    /// of them at once, and they are not all known until the panes have been drawn.
316    tab_slots: Vec<chrome::Slot>,
317    splitters: Vec<Splitter>,
318    pane_order: Vec<PaneId>,
319
320    /// Settings worth writing back, and whether they have changed.
321    config: Config,
322    config_dirty: bool,
323
324    /// Keeps frames coming while the window is being resized, rescaled or restored.
325    settling: Settling,
326    /// `--walk=<dir>`: browse by itself, for measuring a real session's memory.
327    walk: Option<Walk>,
328    /// `--scroll`: run the listing up and down, for the same reason.
329    scrolling: Scrolling,
330
331    /// Names of the actions performed, when something is watching.
332    ///
333    /// Off unless a test turns it on. Click targets are the one part of this program
334    /// that cannot be checked by reading — an interaction rect covered by another one
335    /// looks perfectly correct in the source and simply does not respond — so the
336    /// tests drive real pointer events through real frames, and this is how they see
337    /// what landed.
338    journal: Option<Vec<&'static str>>,
339}
340
341/// What the window looks like to the platform, as far as rendering crisply is concerned.
342#[derive(Clone, Copy, PartialEq, Debug, Default)]
343struct Shape {
344    /// In *physical pixels*, which is what the framebuffer is sized in.
345    pixels: (u32, u32),
346    /// Rounded, because a scale factor arrives as a float and comparing floats for
347    /// equality every frame is a way to request repaints for ever.
348    scale: u32,
349    /// What the *platform* says the scale is, against `scale`, which is what egui rasterised
350    /// the glyph atlas for.
351    ///
352    /// Tracked separately because the two disagreeing is the one thing that would make text
353    /// soft without anything else moving: an atlas built for one scale, drawn at another.
354    /// Zero when the platform has not said.
355    native_scale: u32,
356    focused: bool,
357    minimized: bool,
358}
359
360impl Shape {
361    fn of(ctx: &egui::Context) -> Self {
362        let scale = ctx.pixels_per_point();
363        ctx.input(|i| {
364            let size = i.viewport_rect().size() * scale;
365            Self {
366                pixels: (size.x.round() as u32, size.y.round() as u32),
367                scale: (scale * 1000.0).round() as u32,
368                native_scale: i
369                    .viewport()
370                    .native_pixels_per_point
371                    .map_or(0, |ppp| (ppp * 1000.0).round() as u32),
372                focused: i.viewport().focused.unwrap_or(true),
373                minimized: i.viewport().minimized.unwrap_or(false),
374            }
375        })
376    }
377}
378
379/// This process's private bytes, and its GDI and USER handle counts.
380///
381/// The three numbers a memory report is actually about. Private bytes is Task Manager's
382/// "Memory" column, and it counts COM's allocator and every loaded shell extension's own
383/// heap as well as Rust's. The handle counts are here because a leaked `HICON`, `HBITMAP` or
384/// `HDC` costs memory without a single Rust allocation, and this program asks the shell for
385/// an icon per file type it meets.
386#[cfg(windows)]
387fn process_memory() -> (usize, u32, u32) {
388    use windows::Win32::System::ProcessStatus::{
389        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
390    };
391    use windows::Win32::System::Threading::{
392        GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS, GR_USEROBJECTS,
393    };
394
395    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
396        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
397        ..Default::default()
398    };
399    // SAFETY: the struct is told its own size, and the handle is this process's
400    // pseudo-handle, which needs no closing.
401    unsafe {
402        let me = GetCurrentProcess();
403        let _ = GetProcessMemoryInfo(
404            me,
405            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
406            counters.cb,
407        );
408        (
409            counters.PrivateUsage,
410            GetGuiResources(me, GR_GDIOBJECTS),
411            GetGuiResources(me, GR_USEROBJECTS),
412        )
413    }
414}
415
416#[cfg(not(windows))]
417fn process_memory() -> (usize, u32, u32) {
418    (0, 0, 0)
419}
420
421/// `--walk=<dir>`: browse subfolder after subfolder, on a timer, for as long as the window
422/// is open.
423///
424/// The measurement in `app::click_tests` runs without a renderer, so it can only see the
425/// Rust heap and the handles — not the GL driver, the texture the glyph atlas lives in, or
426/// the shell extensions a real session loads. This drives the *real* window through the same
427/// walk with `--trace` reporting beside it, which is the only way to watch the number a user
428/// is watching.
429struct Walk {
430    dirs: Vec<PathBuf>,
431    at: usize,
432    stepped_at: f64,
433}
434
435impl Walk {
436    /// A step every this often. Faster than a person browses, slow enough that each folder
437    /// is actually read and drawn rather than superseded before its scan lands.
438    const STEP: f64 = 0.25;
439
440    fn collect(from: &Path) -> Self {
441        let mut dirs = Vec::new();
442        let mut queue = std::collections::VecDeque::from([from.to_path_buf()]);
443        while let Some(dir) = queue.pop_front() {
444            if dirs.len() >= 2000 {
445                break;
446            }
447            let Ok(entries) = std::fs::read_dir(&dir) else {
448                continue;
449            };
450            for entry in entries.flatten() {
451                if entry.file_type().is_ok_and(|t| t.is_dir()) {
452                    queue.push_back(entry.path());
453                    dirs.push(entry.path());
454                }
455            }
456        }
457        println!("walking {} folders under {}", dirs.len(), from.display());
458        Self {
459            dirs,
460            at: 0,
461            stepped_at: 0.0,
462        }
463    }
464
465    /// The next folder, if it is time for one.
466    fn step(&mut self, time: f64) -> Option<PathBuf> {
467        if self.dirs.is_empty() || time - self.stepped_at < Self::STEP {
468            return None;
469        }
470        self.stepped_at = time;
471        self.at = (self.at + 1) % self.dirs.len();
472        Some(self.dirs[self.at].clone())
473    }
474}
475
476/// `--scroll`: run the listing up and down for ever, which is the one thing a harness with no
477/// renderer cannot measure.
478///
479/// Scrolling redraws rows that have already been drawn — no new names, no new icons, no new
480/// column widths — so anything that grows under this grows *per frame*, in the painting, and
481/// leaving the folder would not give it back.
482#[derive(Default)]
483struct Scrolling {
484    on: bool,
485    /// Where in the sweep, 0..1 and back.
486    phase: f32,
487    up: bool,
488}
489
490impl Scrolling {
491    /// A step every frame, as fast as the window will paint.
492    fn step(&mut self, rows: usize) -> Option<f32> {
493        if !self.on || rows == 0 {
494            return None;
495        }
496        let span = rows as f32 * crate::pane::ROW_HEIGHT;
497        // Two seconds top to bottom at 60fps, which is faster than a wheel and slower than a
498        // dragged scrollbar.
499        let step = 1.0 / 120.0;
500        if self.up {
501            self.phase -= step;
502            if self.phase <= 0.0 {
503                self.up = false;
504            }
505        } else {
506            self.phase += step;
507            if self.phase >= 1.0 {
508                self.up = true;
509            }
510        }
511        Some(self.phase.clamp(0.0, 1.0) * span)
512    }
513}
514
515/// Asks for frames for a moment after the window's shape changes.
516///
517/// This program paints on demand, which is the right default — a file listing that repaints
518/// sixty times a second to show the same rows is a laptop fan. But it means that between
519/// events the screen holds whatever was painted last, and a *stale* frame is only as good
520/// as the assumption that the window still looks the way it did: restore it from the
521/// taskbar, drag it to a monitor at another scale, or let a maximised window resize when
522/// the taskbar hides, and the compositor has a surface of the wrong size to show. It
523/// stretches it, which arrives as text going soft for as long as nothing asks for a new
524/// frame — and then coming back sharp the moment something does.
525///
526/// So: notice the shape changing, and keep asking for frames until it has been still for
527/// [`Self::QUIET`]. Costs a handful of frames per window gesture and nothing at rest.
528#[derive(Default)]
529struct Settling {
530    was: Shape,
531    /// When the shape last changed, in `InputState::time`.
532    changed_at: Option<f64>,
533    /// `--trace`: report each change on stdout.
534    trace: bool,
535    /// When `--trace` last reported the memory figures.
536    reported_at: f64,
537}
538
539impl Settling {
540    /// How long after a change to keep painting. Long enough to cover a restore animation
541    /// and a scale change, short enough that nobody notices the frames.
542    const QUIET: f64 = 0.75;
543
544    /// How often `--trace` reports the memory figures. Often enough to see a curve over a
545    /// minute of browsing, rare enough that the log stays readable.
546    const REPORT: f64 = 3.0;
547
548    /// Whether a frame should be asked for.
549    fn observe(&mut self, now: Shape, time: f64) -> bool {
550        if now != self.was {
551            if self.trace {
552                println!("{time:8.3}  {:?} -> {now:?}", self.was);
553            }
554            self.was = now;
555            self.changed_at = Some(time);
556        }
557        match self.changed_at {
558            Some(at) if time - at < Self::QUIET => true,
559            Some(_) => {
560                self.changed_at = None;
561                false
562            }
563            None => false,
564        }
565    }
566}
567
568/// One pane as the settings file remembers it: where its tabs point, and which was in front.
569fn pane_tabs(pane: &Pane) -> crate::config::PaneTabs {
570    crate::config::PaneTabs {
571        paths: pane.tabs.iter().map(|tab| tab.path.clone()).collect(),
572        active: pane.active.min(pane.tabs.len().saturating_sub(1)),
573    }
574}
575
576impl App {
577    /// `open` is what the command line asked for: one pane per path, so
578    /// `yafe --open=A --open=B` comes up side by side. Empty means restore the
579    /// remembered tabs instead.
580    /// `side` is where each extra `--open=` path lands.
581    ///
582    /// Only the command line passes anything but `Right`: `--stack` is how a capture run
583    /// gets a stacked layout on screen, which is the one arrangement whose tab strips do
584    /// not live in the title bar, and so the one a screenshot has to be able to show.
585    pub fn opening(ctx: &egui::Context, config: Config, open: Vec<PathBuf>, side: Side) -> Self {
586        let theme = if config.dark {
587            Theme::dark()
588        } else {
589            Theme::light()
590        };
591        let loader = Loader::new(ctx);
592        // Letters now, labels and free space in the background. Asking for a volume
593        // label on the startup path is what makes a file manager take twenty seconds
594        // to open on a machine with a mapped share.
595        let volumes = Volumes::new(ctx);
596        // The known folders are a handful of `SHGetKnownFolderPath` calls -- single
597        // digit milliseconds, and they do not change while the window is open.
598        let places = fs::places::standard();
599
600        let first = 1u32;
601        let mut panes = Vec::new();
602        let mut layout = dock::Node::Leaf(first);
603        let mut next_pane = 2;
604        let mut focused = first;
605
606        if open.is_empty() {
607            // The panes that were open last time, and the tree that arranged them. Both, or
608            // neither: a layout is a tree over exactly these panes, so if it does not read
609            // back as one — a file from before it was written, or one edited into something
610            // else — every tab goes into a single pane, which is what this used to do always.
611            let groups: Vec<&crate::config::PaneTabs> = config
612                .panes
613                .iter()
614                // A pane with no tabs is not a pane. Dropping them here is also what keeps
615                // the numbering honest: `decode` insists on one leaf per pane, so a dropped
616                // one is caught as a count that no longer matches rather than shifting every
617                // pane along by one.
618                .filter(|group| !group.paths.is_empty())
619                .collect();
620            let ids: Vec<PaneId> = (0..groups.len()).map(|i| first + i as u32).collect();
621            let tree = config
622                .layout
623                .as_deref()
624                .filter(|_| groups.len() > 1)
625                .and_then(|text| dock::Node::decode(text, &ids));
626
627            match tree {
628                Some(tree) => {
629                    for (group, &id) in groups.iter().zip(&ids) {
630                        let mut tabs = group.paths.iter().cloned().map(Tab::new);
631                        let mut pane =
632                            Pane::new(id, tabs.next().expect("a group with no tabs was filtered"));
633                        pane.tabs.extend(tabs);
634                        pane.active = group.active.min(pane.tabs.len() - 1);
635                        panes.push(pane);
636                    }
637                    next_pane = first + ids.len() as u32;
638                    focused = ids.get(config.focus).copied().unwrap_or(first);
639                    layout = tree;
640                }
641                None => {
642                    let mut tabs: Vec<Tab> = groups
643                        .iter()
644                        .flat_map(|group| group.paths.iter())
645                        .cloned()
646                        .map(Tab::new)
647                        .collect();
648                    if tabs.is_empty() {
649                        tabs.push(Tab::new(fs::places::default_start()));
650                    }
651                    let mut pane = Pane::new(first, tabs.remove(0));
652                    pane.tabs.extend(tabs);
653                    panes.push(pane);
654                }
655            }
656        } else {
657            let mut paths = open.into_iter();
658            panes.push(Pane::new(first, Tab::new(paths.next().unwrap_or_default())));
659            for path in paths {
660                let id = next_pane;
661                next_pane += 1;
662                panes.push(Pane::new(id, Tab::new(path)));
663                layout.split(id - 1, side, id);
664            }
665        }
666
667        Self {
668            theme,
669            installed: false,
670            zone: LocalZone::current(),
671            loader,
672            layout,
673            panes,
674            next_pane,
675            focused,
676            closed: Vec::new(),
677            drag: None,
678            injected: Vec::new(),
679            watch: crate::watch::Watch::new(ctx),
680            crumbs: CrumbMenu::default(),
681            icons: crate::shell::icons::Icons::new(),
682            links: crate::shell::links::Links::new(ctx),
683            previews: crate::preview::Previews::new(ctx),
684            owner: crate::shell::Owner::default(),
685            ops: crate::shell::ops::Operations::new(),
686            cut: Vec::new(),
687            notice: None,
688            drop_hover: None,
689            clipboard_has_files: false,
690            drops: crate::shell::dnd::Zone::new(),
691            modal: crate::shell::Modal::new(ctx),
692            menu: None,
693            asking: None,
694            menu_builder: crate::shell::menu::Builder::new(ctx),
695            bookmarks_rect: None,
696            bookmark_drag: None,
697            file_drag: None,
698            volumes,
699            places,
700            bookmarks: config.bookmarks.clone(),
701            sections: config.sections,
702            sidebar_width: config.sidebar_width,
703            preview: config.preview,
704            maximized: false,
705            window_size: None,
706            window_position: None,
707            actions: Vec::new(),
708            scratch: String::with_capacity(64),
709            pane_rects: Vec::new(),
710            tab_slots: Vec::new(),
711            splitters: Vec::new(),
712            pane_order: vec![first],
713            config,
714            config_dirty: false,
715            settling: Settling::default(),
716            walk: None,
717            scrolling: Scrolling::default(),
718            journal: None,
719        }
720    }
721
722    /// `--walk=<dir>`: browse subfolder after subfolder by itself. See [`Walk`].
723    pub fn walking(mut self, from: Option<PathBuf>) -> Self {
724        self.walk = from.as_deref().map(Walk::collect);
725        self
726    }
727
728    /// `--flat`: open with every pane flattened, for looking at that view without having to
729    /// press the button first. The same family as `--menu` and `--rename`.
730    pub fn flattened(mut self, on: bool) -> Self {
731        if on {
732            for pane in &mut self.panes {
733                for tab in &mut pane.tabs {
734                    tab.flat = true;
735                }
736            }
737        }
738        self
739    }
740
741    /// `--scroll`: run the listing up and down for ever. See [`Scrolling`].
742    pub fn scrolling(mut self, on: bool) -> Self {
743        self.scrolling.on = on;
744        self
745    }
746
747    /// `--trace`: report every change to the window's shape on stdout.
748    ///
749    /// For the one class of bug this program cannot see from the inside — text that goes
750    /// soft for a few seconds after a window gesture. One line per change says whether it
751    /// was the size, the scale, the focus or the minimised flag that moved, which is the
752    /// difference between a fix and a guess.
753    pub fn tracing(mut self, on: bool) -> Self {
754        self.settling.trace = on;
755        self
756    }
757
758    /// Tell the shell which window to parent its dialogs to.
759    ///
760    /// Set once, from eframe, which is the only thing that knows the handle. Without
761    /// it a progress or conflict dialog appears behind this window and looks hung.
762    pub fn set_owner(&mut self, owner: crate::shell::Owner) {
763        self.owner = owner;
764    }
765
766    /// Select and scroll to an entry in the first pane once its listing arrives.
767    ///
768    /// Deferred rather than applied here because the listing is not read yet — the
769    /// tab carries the name and [`Tab::apply`] acts on it, which is the same path
770    /// going Up uses to highlight the folder you came out of.
771    pub fn revealing(mut self, name: Option<String>) -> Self {
772        if let Some(name) = name {
773            if let Some(pane) = self.panes.first_mut() {
774                pane.tab_mut().reveal = Some(name);
775            }
776        }
777        self
778    }
779
780    /// The colour eframe clears the window to, so a resize does not flash.
781    pub fn clear_color(&self) -> [f32; 4] {
782        self.theme.bg.canvas.to_normalized_gamma_f32()
783    }
784
785    /// Settings to write back on the way out.
786    pub fn settings(&self) -> Config {
787        let mut config = Config {
788            bookmarks: self.bookmarks.clone(),
789            sidebar_width: self.sidebar_width,
790            preview: self.preview,
791            sections: self.sections,
792            panes: Vec::new(),
793            layout: None,
794            focus: 0,
795            window: self.window_size,
796            position: self.window_position,
797            maximized: self.maximized,
798            dark: self.theme.dark,
799        };
800
801        // In layout order, because that is the order [`dock::Node::encode`] numbers the panes
802        // in and the two have to agree — the `pane` lines are what the `layout` line's numbers
803        // point at.
804        //
805        // Nothing is deduplicated. It used to be, because every tab was going into one pane
806        // and the same folder twice would have been two tabs on it; now the same folder open
807        // in two panes is a thing somebody arranged deliberately, and dropping the second copy
808        // would take a pane's only tab away from it.
809        let mut order = Vec::new();
810        self.layout.panes(&mut order);
811        for (slot, id) in order.iter().enumerate() {
812            let Some(pane) = self.panes.iter().find(|p| p.id == *id) else {
813                // A leaf with no pane behind it cannot happen, and if it did the layout would
814                // no longer describe what follows it — so the tree is dropped rather than
815                // written wrong. The tabs are still all here.
816                config.layout = None;
817                config.panes.clear();
818                for pane in &self.panes {
819                    config.panes.push(pane_tabs(pane));
820                }
821                return config;
822            };
823            if *id == self.focused {
824                config.focus = slot;
825            }
826            config.panes.push(pane_tabs(pane));
827        }
828        config.layout = Some(self.layout.encode());
829        config
830    }
831
832    // ---------------------------------------------------------------------
833    // The frame
834    // ---------------------------------------------------------------------
835
836    pub fn frame(&mut self, ui: &mut Ui) {
837        let ctx = ui.ctx().clone();
838        if !self.installed {
839            azur::install(
840                &ctx,
841                self.theme.azur(),
842                &azur::StyleOptions {
843                    // A dense, table-heavy window, which is exactly the case the
844                    // design system documents these two for.
845                    control_height: azur::tokens::control::SMALL,
846                    selectable_labels: false,
847                    ..Default::default()
848                },
849            );
850            self.installed = true;
851        }
852        self.maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
853        self.close_on_blur(&ctx);
854        // Keep painting while the window is being restored, resized or rescaled, so the
855        // compositor never has to stretch a frame that was drawn for a different shape.
856        let now = ctx.input(|i| i.time);
857        if self.settling.observe(Shape::of(&ctx), now) {
858            ctx.request_repaint();
859        }
860        // `--walk`: step to the next folder by itself, so a real window can be measured
861        // browsing rather than sitting still.
862        if let Some(next) = self.walk.as_mut().and_then(|walk| walk.step(now)) {
863            let pane = self.focused;
864            self.perform(&ctx, Action::Navigate { pane, path: next });
865            ctx.request_repaint_after(std::time::Duration::from_secs_f64(Walk::STEP));
866        }
867        // `--scroll`: keep the listing moving, so the painting is measured and not just the
868        // reading.
869        if self.scrolling.on {
870            let rows = self.panes.iter().find(|p| p.id == self.focused).map_or(0, |p| p.tab().order.len());
871            if let Some(to) = self.scrolling.step(rows) {
872                if let Some(pane) = self.panes.iter_mut().find(|p| p.id == self.focused) {
873                    pane.tab_mut().scroll_to = Some(to);
874                }
875                ctx.request_repaint();
876            }
877        }
878        // `--trace`: the memory figures, every few seconds, beside what the cache is holding.
879        // Together they say whether growth is the cache filling up or something being kept.
880        //
881        // A repaint has to be booked for it, because this program is idle between events and
882        // a report that only fires when something else asks for a frame is a report that
883        // never fires while you sit and watch the number climb.
884        if self.settling.trace {
885            ctx.request_repaint_after(std::time::Duration::from_secs_f64(Settling::REPORT));
886            if now - self.settling.reported_at > Settling::REPORT {
887                self.settling.reported_at = now;
888                let (private, gdi, user) = process_memory();
889                let (dirs, entries) = self.loader.held();
890                let (kinds, paths, textures) = self.icons.held();
891                println!(
892                    "{now:8.1}  private {:>7} KB   gdi {gdi:>5}   user {user:>4}   \
893                     cache {dirs:>3} folders / {entries:>8} entries   \
894                     icons {kinds:>4} kinds / {paths:>5} paths / {textures:>4} textures /                      {} bitmaps / {} uploads",
895                    private / 1024,
896                    self.icons.bitmaps,
897                    self.icons.uploads
898                );
899            }
900        }
901        // A drive may have been plugged in or ejected while the window was in the
902        // background, and regaining focus is the one moment it is worth re-probing.
903        if ctx.input(|i| {
904            i.events
905                .iter()
906                .any(|e| matches!(e, egui::Event::WindowFocused(true)))
907        }) {
908            self.volumes.refresh(&ctx);
909        }
910        self.volumes.poll();
911        self.icons.poll(&ctx);
912        // Which listings are on screen, so the icon worker can drop the questions it still has
913        // about folders nobody is looking at any more.
914        let views: Vec<u64> = self
915            .panes
916            .iter()
917            .flat_map(|pane| pane.tabs.iter())
918            .map(|tab| tab.view)
919            .collect();
920        self.icons.only(&views);
921        self.deliver_icons();
922        self.deliver_links();
923        self.collect_previews(&ctx, now);
924        // Both of these give memory back once nothing has wanted it for two minutes, so a
925        // session that browsed a hundred folders and then settled does not hold the
926        // high-water mark until the window closes.
927        self.loader.sweep();
928        self.clipboard_has_files = crate::shell::clipboard::has_files();
929        self.collect_operations();
930        // Registered on the window rather than at startup, because the handle does not
931        // exist until the platform has made one.
932        self.drops.attach(self.owner);
933        self.publish_drop_targets(&ctx);
934        self.collect_drops(&ctx);
935        self.pump_drag(&ctx);
936        self.pump_asking(&ctx);
937        self.collect_changes(&ctx);
938        self.collect_modal();
939        if !self.maximized {
940            // Only a restored window's size is worth remembering; a maximised one is
941            // described by the flag, and saving the screen size would pin the window
942            // to this monitor.
943            let size = ctx.viewport_rect().size();
944            if size.x >= 640.0 && size.y >= 400.0 {
945                self.window_size = Some([size.x, size.y]);
946            }
947            // Where it is, in physical pixels: the *outer* rect, because that is what the
948            // platform places and what it will be given back. egui reports it in points, so
949            // this multiplies by the same scale factor the restore divides by — which is what
950            // makes the round trip exact whatever the monitor's DPI turns out to be.
951            //
952            // `None` while the window is minimised, when there is no position to have; the
953            // last real one stays, which is the one worth reopening at.
954            if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
955                let scale = ctx.pixels_per_point();
956                self.window_position = Some([outer.min.x * scale, outer.min.y * scale]);
957            }
958        }
959
960        self.collect_scans();
961        self.start_scans(&ctx, now);
962
963        // Swapped out rather than borrowed, because the drawing code needs `&Theme`
964        // and `&mut self` at the same time. The placeholder is the same side of the
965        // palette, so nothing can read the wrong one.
966        let placeholder = if self.theme.dark {
967            Theme::dark()
968        } else {
969            Theme::light()
970        };
971        let theme = std::mem::replace(&mut self.theme, placeholder);
972
973        // ---- Geometry, before anything is drawn -------------------------
974        //
975        // A pane's tabs sit directly above that pane, in the title bar or on a band of
976        // their own, so where the panes are has to be known before the bar can be drawn —
977        // and the bands take room off the top of the panes under them, so the panes cannot
978        // be drawn until the bands are decided either. Everything here paints at explicit
979        // rects rather than through egui's panels, which is what makes that ordering free
980        // to choose: the title bar is drawn *last*, over canvas nothing else wanted.
981        let screen = ctx.viewport_rect();
982        let bar = chrome::bar_rect(screen);
983        let body = Rect::from_min_max(pos2(screen.left(), bar.bottom()), screen.max);
984        let plan = self.plan_layout(body, bar);
985
986        self.tab_slots.clear();
987        self.body(ui, &theme, body, &plan);
988        let in_bar = chrome::title_bar(
989            ui,
990            &theme,
991            &self.panes,
992            &plan.in_bar,
993            self.focused,
994            self.maximized,
995            &self.drag,
996            &mut self.icons,
997            &mut self.actions,
998        );
999        self.tab_slots.extend(in_bar);
1000
1001        // Resolved after the panes, so the rects the pointer is tested against are the
1002        // ones drawn this frame rather than last frame's.
1003        if self.drag.is_some() {
1004            let slots = std::mem::take(&mut self.tab_slots);
1005            let mut drag = self.drag.take();
1006            chrome::resolve_drag(ui, &theme, &self.panes, &slots, &mut drag, &mut self.actions);
1007            self.drag = drag;
1008            self.tab_slots = slots;
1009        }
1010
1011        // Over everything, and in the root `Ui` so its coordinates are the screen's.
1012        self.draw_menu(ui, &theme);
1013
1014        // Last, so they are on top of everything — though they are sized to sit in
1015        // canvas the panels do not reach, so there is nothing to be on top of.
1016        chrome::resize_borders(ui, self.maximized);
1017
1018        // An undecorated window has no frame of its own, and on a dark desktop its
1019        // edge would be invisible. `stroke-default` is the line Azur gives a panel.
1020        let edge = ctx.viewport_rect();
1021        ui.painter().rect_stroke(
1022            edge,
1023            egui::CornerRadius::ZERO,
1024            egui::Stroke::new(1.0, self.theme.stroke.default),
1025            egui::StrokeKind::Inside,
1026        );
1027
1028        self.keyboard(&ctx);
1029        self.thumb_buttons(&ctx);
1030        self.apply(&ctx);
1031    }
1032
1033    /// Divide the window up and lay the panes out, without drawing anything.
1034    ///
1035    /// Split out from [`App::body`] because the title bar needs the answer before it can
1036    /// place its tab strips, and because it is arithmetic worth being able to check on its
1037    /// own. Returns where every pane's tabs go, with the panes already reduced by the room
1038    /// their strip bands take.
1039    fn plan_layout(&mut self, body: Rect, bar: Rect) -> chrome::StripPlan {
1040        let (_, panes_area) = Self::split_body(body, self.sidebar_width);
1041        self.layout
1042            .layout(panes_area, &mut self.pane_rects, &mut self.splitters);
1043        self.pane_order = self.pane_rects.iter().map(|(id, _)| *id).collect();
1044        chrome::plan_strips(&mut self.pane_rects, bar)
1045    }
1046
1047    /// The sidebar and the area the panes divide between them.
1048    ///
1049    /// The body, whole: the panels reach the window's edges, and the only thing between a panel
1050    /// and the outside is the window's own one-pixel border painted over the top of it. They used
1051    /// to stop [`GUTTER`] short of it on every side, which put a band of canvas round the block
1052    /// and made it read as a tray of cards.
1053    ///
1054    /// [`crate::ui::SEAM`] between the two, and that is the only division in here.
1055    fn split_body(body: Rect, sidebar_width: f32) -> (Rect, Rect) {
1056        let inner = body;
1057        let width = sidebar_width.clamp(140.0, (inner.width() - 240.0).max(140.0));
1058        let sidebar = Rect::from_min_max(inner.min, pos2(inner.left() + width, inner.bottom()));
1059        let panes_area =
1060            Rect::from_min_max(pos2(sidebar.right() + crate::ui::SEAM, inner.top()), inner.max);
1061        (sidebar, panes_area)
1062    }
1063
1064    /// The sidebar, the splitter between it and the panes, the tab bands, and the panes.
1065    fn body(&mut self, ui: &mut Ui, t: &Theme, full: Rect, plan: &chrome::StripPlan) {
1066        if full.width() < 80.0 || full.height() < 40.0 {
1067            return;
1068        }
1069
1070        let inner = full;
1071        let (sidebar, panes_area) = Self::split_body(full, self.sidebar_width);
1072
1073        // ---- What shows through the seams ---------------------------------
1074        //
1075        // One fill behind the whole block, so every boundary between two panels is whatever
1076        // this leaves showing — and no panel has to know where its neighbours are. That
1077        // matters more than it sounds: panes come from a tree of splits, so "where the seams
1078        // are" is the layout's answer and it changes with every drag, whereas "the panels do
1079        // not quite cover this" is true for free.
1080        ui.painter()
1081            .rect_filled(inner, egui::CornerRadius::ZERO, crate::ui::seam(t));
1082
1083        // ---- The sidebar --------------------------------------------------
1084        //
1085        // Its content fills it. There was a point of inset here, left from when the panel wore a
1086        // one-pixel ring and the content had to start inside it; with the ring gone it was a
1087        // point of nothing, on all four sides of both panels.
1088        self.surface(ui, t, sidebar);
1089        {
1090            let mut child = ui.new_child(
1091                egui::UiBuilder::new()
1092                    .max_rect(sidebar)
1093                    .layout(egui::Layout::top_down(egui::Align::Min)),
1094            );
1095            child.set_clip_rect(sidebar.intersect(ui.clip_rect()));
1096            let current = self
1097                .panes
1098                .iter()
1099                .find(|p| p.id == self.focused)
1100                .map(|p| p.tab().path.clone())
1101                .unwrap_or_default();
1102            self.bookmarks_rect = sidebar::show(
1103                &mut child,
1104                t,
1105                self.volumes.all(),
1106                &self.bookmarks,
1107                &self.places,
1108                &current,
1109                self.focused,
1110                &mut self.sections,
1111                &mut self.icons,
1112                &mut self.bookmark_drag,
1113                &mut self.scratch,
1114                &mut self.actions,
1115            );
1116            // A drag hovering over Bookmarks. Same highlight the listing gets, so pinning
1117            // reads as a drop rather than as nothing happening — and drawn here, after the
1118            // rows, for the same reason it is in the pane.
1119            if let Some(area) = self.bookmarks_preview(ui.ctx().pixels_per_point()) {
1120                crate::ui::drop_target(child.painter(), area, t);
1121            }
1122        }
1123
1124        // ---- The splitter beside it ---------------------------------------
1125        let grip = Rect::from_min_max(
1126            pos2(sidebar.right() - 3.0, inner.top()),
1127            pos2(panes_area.left() + 3.0, inner.bottom()),
1128        );
1129        let response = ui.interact(
1130            grip,
1131            egui::Id::new("sidebar-grip"),
1132            egui::Sense::click_and_drag(),
1133        );
1134        if response.hovered() || response.dragged() {
1135            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
1136        }
1137        if response.dragged() {
1138            self.sidebar_width = (self.sidebar_width + response.drag_delta().x).clamp(140.0, 520.0);
1139            self.config_dirty = true;
1140        }
1141        // Double-clicking a splitter puts it back where it started, which is the same gesture
1142        // the column edges in the listing already answer to.
1143        if response.double_clicked() {
1144            self.sidebar_width = crate::config::SIDEBAR_WIDTH;
1145            self.config_dirty = true;
1146        }
1147
1148        // ---- The panes ----------------------------------------------------
1149        //
1150        // Already laid out by `plan_layout`, which had to run before the title bar.
1151        //
1152        // Splitters first, so a pane's own content is painted over their edges
1153        // rather than under them.
1154        let mut ratios: Vec<(Vec<u8>, f32)> = Vec::new();
1155        for splitter in &self.splitters {
1156            let response = ui.interact(
1157                splitter.rect,
1158                egui::Id::new(("splitter", &splitter.route)),
1159                egui::Sense::click_and_drag(),
1160            );
1161            if response.hovered() || response.dragged() {
1162                ui.ctx().set_cursor_icon(if splitter.horizontal {
1163                    egui::CursorIcon::ResizeHorizontal
1164                } else {
1165                    egui::CursorIcon::ResizeVertical
1166                });
1167            }
1168            if response.dragged() {
1169                let delta = if splitter.horizontal {
1170                    response.drag_delta().x / panes_area.width().max(1.0)
1171                } else {
1172                    response.drag_delta().y / panes_area.height().max(1.0)
1173                };
1174                ratios.push((splitter.route.clone(), delta));
1175            }
1176            // A double click evens the split up again.
1177            if response.double_clicked() {
1178                if let Some(ratio) = self.layout.ratio_at(&splitter.route) {
1179                    *ratio = 0.5;
1180                }
1181            }
1182        }
1183        for (route, delta) in ratios {
1184            if let Some(ratio) = self.layout.ratio_at(&route) {
1185                *ratio = (*ratio + delta).clamp(dock::RATIO_MIN, dock::RATIO_MAX);
1186            }
1187        }
1188
1189        // ---- The tab bands, for the rows the title bar cannot reach --------
1190        //
1191        // Painted like the title bar, because that is what they are for the row below
1192        // them: `background-layer` and a hairline along the bottom.
1193        for row in &plan.rows {
1194            ui.painter()
1195                .rect_filled(row.band, egui::CornerRadius::ZERO, t.bg.layer);
1196            crate::ui::rule_below(ui.painter(), row.band, t);
1197        }
1198
1199        let rects = self.pane_rects.clone();
1200        for (id, rect) in rects {
1201            self.pane(ui, t, id, rect);
1202        }
1203
1204        // The strips themselves after the panes, so a tab is never under a pane's card.
1205        for row in &plan.rows {
1206            for (id, strip) in &row.strips {
1207                let Some(index) = self.panes.iter().position(|p| p.id == *id) else {
1208                    continue;
1209                };
1210                let inner = Rect::from_min_max(
1211                    pos2(strip.left() + GUTTER, strip.top()),
1212                    pos2(strip.right() - GUTTER, strip.bottom()),
1213                );
1214                let focused = *id == self.focused;
1215                let slots = chrome::tab_strip(
1216                    ui,
1217                    t,
1218                    inner,
1219                    &self.panes[index],
1220                    focused,
1221                    &self.drag,
1222                    &mut self.icons,
1223                    &mut self.actions,
1224                );
1225                self.tab_slots.extend(slots);
1226            }
1227        }
1228    }
1229
1230    /// One pane: its surface, its path bar, its listing.
1231    fn pane(&mut self, ui: &mut Ui, t: &Theme, id: PaneId, rect: Rect) {
1232        let Some(index) = self.panes.iter().position(|p| p.id == id) else {
1233            return;
1234        };
1235        self.panes[index].rect = rect;
1236        // The card is drawn whatever the size, so a pane squeezed past the point of
1237        // usefulness still reads as a pane you can drag wider rather than as a hole
1238        // in the window.
1239        self.surface(ui, t, rect);
1240        if rect.width() < 96.0 || rect.height() < 48.0 {
1241            return;
1242        }
1243
1244        // Which pane the keyboard is in is said by the accent under its active tab, and
1245        // only there. A ring around the whole card says the same thing ten times as
1246        // loudly, and in a two-pane window it turns every click into a visible change of
1247        // frame — so the pane itself stays quiet.
1248        let focused = id == self.focused;
1249
1250        // The whole pane, with nothing held back: the path bar reaches the seams on both sides
1251        // and the listing reaches the bottom edge. The point of inset that used to be here was
1252        // room for the ring the panel no longer wears.
1253        let inside = rect;
1254        let bar = Rect::from_min_size(inside.min, vec2(inside.width(), breadcrumb::HEIGHT));
1255        // Everything under the path bar, which the listing and this folder's preview panel divide
1256        // between them. The panel is *inside* the pane — see [`crate::ui::preview`] — so it is
1257        // the pane's own shape that `Where::Auto` reads, and both rects come out of one split.
1258        let body = Rect::from_min_max(bar.left_bottom(), inside.max);
1259        let open = self.panes[index].tab().preview.open;
1260        let (list, panel) = crate::ui::preview::split(body, open, self.preview);
1261
1262        let mut child = ui.new_child(
1263            egui::UiBuilder::new()
1264                .max_rect(inside)
1265                .layout(egui::Layout::top_down(egui::Align::Min)),
1266        );
1267        child.set_clip_rect(inside.intersect(ui.clip_rect()));
1268
1269        // A click anywhere in the pane moves the keyboard here.
1270        let claim = child.interact(rect, egui::Id::new(("pane-claim", id)), egui::Sense::click());
1271        if claim.clicked() || claim.secondary_clicked() {
1272            self.actions.push(Action::Focus(id));
1273        }
1274
1275        // The path bar paints its own surface, in `breadcrumb::show`. It used to be
1276        // `background-layer` like the rest of the pane with a `stroke-subtle` hairline under it
1277        // to say where it ended; now it is [`crate::ui::seam`], the fill says it, and a hairline
1278        // between two fills that already differ is a third line nobody asked for.
1279
1280        let Self {
1281            panes,
1282            crumbs,
1283            actions,
1284            scratch,
1285            zone,
1286            icons,
1287            links,
1288            cut,
1289            notice,
1290            ops,
1291            preview,
1292            ..
1293        } = self;
1294        // A copy in progress, or the last thing that went wrong: whichever there is,
1295        // the pane's status line says so instead of counting files.
1296        let status = ops.in_progress().or(notice.as_deref());
1297        let pane = &mut panes[index];
1298        let tab = pane.tab_mut();
1299
1300        breadcrumb::show(&mut child, t, bar, id, tab, crumbs, icons, preview, actions);
1301        let outcome = filelist::show(
1302            &mut child, t, zone, list, id, tab, focused, icons, links, cut, status, scratch,
1303            actions,
1304        );
1305        // After the listing, so the panel's surface is over it rather than under: the listing
1306        // reaches for the whole body when it measures its own columns, and a panel drawn first
1307        // would have a row's fill painted across it.
1308        if let Some(panel) = panel {
1309            crate::ui::preview::show(
1310                &mut child,
1311                t,
1312                panel,
1313                id,
1314                &mut tab.preview,
1315                preview,
1316                body,
1317                scratch,
1318                actions,
1319            );
1320        }
1321
1322        self.panes[index].drop_rows = outcome.drop_rows;
1323        self.panes[index].drop_area = outcome.drop_area;
1324        if let Some(path) = outcome.prefetch {
1325            self.loader.prefetch(&path);
1326        }
1327
1328        // Where a drag hovering over this pane would land. Painted *after* the listing: a
1329        // selected row's fill is an accent surface drawn over anything underneath it, so a
1330        // highlight drawn first disappeared under the one row most likely to be dropped on.
1331        if let Some(area) = self.preview_rect(id, ui.ctx().pixels_per_point()) {
1332            crate::ui::drop_target(ui.painter(), area, t);
1333        }
1334    }
1335
1336    /// The surface every panel sits on: a plain square fill, and nothing else.
1337    ///
1338    /// It was a card — `radius-medium` and a `stroke-subtle` ring — which is the right
1339    /// treatment for a card floating on canvas and the wrong one for a panel that is part of
1340    /// the window's structure. Two of them side by side gave every boundary two rings and a
1341    /// channel of canvas between them, so the sidebar and the panes read as separate windows
1342    /// that happened to be next to each other.
1343    ///
1344    /// The ring is gone rather than kept-and-thinned because the seam does its job: panels are
1345    /// [`crate::ui::SEAM`] apart, and [`crate::ui::seam`] is already painted behind them, so
1346    /// the one line between two panels is a line neither of them draws.
1347    fn surface(&self, ui: &Ui, t: &Theme, rect: Rect) {
1348        ui.painter()
1349            .rect_filled(rect, egui::CornerRadius::ZERO, t.bg.layer);
1350    }
1351
1352    // ---------------------------------------------------------------------
1353    // Loading
1354    // ---------------------------------------------------------------------
1355
1356    /// Hand every finished scan to the tab that asked for it.
1357    fn collect_scans(&mut self) {
1358        // Collected first so the loader is not borrowed while the panes are.
1359        let arrived: Vec<_> = self.loader.drain().collect();
1360        for loaded in arrived {
1361            for pane in &mut self.panes {
1362                for tab in &mut pane.tabs {
1363                    if tab.awaiting == Some(loaded.token) {
1364                        tab.apply(loaded.dir.clone());
1365                    }
1366                }
1367            }
1368        }
1369    }
1370
1371    // ---------------------------------------------------------------------
1372    // Previews
1373    // ---------------------------------------------------------------------
1374
1375    /// Hand finished reads to the panels that asked, and keep each one pointed at its own
1376    /// keyboard.
1377    ///
1378    /// A panel **follows the selection** rather than being told once: you open it, then arrow
1379    /// down the folder and look at each file in turn without touching the shortcut again. That is
1380    /// only bearable because it waits — see [`crate::ui::preview::FOLLOW_DELAY`] — so holding the
1381    /// arrow key down decodes the file you stop on and not the thirty on the way past.
1382    ///
1383    /// **Every open panel is followed, not only the focused one.** Two panes side by side each
1384    /// showing a build of the same DLL is the case the panel is inside the pane *for*, and a
1385    /// preview that only tracked the pane with the keyboard would go stale in the other one the
1386    /// moment you clicked across to compare them.
1387    fn collect_previews(&mut self, ctx: &egui::Context, now: f64) {
1388        // Collected first, so the reader is not borrowed while the panels are written to. The
1389        // payload goes to the one panel waiting for that token and nowhere else: it is a decoded
1390        // picture or a walked graph, so there is one of it and no copy to hand round.
1391        for loaded in self.previews.drain().collect::<Vec<_>>() {
1392            let wanted = self
1393                .panes
1394                .iter_mut()
1395                .flat_map(|pane| pane.tabs.iter_mut())
1396                .find(|tab| tab.preview.wants(loaded.token));
1397            if let Some(tab) = wanted {
1398                tab.preview.arrived(loaded.token, loaded.payload, ctx);
1399            }
1400        }
1401
1402        // What each open panel should be looking at, worked out before anything is borrowed
1403        // mutably: `selected_preview` reads the tab, and asking for a read writes to it.
1404        type Wanted = (PaneId, usize, Option<crate::preview::Ask>);
1405        let wanted: Vec<Wanted> = self
1406            .panes
1407            .iter()
1408            .flat_map(|pane| {
1409                pane.tabs
1410                    .iter()
1411                    .enumerate()
1412                    .filter(|(at, tab)| tab.preview.open && *at == pane.active)
1413                    .map(|(at, tab)| (pane.id, at, Self::selected_preview(tab)))
1414                    .collect::<Vec<_>>()
1415            })
1416            .collect();
1417
1418        let mut soonest: Option<f64> = None;
1419        for (id, at, what) in wanted {
1420            let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) else {
1421                continue;
1422            };
1423            let Some(tab) = pane.tabs.get_mut(at) else {
1424                continue;
1425            };
1426            tab.preview.follow(what, now);
1427            let (ready, left) = tab.preview.settle(now);
1428            if let Some(ask) = ready {
1429                let token = self.previews.request(&ask);
1430                if let Some(tab) = self
1431                    .panes
1432                    .iter_mut()
1433                    .find(|p| p.id == id)
1434                    .and_then(|p| p.tabs.get_mut(at))
1435                {
1436                    tab.preview.asked(ask, token);
1437                }
1438            }
1439            if let Some(left) = left {
1440                soonest = Some(soonest.map_or(left, |soonest: f64| soonest.min(left)));
1441            }
1442        }
1443        // And the frame that would notice the wait is up. Nothing else would ask for it: this
1444        // program is idle between events, so a deadline nobody books a frame for is a deadline
1445        // that arrives the next time something unrelated happens to want one. The *soonest* of
1446        // them, since one frame serves every panel that is waiting.
1447        if let Some(left) = soonest {
1448            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
1449        }
1450    }
1451
1452    /// What this tab's preview panel should be showing, if anything.
1453    ///
1454    /// **Two pictures selected at once is a comparison**, and that is the one case where the
1455    /// *selection* rather than the cursor decides: picking a second image is a deliberate act with
1456    /// an obvious meaning, and no other pair of files has one. Everything else is the cursor's
1457    /// answer — the cursor is where the keyboard is, it follows a click as well, and a preview is
1458    /// about one file, so a selection of thirty has nothing to show.
1459    fn selected_preview(tab: &Tab) -> Option<crate::preview::Ask> {
1460        use crate::preview::{kind_of, Ask, Kind};
1461
1462        let dir = tab.dir.as_ref()?;
1463        let kind_at = |row: usize| -> Option<Kind> {
1464            let entry = tab.entry_at(row)?;
1465            // From the name alone, which costs nothing to ask about a row. A file that passes and
1466            // turns out to be something else says so in the panel rather than being refused here.
1467            kind_of(
1468                dir.leaf(entry),
1469                dir.ext(entry),
1470                dir.entries[entry].is_dir(),
1471            )
1472        };
1473
1474        if tab.selected_count == 2 {
1475            // In display order, so the left-hand or upper view is the upper row — the pair reads
1476            // the way the listing above it does rather than the way the clicks happened to land.
1477            let two: Vec<usize> = (0..tab.order.len())
1478                .filter(|&row| tab.is_selected(row))
1479                .take(3)
1480                .collect();
1481            if let [a, b] = two[..] {
1482                if kind_at(a) == Some(Kind::Picture) && kind_at(b) == Some(Kind::Picture) {
1483                    return Some(Ask::Pair(tab.target_at(a)?, tab.target_at(b)?));
1484                }
1485            }
1486        }
1487        let at = tab.cursor?;
1488        Some(Ask::One(tab.target_at(at)?, kind_at(at)?))
1489    }
1490
1491    /// Put the selection on the clipboard, as a cut or as a copy.
1492    fn put_on_clipboard(&mut self, pane: PaneId, cutting: bool) {
1493        use crate::shell::clipboard::{put, Effect};
1494
1495        let paths = self
1496            .pane_mut(pane)
1497            .map(|p| p.tab().selection_paths())
1498            .unwrap_or_default();
1499        if paths.is_empty() {
1500            self.notice = Some("Nothing selected".to_owned());
1501            return;
1502        }
1503        let effect = if cutting { Effect::Move } else { Effect::Copy };
1504        match put(&paths, effect) {
1505            // A cut marks its sources rather than moving them — nothing moves until
1506            // something pastes — so until then they have to *look* pending.
1507            Ok(()) => {
1508                self.cut = if cutting { paths } else { Vec::new() };
1509                self.notice = None;
1510            }
1511            Err(why) => self.notice = Some(why),
1512        }
1513    }
1514
1515    /// Act on whatever is on the clipboard, into this pane's folder.
1516    fn paste_into(&mut self, pane: PaneId, ctx: &egui::Context) {
1517        use crate::shell::clipboard::{self, Effect};
1518        use crate::shell::ops::Job;
1519
1520        let Some(into) = self.pane_mut(pane).map(|p| p.tab().path.clone()) else {
1521            return;
1522        };
1523        if into.as_os_str().is_empty() {
1524            self.notice = Some("This PC is not a folder to paste into".to_owned());
1525            return;
1526        }
1527        let Some(pasteable) = clipboard::get() else {
1528            self.notice = Some("There are no files on the clipboard".to_owned());
1529            return;
1530        };
1531
1532        let moving = pasteable.effect == Effect::Move;
1533        let items = pasteable.items;
1534        // A cut is finished when the move is: `After::FinishCut` tells the clipboard's owner
1535        // it worked and then empties it, once the shell says it did. Emptying it here instead
1536        // -- which is what this used to do -- loses the cut for anyone who answers the
1537        // conflict dialog with Cancel, and leaves the sources still faded with nothing on the
1538        // clipboard to paste them from.
1539        let (job, after) = if moving {
1540            (
1541                Job::Move { items, into },
1542                crate::shell::ops::After::FinishCut(clipboard::sequence()),
1543            )
1544        } else {
1545            (Job::Copy { items, into }, crate::shell::ops::After::Nothing)
1546        };
1547        self.notice = None;
1548        self.ops.start_then(job, after, self.owner, ctx);
1549    }
1550
1551    /// Raise the context menu: ask the shell for it, and open it when the answer comes.
1552    ///
1553    /// The shell's entries take 130 ms for a folder and up to most of a second for a file --
1554    /// every time, not just the first -- so they are fetched by
1555    /// [`crate::shell::menu::Builder`] on a thread of its own. Nothing is drawn in the
1556    /// meantime: the menu appears once, whole, at its final size. The window keeps running
1557    /// frames throughout, which is the part that used to be missing.
1558    fn shell_menu(
1559        &mut self,
1560        pane: PaneId,
1561        items: Vec<PathBuf>,
1562        at: (i32, i32),
1563        ctx: &egui::Context,
1564    ) {
1565        let Some(p) = self.panes.iter().find(|p| p.id == pane) else {
1566            return;
1567        };
1568        let folder = p.tab().path.clone();
1569        if folder.as_os_str().is_empty() {
1570            // This PC is a list of volumes, not a directory; the shell has no menu for it
1571            // that would mean anything here.
1572            return;
1573        }
1574
1575        // Whatever was open, or on its way, is not what was asked for.
1576        self.close_menu();
1577        let scale = ctx.pixels_per_point();
1578        let at = egui::pos2(at.0 as f32 / scale, at.1 as f32 / scale);
1579        let depth = Self::menu_depth(&folder, &items);
1580        self.asking = Some(Asking {
1581            token: self.menu_builder.build(&folder, &items, depth),
1582            pane,
1583            at,
1584            items,
1585            folder,
1586            depth,
1587            since: ctx.cumulative_pass_nr(),
1588            asked: std::time::Instant::now(),
1589        });
1590    }
1591
1592    /// How much of a menu to ask the shell for.
1593    ///
1594    /// The whole cost of a context menu is inside one `QueryContextMenu`, which lets every
1595    /// installed extension contribute — so there is no such thing as skipping the slow entries
1596    /// once they exist. They cost what they cost before this program sees any of them. The only
1597    /// choice is how much to ask for, and [`crate::shell::menu::Depth`] has the measurements.
1598    ///
1599    /// So: an executable image on a network drive gets the reduced menu, and everything else
1600    /// gets everything. That is a narrow rule and it is the one the evidence supports. What is
1601    /// slow is not "the network" — a 41 MB `.lib` on the same share builds a full menu in 1.8 s,
1602    /// and the folder itself in 0.2 s — it is an executable on a share, where the time is linear
1603    /// in the file's size at about 570 kB/s because something reads all of it. A blanket rule for
1604    /// network paths would throw away 7-Zip and Send To on every file on the share to fix a
1605    /// problem that only executables have.
1606    ///
1607    /// It is not the last word either: [`App::pump_asking`] downgrades anything on a network
1608    /// drive that turns out to be slow regardless of what it is called, which is what covers the
1609    /// file types this list has not heard of.
1610    fn menu_depth(folder: &Path, items: &[PathBuf]) -> crate::shell::menu::Depth {
1611        use crate::shell::menu::Depth;
1612        // What Windows loads as an executable image, which is the set something is entitled to
1613        // inspect byte by byte before deciding what to offer.
1614        const IMAGES: [&str; 8] = ["exe", "com", "scr", "dll", "ocx", "sys", "cpl", "drv"];
1615
1616        let any_image = items.iter().any(|item| {
1617            item.extension()
1618                .map(|e| e.to_string_lossy().to_ascii_lowercase())
1619                .is_some_and(|ext| IMAGES.contains(&ext.as_str()))
1620        });
1621        if any_image && crate::shell::over_network(folder) {
1622            Depth::Fast
1623        } else {
1624            Depth::Full
1625        }
1626    }
1627
1628    /// While the shell is still being asked, say so — and take Escape or a click as "never mind".
1629    ///
1630    /// Usually there is nothing to say: a folder's menu comes back in a tenth of a second and
1631    /// this is over before a frame has been drawn. The case it is here for is the one measured
1632    /// in [`crate::shell::menu::Builder`] — most of half a minute for an executable on a share
1633    /// — where a window that shows nothing at all is indistinguishable from a window that has
1634    /// stopped working, and where a menu that finally appears long after the click has been
1635    /// forgotten is worse than no menu.
1636    ///
1637    /// Cancelling does not stop `QueryContextMenu`, because nothing stops `QueryContextMenu`.
1638    /// It stops *waiting* for it: the worker is abandoned and its answer will be thrown away.
1639    fn pump_asking(&mut self, ctx: &egui::Context) {
1640        let Some(asking) = &self.asking else { return };
1641        // Not on the pass that asked. The right click that opens a menu is a press, and this
1642        // would take it as the cancellation of the menu it just asked for.
1643        let settled = ctx.cumulative_pass_nr() > asking.since;
1644        let quit = settled
1645            && ctx.input(|i| {
1646                i.key_pressed(egui::Key::Escape) || i.pointer.any_pressed() || i.pointer.any_click()
1647            });
1648        if quit {
1649            self.close_menu();
1650            return;
1651        }
1652
1653        // A full menu on a network drive that has not arrived by now is not going to arrive
1654        // soon: the fast measurement on that share was 1.8 s for a 41 MB file, so anything past
1655        // this is an extension inspecting the file rather than the share being busy. Ask again
1656        // for less. Network only — a local menu is 0.13-0.69 s and should never be quietly
1657        // reduced because the machine happened to be busy for a moment.
1658        const PATIENCE: std::time::Duration = std::time::Duration::from_millis(2_500);
1659        if asking.depth == crate::shell::menu::Depth::Full
1660            && asking.asked.elapsed() > PATIENCE
1661            && crate::shell::over_network(&asking.folder)
1662        {
1663            let (pane, at, items, folder) = (
1664                asking.pane,
1665                asking.at,
1666                asking.items.clone(),
1667                asking.folder.clone(),
1668            );
1669            self.close_menu();
1670            self.asking = Some(Asking {
1671                token: self
1672                    .menu_builder
1673                    .build(&folder, &items, crate::shell::menu::Depth::Fast),
1674                pane,
1675                at,
1676                items,
1677                folder,
1678                depth: crate::shell::menu::Depth::Fast,
1679                since: ctx.cumulative_pass_nr(),
1680                asked: std::time::Instant::now(),
1681            });
1682        }
1683
1684        ctx.set_cursor_icon(egui::CursorIcon::Progress);
1685        // The answer arrives on a channel and the worker asks for a repaint when it has one, so
1686        // this is not how the menu gets drawn. It is here so that the cursor is re-asserted and
1687        // the window demonstrably keeps drawing while the shell takes its time — at ten frames
1688        // a second rather than as fast as possible, since there is nothing to animate.
1689        ctx.request_repaint_after(std::time::Duration::from_millis(100));
1690    }
1691
1692    /// Take delivery of whatever the menu builder has finished, and pass on what the menu
1693    /// on screen has since asked for.
1694    fn pump_menu(&mut self) {
1695        use crate::shell::menu::Said;
1696
1697        while let Some(said) = self.menu_builder.poll() {
1698            // A menu the user has dismissed, or replaced with a second right click, still
1699            // has an answer coming. The token is how it is told apart from the live one, and
1700            // an answer that does not match is dropped.
1701            match said {
1702                Said::Built {
1703                    token,
1704                    entries,
1705                    depth,
1706                } => {
1707                    let Some(asking) = self.asking.take_if(|a| a.token == token) else {
1708                        continue;
1709                    };
1710                    // A short menu is worth admitting to. Somebody who right-clicks an
1711                    // executable on a share and finds 7-Zip missing should be told why rather
1712                    // than left to wonder whether the program is broken.
1713                    // Short enough for the status line to show all of it — the first version of
1714                    // this was cut off at "and thi…", which tells nobody anything.
1715                    if depth == crate::shell::menu::Depth::Fast {
1716                        self.notice = Some(
1717                            "Short menu: Windows' extras read the whole file over the network"
1718                                .to_owned(),
1719                        );
1720                    }
1721                    self.menu = Some(crate::ui::menu::Open::new(
1722                        asking.pane,
1723                        asking.at,
1724                        asking.items,
1725                        asking.folder,
1726                        entries,
1727                        token,
1728                    ));
1729                }
1730                Said::Filled { token, id, children } => {
1731                    if let Some(menu) = self.menu.as_mut().filter(|m| m.token == token) {
1732                        menu.filled(id, children);
1733                    }
1734                }
1735            }
1736        }
1737
1738        if let Some(menu) = self.menu.as_mut() {
1739            let token = menu.token;
1740            for id in std::mem::take(&mut menu.fills) {
1741                self.menu_builder.fill(token, id);
1742            }
1743        }
1744    }
1745
1746    /// Whether a menu has been asked for and has not appeared yet.
1747    ///
1748    /// For `--shot --menu`, which would otherwise photograph the window without one.
1749    pub fn menu_pending(&self) -> bool {
1750        self.asking.is_some()
1751    }
1752
1753    /// Let go of the menu that is closing, and of the one on its way if there is one.
1754    pub fn close_menu(&mut self) {
1755        if let Some(menu) = self.menu.take() {
1756            self.menu_builder.close(menu.token);
1757        }
1758        if self.asking.take().is_some() {
1759            // Nothing to close — the shell has not finished making it. The worker is let go of
1760            // instead, so that the next menu is built on a thread that is not inside a call
1761            // that may have twenty seconds left to run.
1762            self.menu_builder.abandon();
1763        }
1764    }
1765
1766    /// Put every popup away when the window stops being the one you are using.
1767    ///
1768    /// A menu belongs to a moment. Alt-tab to something else and come back ten minutes later and
1769    /// a context menu still standing over the listing is not where you left off — it is a menu
1770    /// about a file you have stopped thinking about, over a window you have to click twice to get
1771    /// back into. Windows itself dismisses a menu when its owner loses activation, and this window
1772    /// has three kinds of its own to dismiss: the context menu, the popups egui tracks in its
1773    /// memory — the application menu under the mark at the top left — and the path bar's
1774    /// dropdowns with the tracking mode they turn on.
1775    ///
1776    /// Every unfocused frame rather than only the one where focus was lost: the transition needs a
1777    /// frame to be noticed in, an unfocused window is not always given one at the moment it goes,
1778    /// and asking "is anything open while we are not in front" has the same answer either way. It
1779    /// is also cheap — the three are already empty every other time this runs.
1780    ///
1781    /// `focused` is the *window's*, not egui's: a focused text field is a different thing entirely
1782    /// and closing a menu because the filter box has the caret would be a bug. `RawInput::focused`
1783    /// defaults to `true`, so a frame from an integration that does not track focus never trips
1784    /// this.
1785    fn close_on_blur(&mut self, ctx: &egui::Context) {
1786        // The design system puts egui's own popups away and reports whether it fired, so this
1787        // window's two hand-tracked ones — the application menu and the breadcrumb's dropdown,
1788        // neither of which is an `egui::Popup` — go with them in the same breath.
1789        if azur_egui_theme::desktop::close_popups_on_blur(ctx) {
1790            self.close_menu();
1791            self.crumbs.close();
1792        }
1793    }
1794
1795    /// Keep a drag in flight painting, and clear up after it once it has landed.
1796    ///
1797    /// The drag runs on its own thread — see [`crate::shell::dnd::Drag`] — and while it does,
1798    /// nothing in egui's own event flow is happening: the pointer belongs to OLE, so no mouse
1799    /// event reaches winit and nothing would ask for a frame. Without a frame the drop
1800    /// highlight never appears and the selection the drag just made is never drawn. So the
1801    /// repaint is asked for unconditionally for the length of the drag, which is the one case
1802    /// in this program where painting is driven by a state rather than by an event.
1803    fn pump_drag(&mut self, ctx: &egui::Context) {
1804        let Some((pane, drag)) = &self.file_drag else {
1805            return;
1806        };
1807        let Some(effect) = drag.finished() else {
1808            ctx.request_repaint();
1809            return;
1810        };
1811        let pane = *pane;
1812        self.file_drag = None;
1813        self.release_buttons(ctx);
1814        // A move took files out of this folder, and OLE does not say which — so the folder
1815        // is re-read. A copy changed nothing here.
1816        if effect == Some(crate::shell::clipboard::Effect::Move) {
1817            self.perform(ctx, Action::Refresh(pane));
1818        }
1819        ctx.request_repaint();
1820    }
1821
1822    /// Tell egui the button came up, because nothing else is going to.
1823    ///
1824    /// **This is why a second drag did nothing.** `DoDragDrop` takes the mouse capture for the
1825    /// length of the drag and its own loop consumes the button-up that ends it, so the window
1826    /// never sees the release: egui goes on believing the button is held, and a press that
1827    /// arrives while a button is already down starts no new drag. One drag per window, and then
1828    /// nothing — until some unrelated click happened to put the state right, which is why it
1829    /// looked intermittent and why every test that ran a single drag in a fresh window passed.
1830    ///
1831    /// It cannot be fixed by watching for the release: it is never delivered here. So the
1832    /// release is stated rather than awaited, for whichever buttons egui still thinks are down,
1833    /// at the position egui already has — moving it would be inventing a gesture rather than
1834    /// finishing one.
1835    fn release_buttons(&mut self, ctx: &egui::Context) {
1836        let (pos, modifiers, down) = ctx.input(|i| {
1837            (
1838                i.pointer.latest_pos(),
1839                i.modifiers,
1840                [
1841                    egui::PointerButton::Primary,
1842                    egui::PointerButton::Secondary,
1843                ]
1844                .into_iter()
1845                .filter(|button| i.pointer.button_down(*button))
1846                .collect::<Vec<_>>(),
1847            )
1848        });
1849        let Some(pos) = pos else { return };
1850        for button in down {
1851            self.injected.push(egui::Event::PointerButton {
1852                pos,
1853                button,
1854                pressed: false,
1855                modifiers,
1856            });
1857        }
1858    }
1859
1860    /// Events egui has to be told about because the platform could not deliver them.
1861    ///
1862    /// Drained by the input hook, which is the only place raw input can be added to.
1863    pub fn take_injected(&mut self) -> Vec<egui::Event> {
1864        std::mem::take(&mut self.injected)
1865    }
1866
1867    /// Hand each per-file icon answer to the view that asked for it, and drop the rest.
1868    ///
1869    /// This is where the folder-scoped rule is enforced. An answer names the view it belongs
1870    /// to; if no tab still holds that view — because it moved on, or was closed, or the
1871    /// folder was refreshed — the answer is discarded here and nothing anywhere remembers the
1872    /// file it was about. Nothing is keyed by path, so nothing outlives the folder.
1873    fn deliver_icons(&mut self) {
1874        let answers = self.icons.answers();
1875        if answers.is_empty() {
1876            return;
1877        }
1878        for (view, row, index) in answers {
1879            let Some(tab) = self
1880                .panes
1881                .iter_mut()
1882                .flat_map(|pane| pane.tabs.iter_mut())
1883                .find(|tab| tab.view == view)
1884            else {
1885                continue;
1886            };
1887            if let Some(slot) = tab.file_icons.get_mut(row as usize) {
1888                *slot = index;
1889            }
1890        }
1891    }
1892
1893    /// The same, for what each shortcut row points at. See [`crate::shell::links`].
1894    ///
1895    /// `None` is stored rather than skipped: it means the shortcut was read and had nothing to
1896    /// show, and storing it is what stops the row asking about it again on every frame.
1897    fn deliver_links(&mut self) {
1898        let answers = self.links.answers();
1899        if answers.is_empty() {
1900            return;
1901        }
1902        for (view, row, target) in answers {
1903            let Some(tab) = self
1904                .panes
1905                .iter_mut()
1906                .flat_map(|pane| pane.tabs.iter_mut())
1907                .find(|tab| tab.view == view)
1908            else {
1909                continue;
1910            };
1911            // Only if the row is still asking. A refresh clears the map, and an answer that
1912            // arrives after that would otherwise put back a row's context for a listing the
1913            // entry indices no longer belong to.
1914            if let Some(slot) = tab.links.get_mut(&row) {
1915                *slot = target;
1916            }
1917        }
1918    }
1919
1920    /// Raise the focused pane's folder menu, over its listing.
1921    ///
1922    /// For `--menu`, which is how a capture run gets a menu on screen: it has no pointer
1923    /// to right-click with, and the menu is the one part of the window a screenshot cannot
1924    /// otherwise reach. Goes through the same action as a real right click, so what it
1925    /// captures is the real menu and not a mock-up of one.
1926    pub fn open_folder_menu(&mut self, ctx: &egui::Context) {
1927        let Some(pane) = self.panes.iter().find(|p| p.id == self.focused) else {
1928            return;
1929        };
1930        let rect = pane.rect;
1931        let scale = ctx.pixels_per_point();
1932        let at = rect.min + egui::vec2(rect.width() * 0.22, rect.height() * 0.30);
1933        self.actions.push(Action::ShellMenu {
1934            pane: pane.id,
1935            items: Vec::new(),
1936            at: ((at.x * scale) as i32, (at.y * scale) as i32),
1937        });
1938    }
1939
1940    /// Say something in the status line: the one place this window has to tell the user
1941    /// anything, and where a failed file operation already goes.
1942    ///
1943    /// Used for the graphics device, which is not this program's to fix but very much its job
1944    /// to admit to: a window that cannot present a frame goes on taking input and showing the
1945    /// last thing it drew, and without a word from it that is indistinguishable from a hang.
1946    pub fn report(&mut self, what: String) {
1947        self.notice = Some(what);
1948    }
1949
1950    /// Whether the focused pane has a listing with anything in it.
1951    pub fn has_rows(&self) -> bool {
1952        self.panes
1953            .iter()
1954            .find(|p| p.id == self.focused)
1955            .is_some_and(|p| !p.tab().order.is_empty())
1956    }
1957
1958    /// Select the first file in the focused pane and open its name for editing.
1959    ///
1960    /// For `--rename`, which is how a capture run gets the rename field on screen: it has no
1961    /// keyboard to press F2 with. A file rather than a folder, so there is an extension there
1962    /// for the caret to leave alone.
1963    pub fn begin_rename_here(&mut self) {
1964        let pane = self.focused;
1965        let Some(p) = self.pane_mut(pane) else { return };
1966        let tab = p.tab_mut();
1967        // A file with an extension for preference, since the extension is the part worth
1968        // photographing: a shot of `Makefile` selected whole says nothing about the rule.
1969        let named = |at: &usize| {
1970            tab.entry_at(*at)
1971                .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
1972                .is_some_and(|name| std::path::Path::new(&name).extension().is_some())
1973        };
1974        let rows = 0..tab.order.len();
1975        let file = rows
1976            .clone()
1977            .find(|at| !tab.is_dir_at(*at) && named(at))
1978            .or_else(|| rows.clone().find(|at| !tab.is_dir_at(*at)));
1979        if let Some(at) = file.or_else(|| (!tab.order.is_empty()).then_some(0)) {
1980            tab.select_only(at);
1981            tab.begin_rename();
1982        }
1983    }
1984
1985    /// Put the keyboard on the first previewable file in the focused pane and open the panel.
1986    ///
1987    /// For `--preview`, which is how a capture run gets the panel on screen: it has no keyboard to
1988    /// press `Ctrl+P` with. Goes through the same action a real key press does, and picks the row
1989    /// the same way, so what it captures is the real panel over a real read.
1990    pub fn open_preview_here(&mut self) {
1991        let pane = self.focused;
1992        let Some(p) = self.pane_mut(pane) else { return };
1993        let tab = p.tab_mut();
1994        let previewable = |tab: &Tab, at: usize| {
1995            tab.entry_at(at)
1996                .zip(tab.dir.as_ref())
1997                .and_then(|(entry, dir)| {
1998                    crate::preview::kind_of(
1999                        dir.leaf(entry),
2000                        dir.ext(entry),
2001                        dir.entries[entry].is_dir(),
2002                    )
2003                })
2004                .is_some()
2005        };
2006        // Whatever `--reveal=` already selected, if that can be previewed — so the two flags
2007        // compose and a capture can name the file it wants. The first previewable row otherwise.
2008        let row = tab
2009            .cursor
2010            .filter(|&at| previewable(tab, at))
2011            .or_else(|| (0..tab.order.len()).find(|&at| previewable(tab, at)));
2012        if let Some(at) = row {
2013            if tab.selected_count < 2 {
2014                tab.select_only(at);
2015            }
2016            self.actions.push(Action::TogglePreview(pane));
2017        }
2018    }
2019
2020    /// Select the *second* previewable file as well, so `--shot --preview --compare` photographs a
2021    /// comparison rather than one picture.
2022    ///
2023    /// For the same reason [`App::open_preview_here`] exists: two selected rows is a mouse gesture
2024    /// and a capture run has no mouse.
2025    pub fn compare_here(&mut self) {
2026        let pane = self.focused;
2027        let Some(p) = self.pane_mut(pane) else { return };
2028        let tab = p.tab_mut();
2029        let pictures: Vec<usize> = (0..tab.order.len())
2030            .filter(|&at| {
2031                tab.entry_at(at)
2032                    .zip(tab.dir.as_ref())
2033                    .and_then(|(entry, dir)| {
2034                        crate::preview::kind_of(
2035                            dir.leaf(entry),
2036                            dir.ext(entry),
2037                            dir.entries[entry].is_dir(),
2038                        )
2039                    })
2040                    == Some(crate::preview::Kind::Picture)
2041            })
2042            .take(2)
2043            .collect();
2044        if let [a, b] = pictures[..] {
2045            tab.select_only(a);
2046            tab.toggle(b);
2047        }
2048    }
2049
2050    /// Whether any preview has been asked for and has not come back yet.
2051    ///
2052    /// For `--shot --preview`, which would otherwise photograph the word `Reading…`: a decode or a
2053    /// dependency walk takes tens of milliseconds, which is several frames.
2054    pub fn preview_pending(&self) -> bool {
2055        self.panes
2056            .iter()
2057            .flat_map(|pane| pane.tabs.iter())
2058            .any(|tab| tab.preview.busy())
2059    }
2060
2061    /// Draw the context menu, if one is open, and act on what it says.
2062    fn draw_menu(&mut self, ui: &mut Ui, theme: &Theme) {
2063        use crate::shell::menu::Command;
2064        use crate::ui::menu::Outcome;
2065
2066        // Unconditionally, and before the early return: this is what *opens* the menu, so a
2067        // version that skipped it while there was nothing on screen would wait for ever for
2068        // a menu it never took delivery of. Cheap when there is nothing to collect — one
2069        // `try_recv` that fails.
2070        self.pump_menu();
2071
2072        let outcome = match &mut self.menu {
2073            Some(menu) => crate::ui::menu::show(ui, theme, menu),
2074            None => return,
2075        };
2076        // A hover during this pass may have asked for a submenu; send it now rather than
2077        // waiting a frame for the next pump.
2078        self.pump_menu();
2079
2080        match outcome {
2081            Outcome::Open => {}
2082            Outcome::Closed => self.close_menu(),
2083            Outcome::Chose(command) => {
2084                let Some(menu) = self.menu.take() else { return };
2085                self.menu_builder.close(menu.token);
2086                match command {
2087                    Command::Own(which) => {
2088                        if let Some(action) = self.own_menu_action(&menu, which) {
2089                            self.actions.push(action);
2090                        }
2091                    }
2092                    // Off to the modal thread: invoking can open anything from a
2093                    // Properties sheet to an installer, and neither belongs in a frame.
2094                    // Nothing is remembered about which pane asked: whatever the command does
2095                    // to the folder, `crate::watch` is what notices. See `collect_modal`.
2096                    Command::Shell { .. } => {
2097                        self.modal.send(crate::shell::Request::Invoke {
2098                            parent: menu.folder.clone(),
2099                            items: menu.items.clone(),
2100                            command,
2101                            owner: self.owner,
2102                        });
2103                    }
2104                }
2105            }
2106        }
2107    }
2108
2109    /// What one of this program's own menu entries means.
2110    ///
2111    /// Only a right-button drop asks anything of its own now. Everything else that used to be
2112    /// here -- Open, Open in new tab, Open in a pane to the right or below, Add to bookmarks,
2113    /// Copy path, Refresh, Select all, Show hidden files, New folder, Open terminal here -- has
2114    /// been taken out of the context menu, which shows Windows' menu and nothing else.
2115    fn own_menu_action(
2116        &self,
2117        menu: &crate::ui::menu::Open,
2118        which: crate::shell::menu::Own,
2119    ) -> Option<Action> {
2120        use crate::shell::menu::Own;
2121
2122        Some(match which {
2123            // A right-button drag, answered. The menu already carries what was dropped and
2124            // where, so there is nothing to look up.
2125            Own::CopyHere | Own::MoveHere => Action::DropHere {
2126                pane: menu.pane,
2127                items: menu.items.clone(),
2128                into: menu.folder.clone(),
2129                moving: which == Own::MoveHere,
2130            },
2131            Own::Cancel => return None,
2132        })
2133    }
2134
2135    /// Apply whatever the modal thread came back with.
2136    ///
2137    /// **Nothing, and that is the point.** A shell command can do anything — rename, delete,
2138    /// extract, commit — and there is no way to be told which, so this used to re-read the folder
2139    /// on the way out of *every* one of them. Which meant a scan and a rebuilt listing after
2140    /// `Copy`, after `Properties`, after `Open with`, after `Scan with Defender`: the whole view
2141    /// thrown away and made again to discover that nothing had changed.
2142    ///
2143    /// [`crate::watch`] is what answers this properly, and it is already running. Every folder on
2144    /// screen has a `ReadDirectoryChangesW` handle on it, so a verb that *did* change something is
2145    /// noticed within a sixth of a second whoever changed it — this program, Explorer, a terminal,
2146    /// or the extension the verb belonged to — and a verb that changed nothing costs nothing.
2147    /// Still drained, because the reply is what tells [`crate::shell::Modal`] the gesture is
2148    /// over — and while one is in flight this window keeps painting for it.
2149    fn collect_modal(&mut self) {
2150        match self.modal.poll() {
2151            Some(crate::shell::Reply::Invoked) | None => {}
2152        }
2153    }
2154
2155    /// Tell the drop target which pane covers which folder.
2156    ///
2157    /// Published every frame because a pane can be split, resized or navigated between
2158    /// one drag and the next, and the OLE callbacks answer `DragOver` synchronously with
2159    /// no way to ask.
2160    fn publish_drop_targets(&self, ctx: &egui::Context) {
2161        use crate::shell::dnd::{Onto, Targets};
2162
2163        let scale = ctx.pixels_per_point();
2164        let physical = |rect: Rect| {
2165            (
2166                (rect.left() * scale) as i32,
2167                (rect.top() * scale) as i32,
2168                (rect.right() * scale) as i32,
2169                (rect.bottom() * scale) as i32,
2170            )
2171        };
2172
2173        /// Big enough to drop on and to draw a highlight around.
2174        fn usable(rect: Rect) -> bool {
2175            rect.width() >= 1.0 && rect.height() >= 1.0
2176        }
2177
2178        let mut zones = Vec::with_capacity(self.panes.len() + 1);
2179        // The bookmarks group first, so it is *behind* the panes: they cannot overlap, and
2180        // if a future layout let them, dropping onto a listing should mean the listing.
2181        if let Some(rect) = self.bookmarks_rect {
2182            zones.push((physical(rect), Onto::Bookmarks));
2183        }
2184        for pane in &self.panes {
2185            let folder = pane.tab().path.clone();
2186            // This PC is a list of volumes rather than a directory, so nothing can be
2187            // dropped into it.
2188            //
2189            // The rows' rectangle rather than the pane's: the column header sorts and the
2190            // status line counts files, and dropping on either of them is not dropping into
2191            // the folder — so neither should light up saying that it is.
2192            if folder.as_os_str().is_empty() || !usable(pane.drop_area) {
2193                continue;
2194            }
2195            zones.push((physical(pane.drop_area), Onto::Folder(folder)));
2196        }
2197        // The folder rows last, so they win: `Targets::at` takes the last match, and dropping
2198        // onto a folder has to mean *into that folder*. Dropping anywhere else in the listing
2199        // still means the folder being shown, which is what the zone above is for.
2200        for pane in &self.panes {
2201            for (row, folder) in &pane.drop_rows {
2202                let row = row.intersect(pane.drop_area);
2203                if usable(row) {
2204                    zones.push((physical(row), Onto::Folder(folder.clone())));
2205                }
2206            }
2207        }
2208        self.drops.publish(Targets { zones });
2209    }
2210
2211    /// Which of these items a drop into `into` can actually act on.
2212    ///
2213    /// Dropping a folder into itself is meaningless whatever button carried it, and the shell would
2214    /// refuse it noisily. A file dropped back into the folder it is already in is meaningless too
2215    /// *for a left drag* — it is a move to where it already is — but not for a right one:
2216    /// right-dragging a file onto its own folder is how Explorer is asked for a copy of it, and the
2217    /// answer is `one - Copy.txt`. Filtering those out before the question was asked meant a right
2218    /// drag inside a folder did nothing at all, which is the most obvious way to try the gesture.
2219    fn droppable(items: Vec<PathBuf>, into: &Path, asked: bool) -> Vec<PathBuf> {
2220        items
2221            .into_iter()
2222            .filter(|item| item != into)
2223            .filter(|item| asked || item.parent() != Some(into))
2224            .collect()
2225    }
2226
2227    /// What the drop highlight should cover in this pane, if anything.
2228    ///
2229    /// The folder row under the pointer when there is one, and the listing otherwise — which
2230    /// mirrors where the drop will actually go, since a row is published as a drop zone of its own
2231    /// and wins over the listing it sits in. A listing-wide highlight over a subfolder would
2232    /// promise the wrong destination.
2233    fn preview_rect(&self, pane: PaneId, scale: f32) -> Option<Rect> {
2234        let (x, y) = self.drop_hover?;
2235        let pane = self.panes.iter().find(|p| p.id == pane)?;
2236        if pane.tab().path.as_os_str().is_empty() {
2237            return None;
2238        }
2239        // The hover point arrives in physical pixels, as the drop zones are published.
2240        let scale = scale.max(0.01);
2241        let at = egui::pos2(x as f32 / scale, y as f32 / scale);
2242        if !pane.drop_area.contains(at) {
2243            return None;
2244        }
2245        Some(
2246            pane.drop_rows
2247                .iter()
2248                .find(|(row, _)| row.contains(at))
2249                .map(|(row, _)| row.intersect(pane.drop_area))
2250                .unwrap_or(pane.drop_area),
2251        )
2252    }
2253
2254    /// [`App::preview_rect`], for the test that checks which rect it picks.
2255    #[cfg(test)]
2256    pub fn preview_rect_for_tests(&self, pane: PaneId, scale: f32) -> Option<Rect> {
2257        self.preview_rect(pane, scale)
2258    }
2259
2260    /// The Bookmarks group, when a drag is over it.
2261    ///
2262    /// The whole group and not a row within it: pinning appends, so there is no position to
2263    /// promise — which is also why the pointer is answered `LINK` rather than copy or move.
2264    fn bookmarks_preview(&self, scale: f32) -> Option<Rect> {
2265        let (x, y) = self.drop_hover?;
2266        let rect = self.bookmarks_rect?;
2267        let at = egui::pos2(x as f32 / scale.max(0.01), y as f32 / scale.max(0.01));
2268        rect.contains(at).then_some(rect)
2269    }
2270
2271    /// Watch the folders on screen, and re-read any that changed underneath us.
2272    ///
2273    /// A listing used to be only as fresh as the last thing *this* program did to it. Anything
2274    /// anybody else did went unseen: a file dragged out to Explorer stayed on screen, because
2275    /// Explorer performs the move after our drag has finished and there was nothing to wait on;
2276    /// a build writing into the folder showed it as it had been; a file deleted from a terminal
2277    /// left a row behind. And a stale row is worse than wrong — dragging one cannot start a
2278    /// drag, so the window looked like it had stopped responding.
2279    fn collect_changes(&mut self, ctx: &egui::Context) {
2280        let folders: Vec<PathBuf> = self
2281            .panes
2282            .iter()
2283            .flat_map(|pane| pane.tabs.iter())
2284            .map(|tab| tab.path.clone())
2285            .collect();
2286        self.watch.keep(&folders);
2287
2288        let now = ctx.input(|i| i.time);
2289        for path in self.watch.changed(now) {
2290            self.folder_changed(&path);
2291        }
2292        // A change inside its settle window is a frame that has to come back for it, and there
2293        // is no input on the way to bring one.
2294        if self.watch.waiting() {
2295            ctx.request_repaint_after(std::time::Duration::from_millis(60));
2296        }
2297    }
2298
2299    /// Re-read a folder that changed on disk, without blanking what is on screen.
2300    ///
2301    /// `Tab::refresh` is deliberately *not* used: it drops the listing, which puts "Reading..."
2302    /// in the pane until the scan lands. That is right for F5, where somebody asked; here it
2303    /// would flash on every file written into the folder being watched. So the old listing stays
2304    /// up and only the request is made — `Tab::apply` then carries the selection across by
2305    /// name, exactly as it does for a refresh.
2306    fn folder_changed(&mut self, path: &Path) {
2307        self.loader.invalidate(path);
2308        let Self { panes, loader, .. } = self;
2309        for tab in panes.iter_mut().flat_map(|pane| pane.tabs.iter_mut()) {
2310            if tab.path == path {
2311                // Replacing a token that is already out means the older answer is dropped when
2312                // it arrives, which is what should happen: it read the folder as it was.
2313                tab.awaiting = Some(loader.request(&tab.path));
2314            }
2315        }
2316    }
2317
2318    /// Act on files dropped onto a pane, and highlight the one being hovered.
2319    fn collect_drops(&mut self, ctx: &egui::Context) {
2320        use crate::shell::clipboard::Effect;
2321        use crate::shell::ops::Job;
2322
2323        // The highlight, while a drag is over the window. A repaint is asked for
2324        // because the OLE callbacks run outside egui's own event flow and nothing else
2325        // would wake it.
2326        let hovering = self.drops.hovering();
2327        if hovering != self.drop_hover {
2328            self.drop_hover = hovering;
2329            ctx.request_repaint();
2330        }
2331
2332        for dropped in self.drops.take_drops() {
2333            // Where the drop actually landed, decided when the pointer was there rather than
2334            // worked out again now. It was worked out again, from the pane under the pointer,
2335            // and so a drop onto a *folder row* went into the folder being shown instead of into
2336            // the folder it was dropped on — the one thing dragging onto a folder means.
2337            let into = match dropped.onto {
2338                // Onto the sidebar: pin the folders and move nothing. Files are ignored rather
2339                // than refused, so dragging a mixed selection over pins what can be pinned.
2340                crate::shell::dnd::Onto::Bookmarks => {
2341                    for item in dropped.items {
2342                        if item.is_dir() {
2343                            self.perform(ctx, Action::AddBookmark(item));
2344                        }
2345                    }
2346                    continue;
2347                }
2348                crate::shell::dnd::Onto::Folder(into) => into,
2349            };
2350            if into.as_os_str().is_empty() {
2351                continue;
2352            }
2353
2354            let scale = ctx.pixels_per_point();
2355            let at = egui::pos2(
2356                dropped.at.0 as f32 / scale,
2357                dropped.at.1 as f32 / scale,
2358            );
2359            // Only to decide which pane the keyboard should follow the drop into; the
2360            // destination is `into`.
2361            let Some(pane) = self
2362                .panes
2363                .iter()
2364                .find(|p| p.rect.contains(at))
2365                .map(|p| p.id)
2366            else {
2367                continue;
2368            };
2369            // Dropping a folder into itself is meaningless whatever button carried it, and the
2370            // shell would refuse it noisily. A file dropped back into the folder it is already in
2371            // is meaningless too *for a left drag* -- it is a move to where it already is -- but
2372            // not for a right one: right-dragging a file onto its own folder is how Explorer is
2373            // asked for a copy of it, and the answer is `one - Copy.txt`. Filtering those out
2374            // before the question was asked meant a right drag inside a folder did nothing at
2375            // all, which is the most obvious way to try the gesture.
2376            let items = Self::droppable(dropped.items, &into, dropped.asked);
2377            if items.is_empty() {
2378                continue;
2379            }
2380            self.focused = pane;
2381            // A right-button drag asks rather than assumes, which is what Windows does and the
2382            // whole reason anybody drags with the right button.
2383            if dropped.asked {
2384                use crate::shell::menu::{Entry, Own};
2385                let own = [Own::CopyHere, Own::MoveHere, Own::Cancel]
2386                    .into_iter()
2387                    .map(Entry::own)
2388                    .collect();
2389                self.close_menu();
2390                // Built here rather than through the builder: these are this program's own
2391                // entries and there is nothing to ask the shell about, so the menu is ready now.
2392                // Token 0 matches no build, which is exactly right -- no answer is coming.
2393                self.menu = Some(crate::ui::menu::Open::new(pane, at, items, into, own, 0));
2394                continue;
2395            }
2396            let job = match dropped.effect {
2397                Effect::Move => Job::Move { items, into },
2398                Effect::Copy => Job::Copy { items, into },
2399            };
2400            self.ops.start(job, self.owner, ctx);
2401        }
2402    }
2403
2404    /// Take delivery of finished file operations and re-read what they changed.
2405    fn collect_operations(&mut self) {
2406        for done in self.ops.drain() {
2407            let worked = done.error.is_none();
2408            if let Some(why) = done.error.filter(|why| !why.is_empty()) {
2409                self.notice = Some(why);
2410            }
2411            // The documented end of a cut, and only when the move actually happened.
2412            if let crate::shell::ops::After::FinishCut(was) = done.after {
2413                if worked {
2414                    crate::shell::clipboard::cut_pasted(was);
2415                    self.cut.clear();
2416                }
2417            }
2418            // A folder this program has just made: select it and open the name for editing as
2419            // soon as the re-read brings it in. `New folder` on its own is only half the
2420            // gesture; nobody wants a folder called `New folder`.
2421            if let crate::shell::ops::After::NameIt(pane) = done.after {
2422                if let (true, Some(name)) = (worked, done.created.clone()) {
2423                    if let Some(p) = self.pane_mut(pane) {
2424                        let tab = p.tab_mut();
2425                        tab.reveal = Some(name);
2426                        tab.rename_revealed = true;
2427                    }
2428                }
2429            }
2430            for path in &done.touched {
2431                self.loader.invalidate(path);
2432            }
2433            // Only a tab showing an affected folder re-reads. A tab elsewhere is left
2434            // alone, which is the point of tracking this by path.
2435            for pane in &mut self.panes {
2436                for tab in &mut pane.tabs {
2437                    if done.touched.contains(&tab.path) {
2438                        tab.refresh();
2439                    }
2440                }
2441            }
2442        }
2443        // A cut whose sources have gone is a cut that has been honoured.
2444        self.cut.retain(|path| path.exists());
2445    }
2446
2447    /// Ask for anything nobody has asked for yet.
2448    ///
2449    /// The cache is probed synchronously first, which is what makes Back, Forward
2450    /// and revisiting a folder appear in the same frame as the click.
2451    fn start_scans(&mut self, ctx: &egui::Context, now: f64) {
2452        let Self { panes, loader, .. } = self;
2453        let mut asked = false;
2454        for pane in panes.iter_mut() {
2455            for tab in pane.tabs.iter_mut() {
2456                if tab.dir.is_some() || tab.awaiting.is_some() {
2457                    continue;
2458                }
2459                // A flattened tab never looks in the cache, in either direction: the
2460                // cache holds the folder's own children under this very path, and handing
2461                // those over would put a shallow listing on screen with the button lit.
2462                // It is not put *in* the cache either — see [`Loader::request_deep`].
2463                if tab.flat {
2464                    tab.awaiting = Some(loader.request_deep(&tab.path));
2465                    tab.asked_at = Some(now);
2466                    asked = true;
2467                    continue;
2468                }
2469                match loader.cached(&tab.path) {
2470                    Some(dir) => tab.apply(dir),
2471                    None => {
2472                        tab.awaiting = Some(loader.request(&tab.path));
2473                        // When it was asked for, which is what decides whether the listing
2474                        // says anything about waiting. See [`crate::pane::SLOW_SCAN`].
2475                        tab.asked_at = Some(now);
2476                        asked = true;
2477                    }
2478                }
2479            }
2480        }
2481        // And the frame that would notice the half-second has passed. Nothing else would ask
2482        // for it: this program is idle between events, and the answer arriving is the only
2483        // other thing that wakes it — so without this, `Reading…` would appear on a slow scan
2484        // only if something else happened to want a frame in the meantime.
2485        if asked {
2486            ctx.request_repaint_after(std::time::Duration::from_secs_f64(crate::pane::SLOW_SCAN));
2487        }
2488    }
2489
2490    // ---------------------------------------------------------------------
2491    // Keyboard
2492    // ---------------------------------------------------------------------
2493
2494    /// Back and forward on the mouse's thumb buttons.
2495    ///
2496    /// The two extra buttons every mouse past a certain price has. Windows sends them as
2497    /// `WM_XBUTTONDOWN` with `XBUTTON1` or `XBUTTON2`; winit turns those into `MouseButton::Back`
2498    /// and `Forward`, and egui into `PointerButton::Extra1` and `Extra2`. That is the whole chain,
2499    /// and it is worth writing down because "button 4 and 5" appear under four different names on
2500    /// the way through and `winit::MouseButton::Other` — which is what anything past the fifth
2501    /// button becomes — is dropped before egui ever sees it.
2502    fn thumb_buttons(&mut self, ctx: &egui::Context) {
2503        use egui::PointerButton as B;
2504
2505        let (back, forward) = ctx.input(|i| {
2506            (
2507                i.pointer.button_pressed(B::Extra1),
2508                i.pointer.button_pressed(B::Extra2),
2509            )
2510        });
2511        let pane = self.focused;
2512        if back {
2513            self.actions.push(Action::Back(pane));
2514        }
2515        if forward {
2516            self.actions.push(Action::Forward(pane));
2517        }
2518    }
2519
2520    fn keyboard(&mut self, ctx: &egui::Context) {
2521        use egui::Key as K;
2522
2523        // A focused text field owns the keyboard. Escape is the one key that still
2524        // has to get through, or a filter box becomes a trap.
2525        let renaming = self
2526            .panes
2527            .iter()
2528            .any(|p| p.tabs.iter().any(|t| t.renaming.is_some()));
2529        // A menu on screen owns the keyboard: its own arrows and Enter are handled where
2530        // it is drawn, and the listing must not move underneath it at the same time.
2531        let typing =
2532            renaming || self.menu.is_some() || ctx.memory(|m| m.focused()).is_some();
2533        if typing {
2534            // **`Ctrl+E` still gets through.** The filter box and the flatten toggle are the
2535            // same question about the same folder — you type two letters, look at what came
2536            // up, and want the rest of the tree — and the filter survives the toggle, so the
2537            // two compose. Having to click out of the box first is a step with no reason
2538            // behind it, and `Ctrl+F` already works the other way round.
2539            //
2540            // Not while renaming, and not under a menu: a rename is an edit of one name that
2541            // re-reading the folder would throw away, and a menu owns the keyboard outright.
2542            // `consume_key` rather than `key_pressed`, so the field it was typed into does not
2543            // also see it.
2544            let allowed = !renaming && self.menu.is_none();
2545            if allowed && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, K::E)) {
2546                self.actions.push(Action::ToggleFlat(self.focused));
2547            }
2548            if ctx.input(|i| i.key_pressed(K::Escape)) {
2549                ctx.memory_mut(|m| m.stop_text_input());
2550            }
2551            return;
2552        }
2553
2554        let pane = self.focused;
2555        let mut push = |action| self.actions.push(action);
2556
2557        ctx.input(|i| {
2558            let m = i.modifiers;
2559
2560            // ---- Tabs and panes ------------------------------------------
2561            // `Shift` is checked here rather than left out, because `m.command` alone would
2562            // fire both of these on `Ctrl+Shift+T` — a new tab *and* the reopened one.
2563            if m.command && i.key_pressed(K::T) {
2564                if m.shift {
2565                    push(Action::ReopenTab);
2566                } else {
2567                    push(Action::NewTab { pane });
2568                }
2569            }
2570            if m.command && i.key_pressed(K::W) {
2571                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
2572                    push(Action::CloseTab {
2573                        pane,
2574                        tab: p.active,
2575                    });
2576                }
2577            }
2578            if m.command && i.key_pressed(K::Tab) {
2579                push(Action::NextTab {
2580                    pane,
2581                    delta: if m.shift { -1 } else { 1 },
2582                });
2583            }
2584            for (index, key) in [K::Num1, K::Num2, K::Num3, K::Num4, K::Num5, K::Num6, K::Num7, K::Num8, K::Num9]
2585                .into_iter()
2586                .enumerate()
2587            {
2588                if m.command && i.key_pressed(key) {
2589                    push(Action::ActivateTab { pane, tab: index });
2590                }
2591            }
2592            // A split of the current folder, so the feature is reachable without
2593            // knowing that tabs can be dragged.
2594            if m.command && i.key_pressed(K::Backslash) {
2595                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
2596                    push(Action::OpenInSplit {
2597                        pane,
2598                        path: p.tab().path.clone(),
2599                        side: Side::Right,
2600                    });
2601                }
2602            }
2603
2604            // ---- Navigation ----------------------------------------------
2605            if (m.alt && i.key_pressed(K::ArrowLeft)) || i.key_pressed(K::Backspace) && !m.alt {
2606                push(if m.alt {
2607                    Action::Back(pane)
2608                } else {
2609                    Action::Up(pane)
2610                });
2611            }
2612            if m.alt && i.key_pressed(K::ArrowRight) {
2613                push(Action::Forward(pane));
2614            }
2615            if m.alt && i.key_pressed(K::ArrowUp) {
2616                push(Action::Up(pane));
2617            }
2618            if i.key_pressed(K::F5) || (m.command && i.key_pressed(K::R)) {
2619                push(Action::Refresh(pane));
2620            }
2621            if (m.command && i.key_pressed(K::L)) || (m.alt && i.key_pressed(K::D)) {
2622                push(Action::EditPath(pane));
2623            }
2624            if m.command && i.key_pressed(K::A) {
2625                push(Action::SelectAll(pane));
2626            }
2627            if m.command && i.key_pressed(K::H) {
2628                push(Action::ToggleHidden(pane));
2629            }
2630            if m.command && i.key_pressed(K::E) {
2631                push(Action::ToggleFlat(pane));
2632            }
2633            if m.command && i.key_pressed(K::D) && !m.alt {
2634                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
2635                    push(Action::ToggleBookmark(p.tab().path.clone()));
2636                }
2637            }
2638            // This folder's preview panel. On the pane the keyboard is in, since that is the
2639            // folder whose selection it would be showing.
2640            if m.command && i.key_pressed(K::P) {
2641                push(Action::TogglePreview(pane));
2642            }
2643            // ---- Files ---------------------------------------------------
2644            //
2645            // Read as events rather than as key presses, because that is what arrives.
2646            // `egui-winit` recognises Ctrl+C, Ctrl+X and Ctrl+V itself and queues `Event::Copy`,
2647            // `Event::Cut` or `Event::Paste` *in place of* the key, so `key_pressed(K::C)` is
2648            // never true for a copy — which is why these three shortcuts did nothing at all
2649            // while looking perfectly well wired. See `paste_keystroke` in `main.rs` for the
2650            // paste half, which arrives only because this program puts it back.
2651            for event in &i.events {
2652                match event {
2653                    // Shift+Delete is a *permanent delete*, and on Windows `egui-winit`
2654                    // recognises it as a legacy cut: it queues `Event::Cut` for it, the very same
2655                    // event Ctrl+X produces, with nothing to tell them apart but the modifiers.
2656                    // Guarding this arm with `!m.shift` and leaving `key_pressed(Delete)` to
2657                    // catch the rest was how Shift+Delete came to do nothing at all — the key
2658                    // does not arrive either.
2659                    //
2660                    // `!m.command` is the discriminator and the direction it fails matters. With
2661                    // Ctrl held this is Ctrl+X, or Ctrl+Shift+X with a thumb resting on Shift,
2662                    // and reading either of those as "delete this for ever" would be the worst
2663                    // mistake this program could make. So anything ambiguous is a cut, which
2664                    // moves nothing until something pastes.
2665                    egui::Event::Cut if m.shift && !m.command => push(Action::Delete {
2666                        pane,
2667                        permanent: true,
2668                    }),
2669                    egui::Event::Cut => push(Action::Cut(pane)),
2670                    egui::Event::Copy => push(Action::Copy(pane)),
2671                    egui::Event::Paste(_) => push(Action::Paste(pane)),
2672                    _ => {}
2673                }
2674            }
2675            if i.key_pressed(K::Delete) {
2676                // Shift is the difference between the Recycle Bin and gone.
2677                push(Action::Delete {
2678                    pane,
2679                    permanent: m.shift,
2680                });
2681            }
2682            if i.key_pressed(K::F2) {
2683                push(Action::BeginRename(pane));
2684            }
2685            if m.command && m.shift && i.key_pressed(K::N) {
2686                push(Action::NewFolder(pane));
2687            }
2688
2689            if m.command && m.shift && i.key_pressed(K::C) {
2690                if let Some(p) = self.panes.iter().find(|p| p.id == pane) {
2691                    let mut paths = p.tab().selection_paths();
2692                    if paths.is_empty() {
2693                        paths.push(p.tab().path.clone());
2694                    }
2695                    push(Action::CopyPaths(paths));
2696                }
2697            }
2698        });
2699
2700        // ---- The listing's own keys --------------------------------------
2701        let Some(index) = self.panes.iter().position(|p| p.id == pane) else {
2702            return;
2703        };
2704        let rows_per_page = self
2705            .panes
2706            .iter()
2707            .find(|p| p.id == pane)
2708            .map(|p| ((p.rect.height() - 80.0) / crate::pane::ROW_HEIGHT).max(1.0) as isize)
2709            .unwrap_or(20);
2710
2711        let mut open: Option<(bool, PathBuf)> = None;
2712        let mut typed: Vec<char> = Vec::new();
2713
2714        {
2715            let tab = self.panes[index].tab_mut();
2716            ctx.input(|i| {
2717                let extend = i.modifiers.shift;
2718                if i.key_pressed(K::ArrowDown) {
2719                    tab.move_cursor(1, extend);
2720                }
2721                if i.key_pressed(K::ArrowUp) {
2722                    tab.move_cursor(-1, extend);
2723                }
2724                if i.key_pressed(K::PageDown) {
2725                    tab.move_cursor(rows_per_page, extend);
2726                }
2727                if i.key_pressed(K::PageUp) {
2728                    tab.move_cursor(-rows_per_page, extend);
2729                }
2730                if i.key_pressed(K::Home) {
2731                    tab.move_cursor_to(0, extend);
2732                }
2733                if i.key_pressed(K::End) {
2734                    tab.move_cursor_to(usize::MAX, extend);
2735                }
2736                if i.key_pressed(K::Escape) {
2737                    tab.clear_selection();
2738                }
2739                if i.key_pressed(K::Enter) {
2740                    if let Some(at) = tab.cursor {
2741                        if let Some(path) = tab.target_at(at) {
2742                            open = Some((tab.is_dir_at(at), path));
2743                        }
2744                    }
2745                }
2746                // Type-ahead: anything printable that is not a shortcut.
2747                if !i.modifiers.command && !i.modifiers.alt {
2748                    for event in &i.events {
2749                        if let egui::Event::Text(text) = event {
2750                            typed.extend(text.chars());
2751                        }
2752                    }
2753                }
2754            });
2755
2756            let now = ctx.input(|i| i.time);
2757            for ch in typed {
2758                tab.type_ahead(ch, now);
2759            }
2760        }
2761
2762        if let Some((is_dir, path)) = open {
2763            self.actions.push(if is_dir {
2764                Action::Navigate { pane, path }
2765            } else {
2766                Action::Open(path)
2767            });
2768        }
2769    }
2770
2771    // ---------------------------------------------------------------------
2772    // Applying
2773    // ---------------------------------------------------------------------
2774
2775    fn apply(&mut self, ctx: &egui::Context) {
2776        let actions = std::mem::take(&mut self.actions);
2777        for action in actions {
2778            self.perform(ctx, action);
2779        }
2780        if self.config_dirty {
2781            // Written on the way out rather than on every drag frame; the cost of
2782            // losing the last few points of a sidebar width is nothing, and the cost
2783            // of a file write per frame is a stutter.
2784            self.config_dirty = false;
2785            self.config = self.settings();
2786            self.config.save();
2787        }
2788    }
2789
2790    fn perform(&mut self, ctx: &egui::Context, action: Action) {
2791        if let Some(journal) = &mut self.journal {
2792            journal.push(action.name());
2793        }
2794        match action {
2795            Action::Focus(id) => {
2796                if self.panes.iter().any(|p| p.id == id) {
2797                    self.focused = id;
2798                }
2799            }
2800
2801            Action::ActivateTab { pane, tab } => {
2802                if let Some(p) = self.pane_mut(pane) {
2803                    if tab < p.tabs.len() {
2804                        p.show_tab(tab);
2805                    }
2806                }
2807                self.focused = pane;
2808            }
2809            Action::NextTab { pane, delta } => {
2810                if let Some(p) = self.pane_mut(pane) {
2811                    let count = p.tabs.len() as isize;
2812                    p.show_tab((((p.active as isize + delta) % count + count) % count) as usize);
2813                }
2814            }
2815            Action::NewTab { pane } => {
2816                if let Some(p) = self.pane_mut(pane) {
2817                    let tab = p.tab().duplicate();
2818                    p.tabs.push(tab);
2819                    p.show_tab(p.tabs.len() - 1);
2820                }
2821                self.focused = pane;
2822                self.config_dirty = true;
2823            }
2824            Action::NewTabFocused => {
2825                let pane = self.focused;
2826                self.perform(ctx, Action::NewTab { pane });
2827            }
2828            Action::SplitFocused { side } => {
2829                let pane = self.focused;
2830                if let Some(path) = self.pane_mut(pane).map(|p| p.tab().path.clone()) {
2831                    self.perform(ctx, Action::OpenInSplit { pane, path, side });
2832                }
2833            }
2834            Action::CloseTab { pane, tab } => self.close_tab(ctx, pane, tab),
2835            Action::ReopenTab => {
2836                // Somewhere to put it, before taking it off the stack: the focused pane, or any
2837                // pane if focus is stale. Popping first and then finding nowhere to open it
2838                // would spend the entry and give nothing back.
2839                let Some(pane) = self
2840                    .panes
2841                    .iter()
2842                    .find(|p| p.id == self.focused)
2843                    .or_else(|| self.panes.first())
2844                    .map(|p| p.id)
2845                else {
2846                    return;
2847                };
2848                let Some(path) = self.closed.pop() else {
2849                    return;
2850                };
2851                self.perform(ctx, Action::NavigateNewTab { pane, path });
2852            }
2853
2854            Action::BeginTabDrag { pane, tab, grab_dx } => {
2855                let title = self
2856                    .pane_mut(pane)
2857                    .and_then(|p| p.tabs.get(tab))
2858                    .map(|t| t.title.clone());
2859                if let Some(title) = title {
2860                    self.drag = Some(TabDrag {
2861                        pane,
2862                        tab,
2863                        grab_dx,
2864                        title,
2865                        live: false,
2866                    });
2867                }
2868            }
2869
2870            Action::MoveTab {
2871                from,
2872                tab,
2873                to,
2874                index,
2875            } => self.move_tab(from, tab, to, index),
2876
2877            Action::SplitTab {
2878                from,
2879                tab,
2880                target,
2881                side,
2882            } => {
2883                let Some(moved) = self.take_tab(from, tab) else {
2884                    return;
2885                };
2886                let id = self.spawn_pane(moved);
2887                if !self.layout.split(target, side, id) {
2888                    // The target vanished between the drop and now. Put the pane
2889                    // beside the focused one rather than losing the tab.
2890                    let anchor = self.focused;
2891                    self.layout.split(anchor, Side::Right, id);
2892                }
2893                self.focused = id;
2894                self.config_dirty = true;
2895            }
2896
2897            Action::OpenInSplit { pane, path, side } => {
2898                let id = self.spawn_pane(Tab::new(path));
2899                if !self.layout.split(pane, side, id) {
2900                    self.panes.retain(|p| p.id != id);
2901                    return;
2902                }
2903                self.focused = id;
2904                self.config_dirty = true;
2905            }
2906
2907            Action::Navigate { pane, path } => {
2908                if let Some(p) = self.pane_mut(pane) {
2909                    p.tab_mut().navigate(path);
2910                }
2911                self.focused = pane;
2912                self.config_dirty = true;
2913            }
2914            Action::NavigateNewTab { pane, path } => {
2915                if let Some(p) = self.pane_mut(pane) {
2916                    p.tabs.push(Tab::new(path));
2917                    p.show_tab(p.tabs.len() - 1);
2918                }
2919                self.focused = pane;
2920                self.config_dirty = true;
2921            }
2922            Action::Back(pane) => {
2923                if let Some(p) = self.pane_mut(pane) {
2924                    p.tab_mut().go_back();
2925                }
2926            }
2927            Action::Forward(pane) => {
2928                if let Some(p) = self.pane_mut(pane) {
2929                    p.tab_mut().go_forward();
2930                }
2931            }
2932            Action::Up(pane) => {
2933                if let Some(p) = self.pane_mut(pane) {
2934                    p.tab_mut().go_up();
2935                }
2936            }
2937            Action::Refresh(pane) => {
2938                // Re-probe the volumes too: a full disk or an ejected card is exactly
2939                // the kind of thing someone presses F5 about. The probes are threads,
2940                // so this costs nothing here.
2941                self.volumes.refresh(ctx);
2942                let path = self.pane_mut(pane).map(|p| p.tab().path.clone());
2943                if let Some(path) = path {
2944                    self.loader.invalidate(&path);
2945                    if let Some(p) = self.pane_mut(pane) {
2946                        let tab = p.tab_mut();
2947                        // Keep the cursor where it was: a refresh should not move
2948                        // what you were looking at.
2949                        let keep = tab
2950                            .cursor
2951                            .and_then(|at| tab.entry_at(at))
2952                            .and_then(|i| tab.dir.as_ref().map(|d| d.name(i).to_owned()));
2953                        tab.refresh();
2954                        tab.reveal = keep;
2955                    }
2956                }
2957            }
2958            Action::EditPath(pane) => {
2959                if let Some(p) = self.pane_mut(pane) {
2960                    breadcrumb::start_editing(p.tab_mut());
2961                }
2962            }
2963
2964            Action::Sort { pane, column } => {
2965                if let Some(p) = self.pane_mut(pane) {
2966                    p.tab_mut().sort_by_column(column);
2967                }
2968            }
2969            Action::SelectAll(pane) => {
2970                if let Some(p) = self.pane_mut(pane) {
2971                    p.tab_mut().select_all();
2972                }
2973            }
2974            Action::ToggleHidden(pane) => {
2975                if let Some(p) = self.pane_mut(pane) {
2976                    let tab = p.tab_mut();
2977                    tab.show_hidden = !tab.show_hidden;
2978                    tab.rebuild_order();
2979                    tab.widths_measured = false;
2980                }
2981            }
2982            // Unlike the other view toggles, this one changes what was *read* rather than
2983            // what is shown of it, so the listing goes and `start_scans` asks again.
2984            Action::ToggleFlat(pane) => {
2985                if let Some(p) = self.pane_mut(pane) {
2986                    p.tab_mut().toggle_flat();
2987                }
2988            }
2989            Action::TogglePreview(pane) => {
2990                let Some(p) = self.pane_mut(pane) else { return };
2991                let tab = p.tab_mut();
2992                if tab.preview.open {
2993                    tab.preview.close();
2994                } else {
2995                    // Opened with nothing previewable selected, the panel still opens and says
2996                    // what it would show. A shortcut that silently does nothing is a shortcut
2997                    // people conclude is broken.
2998                    match Self::selected_preview(tab) {
2999                        Some(ask) => tab.preview.ask_for(ask),
3000                        None => tab.preview.open = true,
3001                    }
3002                }
3003                self.config_dirty = true;
3004            }
3005            Action::ClosePreview(pane) => {
3006                if let Some(p) = self.pane_mut(pane) {
3007                    p.tab_mut().preview.close();
3008                }
3009                self.config_dirty = true;
3010            }
3011            Action::RememberLayout => self.config_dirty = true,
3012
3013            Action::Cut(pane) => self.put_on_clipboard(pane, true),
3014            Action::Copy(pane) => self.put_on_clipboard(pane, false),
3015            Action::Paste(pane) => self.paste_into(pane, ctx),
3016            Action::Delete { pane, permanent } => {
3017                let items = self
3018                    .pane_mut(pane)
3019                    .map(|p| p.tab().selection_paths())
3020                    .unwrap_or_default();
3021                if items.is_empty() {
3022                    self.notice = Some("Nothing selected".to_owned());
3023                    return;
3024                }
3025                // No confirmation of our own: the shell asks, and being asked twice
3026                // about the same thing is how a prompt becomes something people click
3027                // through without reading.
3028                self.ops.start(
3029                    crate::shell::ops::Job::Delete {
3030                        items,
3031                        to_bin: !permanent,
3032                    },
3033                    self.owner,
3034                    ctx,
3035                );
3036            }
3037            Action::BeginRename(pane) => {
3038                if let Some(p) = self.pane_mut(pane) {
3039                    p.tab_mut().begin_rename();
3040                }
3041            }
3042            Action::CommitRename { pane, name } => {
3043                let owner = self.owner;
3044                let Some(p) = self.pane_mut(pane) else { return };
3045                let tab = p.tab_mut();
3046                let Some((entry, _)) = tab.renaming.take() else {
3047                    return;
3048                };
3049                let Some(dir) = tab.dir.clone() else { return };
3050                let name = name.trim().to_owned();
3051                // Against the leaf, which is what the field was seeded with — in a
3052                // flattened listing the entry's *name* is a relative path, and comparing
3053                // against that would make every rename look like a change.
3054                if name.is_empty() || name == dir.leaf(entry) {
3055                    return;
3056                }
3057                let item = dir.target(entry);
3058                // Selected again once the folder is re-read, so the renamed file is
3059                // still the thing you were looking at.
3060                tab.reveal = Some(name.clone());
3061                self.ops
3062                    .start(crate::shell::ops::Job::Rename { item, name }, owner, ctx);
3063            }
3064            Action::CancelRename(pane) => {
3065                if let Some(p) = self.pane_mut(pane) {
3066                    p.tab_mut().renaming = None;
3067                }
3068            }
3069            Action::NewFolder(pane) => {
3070                let owner = self.owner;
3071                let Some(parent) = self.pane_mut(pane).map(|p| p.tab().path.clone()) else {
3072                    return;
3073                };
3074                if parent.as_os_str().is_empty() {
3075                    self.notice = Some("This PC is not a folder to create in".to_owned());
3076                    return;
3077                }
3078                // The shell picks a free name from this one, so "New folder (2)" and the
3079                // rest come out right without this program having to count — and reports back
3080                // which it chose, so the row can be named the moment it appears.
3081                let name = "New folder".to_owned();
3082                self.ops.start_then(
3083                    crate::shell::ops::Job::NewFolder { parent, name },
3084                    crate::shell::ops::After::NameIt(pane),
3085                    owner,
3086                    ctx,
3087                );
3088            }
3089            Action::DropHere {
3090                pane,
3091                items,
3092                into,
3093                moving,
3094            } => {
3095                self.focused = pane;
3096                let job = if moving {
3097                    crate::shell::ops::Job::Move { items, into }
3098                } else {
3099                    crate::shell::ops::Job::Copy { items, into }
3100                };
3101                self.ops.start(job, self.owner, ctx);
3102            }
3103            Action::DragOut { pane, items } => {
3104                // Started here and now rather than parked for later: the drag has its own
3105                // thread, so nothing about it re-enters this pass. One at a time, since the
3106                // second would be following a button the first is already holding — and only
3107                // with a window, because the drag joins *this* thread's input queue to find
3108                // the button it is following and a thread with no window has no gesture to
3109                // follow. That last one is also what keeps the tests off the real pointer.
3110                if self.file_drag.is_none() && self.owner.0 != 0 {
3111                    self.file_drag =
3112                        crate::shell::dnd::drag_out(items).map(|drag| (pane, drag));
3113                }
3114            }
3115            Action::ShellMenu {
3116                pane,
3117                items,
3118                at,
3119            } => self.shell_menu(pane, items, at, ctx),
3120            // **A shortcut to a folder opens here, not in Explorer.** Handing it to the shell is
3121            // what `.lnk` files get by default, and for a folder that means a second file
3122            // manager opening over the top of this one — which is not what clicking a row in
3123            // this window can be allowed to do. Every route into this arm gets it: a double
3124            // click, `Enter`, and a path typed into the bar.
3125            //
3126            // A directory *reparse point* — a junction or a directory symlink — never comes
3127            // through here at all: the enumeration reports it as a directory, so it is a
3128            // `Navigate` before this is reached.
3129            //
3130            // The pane is the focused one because that is where the gesture was: a click on a
3131            // row focuses its pane first, and `Enter` acts on the focused pane by definition.
3132            Action::Open(path) => match crate::shell::links::folder_target(&path) {
3133                Some(folder) => {
3134                    let pane = self.focused;
3135                    self.perform(ctx, Action::Navigate { pane, path: folder });
3136                }
3137                None => fs::shell::open(&path),
3138            },
3139            // The same, in a tab of its own — a middle click on a folder shortcut. A shortcut to
3140            // a *file* does nothing here rather than opening it somewhere it cannot be shown: a
3141            // new tab is a place, and a file is not one.
3142            Action::OpenNewTab(path) => {
3143                if let Some(folder) = crate::shell::links::folder_target(&path) {
3144                    let pane = self.focused;
3145                    self.perform(ctx, Action::NavigateNewTab { pane, path: folder });
3146                }
3147            }
3148            Action::Reveal(path) => fs::shell::reveal(&path),
3149            Action::OpenTerminal(path) => fs::shell::open_terminal(&path),
3150            Action::CopyPaths(paths) => {
3151                let text = paths
3152                    .iter()
3153                    .map(|p| p.to_string_lossy().into_owned())
3154                    .collect::<Vec<_>>()
3155                    .join("\r\n");
3156                ctx.copy_text(text);
3157            }
3158            Action::AddBookmark(path) => {
3159                if !path.as_os_str().is_empty() && !self.bookmarks.contains(&path) {
3160                    self.bookmarks.push(path);
3161                    self.config_dirty = true;
3162                }
3163            }
3164            Action::RemoveBookmark(path) => {
3165                self.bookmarks.retain(|p| *p != path);
3166                self.config_dirty = true;
3167            }
3168            Action::MoveBookmark { from, to } => {
3169                // `to` is an insertion point in the list *before* the move, so removing
3170                // first shifts every later position down by one.
3171                if from < self.bookmarks.len() && to <= self.bookmarks.len() {
3172                    let moved = self.bookmarks.remove(from);
3173                    let at = if to > from { to - 1 } else { to };
3174                    self.bookmarks.insert(at.min(self.bookmarks.len()), moved);
3175                    self.config_dirty = true;
3176                }
3177            }
3178            Action::ToggleBookmark(path) => {
3179                if self.is_bookmarked(&path) {
3180                    self.bookmarks.retain(|p| *p != path);
3181                } else if !path.as_os_str().is_empty() {
3182                    self.bookmarks.push(path);
3183                }
3184                self.config_dirty = true;
3185            }
3186            Action::SetTheme { dark } => {
3187                if dark == self.theme.dark {
3188                    return;
3189                }
3190                self.theme = if dark { Theme::dark() } else { Theme::light() };
3191                // The style has to be reinstalled, and only then — installing it every
3192                // frame would throw away egui's galley and shape caches.
3193                self.installed = false;
3194                self.config_dirty = true;
3195            }
3196
3197            Action::Window(what) => {
3198                use egui::ViewportCommand as Cmd;
3199                match what {
3200                    WindowAction::Minimize => ctx.send_viewport_cmd(Cmd::Minimized(true)),
3201                    WindowAction::ToggleMaximize => {
3202                        ctx.send_viewport_cmd(Cmd::Maximized(!self.maximized));
3203                    }
3204                    WindowAction::ResetSize => {
3205                        let [w, h] = crate::config::WINDOW_SIZE;
3206                        // Un-maximised first, and said out loud rather than relied on. On
3207                        // Windows an `InnerSize` alone is enough — measured: from a maximised
3208                        // 2560×1392 the window comes back to 1024×600 with this line taken out,
3209                        // because `SetWindowPos` on a maximised window restores it on the way.
3210                        // That is winit's platform behaviour and not a promise, and asking for
3211                        // the state this wants costs one command.
3212                        if self.maximized {
3213                            ctx.send_viewport_cmd(Cmd::Maximized(false));
3214                        }
3215                        ctx.send_viewport_cmd(Cmd::InnerSize(egui::vec2(w, h)));
3216                        // Both remembered now rather than left to the frame that observes the
3217                        // new shape. That frame does set them, so this is belt and braces for
3218                        // the window being closed in between — which would otherwise save the
3219                        // size and the maximised flag this has just replaced.
3220                        self.maximized = false;
3221                        self.window_size = Some([w, h]);
3222                        self.config_dirty = true;
3223                    }
3224                    WindowAction::Close => ctx.send_viewport_cmd(Cmd::Close),
3225                    WindowAction::Drag => ctx.send_viewport_cmd(Cmd::StartDrag),
3226                }
3227            }
3228        }
3229    }
3230
3231    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
3232        self.panes.iter_mut().find(|p| p.id == id)
3233    }
3234
3235    fn spawn_pane(&mut self, tab: Tab) -> PaneId {
3236        let id = self.next_pane;
3237        self.next_pane += 1;
3238        self.panes.push(Pane::new(id, tab));
3239        id
3240    }
3241
3242    /// Take a tab out of a pane, dropping the pane if that was its last.
3243    fn take_tab(&mut self, pane: PaneId, index: usize) -> Option<Tab> {
3244        let position = self.panes.iter().position(|p| p.id == pane)?;
3245        if index >= self.panes[position].tabs.len() {
3246            return None;
3247        }
3248        let tab = self.panes[position].tabs.remove(index);
3249        let p = &mut self.panes[position];
3250        if p.tabs.is_empty() {
3251            // Only close the pane if the tree can absorb it; a lone root pane has
3252            // nowhere to collapse into, and is refilled by the caller.
3253            if self.layout.remove(pane) {
3254                self.panes.remove(position);
3255                if self.focused == pane {
3256                    self.focused = self.panes.first().map(|p| p.id).unwrap_or(pane);
3257                }
3258            }
3259        } else if p.active >= p.tabs.len() {
3260            p.show_tab(p.tabs.len() - 1);
3261        }
3262        Some(tab)
3263    }
3264
3265    fn move_tab(&mut self, from: PaneId, index: usize, to: PaneId, at: usize) {
3266        if from == to {
3267            // A reorder inside one strip. The insertion point was measured with the
3268            // tab still in place, so removing it shifts everything after it.
3269            let Some(p) = self.pane_mut(from) else { return };
3270            if index >= p.tabs.len() {
3271                return;
3272            }
3273            let target = at.min(p.tabs.len());
3274            let tab = p.tabs.remove(index);
3275            let target = if target > index { target - 1 } else { target };
3276            let target = target.min(p.tabs.len());
3277            p.tabs.insert(target, tab);
3278            p.show_tab(target);
3279            self.focused = from;
3280            self.config_dirty = true;
3281            return;
3282        }
3283
3284        let Some(tab) = self.take_tab(from, index) else {
3285            return;
3286        };
3287        let Some(p) = self.pane_mut(to) else {
3288            // The destination went away; keep the tab by putting it back somewhere.
3289            let anchor = self.focused;
3290            if let Some(p) = self.pane_mut(anchor) {
3291                p.tabs.push(tab);
3292                p.show_tab(p.tabs.len() - 1);
3293            }
3294            return;
3295        };
3296        let target = at.min(p.tabs.len());
3297        p.tabs.insert(target, tab);
3298        p.show_tab(target);
3299        self.focused = to;
3300        self.config_dirty = true;
3301    }
3302
3303    fn close_tab(&mut self, ctx: &egui::Context, pane: PaneId, index: usize) {
3304        let Some(position) = self.panes.iter().position(|p| p.id == pane) else {
3305            return;
3306        };
3307        let last_pane = self.panes.len() == 1;
3308        let last_tab = self.panes[position].tabs.len() == 1;
3309
3310        if last_tab && last_pane {
3311            // The last tab of the last pane: the window is what is being closed.
3312            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
3313            return;
3314        }
3315        // Remembered before it goes, so `Ctrl+Shift+T` has something to put back. Guarded by
3316        // `get` for the same reason `Pane::close_tab` guards: an index past the end closes
3317        // nothing, and a history of tabs that were never closed would hand back folders that
3318        // are still open.
3319        if let Some(path) = self.panes[position].tabs.get(index).map(|t| t.path.clone()) {
3320            self.closed.push(path);
3321            if self.closed.len() > CLOSED_TABS {
3322                self.closed.remove(0);
3323            }
3324        }
3325        if !self.panes[position].close_tab(index) {
3326            // The pane is empty now, so it goes too.
3327            if self.layout.remove(pane) {
3328                self.panes.remove(position);
3329                if self.focused == pane {
3330                    self.focused = self.panes.first().map(|p| p.id).unwrap_or(pane);
3331                }
3332            }
3333        }
3334        self.config_dirty = true;
3335    }
3336
3337    /// Whether a path is bookmarked, for the sidebar's star.
3338    pub fn is_bookmarked(&self, path: &Path) -> bool {
3339        self.bookmarks.iter().any(|p| p == path)
3340    }
3341}
3342
3343#[cfg(test)]
3344mod tests {
3345    use super::*;
3346
3347    /// An app with one pane per path, and no scan allowed to land — the tests are
3348    /// about structure, and a listing arriving would only add noise.
3349    fn app(paths: &[&str]) -> (App, egui::Context) {
3350        let ctx = egui::Context::default();
3351        let open: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
3352        let app = App::opening(&ctx, Config::default(), open, Side::Right);
3353        (app, ctx)
3354    }
3355
3356    #[test]
3357    fn a_window_gesture_keeps_frames_coming_and_then_stops() {
3358        let mut s = Settling::default();
3359        let at_rest = Shape {
3360            pixels: (1024, 600),
3361            scale: 1000,
3362            native_scale: 1000,
3363            focused: true,
3364            minimized: false,
3365        };
3366
3367        // The first frame is a change from nothing, so it paints -- which is correct and
3368        // also why the baseline has to be established before anything is asserted.
3369        assert!(s.observe(at_rest, 1.0));
3370
3371        // Then nothing is moving, and no frames are asked for. This is the case that must
3372        // cost nothing, because it is every frame of every session.
3373        assert!(!s.observe(at_rest, 1.0 + Settling::QUIET));
3374        assert!(!s.observe(at_rest, 9.0));
3375
3376        // Restored onto a monitor at 125%: both the pixel size and the scale change.
3377        let rescaled = Shape {
3378            pixels: (1280, 750),
3379            scale: 1250,
3380            ..at_rest
3381        };
3382        assert!(s.observe(rescaled, 10.0), "the change itself has to repaint");
3383        // And it keeps painting while the gesture settles, without needing more changes --
3384        // which is the point: the stretched frame is on screen during these.
3385        assert!(s.observe(rescaled, 10.2));
3386        assert!(s.observe(rescaled, 10.0 + Settling::QUIET - 0.01));
3387        // Then stops.
3388        assert!(!s.observe(rescaled, 10.0 + Settling::QUIET));
3389        assert!(!s.observe(rescaled, 20.0));
3390
3391        // Losing focus counts too: it is what a restore animation and an occlusion both
3392        // come with, and it was the reported trigger.
3393        assert!(s.observe(
3394            Shape {
3395                focused: false,
3396                ..rescaled
3397            },
3398            21.0
3399        ));
3400    }
3401
3402    #[test]
3403    fn a_scale_change_too_small_to_see_is_not_a_change() {
3404        // The scale arrives as an `f32`. Comparing it exactly would let a value that
3405        // differs in its last bit request repaints for the rest of the session.
3406        let mut s = Settling::default();
3407        let a = Shape {
3408            pixels: (1024, 600),
3409            scale: (1.0_f32 * 1000.0).round() as u32,
3410            native_scale: 1000,
3411            focused: true,
3412            minimized: false,
3413        };
3414        let b = Shape {
3415            scale: (1.000_04_f32 * 1000.0).round() as u32,
3416            ..a
3417        };
3418        assert_eq!(a, b, "a difference this small is not a rescale");
3419        s.observe(a, 1.0); // the baseline
3420        assert!(!s.observe(b, 1.0 + Settling::QUIET));
3421    }
3422
3423    fn titles(app: &App, pane: PaneId) -> Vec<String> {
3424        app.panes
3425            .iter()
3426            .find(|p| p.id == pane)
3427            .map(|p| p.tabs.iter().map(|t| t.title.clone()).collect())
3428            .unwrap_or_default()
3429    }
3430
3431    fn tree(app: &App) -> Vec<PaneId> {
3432        let mut out = Vec::new();
3433        app.layout.panes(&mut out);
3434        out
3435    }
3436
3437    #[test]
3438    fn one_pane_per_open_path() {
3439        let (app, _ctx) = app(&["/a", "/b", "/c"]);
3440        assert_eq!(app.panes.len(), 3);
3441        assert_eq!(tree(&app).len(), 3);
3442        assert_eq!(app.layout.count(), 3);
3443    }
3444
3445    /// A window comes back divided the way it was left.
3446    ///
3447    /// The end-to-end claim, and the only place the two halves of it meet: `settings` writes
3448    /// the tree over pane *numbers* and the panes in the order the tree numbers them, and
3449    /// `opening` has to line those numbers back up with the panes it builds. Either half alone
3450    /// can be right while the pair is wrong — a window whose panes come back in the other
3451    /// order, showing the right folders in the wrong places — so this goes all the way through
3452    /// the text of the file.
3453    #[test]
3454    fn the_panes_come_back_the_way_they_were_left() {
3455        let (mut app, ctx) = app(&["/left", "/right"]);
3456        let (left, right) = (app.panes[0].id, app.panes[1].id);
3457        // A second tab in the left pane, a third pane under the right one, and the focus
3458        // somewhere that is not the first pane — so that every part of what is written down
3459        // has something to say.
3460        app.perform(&ctx, Action::NewTab { pane: left });
3461        app.perform(
3462            &ctx,
3463            Action::Navigate {
3464                pane: left,
3465                path: PathBuf::from("/left/deeper"),
3466            },
3467        );
3468        // To the *left* of the right-hand pane, deliberately: the new pane goes in front of it
3469        // in the tree while being pushed to the back of `panes`, so layout order and the order
3470        // the panes happen to sit in the vector are no longer the same list. Written in the
3471        // wrong one of those two, everything below still passes.
3472        app.perform(
3473            &ctx,
3474            Action::OpenInSplit {
3475                pane: right,
3476                path: PathBuf::from("/under"),
3477                side: Side::Left,
3478            },
3479        );
3480        app.perform(&ctx, Action::Focus(right));
3481        *app.layout.ratio_at(&[]).unwrap() = 0.4;
3482
3483        let order = tree(&app);
3484        assert_eq!(order.len(), 3, "three panes to write down");
3485        assert_ne!(
3486            order,
3487            app.panes.iter().map(|p| p.id).collect::<Vec<_>>(),
3488            "this test is only worth running while the two orders differ"
3489        );
3490        let titles: Vec<Vec<String>> = order.iter().map(|id| self::titles(&app, *id)).collect();
3491        let had_focus = order.iter().position(|id| *id == right).expect("in the tree");
3492        let shape = app.layout.encode();
3493
3494        // Through the text of the file, not merely through the struct: the numbering is the
3495        // part that can go wrong, and it only exists in the text.
3496        let reopened = Config::parse(&app.settings().to_text());
3497        let back = App::opening(&ctx, reopened, Vec::new(), Side::Right);
3498
3499        let recovered = tree(&back);
3500        assert_eq!(recovered.len(), order.len(), "a pane went missing");
3501        assert_eq!(back.layout.encode(), shape, "the same shape, with the same ratios");
3502        assert_eq!(
3503            recovered
3504                .iter()
3505                .map(|id| self::titles(&back, *id))
3506                .collect::<Vec<_>>(),
3507            titles,
3508            "the folders came back in the wrong panes"
3509        );
3510        assert_eq!(
3511            back.focused, recovered[had_focus],
3512            "the pane that had the keyboard has to be the one that gets it"
3513        );
3514        assert_eq!(
3515            back.panes
3516                .iter()
3517                .find(|p| p.id == recovered[0])
3518                .map(|p| p.active),
3519            Some(1),
3520            "and the tab that was in front stays in front"
3521        );
3522    }
3523
3524    /// A settings file from before panes were remembered still opens every tab it names.
3525    #[test]
3526    fn remembered_tabs_with_no_layout_reopen_in_one_pane() {
3527        let ctx = egui::Context::default();
3528        let config = Config::parse("path=/a\npath=/b\npath=/c\n");
3529        let app = App::opening(&ctx, config, Vec::new(), Side::Right);
3530        assert_eq!(app.panes.len(), 1, "there is no layout to build");
3531        assert_eq!(app.panes[0].tabs.len(), 3, "but every tab is still opened");
3532    }
3533
3534    /// And a layout that does not describe the panes beside it is not half-applied.
3535    #[test]
3536    fn a_layout_that_does_not_fit_its_panes_opens_plainly() {
3537        let ctx = egui::Context::default();
3538        // Two panes named, three in the tree.
3539        let config = Config::parse("layout=h0.5(0,v0.5(1,2))\npane=0\npath=/a\npane=0\npath=/b\n");
3540        let app = App::opening(&ctx, config, Vec::new(), Side::Right);
3541        assert_eq!(app.panes.len(), 1);
3542        assert_eq!(
3543            app.panes[0].tabs.len(),
3544            2,
3545            "the folders that were open have to open, whatever the tree said"
3546        );
3547    }
3548
3549    #[test]
3550    fn a_tab_dragged_to_another_pane_changes_hands() {
3551        let (mut app, ctx) = app(&["/left", "/right"]);
3552        let (left, right) = (app.panes[0].id, app.panes[1].id);
3553        app.perform(&ctx, Action::NewTab { pane: left });
3554        assert_eq!(titles(&app, left).len(), 2);
3555
3556        app.perform(
3557            &ctx,
3558            Action::MoveTab {
3559                from: left,
3560                tab: 0,
3561                to: right,
3562                index: 0,
3563            },
3564        );
3565        assert_eq!(titles(&app, left).len(), 1);
3566        assert_eq!(titles(&app, right), ["left", "right"]);
3567        assert_eq!(app.focused, right, "the tab you moved is the one you wanted");
3568    }
3569
3570    #[test]
3571    fn moving_a_panes_last_tab_away_collapses_the_split() {
3572        let (mut app, ctx) = app(&["/left", "/right"]);
3573        let (left, right) = (app.panes[0].id, app.panes[1].id);
3574
3575        app.perform(
3576            &ctx,
3577            Action::MoveTab {
3578                from: left,
3579                tab: 0,
3580                to: right,
3581                index: usize::MAX,
3582            },
3583        );
3584        assert_eq!(app.panes.len(), 1, "the emptied pane goes");
3585        assert_eq!(tree(&app), [right], "and so does its half of the tree");
3586        assert_eq!(titles(&app, right), ["right", "left"]);
3587    }
3588
3589    #[test]
3590    fn reordering_inside_a_strip_accounts_for_the_gap_left_behind() {
3591        let (mut app, ctx) = app(&["/a"]);
3592        let pane = app.panes[0].id;
3593        // Three tabs: a, a, a -- retitled so the order is checkable.
3594        app.perform(&ctx, Action::NewTab { pane });
3595        app.perform(&ctx, Action::NewTab { pane });
3596        for (index, name) in ["one", "two", "three"].into_iter().enumerate() {
3597            app.panes[0].tabs[index].title = name.to_owned();
3598        }
3599
3600        // Drop the first tab where the third one starts: it lands between two and
3601        // three, because removing it shifted everything after it down.
3602        app.perform(
3603            &ctx,
3604            Action::MoveTab {
3605                from: pane,
3606                tab: 0,
3607                to: pane,
3608                index: 2,
3609            },
3610        );
3611        assert_eq!(titles(&app, pane), ["two", "one", "three"]);
3612        assert_eq!(app.panes[0].active, 1, "the moved tab stays the active one");
3613    }
3614
3615    #[test]
3616    fn appending_to_a_strip_puts_the_tab_last() {
3617        let (mut app, ctx) = app(&["/a"]);
3618        let pane = app.panes[0].id;
3619        app.perform(&ctx, Action::NewTab { pane });
3620        app.panes[0].tabs[0].title = "one".to_owned();
3621        app.panes[0].tabs[1].title = "two".to_owned();
3622
3623        app.perform(
3624            &ctx,
3625            Action::MoveTab {
3626                from: pane,
3627                tab: 0,
3628                to: pane,
3629                index: usize::MAX,
3630            },
3631        );
3632        assert_eq!(titles(&app, pane), ["two", "one"]);
3633    }
3634
3635    #[test]
3636    fn dropping_a_tab_on_a_pane_edge_splits_it() {
3637        let (mut app, ctx) = app(&["/a"]);
3638        let pane = app.panes[0].id;
3639        app.perform(&ctx, Action::NewTab { pane });
3640
3641        app.perform(
3642            &ctx,
3643            Action::SplitTab {
3644                from: pane,
3645                tab: 1,
3646                target: pane,
3647                side: Side::Right,
3648            },
3649        );
3650        assert_eq!(app.panes.len(), 2);
3651        assert_eq!(app.layout.count(), 2);
3652        let new = tree(&app)[1];
3653        assert_eq!(new, app.focused, "focus follows the tab you pulled out");
3654        assert_eq!(titles(&app, pane).len(), 1);
3655        assert_eq!(titles(&app, new).len(), 1);
3656    }
3657
3658    #[test]
3659    fn closing_the_last_tab_of_a_split_pane_removes_the_pane() {
3660        let (mut app, ctx) = app(&["/left", "/right"]);
3661        let (left, right) = (app.panes[0].id, app.panes[1].id);
3662
3663        app.perform(&ctx, Action::CloseTab { pane: left, tab: 0 });
3664        assert_eq!(app.panes.len(), 1);
3665        assert_eq!(tree(&app), [right]);
3666        assert_eq!(app.focused, right, "focus cannot stay on a pane that is gone");
3667    }
3668
3669    #[test]
3670    fn the_last_tab_of_the_last_pane_is_the_window() {
3671        let (mut app, ctx) = app(&["/only"]);
3672        let pane = app.panes[0].id;
3673        app.perform(&ctx, Action::CloseTab { pane, tab: 0 });
3674        // Nothing is torn down -- the close is a viewport command, and the state has
3675        // to stay coherent for however many frames it takes to arrive.
3676        assert_eq!(app.panes.len(), 1);
3677        assert_eq!(app.panes[0].tabs.len(), 1);
3678    }
3679
3680    #[test]
3681    fn a_new_tab_points_where_the_old_one_did() {
3682        let (mut app, ctx) = app(&["/somewhere/deep"]);
3683        let pane = app.panes[0].id;
3684        app.perform(&ctx, Action::NewTab { pane });
3685        assert_eq!(app.panes[0].tabs[0].path, app.panes[0].tabs[1].path);
3686        assert_eq!(app.panes[0].active, 1);
3687    }
3688
3689    #[test]
3690    fn a_closed_tab_comes_back_with_nothing_behind_it() {
3691        let (mut app, ctx) = app(&["/one"]);
3692        let pane = app.panes[0].id;
3693        app.perform(
3694            &ctx,
3695            Action::NavigateNewTab {
3696                pane,
3697                path: PathBuf::from("/two"),
3698            },
3699        );
3700        // Somewhere for it to have come *from*, so that the assertion below is about a history
3701        // being dropped rather than about a tab that never had one.
3702        app.perform(
3703            &ctx,
3704            Action::Navigate {
3705                pane,
3706                path: PathBuf::from("/two/deep"),
3707            },
3708        );
3709        assert_eq!(app.panes[0].tabs[1].history.len(), 2);
3710
3711        app.perform(&ctx, Action::CloseTab { pane, tab: 1 });
3712        assert_eq!(app.panes[0].tabs.len(), 1);
3713
3714        app.perform(&ctx, Action::ReopenTab);
3715        let back = app.panes[0].tabs.last().expect("the tab that came back");
3716        assert_eq!(
3717            back.path,
3718            PathBuf::from("/two/deep"),
3719            "the folder it was showing"
3720        );
3721        assert_eq!(
3722            back.history,
3723            [PathBuf::from("/two/deep")],
3724            "the path and nothing else -- not the trail it got there by"
3725        );
3726        assert_eq!(back.at, 0);
3727        assert!(!back.can_go_back());
3728        assert!(app.closed.is_empty(), "and it is spent, not repeatable");
3729    }
3730
3731    #[test]
3732    fn only_the_last_ten_closed_tabs_are_kept() {
3733        let (mut app, ctx) = app(&["/keep"]);
3734        let pane = app.panes[0].id;
3735        let path = |n: usize| PathBuf::from(format!("/gone/{n}"));
3736
3737        // Twelve opened and closed, so the two oldest fall off the back.
3738        for n in 0..12 {
3739            app.perform(&ctx, Action::NavigateNewTab { pane, path: path(n) });
3740            app.perform(&ctx, Action::CloseTab { pane, tab: 1 });
3741        }
3742        assert_eq!(app.closed.len(), CLOSED_TABS);
3743        assert_eq!(app.closed.first(), Some(&path(2)), "0 and 1 are gone");
3744
3745        // Two more presses than there is anything to answer them with.
3746        for _ in 0..CLOSED_TABS + 2 {
3747            app.perform(&ctx, Action::ReopenTab);
3748        }
3749        let back: Vec<PathBuf> = app.panes[0].tabs[1..].iter().map(|t| t.path.clone()).collect();
3750        let expected: Vec<PathBuf> = (2..12).rev().map(path).collect();
3751        assert_eq!(
3752            back, expected,
3753            "most recently closed comes back first, and nothing older than ten comes back at all"
3754        );
3755    }
3756
3757    #[test]
3758    fn reopening_with_nothing_closed_does_nothing() {
3759        let (mut app, ctx) = app(&["/only"]);
3760        app.perform(&ctx, Action::ReopenTab);
3761        assert_eq!(app.panes[0].tabs.len(), 1);
3762    }
3763
3764    #[test]
3765    fn the_close_that_is_the_window_is_not_remembered() {
3766        // That close is the window going, and the history goes with it. Recording it would put
3767        // the folder back into a window that is on its way out.
3768        let (mut app, ctx) = app(&["/only"]);
3769        let pane = app.panes[0].id;
3770        app.perform(&ctx, Action::CloseTab { pane, tab: 0 });
3771        assert!(app.closed.is_empty());
3772    }
3773
3774    #[test]
3775    fn a_tab_pulled_out_into_a_pane_of_its_own_was_not_closed() {
3776        let (mut app, ctx) = app(&["/a"]);
3777        let pane = app.panes[0].id;
3778        app.perform(
3779            &ctx,
3780            Action::NavigateNewTab {
3781                pane,
3782                path: PathBuf::from("/b"),
3783            },
3784        );
3785        app.perform(
3786            &ctx,
3787            Action::SplitTab {
3788                from: pane,
3789                tab: 1,
3790                target: pane,
3791                side: Side::Right,
3792            },
3793        );
3794        assert_eq!(app.panes.len(), 2);
3795        assert!(
3796            app.closed.is_empty(),
3797            "a tab that moved somewhere else was never closed"
3798        );
3799    }
3800
3801    #[test]
3802    fn bookmarks_toggle_and_never_pin_this_pc() {
3803        let (mut app, ctx) = app(&["/a"]);
3804        let path = PathBuf::from("/a");
3805        app.perform(&ctx, Action::ToggleBookmark(path.clone()));
3806        assert!(app.is_bookmarked(&path));
3807        app.perform(&ctx, Action::ToggleBookmark(path.clone()));
3808        assert!(!app.is_bookmarked(&path));
3809
3810        app.perform(&ctx, Action::ToggleBookmark(PathBuf::new()));
3811        assert!(app.bookmarks.is_empty(), "This PC is not a folder to pin");
3812    }
3813
3814    #[test]
3815    fn split_focused_opens_the_folder_already_showing() {
3816        let (mut app, ctx) = app(&["/here"]);
3817        app.perform(&ctx, Action::SplitFocused { side: Side::Bottom });
3818        assert_eq!(app.panes.len(), 2);
3819        assert_eq!(app.panes[1].tab().path, PathBuf::from("/here"));
3820    }
3821}
3822
3823/// Driving the interface with real pointer events.
3824///
3825/// Everything else in this program can be checked by reading it. Click targets
3826/// cannot: an interaction rect that another one happens to cover reads perfectly
3827/// correctly at the call site and simply does not respond, and the only way to know is
3828/// to press the pointer down at a coordinate and see what moves. So these tests run
3829/// whole frames through a real [`egui::Context`] with synthetic input and assert on
3830/// what the application actually did.
3831#[cfg(test)]
3832mod click_tests {
3833    use super::*;
3834    use egui::{pos2, vec2, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect};
3835
3836    struct Harness {
3837        app: App,
3838        ctx: egui::Context,
3839        size: egui::Vec2,
3840        /// The harness drives the clock rather than letting it drift. egui counts
3841        /// clicks that arrive within 300ms of each other as a double or a triple, so
3842        /// two gestures in quick succession would run together — which is a property
3843        /// of the test, not of the program.
3844        time: f64,
3845        /// Held modifiers. `Event::PointerButton` carries a copy, but the code under
3846        /// test reads `InputState::modifiers`, which comes from this.
3847        modifiers: Modifiers,
3848        /// Whether the window has the platform's focus. `true` as `RawInput`'s own default is,
3849        /// so every test but the one about losing it is unaffected.
3850        focused: bool,
3851        /// What the last frame asked the pointer to look like.
3852        cursor: egui::CursorIcon,
3853        /// Everything the frames so far have asked the platform to do to the window, in order.
3854        commands: Vec<egui::ViewportCommand>,
3855        /// Every shape the last frame painted, in paint order.
3856        ///
3857        /// Replaced each frame rather than accumulated, because the question these answer is
3858        /// "what does the window look like now". They are how a test can assert on a *fill* —
3859        /// a colour is not a click target, and reading the source only proves the source says
3860        /// what it says.
3861        shapes: Vec<egui::Shape>,
3862    }
3863
3864    impl Harness {
3865        /// One pane per requested count, all showing directories that exist and have
3866        /// something in them to click on.
3867        fn with_panes(count: usize) -> Self {
3868            let ctx = egui::Context::default();
3869            azur_egui_theme::fonts::install(&ctx);
3870            let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
3871            let open: Vec<PathBuf> = (0..count)
3872                .map(|i| if i == 0 { here.clone() } else { here.join("src") })
3873                .collect();
3874            let mut app = App::opening(&ctx, Config::default(), open, Side::Right);
3875            app.journal = Some(Vec::new());
3876
3877            let mut harness = Harness {
3878                app,
3879                ctx,
3880                size: vec2(1024.0, 650.0),
3881                time: 1.0,
3882                modifiers: Modifiers::NONE,
3883                focused: true,
3884                cursor: egui::CursorIcon::Default,
3885                commands: Vec::new(),
3886                shapes: Vec::new(),
3887            };
3888            // The listing arrives by channel, so rects that depend on it do not exist
3889            // until a few frames have gone by.
3890            harness.settle();
3891            harness
3892        }
3893
3894        fn new() -> Self {
3895            Self::with_panes(1)
3896        }
3897
3898        /// Run frames until the listings have arrived.
3899        ///
3900        /// Not a fixed count: a scan comes back by channel from a worker thread, and eight
3901        /// frames of a test process sharing a machine with seven other test threads is
3902        /// sometimes not long enough — which showed up as this suite failing one run in
3903        /// three on an assertion about the *listing* rather than about anything the test
3904        /// was for. So it waits for the thing it is waiting for.
3905        fn settle(&mut self) {
3906            for attempt in 0..200 {
3907                self.frame(Vec::new());
3908                let arrived = self
3909                    .app
3910                    .panes
3911                    .iter()
3912                    .all(|pane| pane.tab().dir.is_some());
3913                if arrived && attempt >= 4 {
3914                    break;
3915                }
3916                if attempt >= 4 {
3917                    // Frames are free here; the disk is not.
3918                    std::thread::sleep(std::time::Duration::from_millis(2));
3919                }
3920            }
3921            self.take_journal();
3922        }
3923
3924        /// One pass, with whatever the platform brought and whatever the app had to add.
3925        ///
3926        /// The injected events matter as much as the given ones and are appended in the same
3927        /// order the real input hook appends them. A harness that skipped them would be testing
3928        /// a program that does not exist — which is how "the second drag does nothing" survived
3929        /// a suite that was green.
3930        fn frame(&mut self, events: Vec<Event>) {
3931            let mut events = events;
3932            events.extend(self.app.take_injected());
3933            self.time += 1.0 / 60.0;
3934            let input = RawInput {
3935                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
3936                time: Some(self.time),
3937                modifiers: self.modifiers,
3938                focused: self.focused,
3939                events,
3940                ..Default::default()
3941            };
3942            let app = &mut self.app;
3943            let out = self.ctx.run_ui(input, |ui| app.frame(ui));
3944            // Taken from the frame's own output rather than read back off the context
3945            // afterwards: `run_ui` moves the platform output out on the way through, so a test
3946            // that asked the context what the cursor was got whatever the *next* frame had
3947            // accumulated so far, which is nothing.
3948            self.cursor = out.platform_output.cursor_icon;
3949            // What the frame asked the platform to do to the window. Collected rather than
3950            // sampled, because the interesting thing about a pair of these is their order.
3951            for output in out.viewport_output.values() {
3952                self.commands.extend(output.commands.iter().cloned());
3953            }
3954            self.shapes.clear();
3955            self.shapes
3956                .extend(out.shapes.into_iter().map(|clipped| clipped.shape));
3957        }
3958
3959        /// Every rectangle the last frame filled, innermost last, as `(rect, corner, fill)`.
3960        ///
3961        /// Flattened out of the nested `Shape::Vec`s egui builds, because a shape's depth in
3962        /// that tree is an artefact of which `Ui` painted it.
3963        fn rects(&self) -> Vec<(Rect, egui::CornerRadius, egui::Color32)> {
3964            fn walk(shape: &egui::Shape, into: &mut Vec<(Rect, egui::CornerRadius, egui::Color32)>) {
3965                match shape {
3966                    egui::Shape::Rect(r) => into.push((r.rect, r.corner_radius, r.fill)),
3967                    egui::Shape::Vec(shapes) => {
3968                        for shape in shapes {
3969                            walk(shape, into);
3970                        }
3971                    }
3972                    _ => {}
3973                }
3974            }
3975            let mut out = Vec::new();
3976            for shape in &self.shapes {
3977                walk(shape, &mut out);
3978            }
3979            out
3980        }
3981
3982        /// Where the last frame put each run of text, as `(top-left, the string)`.
3983        ///
3984        /// The counterpart to [`Self::rects`], and what lets a test assert that a word is in the
3985        /// same place in two different frames without re-deriving where either frame put it.
3986        fn texts(&self) -> Vec<(egui::Pos2, String)> {
3987            fn walk(shape: &egui::Shape, into: &mut Vec<(egui::Pos2, String)>) {
3988                match shape {
3989                    egui::Shape::Text(text) => {
3990                        into.push((text.pos, text.galley.text().to_owned()));
3991                    }
3992                    egui::Shape::Vec(shapes) => {
3993                        for shape in shapes {
3994                            walk(shape, into);
3995                        }
3996                    }
3997                    _ => {}
3998                }
3999            }
4000            let mut out = Vec::new();
4001            for shape in &self.shapes {
4002                walk(shape, &mut out);
4003            }
4004            out
4005        }
4006
4007        /// Every run of text the last frame painted, as `(left edge and **baseline**, the string)`.
4008        ///
4009        /// The counterpart to [`Self::texts`], which gives where a galley's *box* was put. A box
4010        /// says nothing about whether two texts are level: two fonts have different line heights
4011        /// and different ascents, so galleys centred in the same rect end up with their baselines
4012        /// a point or two apart — which is exactly the fault that is invisible in the source and
4013        /// obvious on screen. This asks the question the eye asks.
4014        ///
4015        /// The x is the galley's left edge, unchanged, so a caller can pick out the runs inside
4016        /// one panel with `rect.contains`.
4017        fn baselines(&self) -> Vec<(Pos2, String)> {
4018            fn walk(shape: &egui::Shape, into: &mut Vec<(Pos2, String)>) {
4019                match shape {
4020                    egui::Shape::Text(text) => into.push((
4021                        pos2(
4022                            text.pos.x,
4023                            text.pos.y + azur::components::galley_baseline(&text.galley),
4024                        ),
4025                        text.galley.text().to_owned(),
4026                    )),
4027                    egui::Shape::Vec(shapes) => {
4028                        for shape in shapes {
4029                            walk(shape, into);
4030                        }
4031                    }
4032                    _ => {}
4033                }
4034            }
4035            let mut out = Vec::new();
4036            for shape in &self.shapes {
4037                walk(shape, &mut out);
4038            }
4039            out
4040        }
4041
4042        /// Every outlined rectangle the last frame painted, as `(rect, the stroke's colour)`.
4043        ///
4044        /// The counterpart to [`Self::rects`], for the things that are a border and not a fill —
4045        /// a field's outline says what it is with its edge, and a test that only looked at fills
4046        /// could not tell it from nothing at all.
4047        fn outlines(&self) -> Vec<(Rect, egui::Color32)> {
4048            fn walk(shape: &egui::Shape, into: &mut Vec<(Rect, egui::Color32)>) {
4049                match shape {
4050                    egui::Shape::Rect(r) if r.stroke.width > 0.0 => {
4051                        into.push((r.rect, r.stroke.color));
4052                    }
4053                    egui::Shape::Vec(shapes) => {
4054                        for shape in shapes {
4055                            walk(shape, into);
4056                        }
4057                    }
4058                    _ => {}
4059                }
4060            }
4061            let mut out = Vec::new();
4062            for shape in &self.shapes {
4063                walk(shape, &mut out);
4064            }
4065            out
4066        }
4067
4068        /// The fill of every polygon the last frame painted inside `rect`.
4069        ///
4070        /// How a painted glyph is asserted on: this program's icons are convex polygons, so
4071        /// "there is a glyph here, in this ink" is a question about the polygons whose ink lands
4072        /// in a given box.
4073        fn glyph_inks(&self, rect: Rect) -> Vec<egui::Color32> {
4074            self.glyph_shapes(rect)
4075                .into_iter()
4076                .map(|(_, fill)| fill)
4077                .collect()
4078        }
4079
4080        /// Where a glyph's ink actually is: the union of the polygons painted inside `rect`.
4081        ///
4082        /// The rect a glyph is *handed* says nothing about where the drawing inside it ended up,
4083        /// which is exactly how a glyph in a row comes out misaligned while every measurement in
4084        /// the source reads `center()`.
4085        fn glyph_bounds(&self, rect: Rect) -> Option<Rect> {
4086            self.glyph_shapes(rect)
4087                .into_iter()
4088                .map(|(bounds, _)| bounds)
4089                .reduce(|a, b| a.union(b))
4090        }
4091
4092        fn glyph_shapes(&self, rect: Rect) -> Vec<(Rect, egui::Color32)> {
4093            fn walk(shape: &egui::Shape, want: Rect, into: &mut Vec<(Rect, egui::Color32)>) {
4094                match shape {
4095                    egui::Shape::Path(path) => {
4096                        let bounds = path.visual_bounding_rect();
4097                        if want.contains_rect(bounds) {
4098                            into.push((bounds, path.fill));
4099                        }
4100                    }
4101                    egui::Shape::Vec(shapes) => {
4102                        for shape in shapes {
4103                            walk(shape, want, into);
4104                        }
4105                    }
4106                    _ => {}
4107                }
4108            }
4109            let mut out = Vec::new();
4110            for shape in &self.shapes {
4111                walk(shape, rect, &mut out);
4112            }
4113            out
4114        }
4115
4116        /// The last fill painted at `rect`, to within half a point on every edge.
4117        ///
4118        /// The *last*, because that is the one you can see.
4119        fn fill_at(&self, rect: Rect) -> Option<(egui::CornerRadius, egui::Color32)> {
4120            self.rects()
4121                .into_iter()
4122                .rev()
4123                .find(|(painted, _, _)| {
4124                    painted.min.distance(rect.min) < 0.5 && painted.max.distance(rect.max) < 0.5
4125                })
4126                .map(|(_, corner, fill)| (corner, fill))
4127        }
4128
4129        /// Run frames until the window stops asking for more, and say whether it did.
4130        ///
4131        /// A window with animations still running or icons still arriving asks for a repaint
4132        /// every frame, which makes "did anything ask for a repaint" an assertion that passes
4133        /// on its own. This is how a test gets a silent window to measure against.
4134        fn quiesce(&mut self) -> bool {
4135            for _ in 0..240 {
4136                self.frame(Vec::new());
4137                if !self.ctx.has_requested_repaint() {
4138                    self.take_journal();
4139                    return true;
4140                }
4141                self.time += 1.0 / 60.0;
4142            }
4143            false
4144        }
4145
4146        /// Let enough time pass that the next click starts a fresh gesture.
4147        fn wait(&mut self) {
4148            self.time += 1.0;
4149            self.frame(Vec::new());
4150        }
4151
4152        /// Move the pointer there and report whether `id` is the widget under it.
4153        ///
4154        /// This is the assertion that actually matters: a widget the pointer cannot
4155        /// reach is a widget that does not work, however correct its rect looks.
4156        fn hovers(&mut self, id: Id, at: Pos2) -> bool {
4157            self.frame(vec![Event::PointerMoved(at)]);
4158            self.ctx
4159                .read_response(id)
4160                .is_some_and(|response| response.hovered())
4161        }
4162
4163        /// Sweep down a column of the window looking for `id`, and return where it
4164        /// was found. Beats hard-coding a y that any layout change invalidates.
4165        fn find(&mut self, id: Id, x: f32, ys: std::ops::Range<i32>) -> Option<Pos2> {
4166            for y in ys.step_by(2) {
4167                let at = pos2(x, y as f32);
4168                if self.hovers(id, at) {
4169                    return Some(at);
4170                }
4171            }
4172            None
4173        }
4174
4175        /// Move, press, release — three frames, which is how a real click arrives.
4176        fn click_at(&mut self, at: Pos2) -> Vec<&'static str> {
4177            self.click_with(at, PointerButton::Primary, Modifiers::NONE)
4178        }
4179
4180        fn click_with(
4181            &mut self,
4182            at: Pos2,
4183            button: PointerButton,
4184            modifiers: Modifiers,
4185        ) -> Vec<&'static str> {
4186            self.take_journal();
4187            self.modifiers = modifiers;
4188            self.frame(vec![Event::PointerMoved(at)]);
4189            for pressed in [true, false] {
4190                self.frame(vec![Event::PointerButton {
4191                    pos: at,
4192                    button,
4193                    pressed,
4194                    modifiers,
4195                }]);
4196            }
4197            // One more, so an action queued by the release is applied and drawn.
4198            self.frame(Vec::new());
4199            self.modifiers = Modifiers::NONE;
4200            self.take_journal()
4201        }
4202
4203        fn double_click_at(&mut self, at: Pos2) -> Vec<&'static str> {
4204            // A fresh gesture: without this, the clicks of the previous one are still
4205            // inside egui's double-click window and these two count as a triple and a
4206            // quadruple.
4207            self.wait();
4208            self.take_journal();
4209            self.frame(vec![Event::PointerMoved(at)]);
4210            for _ in 0..2 {
4211                self.frame(vec![
4212                    Event::PointerButton {
4213                        pos: at,
4214                        button: PointerButton::Primary,
4215                        pressed: true,
4216                        modifiers: Modifiers::NONE,
4217                    },
4218                    Event::PointerButton {
4219                        pos: at,
4220                        button: PointerButton::Primary,
4221                        pressed: false,
4222                        modifiers: Modifiers::NONE,
4223                    },
4224                ]);
4225            }
4226            self.frame(Vec::new());
4227            self.take_journal()
4228        }
4229
4230        /// Press, travel, release.
4231        fn drag(&mut self, from: Pos2, to: Pos2) -> Vec<&'static str> {
4232            self.take_journal();
4233            self.frame(vec![Event::PointerMoved(from)]);
4234            self.frame(vec![Event::PointerButton {
4235                pos: from,
4236                button: PointerButton::Primary,
4237                pressed: true,
4238                modifiers: Modifiers::NONE,
4239            }]);
4240            // Several steps, because a drag threshold is a distance travelled.
4241            for step in 1..=6 {
4242                let t = step as f32 / 6.0;
4243                self.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
4244            }
4245            self.frame(vec![Event::PointerButton {
4246                pos: to,
4247                button: PointerButton::Primary,
4248                pressed: false,
4249                modifiers: Modifiers::NONE,
4250            }]);
4251            self.frame(Vec::new());
4252            self.take_journal()
4253        }
4254
4255        /// Press and travel, and never release — which is what an OLE drag looks like from
4256        /// here. `DoDragDrop` takes the capture and its own loop swallows the button-up, so
4257        /// the release genuinely never arrives.
4258        fn drag_and_hold(&mut self, from: Pos2, to: Pos2) -> Vec<&'static str> {
4259            self.take_journal();
4260            self.frame(vec![Event::PointerMoved(from)]);
4261            self.frame(vec![Event::PointerButton {
4262                pos: from,
4263                button: PointerButton::Primary,
4264                pressed: true,
4265                modifiers: Modifiers::NONE,
4266            }]);
4267            for step in 1..=6 {
4268                let t = step as f32 / 6.0;
4269                self.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
4270            }
4271            self.take_journal()
4272        }
4273
4274        fn take_journal(&mut self) -> Vec<&'static str> {
4275            let journal = self.app.journal.take().unwrap_or_default();
4276            self.app.journal = Some(Vec::new());
4277            journal
4278        }
4279
4280        fn pane_rect(&self, index: usize) -> Rect {
4281            self.app.panes[index].rect
4282        }
4283
4284        /// Where a pane's own content starts: its path bar, one hairline in.
4285        fn pane_content_top(&self, index: usize) -> f32 {
4286            self.pane_rect(index).top() + 1.0
4287        }
4288
4289        /// The vertical middle of the path bar of a pane.
4290        fn path_bar_y(&self, index: usize) -> f32 {
4291            self.pane_content_top(index) + crate::ui::breadcrumb::HEIGHT * 0.5
4292        }
4293
4294        /// The vertical middle of the column header of a pane.
4295        fn header_y(&self, index: usize) -> f32 {
4296            self.pane_content_top(index)
4297                + crate::ui::breadcrumb::HEIGHT
4298                + crate::ui::filelist::HEADER_HEIGHT * 0.5
4299        }
4300
4301        /// The middle of row `row` of a pane's listing.
4302        fn row_center(&self, index: usize, row: usize) -> Pos2 {
4303            let pane = self.pane_rect(index);
4304            let top = self.pane_content_top(index)
4305                + crate::ui::breadcrumb::HEIGHT
4306                + crate::ui::filelist::HEADER_HEIGHT;
4307            pos2(
4308                pane.center().x,
4309                top + crate::pane::ROW_HEIGHT * (row as f32 + 0.5),
4310            )
4311        }
4312
4313        fn tab(&self, index: usize) -> &Tab {
4314            self.app.panes[index].tab()
4315        }
4316    }
4317
4318
4319    /// Which widget is actually on top at a position, and which merely contain it.
4320    ///
4321    /// A blocked click is invisible in the source, so this asks egui directly: any id
4322    /// that `contains_pointer` but is not `hovered` has something over it, and the one
4323    /// that is `hovered` is what took the click.
4324    #[test]
4325    #[ignore = "diagnostic; run explicitly"]
4326    fn what_is_under_the_pointer() {
4327        let mut h = Harness::with_panes(1);
4328        let pane = h.app.panes[0].id;
4329        let rect = h.pane_rect(0);
4330        println!("window {:?}  pane {:?}", h.size, rect);
4331
4332        let probes: Vec<(String, Id)> = vec![
4333            ("rows-hit".into(), Id::new(("rows-hit", pane))),
4334            ("pane-claim".into(), Id::new(("pane-claim", pane))),
4335            ("caption-drag".into(), Id::new("caption-drag")),
4336            ("sidebar-grip".into(), Id::new("sidebar-grip")),
4337            ("app-menu".into(), Id::new("app-menu")),
4338            ("th0".into(), Id::new(("th", pane, 0usize))),
4339            ("th1".into(), Id::new(("th", pane, 1usize))),
4340            ("th2".into(), Id::new(("th", pane, 2usize))),
4341            ("th3".into(), Id::new(("th", pane, 3usize))),
4342            ("refresh".into(), Id::new(("refresh", pane))),
4343            ("bookmark".into(), Id::new(("bookmark-toggle", pane))),
4344            ("resize-n".into(), Id::new(("yafe-resize", "n"))),
4345            ("resize-s".into(), Id::new(("yafe-resize", "s"))),
4346            ("resize-e".into(), Id::new(("yafe-resize", "e"))),
4347            ("resize-w".into(), Id::new(("yafe-resize", "w"))),
4348            ("resize-nw".into(), Id::new(("yafe-resize", "nw"))),
4349            ("resize-ne".into(), Id::new(("yafe-resize", "ne"))),
4350            ("resize-sw".into(), Id::new(("yafe-resize", "sw"))),
4351            ("resize-se".into(), Id::new(("yafe-resize", "se"))),
4352        ];
4353
4354        let row_y = h.row_center(0, 0).y;
4355        let header_y = h.header_y(0);
4356        let bar_y = h.path_bar_y(0);
4357        let spots = [
4358            ("row left", pos2(rect.left() + 14.0, row_y)),
4359            ("row name", pos2(rect.left() + 120.0, row_y)),
4360            ("row middle", pos2(rect.center().x, row_y)),
4361            ("row right", pos2(rect.right() - 12.0, row_y)),
4362            ("header left", pos2(rect.left() + 60.0, header_y)),
4363            ("header right", pos2(rect.right() - 60.0, header_y)),
4364            ("path bar right", pos2(rect.right() - 30.0, bar_y)),
4365            ("title bar left", pos2(16.0, crate::ui::chrome::HEIGHT * 0.5)),
4366            ("sidebar row", pos2(crate::ui::GUTTER + 80.0, 360.0)),
4367        ];
4368
4369        for (label, at) in spots {
4370            h.frame(vec![Event::PointerMoved(at)]);
4371            let mut hovered = Vec::new();
4372            let mut blocked = Vec::new();
4373            for (name, id) in &probes {
4374                if let Some(r) = h.ctx.read_response(*id) {
4375                    if r.hovered() {
4376                        hovered.push(name.clone());
4377                    } else if r.contains_pointer() {
4378                        blocked.push(name.clone());
4379                    }
4380                }
4381            }
4382            println!(
4383                "{at:?} {label:<16} hovered={hovered:?}  blocked={blocked:?}"
4384            );
4385        }
4386
4387        println!("
4388--- rects ---");
4389        h.frame(vec![Event::PointerMoved(pos2(rect.center().x, row_y))]);
4390        for (name, id) in &probes {
4391            if let Some(r) = h.ctx.read_response(*id) {
4392                println!(
4393                    "{name:<14} rect={:?}  interact={:?}",
4394                    r.rect, r.interact_rect
4395                );
4396            }
4397        }
4398    }
4399
4400    #[test]
4401    fn the_harness_has_something_to_click() {
4402        let h = Harness::new();
4403        assert!(h.tab(0).dir.is_some(), "the listing has to have arrived");
4404        assert!(!h.tab(0).order.is_empty());
4405        assert!(h.pane_rect(0).width() > 200.0, "and the pane has to be laid out");
4406    }
4407
4408    // ---- The details view ----------------------------------------------
4409
4410    #[test]
4411    fn a_row_is_clickable_across_its_whole_width() {
4412        let mut h = Harness::new();
4413        let pane = h.pane_rect(0);
4414        let y = h.row_center(0, 0).y;
4415
4416        for (label, x) in [
4417            ("the glyph", pane.left() + 14.0),
4418            ("the name", pane.left() + 120.0),
4419            ("the middle", pane.center().x),
4420            ("the far right", pane.right() - 12.0),
4421        ] {
4422            h.app.panes[0].tab_mut().clear_selection();
4423            h.click_at(pos2(x, y));
4424            assert_eq!(
4425                h.tab(0).selected_count,
4426                1,
4427                "clicking {label} (x={x}) did not select the row"
4428            );
4429            assert_eq!(h.tab(0).cursor, Some(0), "and it is the first row");
4430        }
4431    }
4432
4433    #[test]
4434    fn double_clicking_anywhere_on_a_row_opens_it() {
4435        let mut h = Harness::new();
4436        assert!(h.tab(0).is_dir_at(0), "the first row should be a folder");
4437        let pane = h.pane_rect(0);
4438
4439        for x in [pane.left() + 120.0, pane.center().x, pane.right() - 12.0] {
4440            let done = h.double_click_at(pos2(x, h.row_center(0, 0).y));
4441            assert!(
4442                done.contains(&"Navigate"),
4443                "a double click at x={x} has to open the folder, got {done:?}"
4444            );
4445            h.app.panes[0].tab_mut().navigate(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
4446            h.settle();
4447            h.wait();
4448        }
4449    }
4450
4451    #[test]
4452    fn clicking_below_the_rows_clears_the_selection() {
4453        let mut h = Harness::new();
4454        h.click_at(h.row_center(0, 0));
4455        assert_eq!(h.tab(0).selected_count, 1);
4456
4457        let pane = h.pane_rect(0);
4458        h.click_at(pos2(pane.center().x, pane.bottom() - 40.0));
4459        assert_eq!(
4460            h.tab(0).selected_count,
4461            0,
4462            "empty space below the rows cancels a selection"
4463        );
4464    }
4465
4466    #[test]
4467    fn ctrl_click_adds_to_the_selection() {
4468        let mut h = Harness::new();
4469        h.click_at(h.row_center(0, 0));
4470        h.click_with(
4471            h.row_center(0, 2),
4472            PointerButton::Primary,
4473            Modifiers::COMMAND,
4474        );
4475        assert_eq!(h.tab(0).selected_count, 2);
4476    }
4477
4478    #[test]
4479    fn shift_click_selects_a_range() {
4480        let mut h = Harness::new();
4481        h.click_at(h.row_center(0, 0));
4482        h.click_with(h.row_center(0, 3), PointerButton::Primary, Modifiers::SHIFT);
4483        assert_eq!(h.tab(0).selected_count, 4);
4484    }
4485
4486    #[test]
4487    fn a_column_header_sorts() {
4488        let mut h = Harness::new();
4489        let pane = h.pane_rect(0);
4490        let before = (h.tab(0).sort_by, h.tab(0).ascending);
4491        let done = h.click_at(pos2(pane.left() + 60.0, h.header_y(0)));
4492        assert!(done.contains(&"Sort"), "the Name header has to sort, got {done:?}");
4493        assert_ne!(
4494            (h.tab(0).sort_by, h.tab(0).ascending),
4495            before,
4496            "and the order has to change"
4497        );
4498    }
4499
4500    #[test]
4501    fn every_column_header_is_reachable() {
4502        let mut h = Harness::new();
4503        let y = h.header_y(0);
4504        for (index, column) in crate::fs::Column::ALL.into_iter().enumerate() {
4505            let id = Id::new(("th", h.app.panes[0].id, index));
4506            let pane = h.pane_rect(0);
4507            let found = (0..pane.width() as i32)
4508                .step_by(4)
4509                .map(|dx| pos2(pane.left() + dx as f32, y))
4510                .find(|at| h.hovers(id, *at));
4511            assert!(
4512                found.is_some(),
4513                "the {} header is not reachable by the pointer",
4514                column.header()
4515            );
4516        }
4517    }
4518
4519    // ---- The path bar ---------------------------------------------------
4520
4521    #[test]
4522    fn the_history_buttons_respond() {
4523        let mut h = Harness::new();
4524        let y = h.path_bar_y(0);
4525        let pane = h.app.panes[0].id;
4526        let id = Id::new(("nav", pane, "Up (Alt+Up)"));
4527        let left = h.pane_rect(0).left();
4528        let at = (0..120)
4529            .step_by(2)
4530            .map(|dx| pos2(left + dx as f32, y))
4531            .find(|at| h.hovers(id, *at))
4532            .expect("the Up button is not reachable by the pointer");
4533        let done = h.click_at(at);
4534        assert!(done.contains(&"Up"), "Up did not respond, got {done:?}");
4535    }
4536
4537    /// Refresh is in the group at the left, and the star that was beside the filter is gone.
4538    ///
4539    /// Both halves matter. Refresh moved *into* the never-dropped group, so it is found by
4540    /// scanning from the pane's left edge and its id is the group's — a test still looking for
4541    /// `("refresh", pane)` out on the right would pass on a bar that had lost the button
4542    /// entirely. And a button removed from a toolbar has to leave its *function* reachable, or
4543    /// "we tidied the bar" means "we deleted the feature": `Ctrl+D` is checked here for that
4544    /// reason, not for the shortcut's own sake.
4545    #[test]
4546    fn refresh_is_beside_up_and_the_bookmark_star_is_gone() {
4547        let mut h = Harness::new();
4548        let y = h.path_bar_y(0);
4549        let pane = h.app.panes[0].id;
4550        let left = h.pane_rect(0).left();
4551
4552        let id = Id::new(("nav", pane, "Refresh (F5)"));
4553        let at = (0..140)
4554            .step_by(2)
4555            .map(|dx| pos2(left + dx as f32, y))
4556            .find(|at| h.hovers(id, *at))
4557            .expect("Refresh is not reachable from the left-hand group");
4558        let done = h.click_at(at);
4559        assert!(done.contains(&"Refresh"), "Refresh did not fire, got {done:?}");
4560
4561        // Immediately after Up: the buttons touch, so this is a claim about the two rects and not
4562        // about wherever the sweep above happened to land.
4563        let rect_of = |h: &Harness, tip: &str| {
4564            h.ctx
4565                .read_response(Id::new(("nav", pane, tip)))
4566                .map(|r| r.rect)
4567                .unwrap_or_else(|| panic!("the {tip} button was not laid out"))
4568        };
4569        let up = rect_of(&h, "Up (Alt+Up)");
4570        let refresh = rect_of(&h, "Refresh (F5)");
4571        assert_eq!(
4572            refresh.left(),
4573            up.right(),
4574            "Refresh starts at {} and Up ends at {}",
4575            refresh.left(),
4576            up.right()
4577        );
4578
4579        h.frame(Vec::new());
4580        assert!(
4581            h.ctx
4582                .read_response(Id::new(("bookmark-toggle", pane)))
4583                .is_none(),
4584            "the bookmark star is still on the bar"
4585        );
4586
4587        // And the thing it used to do is still done. The modifiers go on the harness as well as
4588        // on the event, because the code under test reads `InputState::modifiers`.
4589        h.take_journal();
4590        h.modifiers = Modifiers::COMMAND;
4591        h.frame(vec![Event::Key {
4592            key: egui::Key::D,
4593            physical_key: None,
4594            pressed: true,
4595            repeat: false,
4596            modifiers: Modifiers::COMMAND,
4597        }]);
4598        h.modifiers = Modifiers::NONE;
4599        h.frame(Vec::new());
4600        let done = h.take_journal();
4601        assert!(
4602            done.contains(&"ToggleBookmark"),
4603            "Ctrl+D no longer bookmarks, so removing the star removed the feature: {done:?}"
4604        );
4605    }
4606
4607    #[test]
4608    #[ignore = "diagnostic; run explicitly"]
4609    fn where_are_the_crumbs() {
4610        let mut h = Harness::new();
4611        let pane = h.app.panes[0].id;
4612        let crumbs = crate::fs::breadcrumb_segments(&h.tab(0).trail);
4613        println!(
4614            "path={:?}\ntrail={:?}\nactive={} of {}",
4615            h.tab(0).path,
4616            h.tab(0).trail,
4617            crate::ui::breadcrumb::active_index(&crumbs, &h.tab(0).path),
4618            crumbs.len()
4619        );
4620        let y = h.path_bar_y(0);
4621        h.frame(vec![Event::PointerMoved(pos2(h.pane_rect(0).left() + 40.0, y))]);
4622        for (index, (label, _)) in crumbs.iter().enumerate() {
4623            let rect = h.ctx.read_response(Id::new(("crumb", pane, index))).map(|r| r.rect);
4624            println!("  {index} {label:<32} {rect:?}");
4625        }
4626        println!(
4627            "  overflow {:?}\n  pane {:?}  bar_y {y}",
4628            h.ctx
4629                .read_response(Id::new(("crumb-overflow", pane)))
4630                .map(|r| r.rect),
4631            h.pane_rect(0)
4632        );
4633    }
4634
4635    #[test]
4636    fn a_breadcrumb_segment_navigates() {
4637        let mut h = Harness::new();
4638        let y = h.path_bar_y(0);
4639        let pane = h.app.panes[0].id;
4640
4641        // The parent of the folder being shown, found rather than assumed: which segments
4642        // are on the bar depends on how much of the path fits, and the leading ones collapse
4643        // into the `…`. This one is next to the current folder, so it is there whenever
4644        // anything is. (It used to look for segment 0, This PC, on the grounds that it is
4645        // always present — which stopped being true the day the path bar got 27px narrower.)
4646        let crumbs = crate::fs::breadcrumb_segments(&h.tab(0).trail);
4647        let parent = crate::ui::breadcrumb::active_index(&crumbs, &h.tab(0).path) - 1;
4648        let id = Id::new(("crumb", pane, parent));
4649        let left = h.pane_rect(0).left();
4650        let at = (0..900)
4651            .step_by(2)
4652            .map(|dx| pos2(left + dx as f32, y))
4653            .find(|at| h.hovers(id, *at))
4654            .expect("the parent breadcrumb segment is not reachable");
4655
4656        let done = h.click_at(at);
4657        assert!(
4658            done.contains(&"Navigate"),
4659            "a breadcrumb segment has to navigate, got {done:?}"
4660        );
4661        assert_eq!(
4662            h.tab(0).path,
4663            crumbs[parent].1,
4664            "and it has to go to the folder it names"
4665        );
4666    }
4667
4668    #[test]
4669    fn after_going_up_the_folder_left_is_still_on_the_breadcrumb() {
4670        // The whole point of `Tab::trail`: walk up, and the folder just left is a segment
4671        // you can click rather than a name to go hunting for in a chevron menu.
4672        let mut h = Harness::new();
4673        let pane = h.app.panes[0].id;
4674        let was = h.tab(0).path.clone();
4675
4676        h.app.panes[0].tab_mut().go_up();
4677        h.settle();
4678        assert_eq!(h.tab(0).trail, was, "the trail has to outlive the move");
4679
4680        // The segment for the folder we came out of is the last one on the bar, past the
4681        // one now in bold.
4682        let crumbs = crate::fs::breadcrumb_segments(&h.tab(0).trail);
4683        let deepest = crumbs.len() - 1;
4684        assert_eq!(crumbs[deepest].1, was);
4685        assert!(
4686            crate::ui::breadcrumb::active_index(&crumbs, &h.tab(0).path) < deepest,
4687            "the bold segment should be an ancestor of the end of the trail"
4688        );
4689
4690        // And it is reachable by the pointer and navigates -- the two things a segment
4691        // drawn in the right place can still fail to do.
4692        let y = h.path_bar_y(0);
4693        let left = h.pane_rect(0).left();
4694        let at = (0..900)
4695            .step_by(2)
4696            .map(|dx| pos2(left + dx as f32, y))
4697            .find(|at| h.hovers(Id::new(("crumb", pane, deepest)), *at))
4698            .expect("the segment past the current folder cannot be reached");
4699        let done = h.click_at(at);
4700        assert!(
4701            done.contains(&"Navigate"),
4702            "clicking back down the trail has to navigate, got {done:?}"
4703        );
4704        assert_eq!(h.tab(0).path, was, "and it goes back where it came from");
4705    }
4706
4707    // ---- The title bar --------------------------------------------------
4708
4709    #[test]
4710    fn the_application_mark_is_reachable() {
4711        let mut h = Harness::new();
4712        let found = h.find(
4713            Id::new("app-menu"),
4714            crate::ui::GUTTER + crate::ui::TOOL_SIZE * 0.5,
4715            0..crate::ui::chrome::HEIGHT as i32,
4716        );
4717        assert!(
4718            found.is_some(),
4719            "the application mark cannot be reached -- something is covering the \
4720             top-left corner of the title bar"
4721        );
4722    }
4723
4724    #[test]
4725    fn the_window_buttons_are_reachable() {
4726        let mut h = Harness::new();
4727        for (which, from_right) in [
4728            (WindowAction::Close, 23.0),
4729            (WindowAction::ToggleMaximize, 69.0),
4730            (WindowAction::Minimize, 115.0),
4731        ] {
4732            let id = Id::new(("caption", which as u8));
4733            let found = h.find(
4734                id,
4735                h.size.x - from_right,
4736                0..crate::ui::chrome::HEIGHT as i32,
4737            );
4738            assert!(
4739                found.is_some(),
4740                "{which:?} cannot be reached -- the top-right corner is covered"
4741            );
4742        }
4743    }
4744
4745    #[test]
4746    fn a_tab_and_its_close_button_are_clickable() {
4747        let mut h = Harness::new();
4748        h.app.panes[0]
4749            .tabs
4750            .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
4751        h.app.panes[0].active = 1;
4752        h.settle();
4753
4754        let pane = h.app.panes[0].id;
4755        let strip_y = crate::ui::chrome::HEIGHT * 0.5;
4756
4757        let at = (0..500)
4758            .step_by(2)
4759            .map(|dx| pos2(dx as f32, strip_y))
4760            .find(|at| h.hovers(Id::new(("tab", pane, 0usize)), *at))
4761            .expect("the first tab is not reachable");
4762        let done = h.click_at(at);
4763        assert!(
4764            done.contains(&"ActivateTab"),
4765            "a tab did not respond, got {done:?}"
4766        );
4767        assert_eq!(h.app.panes[0].active, 0);
4768
4769        let close = (0..500)
4770            .step_by(2)
4771            .map(|dx| pos2(dx as f32, strip_y))
4772            .find(|at| h.hovers(Id::new(("tab-close", pane, 0usize)), *at))
4773            .expect("a tab's close button is not reachable");
4774        let done = h.click_at(close);
4775        assert!(
4776            done.contains(&"CloseTab"),
4777            "the close button did not fire, got {done:?}"
4778        );
4779        assert_eq!(h.app.panes[0].tabs.len(), 1);
4780    }
4781
4782    #[test]
4783    fn the_new_tab_button_makes_a_tab() {
4784        let mut h = Harness::new();
4785        let pane = h.app.panes[0].id;
4786        let before = h.app.panes[0].tabs.len();
4787        let at = (0..600)
4788            .step_by(2)
4789            .map(|dx| pos2(dx as f32, crate::ui::chrome::HEIGHT * 0.5))
4790            .find(|at| h.hovers(Id::new(("new-tab", pane)), *at))
4791            .expect("the + button is not reachable");
4792        let done = h.click_at(at);
4793        assert!(done.contains(&"NewTab"), "the + did not fire, got {done:?}");
4794        assert_eq!(h.app.panes[0].tabs.len(), before + 1);
4795    }
4796
4797    /// One hover grey, everywhere, whatever kind of thing is under the pointer.
4798    ///
4799    /// The window had two for a while and it was not obvious which: the rows, the sidebar and the
4800    /// path bar's segments moved to this program's own grey, and Back, Forward, Up and Refresh went
4801    /// on wearing Azur's `control-hover` because they reach it through `ui::control_fills` rather
4802    /// than through `ui::row_fill`. Four buttons in the middle of the window, a rung and a half off
4803    /// everything around them. The fix was to stop having two sources — `background-control-hover`
4804    /// is set by `crate::theme` and everything reads it — and this is the guard, over four widgets
4805    /// that get there by four different routes.
4806    #[test]
4807    fn every_hover_in_the_window_is_the_same_grey() {
4808        let mut h = Harness::new();
4809        let pane = h.app.panes[0].id;
4810        let grey = crate::ui::hover_fill(&h.app.theme);
4811        let bar = h.path_bar_y(0);
4812        let strip = crate::ui::chrome::HEIGHT * 0.5;
4813
4814        // `Back` and `Forward` are disabled in a tab that has been nowhere, and a disabled button
4815        // paints no fill at all — so the two of the four that are always live stand for them.
4816        let widgets: [(Id, f32, &str); 4] = [
4817            (Id::new(("nav", pane, "Up (Alt+Up)")), bar, "Up"),
4818            (Id::new(("nav", pane, "Refresh (F5)")), bar, "Refresh"),
4819            (Id::new(("th", pane, 0usize)), h.header_y(0), "a column header"),
4820            (Id::new(("new-tab", pane)), strip, "the new-tab button"),
4821        ];
4822        for (id, y, what) in widgets {
4823            let at = (0..1024)
4824                .step_by(2)
4825                .map(|x| pos2(x as f32, y))
4826                .find(|at| h.hovers(id, *at))
4827                .unwrap_or_else(|| panic!("{what} is not reachable at y {y}"));
4828            let _ = at;
4829            let rect = h
4830                .ctx
4831                .read_response(id)
4832                .unwrap_or_else(|| panic!("{what} was not drawn"))
4833                .rect;
4834            assert_eq!(
4835                h.fill_at(rect).map(|(_, fill)| fill),
4836                Some(grey),
4837                "{what} hovers in a grey of its own"
4838            );
4839        }
4840    }
4841
4842    #[test]
4843    fn losing_the_window_puts_the_path_bar_and_the_context_menu_away() {
4844        let mut h = Harness::new();
4845        let pane = h.app.panes[0].id;
4846
4847        // The path bar's dropdown, opened the way anyone opens it.
4848        let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
4849        let index = crumbs.len() - 1;
4850        let y = h.path_bar_y(0);
4851        let at = (0..1200)
4852            .step_by(2)
4853            .map(|x| pos2(x as f32, y))
4854            .find(|at| h.hovers(Id::new(("crumb-chevron", pane, index)), *at))
4855            .expect("the chevron before the current folder is not reachable");
4856        h.wait();
4857        h.click_at(at);
4858        h.frame(Vec::new());
4859        assert_eq!(h.app.crumbs.showing(), Some((pane, index)), "nothing opened");
4860
4861        // And a context menu. Built from this program's own entries, the way a right-button drag
4862        // builds one, so that opening it does not involve asking the shell anything.
4863        let own = vec![crate::shell::menu::Entry::own(
4864            crate::shell::menu::Own::Cancel,
4865        )];
4866        h.app.menu = Some(crate::ui::menu::Open::new(
4867            pane,
4868            pos2(200.0, 300.0),
4869            Vec::new(),
4870            PathBuf::new(),
4871            own,
4872            0,
4873        ));
4874
4875        h.focused = false;
4876        h.frame(Vec::new());
4877
4878        assert!(h.app.menu.is_none(), "the context menu is still up");
4879        assert_eq!(
4880            h.app.crumbs.showing(),
4881            None,
4882            "the path bar's dropdown is still up, and with it the tracking mode"
4883        );
4884    }
4885
4886    #[test]
4887    fn losing_the_window_puts_the_application_menu_away() {
4888        // The one at the top left, which egui tracks in its own memory rather than this program
4889        // — so it needs `Popup::close_all` and would not be caught by the test above.
4890        let mut h = Harness::new();
4891        let at = (0..64)
4892            .step_by(2)
4893            .map(|x| pos2(x as f32, crate::ui::chrome::HEIGHT * 0.5))
4894            .find(|at| h.hovers(Id::new("app-menu"), *at))
4895            .expect("the application mark is not reachable");
4896        h.click_at(at);
4897        h.frame(Vec::new());
4898        assert!(
4899            egui::Popup::is_any_open(&h.ctx),
4900            "the mark did not open its menu"
4901        );
4902
4903        h.focused = false;
4904        h.frame(Vec::new());
4905        assert!(
4906            !egui::Popup::is_any_open(&h.ctx),
4907            "the application menu is still open"
4908        );
4909    }
4910
4911    #[test]
4912    fn ctrl_shift_t_puts_a_tab_back_and_does_not_also_open_a_new_one() {
4913        // Both of these hang off `Ctrl` and the same key, so the interesting half of the
4914        // assertion is the one about `NewTab` *not* being in the journal: without the `Shift`
4915        // test on the other arm, this gesture reopened the closed tab and opened a blank one
4916        // beside it, and only a test that drives the real keyboard path can see that.
4917        let mut h = Harness::new();
4918        let pane = h.app.panes[0].id;
4919        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
4920        h.app.panes[0].tabs.push(Tab::new(path.clone()));
4921        h.app.panes[0].active = 1;
4922        h.settle();
4923
4924        h.app
4925            .perform(&h.ctx.clone(), Action::CloseTab { pane, tab: 1 });
4926        assert_eq!(h.app.panes[0].tabs.len(), 1);
4927
4928        h.take_journal();
4929        let held = Modifiers::COMMAND | Modifiers::SHIFT;
4930        h.modifiers = held;
4931        h.frame(vec![Event::Key {
4932            key: egui::Key::T,
4933            physical_key: None,
4934            pressed: true,
4935            repeat: false,
4936            modifiers: held,
4937        }]);
4938        h.modifiers = Modifiers::NONE;
4939        h.frame(Vec::new());
4940        let done = h.take_journal();
4941
4942        assert!(
4943            done.contains(&"ReopenTab"),
4944            "Ctrl+Shift+T did nothing, got {done:?}"
4945        );
4946        assert!(
4947            !done.contains(&"NewTab"),
4948            "Ctrl+Shift+T opened a blank tab as well, got {done:?}"
4949        );
4950        assert_eq!(h.app.panes[0].tabs.len(), 2);
4951        assert_eq!(h.app.panes[0].tabs[1].path, path);
4952    }
4953
4954    // ---- The sidebar ----------------------------------------------------
4955
4956    #[test]
4957    fn a_sidebar_place_navigates() {
4958        let mut h = Harness::new();
4959        let at = h
4960            .find(Id::new(("place", "Home")), crate::ui::GUTTER + 80.0, 40..700)
4961            .expect("the Home row is not reachable by the pointer");
4962        let done = h.click_at(at);
4963        assert!(
4964            done.contains(&"Navigate"),
4965            "a sidebar place did not navigate, got {done:?}"
4966        );
4967    }
4968
4969    #[test]
4970    fn a_sidebar_group_header_folds_it() {
4971        let mut h = Harness::new();
4972        let before = h.app.sections.drives;
4973        let at = h
4974            .find(
4975                Id::new(("sidebar-group", "Drives")),
4976                crate::ui::GUTTER + 80.0,
4977                30..200,
4978            )
4979            .expect("the Drives heading is not reachable");
4980        h.click_at(at);
4981        assert_ne!(
4982            h.app.sections.drives, before,
4983            "a group heading has to fold its rows"
4984        );
4985    }
4986
4987    #[test]
4988    fn a_drive_row_navigates() {
4989        let mut h = Harness::new();
4990        let letter = h
4991            .app
4992            .volumes
4993            .all()
4994            .first()
4995            .map(|d| d.letter.clone())
4996            .expect("this machine has at least one volume");
4997        let at = h
4998            .find(Id::new(("drive", &letter)), crate::ui::GUTTER + 100.0, 40..300)
4999            .expect("no drive row is reachable by the pointer");
5000        let done = h.click_at(at);
5001        assert!(
5002            done.contains(&"Navigate"),
5003            "a drive row did not navigate, got {done:?}"
5004        );
5005    }
5006
5007    #[test]
5008    fn every_sidebar_place_is_reachable() {
5009        let mut h = Harness::new();
5010        let labels: Vec<String> = h.app.places.iter().map(|p| p.label.clone()).collect();
5011        for label in labels {
5012            let found = h.find(
5013                Id::new(("place", &label)),
5014                crate::ui::GUTTER + 80.0,
5015                30..760,
5016            );
5017            assert!(found.is_some(), "the `{label}` row is not reachable");
5018        }
5019    }
5020
5021    #[test]
5022    fn hovering_a_drive_puts_its_free_space_in_a_tooltip() {
5023        // The free-space numbers left the row and became a tooltip, so the tooltip is now
5024        // the only place they exist. Whether one actually appears is not something the
5025        // source shows: it needs a real hover, which is what this harness is for.
5026        let mut h = Harness::new();
5027        let letter = h
5028            .app
5029            .volumes
5030            .all()
5031            .first()
5032            .expect("a machine has a drive")
5033            .letter
5034            .clone();
5035        let at = h
5036            .find(Id::new(("drive", &letter)), crate::ui::GUTTER + 80.0, 30..300)
5037            .unwrap_or_else(|| panic!("the `{letter}` row is not reachable"));
5038
5039        // A tooltip is an area in its own layer order, so this asks egui whether one is up
5040        // rather than hunting for the text.
5041        let shown = |h: &Harness| {
5042            h.ctx.memory(|m| {
5043                m.areas()
5044                    .visible_layer_ids()
5045                    .iter()
5046                    .any(|layer| layer.order == egui::Order::Tooltip)
5047            })
5048        };
5049        assert!(!shown(&h), "a tooltip is up before anything was hovered");
5050
5051        // Held still, not moved repeatedly: egui delays a tooltip until the pointer has
5052        // stopped, so a test that re-sends `PointerMoved` every frame resets the timer and
5053        // waits for ever. Move once, then let time pass.
5054        h.frame(vec![Event::PointerMoved(at)]);
5055        for _ in 0..20 {
5056            h.time += 0.25;
5057            h.frame(Vec::new());
5058            if shown(&h) {
5059                return;
5060            }
5061        }
5062        panic!("hovering the `{letter}` drive row shows no tooltip");
5063    }
5064
5065
5066    // ---- The rubber band ------------------------------------------------
5067
5068    #[test]
5069    fn dragging_from_empty_space_bands_over_rows() {
5070        let mut h = Harness::new();
5071        let pane = h.pane_rect(0);
5072        let rows = h.tab(0).order.len();
5073        assert!(rows >= 4, "need a few rows to band over");
5074
5075        // Start below the last row and drag up across the first three.
5076        let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
5077        let up_to = h.row_center(0, 2);
5078        h.drag(below, up_to);
5079
5080        assert_eq!(
5081            h.tab(0).selected_count,
5082            rows - 2,
5083            "the band has to select every row it crossed"
5084        );
5085        assert!(
5086            h.tab(0).band.is_none(),
5087            "and let go of the band when the button comes up"
5088        );
5089    }
5090
5091    #[test]
5092    fn a_band_shrinking_back_deselects() {
5093        let mut h = Harness::new();
5094        let pane = h.pane_rect(0);
5095        let rows = h.tab(0).order.len();
5096        let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
5097
5098        h.wait();
5099        h.frame(vec![Event::PointerMoved(below)]);
5100        h.frame(vec![Event::PointerButton {
5101            pos: below,
5102            button: PointerButton::Primary,
5103            pressed: true,
5104            modifiers: Modifiers::NONE,
5105        }]);
5106        // Out to the top of the list...
5107        h.frame(vec![Event::PointerMoved(h.row_center(0, 0))]);
5108        let wide = h.tab(0).selected_count;
5109        assert!(wide > 1, "the band should have caught several rows");
5110        // ...and back down to just below the last row.
5111        h.frame(vec![Event::PointerMoved(below)]);
5112        assert!(
5113            h.tab(0).selected_count < wide,
5114            "pulling the band back has to let rows go again, not keep them"
5115        );
5116        h.frame(vec![Event::PointerButton {
5117            pos: below,
5118            button: PointerButton::Primary,
5119            pressed: false,
5120            modifiers: Modifiers::NONE,
5121        }]);
5122    }
5123
5124    #[test]
5125    fn ctrl_dragging_a_band_keeps_what_was_selected() {
5126        let mut h = Harness::new();
5127        let pane = h.pane_rect(0);
5128        let rows = h.tab(0).order.len();
5129
5130        // Select the last row on its own first.
5131        h.click_at(h.row_center(0, rows - 1));
5132        assert_eq!(h.tab(0).selected_count, 1);
5133
5134        // Then Ctrl-band across the first two, which must not lose it.
5135        let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
5136        h.wait();
5137        h.modifiers = Modifiers::COMMAND;
5138        h.frame(vec![Event::PointerMoved(below)]);
5139        h.frame(vec![Event::PointerButton {
5140            pos: below,
5141            button: PointerButton::Primary,
5142            pressed: true,
5143            modifiers: Modifiers::COMMAND,
5144        }]);
5145        h.frame(vec![Event::PointerMoved(h.row_center(0, rows - 2))]);
5146        h.frame(vec![Event::PointerButton {
5147            pos: below,
5148            button: PointerButton::Primary,
5149            pressed: false,
5150            modifiers: Modifiers::COMMAND,
5151        }]);
5152        h.modifiers = Modifiers::NONE;
5153        h.frame(Vec::new());
5154
5155        assert!(
5156            h.tab(0).selected_count >= 2,
5157            "a Ctrl band adds to the selection instead of replacing it"
5158        );
5159    }
5160
5161    #[test]
5162    fn a_plain_band_replaces_the_selection() {
5163        let mut h = Harness::new();
5164        let pane = h.pane_rect(0);
5165        let rows = h.tab(0).order.len();
5166
5167        h.click_at(h.row_center(0, 0));
5168        let below = pos2(pane.center().x, h.row_center(0, rows + 2).y);
5169        h.drag(below, h.row_center(0, rows - 1));
5170
5171        // Only the last row was crossed, so the first one is no longer selected.
5172        assert!(!h.tab(0).is_selected(0), "a plain band starts from nothing");
5173        assert!(h.tab(0).is_selected(rows - 1));
5174    }
5175
5176    // ---- Panes ----------------------------------------------------------
5177
5178    #[test]
5179    fn clicking_a_pane_focuses_it() {
5180        let mut h = Harness::with_panes(2);
5181        assert_eq!(h.app.panes.len(), 2);
5182        let second = h.app.panes[1].id;
5183        h.app.focused = h.app.panes[0].id;
5184
5185        h.click_at(h.row_center(1, 0));
5186        assert_eq!(
5187            h.app.focused, second,
5188            "clicking in a pane has to move the keyboard there"
5189        );
5190    }
5191
5192    #[test]
5193    fn dragging_a_tab_onto_a_pane_edge_splits_it() {
5194        let mut h = Harness::new();
5195        h.app.panes[0]
5196            .tabs
5197            .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
5198        h.settle();
5199
5200        let pane_id = h.app.panes[0].id;
5201        let strip_y = crate::ui::chrome::HEIGHT * 0.5;
5202        let from = (0..500)
5203            .step_by(2)
5204            .map(|dx| pos2(dx as f32, strip_y))
5205            .find(|at| h.hovers(Id::new(("tab", pane_id, 1usize)), *at))
5206            .expect("the second tab is not reachable");
5207
5208        let pane = h.pane_rect(0);
5209        let to = pos2(pane.right() - 12.0, pane.center().y);
5210        let done = h.drag(from, to);
5211
5212        assert!(
5213            done.contains(&"BeginTabDrag"),
5214            "the drag never started, got {done:?}"
5215        );
5216        assert!(
5217            done.contains(&"SplitTab"),
5218            "dropping a tab on a pane's right edge has to split it, got {done:?}"
5219        );
5220        assert_eq!(h.app.layout.count(), 2);
5221    }
5222
5223    #[test]
5224    fn a_breadcrumb_chevron_opens_the_folder_it_points_at() {
5225        // It could not, and nothing about the code said so: the dropdown was built only
5226        // when it was *already* open, and the thing that opens it is the same call that
5227        // builds it. So this asserts the popup is open after the click and that the folder
5228        // behind it was actually read.
5229        let mut h = Harness::new();
5230        let pane = h.app.panes[0].id;
5231        let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
5232        assert!(crumbs.len() > 2, "the test folder has a path to walk");
5233
5234        // The chevron before the last segment: its menu lists the parent's subfolders,
5235        // one of which is the folder now open.
5236        let index = crumbs.len() - 1;
5237        let id = Id::new(("crumb-chevron", pane, index));
5238        let y = h.path_bar_y(0);
5239        let at = (0..1200)
5240            .step_by(2)
5241            .map(|x| pos2(x as f32, y))
5242            .find(|at| h.hovers(id, *at))
5243            .expect("the chevron before the current folder is not reachable");
5244
5245        h.wait();
5246        h.click_at(at);
5247        h.frame(Vec::new());
5248
5249        assert_eq!(
5250            h.app.crumbs.showing(),
5251            Some((pane, index)),
5252            "clicking the chevron did not open anything"
5253        );
5254        let (read, count) = h
5255            .app
5256            .crumbs
5257            .listing()
5258            .expect("the dropdown never read a folder");
5259        assert_eq!(read, crumbs[index - 1].1, "it read the wrong folder");
5260        assert!(count > 0, "this crate's parent has subfolders in it");
5261    }
5262
5263    /// An open chevron is welded to the segment before it: one fill over both.
5264    ///
5265    /// They are one control — the chevron lists that folder's subfolders, so the pair reads
5266    /// "this folder, and what is inside it" — and a fill each would put a seam down the middle
5267    /// of it. The rect is what makes this checkable: no arrangement of two fills lands a single
5268    /// rectangle on the union of the two.
5269    #[test]
5270    fn an_open_chevron_and_the_name_before_it_are_highlighted_together() {
5271        let mut h = Harness::new();
5272        let pane = h.app.panes[0].id;
5273        let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
5274        let index = crumbs.len() - 1;
5275
5276        let y = h.path_bar_y(0);
5277        let at = (0..1200)
5278            .step_by(2)
5279            .map(|x| pos2(x as f32, y))
5280            .find(|at| h.hovers(Id::new(("crumb-chevron", pane, index)), *at))
5281            .expect("the chevron before the current folder is not reachable");
5282        h.wait();
5283        h.click_at(at);
5284        h.frame(Vec::new());
5285        assert_eq!(h.app.crumbs.showing(), Some((pane, index)), "nothing opened");
5286
5287        let of = |id: Id| {
5288            h.ctx
5289                .read_response(id)
5290                .map(|r| r.rect)
5291                .unwrap_or_else(|| panic!("{id:?} was not drawn"))
5292        };
5293        let text = of(Id::new(("crumb", pane, index - 1)));
5294        let chevron = of(Id::new(("crumb-chevron", pane, index)));
5295
5296        // In the hover grey, not the pressed one: the pair, the dropdown's own entries and a
5297        // plain hover on the bar are one gesture and wear one colour.
5298        assert_eq!(
5299            h.fill_at(text.union(chevron)).map(|(_, fill)| fill),
5300            Some(crate::ui::hover_fill(&h.app.theme)),
5301            "the open chevron and the name before it are not one shape"
5302        );
5303    }
5304
5305    /// Once a dropdown is open the whole bar is one control: the pointer carries it along.
5306    ///
5307    /// Explorer's address bar does this, and it is the difference between reading a menu to find
5308    /// a sibling folder and running the pointer along the trail until the right list appears.
5309    /// Hovering a *name* opens the chevron that belongs to it — the one after it, which lists
5310    /// that folder's subfolders — so the name and its chevron are one target in both directions.
5311    #[test]
5312    fn with_a_dropdown_open_hovering_the_bar_moves_it() {
5313        let mut h = Harness::new();
5314        let pane = h.app.panes[0].id;
5315        let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
5316        assert!(crumbs.len() > 3, "the test folder has a path to walk");
5317        let index = crumbs.len() - 1;
5318
5319        let y = h.path_bar_y(0);
5320        let sweep = |h: &mut Harness, id: Id| {
5321            (0..1200)
5322                .step_by(2)
5323                .map(|x| pos2(x as f32, y))
5324                .find(|at| h.hovers(id, *at))
5325                .unwrap_or_else(|| panic!("{id:?} is not reachable"))
5326        };
5327
5328        // Open the last chevron, the ordinary way.
5329        let at = sweep(&mut h, Id::new(("crumb-chevron", pane, index)));
5330        h.wait();
5331        h.click_at(at);
5332        h.frame(Vec::new());
5333        assert_eq!(h.app.crumbs.showing(), Some((pane, index)));
5334
5335        // Now hover a name further back along the trail. No click.
5336        let target = index - 2;
5337        let over = sweep(&mut h, Id::new(("crumb", pane, target)));
5338        h.frame(vec![Event::PointerMoved(over)]);
5339        // One more, because the switch is applied at the end of the frame that notices it: the
5340        // dropdown going away is already painted by then.
5341        h.frame(Vec::new());
5342
5343        assert_eq!(
5344            h.app.crumbs.showing(),
5345            Some((pane, target + 1)),
5346            "the dropdown did not follow the pointer to the name at {target}"
5347        );
5348        let (read, _) = h
5349            .app
5350            .crumbs
5351            .listing()
5352            .expect("the dropdown that moved never read a folder");
5353        assert_eq!(
5354            read, crumbs[target].1,
5355            "it moved to the chevron of the wrong name"
5356        );
5357
5358        // And it lets go when something is clicked. The name under the pointer is a link, so
5359        // this also navigates — which is the same click doing both, as it does in Explorer.
5360        h.click_at(over);
5361        assert_eq!(
5362            h.app.crumbs.showing(),
5363            None,
5364            "the bar is still tracking after a click"
5365        );
5366    }
5367
5368    /// The bar says where a click will open the path field: a pen, and a border.
5369    ///
5370    /// The pointer already turned into an I-beam over the empty space past the last segment and
5371    /// the bar itself said nothing that could be seen — its hairline was `stroke-subtle`, which
5372    /// is the colour the bar is *painted*, so it was a line drawn in the colour behind it.
5373    ///
5374    /// Two cues now, sharing one ink. The **pen** is always there, because a hint that appears
5375    /// only once the pointer has arrived is not a hint; the **border** appears with the pointer,
5376    /// over the whole shape the field will take, and the pen brightens to match it. No fill:
5377    /// washing the bar to announce something that is only an announcement was too much.
5378    #[test]
5379    fn the_breadcrumb_shows_where_the_path_field_opens() {
5380        use crate::ui::breadcrumb::pen_ink;
5381
5382        let mut h = Harness::new();
5383        let pane = h.app.panes[0].id;
5384        let (rest, lit) = (pen_ink(&h.app.theme, false), pen_ink(&h.app.theme, true));
5385        assert_ne!(rest, lit, "the two states of the hint are the same colour");
5386        assert_ne!(
5387            rest,
5388            crate::ui::seam(&h.app.theme),
5389            "the hint is drawn in the colour of the bar it is drawn on"
5390        );
5391
5392        let of = |id: Id| {
5393            h.ctx
5394                .read_response(id)
5395                .map(|r| r.rect)
5396                .unwrap_or_else(|| panic!("{id:?} was not drawn"))
5397        };
5398        let empty = of(Id::new(("crumb-empty", pane)));
5399        // The pen sits at the right-hand end of the bar, in the room reserved out of the trail's.
5400        // A couple of points of slack around it: the glyph fills its box corner to corner.
5401        let pen = Rect::from_center_size(
5402            pos2(empty.right() - crate::ui::breadcrumb::PEN * 0.5, empty.center().y),
5403            egui::Vec2::splat(crate::ui::breadcrumb::PEN + 6.0),
5404        );
5405        // The field's own shape: the whole path area, so it ends where the bar does and starts
5406        // well before the empty part the pointer is over.
5407        let bar_wide = |(rect, _): &(Rect, egui::Color32)| {
5408            (rect.right() - empty.right()).abs() < 0.5 && rect.left() < empty.left() - 1.0
5409        };
5410
5411        // ---- At rest: the pen, and no border ------------------------------
5412        assert_eq!(
5413            h.glyph_inks(pen),
5414            vec![rest, rest],
5415            "the pen is not drawn at rest, or not in the resting ink"
5416        );
5417
5418        // **Centred on the row by its ink**, which is not the same claim as its box being
5419        // centred: the box was right the whole time the drawing inside it sat two units low.
5420        // See `icons::pencil`, and the rule this is an instance of — anything in a row is
5421        // vertically centred, and text on its baseline.
5422        let ink = h.glyph_bounds(pen).expect("the pen's own shapes");
5423        let off = ink.center().y - empty.center().y;
5424        assert!(
5425            off.abs() < 0.5,
5426            "the pen's ink sits {off:+.2} points off the middle of the bar ({:?} in {:?})",
5427            ink,
5428            empty
5429        );
5430        assert!(
5431            !h.outlines().iter().any(bar_wide),
5432            "the bar is outlined before the pointer is anywhere near it"
5433        );
5434
5435        // ---- Under the pointer: both, in the lit ink ----------------------
5436        h.frame(vec![Event::PointerMoved(pos2(
5437            empty.center().x,
5438            h.path_bar_y(0),
5439        ))]);
5440        assert_eq!(
5441            h.cursor,
5442            egui::CursorIcon::Text,
5443            "this is not the part of the bar that opens the field"
5444        );
5445        assert_eq!(
5446            h.glyph_inks(pen),
5447            vec![lit, lit],
5448            "the pen did not light up with the border"
5449        );
5450        let outline = h
5451            .outlines()
5452            .into_iter()
5453            .find(bar_wide)
5454            .expect("the pointer says the field opens here and the bar does not");
5455        assert_eq!(outline.1, lit, "the border is not the hint's own ink");
5456        assert!(
5457            outline.0.contains(pen.center()),
5458            "the border stops short of the pen: {:?} against {:?}",
5459            outline.0,
5460            pen
5461        );
5462
5463        // And no fill: the bar does not change colour to say this. Against the hover grey, which is
5464        // what a segment of the bar is washed with -- naming `control_fills`'s hover, as this once
5465        // did, stopped naming a colour the bar is ever painted, and the assertion passed because
5466        // nothing could match it rather than because nothing was washed.
5467        let hover = crate::ui::hover_fill(&h.app.theme);
5468        assert!(
5469            !h.rects()
5470                .into_iter()
5471                .any(|(rect, _, fill)| fill == hover && bar_wide(&(rect, fill))),
5472            "the bar is washed as well as outlined"
5473        );
5474    }
5475
5476    /// A field's border lights up wherever the pointer is over it, not only over its icon.
5477    ///
5478    /// The design system's fault, and worth a test here because this is where it showed: a
5479    /// field's wrapper is allocated *before* the `TextEdit` that goes inside it, so the edit is
5480    /// the topmost widget over the text area and took the hover from it. The border therefore lit
5481    /// up only where the edit was not — the prefix icon and the padding — so hovering the part of
5482    /// the filter box you type in did nothing, which reads as a control that does not respond.
5483    ///
5484    /// Asserted at the *far* end of the box from its icon, which is the part that was dead.
5485    #[test]
5486    fn a_field_lights_up_over_all_of_itself() {
5487        let mut h = Harness::new();
5488        let y = h.path_bar_y(0);
5489        let right = h.pane_rect(0).right();
5490
5491        // The run of x where the pointer says "text", coming in from the right-hand end of the
5492        // bar: the filter box. Found by sweeping rather than by deriving its rect, for the same
5493        // reason `arriving_in_a_field_selects_what_is_there` does it — the box's own id is
5494        // generated inside the design system.
5495        let mut run: Vec<f32> = Vec::new();
5496        for dx in (8..400).step_by(2) {
5497            let at = pos2(right - dx as f32, y);
5498            h.frame(vec![Event::PointerMoved(at)]);
5499            if h.cursor == egui::CursorIcon::Text {
5500                run.push(at.x);
5501            } else if !run.is_empty() {
5502                break;
5503            }
5504        }
5505        assert!(run.len() > 8, "the filter box is not reachable by the pointer");
5506
5507        // The rightmost end of the run: the text area. The icon is at the other end, and the
5508        // clearable ✕ only exists once something has been typed.
5509        let text_area = pos2(run[0], y);
5510        h.frame(vec![Event::PointerMoved(text_area)]);
5511        let border = |h: &Harness, at: Pos2| {
5512            h.outlines()
5513                .into_iter()
5514                .find(|(rect, _)| rect.contains(at))
5515                .map(|(_, color)| color)
5516        };
5517        assert_eq!(
5518            border(&h, text_area),
5519            Some(h.app.theme.stroke.strong),
5520            "hovering the part of the field you type in did not light its border"
5521        );
5522
5523        // And it goes out again, which is what makes the assertion above about the hover rather
5524        // than about the border always being that colour.
5525        h.frame(vec![Event::PointerMoved(pos2(right - 500.0, y))]);
5526        assert_eq!(
5527            border(&h, text_area),
5528            Some(h.app.theme.stroke.control),
5529            "the field stayed lit with the pointer somewhere else"
5530        );
5531    }
5532
5533    /// The rows a tab is showing, in display order, by the name each one carries.
5534    ///
5535    /// Which is the whole of what "flattened" means from the outside: the same listing, with
5536    /// relative paths in it instead of bare names.
5537    fn shown_names(h: &Harness, pane: usize) -> Vec<String> {
5538        let tab = h.app.panes[pane].tab();
5539        let dir = tab.dir.as_ref().expect("the listing has not arrived");
5540        tab.order
5541            .iter()
5542            .map(|&i| dir.name(i as usize).to_owned())
5543            .collect()
5544    }
5545
5546    /// Flatten: the button on the bar, the shortcut, and the listing each of them produces.
5547    ///
5548    /// Driven through the real button and the real keyboard rather than through `perform`,
5549    /// because the two things most likely to be wrong are the ones only that can see: a
5550    /// button nothing can reach, and a shortcut that never arrives.
5551    #[test]
5552    fn flattening_a_folder_shows_its_whole_tree_and_turning_it_off_puts_it_back() {
5553        let mut h = Harness::new();
5554        let pane = h.app.panes[0].id;
5555        // `src`, not the crate root — the root has `target` in it, and walking a few hundred
5556        // thousand build artefacts would prove nothing this does not.
5557        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
5558        h.app.perform(
5559            &h.ctx.clone(),
5560            Action::Navigate {
5561                pane,
5562                path: sources,
5563            },
5564        );
5565        h.settle();
5566        assert!(
5567            shown_names(&h, 0).iter().all(|name| !name.contains('\\')),
5568            "a folder's own listing is one level deep"
5569        );
5570
5571        // ---- Where the button is ------------------------------------------
5572        let rect_of = |h: &Harness, id: Id| {
5573            h.ctx
5574                .read_response(id)
5575                .map(|r| r.rect)
5576                .unwrap_or_else(|| panic!("{id:?} was not laid out"))
5577        };
5578        let button = rect_of(&h, Id::new(("flatten", pane)));
5579        let bar = h.app.panes[0].rect;
5580        // Past the end of the path — it is one of the right-hand group, not part of the trail —
5581        // and with a filter box's worth of room still to its right, which is what "just before
5582        // the filter" comes to in geometry. 90 is the width below which `breadcrumb` gives up
5583        // drawing the field at all.
5584        let empty = rect_of(&h, Id::new(("crumb-empty", pane)));
5585        assert!(
5586            button.left() >= empty.right(),
5587            "the flatten button is sitting in the path's room: {button:?} against {empty:?}"
5588        );
5589        assert!(
5590            bar.right() - button.right() >= 90.0,
5591            "nothing but {} points to the right of it, so the filter is not there",
5592            bar.right() - button.right()
5593        );
5594
5595        // ---- Clicking it --------------------------------------------------
5596        let done = h.click_at(button.center());
5597        assert!(
5598            done.contains(&"ToggleFlat"),
5599            "the button did nothing, got {done:?}"
5600        );
5601        h.settle();
5602        assert!(h.app.panes[0].tab().flat, "the tab is not flattened");
5603
5604        let flat = shown_names(&h, 0);
5605        assert!(
5606            flat.iter().any(|name| name == "ui\\filelist.rs"),
5607            "the listing did not reach a second level: {} rows, first few {:?}",
5608            flat.len(),
5609            &flat[..flat.len().min(5)]
5610        );
5611        assert!(
5612            flat.len() > shown_names_at_rest(),
5613            "a flattened tree should have more rows in it than the folder did"
5614        );
5615
5616        // A rename edits the *file's* name, not the whole of what the row shows. Otherwise the
5617        // field would open with `ui\filelist.rs` in it, and accepting that unchanged would ask
5618        // the shell to rename a file to a path.
5619        {
5620            let tab = h.app.panes[0].tab_mut();
5621            let at = tab
5622                .order
5623                .iter()
5624                .position(|&i| {
5625                    tab.dir
5626                        .as_ref()
5627                        .is_some_and(|dir| dir.name(i as usize) == "ui\\filelist.rs")
5628                })
5629                .expect("the row is in the listing");
5630            tab.select_only(at);
5631            tab.begin_rename();
5632            assert_eq!(
5633                tab.renaming.as_ref().map(|(_, text)| text.as_str()),
5634                Some("filelist.rs"),
5635                "the rename field opened on the path rather than on the name"
5636            );
5637            tab.renaming = None;
5638        }
5639
5640        // ---- And Ctrl+E, which is the same gesture from the keyboard ------
5641        h.take_journal();
5642        let held = Modifiers::COMMAND;
5643        h.modifiers = held;
5644        h.frame(vec![Event::Key {
5645            key: egui::Key::E,
5646            physical_key: None,
5647            pressed: true,
5648            repeat: false,
5649            modifiers: held,
5650        }]);
5651        h.modifiers = Modifiers::NONE;
5652        let done = h.take_journal();
5653        assert!(
5654            done.contains(&"ToggleFlat"),
5655            "Ctrl+E did nothing, got {done:?}"
5656        );
5657        h.settle();
5658        assert!(!h.app.panes[0].tab().flat);
5659        assert!(
5660            shown_names(&h, 0).iter().all(|name| !name.contains('\\')),
5661            "turning it off left the tree on screen"
5662        );
5663    }
5664
5665    /// How many rows `src` itself has. Read once, so the comparison above is against the
5666    /// folder rather than against a number written down here.
5667    fn shown_names_at_rest() -> usize {
5668        let dir = crate::fs::scan::scan(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"));
5669        dir.len()
5670    }
5671
5672    /// **A folder shortcut opens in this window.**
5673    ///
5674    /// Handing a `.lnk` to the shell is what it gets by default, and for one pointing at a
5675    /// folder that means Explorer opening over the top of this program — a file manager whose
5676    /// rows open a *different* file manager. Both gestures are driven for real here, because
5677    /// what could break is the wiring rather than the resolving: `Enter` through the keyboard
5678    /// path, and a middle click through the pointer's.
5679    #[cfg(windows)]
5680    #[test]
5681    fn opening_a_folder_shortcut_stays_in_this_window() {
5682        let root = std::env::temp_dir().join(format!("yafe-open-lnk-{}", std::process::id()));
5683        let folder = root.join("somewhere");
5684        let _ = std::fs::remove_dir_all(&root);
5685        std::fs::create_dir_all(&folder).expect("a directory in the temp folder");
5686        let link = root.join("somewhere.lnk");
5687        if !crate::shell::links::write_shortcut(&link, &folder) {
5688            println!("the shell would not write a shortcut here; skipping");
5689            let _ = std::fs::remove_dir_all(&root);
5690            return;
5691        }
5692
5693        let mut h = Harness::new();
5694        let pane = h.app.panes[0].id;
5695        h.app.perform(
5696            &h.ctx.clone(),
5697            Action::Navigate {
5698                pane,
5699                path: root.clone(),
5700            },
5701        );
5702        h.settle();
5703
5704        let position = {
5705            let tab = h.app.panes[0].tab();
5706            let dir = tab.dir.as_ref().expect("the listing");
5707            tab.order
5708                .iter()
5709                .position(|&i| dir.name(i as usize) == "somewhere.lnk")
5710                .expect("the shortcut is in the listing")
5711        };
5712        assert!(
5713            h.app.panes[0].tab().is_shortcut_at(position),
5714            "the row is not recognised as a shortcut, so nothing below can work"
5715        );
5716        assert!(
5717            !h.app.panes[0].tab().is_dir_at(position),
5718            "a `.lnk` is a file as far as the enumeration is concerned — if it were not, this \
5719             test would be passing for the wrong reason"
5720        );
5721
5722        // ---- Enter ---------------------------------------------------------
5723        h.app.panes[0].tab_mut().select_only(position);
5724        h.frame(Vec::new());
5725        h.take_journal();
5726        h.frame(vec![Event::Key {
5727            key: egui::Key::Enter,
5728            physical_key: None,
5729            pressed: true,
5730            repeat: false,
5731            modifiers: Modifiers::NONE,
5732        }]);
5733        let done = h.take_journal();
5734        assert!(
5735            done.contains(&"Navigate"),
5736            "Enter on a folder shortcut went to the shell instead of navigating, got {done:?}"
5737        );
5738        h.settle();
5739        assert_eq!(
5740            h.app.panes[0].tab().path,
5741            folder,
5742            "it navigated somewhere else"
5743        );
5744
5745        // ---- And a middle click, into a tab of its own ----------------------
5746        h.app.perform(
5747            &h.ctx.clone(),
5748            Action::Navigate {
5749                pane,
5750                path: root.clone(),
5751            },
5752        );
5753        h.settle();
5754        let tabs = h.app.panes[0].tabs.len();
5755        let at = h.row_center(0, position);
5756        h.take_journal();
5757        let done = h.click_with(at, PointerButton::Middle, Modifiers::NONE);
5758        assert!(
5759            done.contains(&"OpenNewTab"),
5760            "a middle click on a folder shortcut did nothing, got {done:?}"
5761        );
5762        h.settle();
5763        assert_eq!(
5764            h.app.panes[0].tabs.len(),
5765            tabs + 1,
5766            "no tab was opened for it"
5767        );
5768        assert_eq!(h.app.panes[0].tab().path, folder);
5769
5770        let _ = std::fs::remove_dir_all(&root);
5771    }
5772
5773    /// **The filter waits for the typing to stop, and each keystroke restarts the wait.**
5774    ///
5775    /// One pass costs up to 240 ms on a large listing — see [`crate::pane::FILTER_DELAY`] — so
5776    /// what this is really about is that typing a word should cost one pass and not one per
5777    /// letter. Driven through the real field with the real clock, because the whole behaviour is
5778    /// a relationship between keystrokes and time.
5779    #[test]
5780    fn the_filter_waits_for_the_typing_to_stop() {
5781        let mut h = Harness::new();
5782        let pane = h.app.panes[0].id;
5783        h.app.perform(
5784            &h.ctx.clone(),
5785            Action::Navigate {
5786                pane,
5787                path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
5788            },
5789        );
5790        h.settle();
5791        let all = h.tab(0).order.len();
5792        assert!(all > 8, "the fixture folder is too small to filter");
5793
5794        // The caret into the box, which is where the keystrokes have to land.
5795        let held = Modifiers::COMMAND;
5796        h.modifiers = held;
5797        h.frame(vec![Event::Key {
5798            key: egui::Key::F,
5799            physical_key: None,
5800            pressed: true,
5801            repeat: false,
5802            modifiers: held,
5803        }]);
5804        h.modifiers = Modifiers::NONE;
5805        h.frame(Vec::new());
5806        assert!(
5807            h.ctx.memory(|m| m.focused()).is_some(),
5808            "the caret is not in the filter box, so nothing below is being tested"
5809        );
5810
5811        // Each step is most of the wait but not all of it, so two of them cross the deadline and
5812        // one does not. In terms of the constant rather than in milliseconds: what is being
5813        // tested is the relationship, and it should still hold when the wait is retuned.
5814        let step = crate::pane::FILTER_DELAY * 0.6;
5815
5816        // ---- One letter: noted, not applied ---------------------------------
5817        h.frame(vec![Event::Text("c".to_owned())]);
5818        assert_eq!(h.tab(0).filter, "c", "the keystroke never reached the field");
5819        assert!(h.tab(0).filter_at.is_some(), "no deadline was set");
5820        assert_eq!(
5821            h.tab(0).order.len(),
5822            all,
5823            "the filter was applied on the keystroke, which is the thing this prevents"
5824        );
5825
5826        // A frame most of the way through the wait changes nothing.
5827        h.time += step;
5828        h.frame(Vec::new());
5829        assert_eq!(h.tab(0).order.len(), all, "applied before the wait was up");
5830
5831        // ---- A second letter restarts it ------------------------------------
5832        h.frame(vec![Event::Text("o".to_owned())]);
5833        assert_eq!(h.tab(0).filter, "co");
5834        // The same step again, which is past the deadline the *first* letter set and short of the
5835        // one the second set. This is where it would have fired without the restart.
5836        h.time += step;
5837        h.frame(Vec::new());
5838        assert_eq!(
5839            h.tab(0).order.len(),
5840            all,
5841            "the second keystroke did not restart the wait"
5842        );
5843
5844        // ---- And then it fires, once, on the whole word ---------------------
5845        h.time += step;
5846        h.frame(Vec::new());
5847        assert!(h.tab(0).filter_at.is_none(), "the deadline is still standing");
5848        let rows = h.tab(0).order.len();
5849        assert!(
5850            rows < all,
5851            "the filter never applied at all: still {rows} of {all} rows"
5852        );
5853        let dir = h.tab(0).dir.clone().expect("the listing");
5854        for &i in &h.tab(0).order {
5855            let name = dir.name(i as usize).to_lowercase();
5856            assert!(
5857                name.contains("co"),
5858                "`{name}` does not match the filter that was typed"
5859            );
5860        }
5861    }
5862
5863    // -----------------------------------------------------------------------
5864    // The preview panel
5865    // -----------------------------------------------------------------------
5866
5867    /// Put the focused pane in the folder the test binary is in, select it, and open the preview
5868    /// panel on it — then let the walk land.
5869    ///
5870    /// The binary is this test process, which is the one fixture on any machine that is a real PE
5871    /// image, is always there, and imports something. Selected rather than handed to the panel
5872    /// directly, because the panel follows the selection every frame: pointing it at a file that
5873    /// is *not* selected is a state the running program never reaches, and one that would be
5874    /// cleared on the next frame anyway.
5875    fn open_preview(h: &mut Harness) {
5876        let me = std::env::current_exe().expect("a test process has an executable");
5877        let folder = me.parent().expect("it is in a folder").to_path_buf();
5878        let pane = h.app.panes[0].id;
5879        h.app
5880            .perform(&h.ctx.clone(), Action::Navigate { pane, path: folder });
5881        h.settle();
5882
5883        let name = me
5884            .file_name()
5885            .expect("it has a name")
5886            .to_string_lossy()
5887            .into_owned();
5888        let at = {
5889            let tab = h.app.panes[0].tab();
5890            let dir = tab.dir.as_ref().expect("the listing arrived");
5891            tab.order
5892                .iter()
5893                .position(|&i| dir.name(i as usize) == name)
5894                .unwrap_or_else(|| panic!("{name} is not in its own folder's listing"))
5895        };
5896        h.app.panes[0].tab_mut().select_only(at);
5897        h.app.panes[0].tab_mut().preview.open = true;
5898        h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
5899        for attempt in 0..400 {
5900            h.frame(Vec::new());
5901            if !h.app.preview_pending() && attempt > 2 {
5902                break;
5903            }
5904            // Frames are free here; the walk is on a worker thread sharing a machine with
5905            // seven other test threads.
5906            std::thread::sleep(std::time::Duration::from_millis(2));
5907        }
5908        assert!(!h.app.preview_pending(), "the read never came back");
5909        assert_eq!(
5910            h.app.panes[0].tab().preview.showing(),
5911            Some(me.as_path()),
5912            "the panel is not showing the binary that was selected"
5913        );
5914        h.take_journal();
5915    }
5916
5917    /// Where the panel is, once the layout has looked at the pane it is in.
5918    fn preview_rect(h: &Harness) -> Rect {
5919        crate::ui::preview::split(pane_body(h), true, h.app.preview)
5920            .1
5921            .expect("the panel has room in a harness-sized pane")
5922    }
5923
5924    /// Everything under the focused pane's path bar, which the listing and the panel share.
5925    fn pane_body(h: &Harness) -> Rect {
5926        let pane = h.pane_rect(0);
5927        Rect::from_min_max(
5928            pos2(
5929                pane.left(),
5930                h.pane_content_top(0) + crate::ui::breadcrumb::HEIGHT,
5931            ),
5932            pane.max,
5933        )
5934    }
5935
5936    /// How many rows the focused pane's dependency view is showing.
5937    fn shown_deps(h: &Harness) -> usize {
5938        h.app.panes[0]
5939            .tab()
5940            .preview
5941            .dependency_rows()
5942            .expect("the panel is showing a dependency tree")
5943    }
5944
5945    /// **The panel is inside the pane**, on whichever side the layout says, and its close button
5946    /// gives the room back.
5947    ///
5948    /// Driven through the real button rather than through `perform`, because the one thing only
5949    /// that can catch is a button nothing can reach — and this one sits on a bar inside a pane,
5950    /// over a listing that also claims every point of it for its own hit-testing.
5951    #[test]
5952    fn the_preview_panel_takes_room_from_the_listing_and_its_close_button_gives_it_back() {
5953        use crate::ui::preview::{Side, Where};
5954
5955        for at in [Where::Right, Where::Bottom] {
5956            let mut h = Harness::new();
5957            h.app.preview.at = at;
5958            open_preview(&mut h);
5959            let body = pane_body(&h);
5960            let panel = preview_rect(&h);
5961
5962            // The listing gave up room on one side, and only there.
5963            match at.side(body) {
5964                Side::Right => {
5965                    assert!(panel.right() >= body.right() - 0.5, "{at:?}: not at the edge");
5966                    assert_eq!(panel.top(), body.top(), "{at:?}: it is not full height");
5967                    assert!(panel.width() < body.width() * 0.6);
5968                }
5969                Side::Bottom => {
5970                    assert!(panel.bottom() >= body.bottom() - 0.5, "{at:?}");
5971                    assert_eq!(panel.left(), body.left(), "{at:?}: it is not full width");
5972                    assert!(panel.height() < body.height() * 0.6);
5973                }
5974            }
5975            // And it is showing the walk rather than the word `Reading…`.
5976            let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
5977            assert!(
5978                texts.iter().any(|text| text.contains("API set")),
5979                "{at:?}: the panel is not showing a finished walk: {texts:?}"
5980            );
5981
5982            // **A row folds when it is clicked.** Worth driving for real rather than through the
5983            // view's own method, because the rows live inside a `ScrollArea` inside a panel inside
5984            // a pane — and a click landing on any of those instead of on the row is precisely the
5985            // kind of thing that looks correct in the source and does nothing at all.
5986            let before = shown_deps(&h);
5987            assert!(before > 2, "{at:?}: the root's imports are not on show");
5988            let first_import = pos2(
5989                panel.left() + 80.0,
5990                panel.top() + crate::ui::preview::HEADER + crate::ui::deps::ROW * 1.5,
5991            );
5992            h.click_at(first_import);
5993            let opened = shown_deps(&h);
5994            assert!(
5995                opened > before,
5996                "{at:?}: clicking a row did not unfold it: {before} rows, then {opened}"
5997            );
5998            h.wait();
5999            h.click_at(first_import);
6000            assert_eq!(shown_deps(&h), before, "{at:?}: it did not fold up again");
6001
6002            // The close button, found by hovering rather than by arithmetic.
6003            let close = egui::Id::new(("preview-close", h.app.panes[0].id));
6004            let found = h
6005                .find(
6006                    close,
6007                    panel.right() - 16.0,
6008                    (panel.top() as i32)..(panel.top() as i32 + 34),
6009                )
6010                .unwrap_or_else(|| panic!("{at:?}: nothing answers to the close button"));
6011            assert_eq!(h.click_at(found), vec!["ClosePreview"]);
6012            assert!(!h.app.panes[0].tab().preview.open);
6013            // And the listing has the whole body back.
6014            assert_eq!(
6015                crate::ui::preview::split(pane_body(&h), false, h.app.preview),
6016                (pane_body(&h), None),
6017                "{at:?}: the room did not come back"
6018            );
6019        }
6020    }
6021
6022    /// **The preview button's context menu**: show or hide, and then the three positions.
6023    ///
6024    /// Driven with a real right click, because a menu hung off a button on a bar that a listing
6025    /// also hit-tests is the kind of thing that looks perfectly correct in the source and simply
6026    /// never opens. And it is **sticky** — ticking a position leaves it up, since three radio
6027    /// buttons you have to reopen the menu between are three menus.
6028    #[test]
6029    fn the_preview_button_carries_the_panels_position() {
6030        use crate::ui::preview::Where;
6031
6032        let mut h = Harness::new();
6033        let pane = h.app.panes[0].id;
6034        let eye = egui::Id::new(("preview", pane));
6035
6036        // Find the button, which sits between the path and the flatten toggle rather than at a
6037        // position this test gets to assume.
6038        let y = h.path_bar_y(0);
6039        let right = h.pane_rect(0).right();
6040        let sweep: Vec<Pos2> = (0..40)
6041            .map(|step| pos2(right - 20.0 - step as f32 * 6.0, y))
6042            .collect();
6043        let at = sweep
6044            .into_iter()
6045            .find(|&at| h.hovers(eye, at))
6046            .expect("nothing on the path bar answers to the preview button");
6047
6048        // A right click puts the menu up, with the position group in it.
6049        h.frame(vec![Event::PointerButton {
6050            pos: at,
6051            button: PointerButton::Secondary,
6052            pressed: true,
6053            modifiers: Modifiers::NONE,
6054        }]);
6055        h.frame(vec![Event::PointerButton {
6056            pos: at,
6057            button: PointerButton::Secondary,
6058            pressed: false,
6059            modifiers: Modifiers::NONE,
6060        }]);
6061        h.frame(Vec::new());
6062        let entries: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
6063        for want in ["Show preview", "Position", "Right", "Bottom", "Auto"] {
6064            assert!(
6065                entries.iter().any(|text| text == want),
6066                "`{want}` is not in the menu: {entries:?}"
6067            );
6068        }
6069
6070        // Ticking a position changes the window's preference — and the menu stays up, which is
6071        // what `sticky` is for. Found by its label rather than by arithmetic over menu rows.
6072        let bottom = h
6073            .texts()
6074            .into_iter()
6075            .find(|(_, text)| text == "Bottom")
6076            .map(|(at, _)| at + vec2(8.0, 6.0))
6077            .expect("the entry was drawn a moment ago");
6078        assert_eq!(h.app.preview.at, Where::Right, "the default has moved");
6079        h.click_at(bottom);
6080        assert_eq!(
6081            h.app.preview.at,
6082            Where::Bottom,
6083            "clicking the entry did not move the panel"
6084        );
6085        assert!(
6086            h.texts().iter().any(|(_, text)| text == "Position"),
6087            "the menu closed on a tick, so setting two of these means opening it twice"
6088        );
6089    }
6090
6091    /// `Ctrl+P` opens this folder's preview panel, and closes it again.
6092    ///
6093    /// On the pane the keyboard is in and on that pane's tab, which is the whole of what "each
6094    /// folder has one" means: the shortcut is not a window-wide switch.
6095    #[test]
6096    fn ctrl_p_opens_and_shuts_this_folders_preview() {
6097        let mut h = Harness::with_panes(2);
6098        let second = h.app.panes[1].id;
6099        h.app.perform(&h.ctx.clone(), Action::Focus(second));
6100        h.frame(Vec::new());
6101
6102        let press = |h: &mut Harness| {
6103            h.take_journal();
6104            h.modifiers = Modifiers::COMMAND;
6105            h.frame(vec![Event::Key {
6106                key: egui::Key::P,
6107                physical_key: None,
6108                pressed: true,
6109                repeat: false,
6110                modifiers: Modifiers::COMMAND,
6111            }]);
6112            h.modifiers = Modifiers::NONE;
6113            h.frame(Vec::new());
6114            h.take_journal()
6115        };
6116
6117        assert!(!h.app.panes[1].tab().preview.open);
6118        assert_eq!(press(&mut h), vec!["TogglePreview"]);
6119        assert!(h.app.panes[1].tab().preview.open, "it did not open");
6120        // And only in the pane the keyboard is in.
6121        assert!(
6122            !h.app.panes[0].tab().preview.open,
6123            "it opened in the other pane as well"
6124        );
6125        assert_eq!(press(&mut h), vec!["TogglePreview"]);
6126        assert!(!h.app.panes[1].tab().preview.open, "it did not shut again");
6127    }
6128
6129    /// **Two selected pictures become a comparison**: three views, and a toggle down to one.
6130    ///
6131    /// End to end, because the interesting part is the *decision* — two selected rows rather than
6132    /// one cursor — and that lives in `App::selected_preview` where the panel cannot see it. The
6133    /// fixtures are written here: two PNGs that differ in one corner, which is a diff with a known
6134    /// answer.
6135    #[test]
6136    fn two_selected_pictures_are_compared_in_three_views() {
6137        let root = std::env::temp_dir().join(format!("yafe-diff-{}", std::process::id()));
6138        let _ = std::fs::remove_dir_all(&root);
6139        std::fs::create_dir_all(&root).expect("a directory in the temp folder");
6140        // Three of them, because the last assertion is about what *three* selected pictures do and
6141        // `select_all` over two would still be a pair.
6142        for (name, tint) in [("a.png", 40u8), ("b.png", 200), ("c.png", 40)] {
6143            let mut buffer = image::RgbaImage::from_pixel(60, 40, image::Rgba([9, 9, 9, 255]));
6144            buffer.put_pixel(50, 30, image::Rgba([tint, 9, 9, 255]));
6145            buffer
6146                .save(root.join(name))
6147                .expect("a PNG in the temp folder");
6148        }
6149
6150        let mut h = Harness::new();
6151        let pane = h.app.panes[0].id;
6152        h.app.perform(
6153            &h.ctx.clone(),
6154            Action::Navigate {
6155                pane,
6156                path: root.clone(),
6157            },
6158        );
6159        h.settle();
6160        h.app.panes[0].tab_mut().preview.open = true;
6161
6162        // One picture selected is one picture: the comparison is not something a single selection
6163        // can produce by accident.
6164        h.app.panes[0].tab_mut().select_only(0);
6165        settle_preview(&mut h);
6166        assert_eq!(
6167            h.app.panes[0].tab().preview.frames(),
6168            Some((1, 1)),
6169            "one selected picture is not one view"
6170        );
6171
6172        // And the second one turns it into a comparison, without a shortcut or a menu.
6173        h.app.panes[0].tab_mut().toggle(1);
6174        settle_preview(&mut h);
6175        assert_eq!(
6176            h.app.panes[0].tab().preview.frames(),
6177            Some((3, 3)),
6178            "two selected pictures did not become three views"
6179        );
6180        let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
6181        assert!(
6182            texts.iter().any(|text| text.contains("↔")),
6183            "the bar does not name both files: {texts:?}"
6184        );
6185        assert!(
6186            texts.iter().any(|text| text == "differences"),
6187            "the third view is not captioned: {texts:?}"
6188        );
6189        // 1 pixel of 2,400 differs — the corner each file tinted differently. Reported on a panel
6190        // given room for it: at the default share the bar's priority rule has already dropped the
6191        // comment for a name this long, which the next assertion is about.
6192        h.app.preview.share = 0.66;
6193        h.frame(Vec::new());
6194        let wide: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
6195        assert!(
6196            wide.iter().any(|text| text.contains("0.04% differs")),
6197            "the share that differs is not reported: {wide:?}"
6198        );
6199        assert!(
6200            wide.iter().any(|text| text.contains("60 × 40")),
6201            "the size is not reported either: {wide:?}"
6202        );
6203
6204        // **And the rule, end to end.** Swept from a wide bar to a narrow one, the two details go
6205        // in order and the name is on the bar the whole way. The invariant that says it is
6206        // *`comment` never survives `size`* — which holds at every width and does not depend on
6207        // what this machine's font measures. `what_fits` has the unit test for the arithmetic; this
6208        // is about what is actually drawn.
6209        let mut seen = Vec::new();
6210        for share in [0.66, 0.58, 0.50, 0.42, 0.34, 0.26] {
6211            h.app.preview.share = share;
6212            h.frame(Vec::new());
6213            let bar: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
6214            let comment = bar.iter().any(|text| text.contains("differs"));
6215            let size = bar.iter().any(|text| text.contains("60 × 40"));
6216            assert!(
6217                !(comment && !size),
6218                "at {share} the comment is on the bar and the size is not, which is backwards: \
6219                 {bar:?}"
6220            );
6221            assert!(
6222                bar.iter().any(|text| text.starts_with("a.png")),
6223                "at {share} the name is gone, and the details were droppable: {bar:?}"
6224            );
6225            seen.push((comment, size));
6226        }
6227        assert_eq!(seen.first(), Some(&(true, true)), "the widest bar: {seen:?}");
6228        assert_eq!(seen.last(), Some(&(false, false)), "the narrowest: {seen:?}");
6229        // Monotone: nothing comes back as the bar narrows.
6230        for pair in seen.windows(2) {
6231            assert!(
6232                !(pair[1].0 && !pair[0].0) && !(pair[1].1 && !pair[0].1),
6233                "a detail reappeared on a narrower bar: {seen:?}"
6234            );
6235        }
6236        h.app.preview.share = crate::ui::preview::SHARE;
6237
6238        // The toggle takes it down to the difference alone, and back.
6239        h.app.panes[0].tab_mut().preview.toggle_all();
6240        h.frame(Vec::new());
6241        assert_eq!(h.app.panes[0].tab().preview.frames(), Some((3, 1)));
6242        h.app.panes[0].tab_mut().preview.toggle_all();
6243        h.frame(Vec::new());
6244        assert_eq!(h.app.panes[0].tab().preview.frames(), Some((3, 3)));
6245
6246        // A third selected picture is not a comparison of anything, so the panel goes back to
6247        // having nothing to say rather than picking two of them.
6248        h.app.panes[0].tab_mut().toggle(0);
6249        h.app.panes[0].tab_mut().toggle(1);
6250        h.app.panes[0].tab_mut().select_all();
6251        settle_preview(&mut h);
6252        assert!(
6253            h.app.panes[0]
6254                .tab()
6255                .preview
6256                .frames()
6257                .is_none_or(|(_, shown)| shown == 1),
6258            "three selected pictures produced a comparison"
6259        );
6260        let _ = std::fs::remove_dir_all(&root);
6261    }
6262
6263    /// Let the focused pane's preview panel notice the selection and finish reading.
6264    fn settle_preview(h: &mut Harness) {
6265        h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
6266        for attempt in 0..400 {
6267            h.frame(Vec::new());
6268            if !h.app.preview_pending() && attempt > 2 {
6269                break;
6270            }
6271            std::thread::sleep(std::time::Duration::from_millis(2));
6272        }
6273        assert!(!h.app.preview_pending(), "the read never came back");
6274    }
6275
6276    /// **The panel follows the keyboard**, once the keyboard stops moving.
6277    ///
6278    /// End to end through the real frame loop: the cursor lands on a binary, nothing happens for
6279    /// a quarter of a second, and then a walk of *that* file is what the panel is showing. Which
6280    /// is the whole of the gesture — open it once, then arrow down a folder and look at each file
6281    /// in turn — and four separate things have to be right for it: the cursor being noticed, the
6282    /// kind being worked out from the name, the wait, and the answer finding its way back to the
6283    /// panel that asked.
6284    ///
6285    /// The fixture is the test binary and the folder it is in, which is the one place on any
6286    /// machine guaranteed to hold a real PE image.
6287    #[test]
6288    fn the_preview_panel_follows_the_keyboard() {
6289        let mut h = Harness::new();
6290        let pane = h.app.panes[0].id;
6291        let me = std::env::current_exe().expect("a test process has an executable");
6292        let folder = me.parent().expect("it is in a folder").to_path_buf();
6293        h.app
6294            .perform(&h.ctx.clone(), Action::Navigate { pane, path: folder });
6295        h.settle();
6296
6297        let showing = |h: &Harness| {
6298            h.app.panes[0]
6299                .tab()
6300                .preview
6301                .showing()
6302                .map(|path| path.to_path_buf())
6303        };
6304
6305        // The panel, open and pointed at nothing yet.
6306        h.app.panes[0].tab_mut().preview.open = true;
6307        h.app.panes[0].tab_mut().clear_selection();
6308        h.frame(Vec::new());
6309
6310        // The keyboard onto the test binary, the way a click leaves it.
6311        let name = me
6312            .file_name()
6313            .expect("it has a name")
6314            .to_string_lossy()
6315            .into_owned();
6316        let at = {
6317            let tab = h.app.panes[0].tab();
6318            let dir = tab.dir.as_ref().expect("the listing arrived");
6319            tab.order
6320                .iter()
6321                .position(|&i| dir.name(i as usize) == name)
6322                .unwrap_or_else(|| panic!("{name} is not in its own folder's listing"))
6323        };
6324        h.app.panes[0].tab_mut().select_only(at);
6325
6326        // Not yet. This is the assertion the wait exists for: holding an arrow key through a
6327        // folder of images must not decode thirty of them.
6328        h.frame(Vec::new());
6329        assert!(
6330            showing(&h).is_none() && h.app.preview_pending(),
6331            "the read started on the keystroke rather than waiting for it to stop"
6332        );
6333
6334        // And then it does, on that file, once.
6335        h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
6336        for attempt in 0..400 {
6337            h.frame(Vec::new());
6338            if !h.app.preview_pending() && attempt > 2 {
6339                break;
6340            }
6341            std::thread::sleep(std::time::Duration::from_millis(2));
6342        }
6343        assert_eq!(
6344            showing(&h).as_deref(),
6345            Some(me.as_path()),
6346            "the panel is showing something else"
6347        );
6348        let texts: Vec<String> = h.texts().into_iter().map(|(_, text)| text).collect();
6349        assert!(
6350            texts.iter().any(|text| text.contains("API set")),
6351            "the walk landed and the panel is not showing it: {texts:?}"
6352        );
6353
6354        // The keyboard moving onto something with no preview clears it, rather than leaving a
6355        // stale answer beside a different row. This panel is *inside* the pane, so what it shows
6356        // is read as being about the selection next to it.
6357        let plain = {
6358            let tab = h.app.panes[0].tab();
6359            let dir = tab.dir.as_ref().expect("the listing");
6360            (0..tab.order.len()).find(|&row| {
6361                tab.entry_at(row).is_some_and(|entry| {
6362                    crate::preview::kind_of(
6363                        dir.leaf(entry),
6364                        dir.ext(entry),
6365                        dir.entries[entry].is_dir(),
6366                    )
6367                    .is_none()
6368                })
6369            })
6370        }
6371        .expect("a build folder holds something with no preview");
6372        h.app.panes[0].tab_mut().select_only(plain);
6373        h.time += crate::ui::preview::FOLLOW_DELAY * 2.0;
6374        h.frame(Vec::new());
6375        h.frame(Vec::new());
6376        assert_eq!(showing(&h), None, "the panel kept a stale answer");
6377    }
6378
6379    /// **Everything in a dependency row is on one baseline.**
6380    ///
6381    /// Three texts at two sizes — a 14-point name, a 12-point location, a 12-point processor
6382    /// tag — and the assertion is *exact* rather than within a tolerance, because the property
6383    /// is exact: the row commits to one `azur::components::ink_baseline` and every galley is
6384    /// placed by subtracting its own ascent from it.
6385    ///
6386    /// What it catches is the way this is usually written. Centring each galley in the row —
6387    /// which is what every other listing in this window does — leaves the two 12-point columns
6388    /// **1.5 points above** the name beside them, because the two fonts differ in line height
6389    /// *and* in ascent and centring the boxes cancels neither. That is invisible in the source,
6390    /// visible on screen as a column that steps down as the eye crosses the row, and this is the
6391    /// test that would fail.
6392    #[test]
6393    fn everything_in_a_dependency_row_sits_on_one_line() {
6394        let mut h = Harness::new();
6395        open_preview(&mut h);
6396
6397        // The panel's rows, which is everything drawn below its header. Taken from what was
6398        // painted rather than from the geometry, so this does not have to re-derive the layout.
6399        let panel = preview_rect(&h);
6400        let rows_area = Rect::from_min_max(
6401            pos2(panel.left(), panel.top() + crate::ui::preview::HEADER),
6402            panel.max,
6403        );
6404        let mut rows: std::collections::BTreeMap<i64, Vec<(f32, String)>> = Default::default();
6405        for (at, text) in h.baselines() {
6406            // By rect and not by a y threshold: the panel is *inside* a pane now, so the sidebar
6407            // and the listing have text at the same heights and only the x tells them apart.
6408            if !rows_area.contains(at) || text.is_empty() {
6409                continue;
6410            }
6411            // Which row it is in, from the baseline itself: rows are `ROW` apart, so anything
6412            // within one of them belongs to the same one.
6413            rows.entry(((at.y - rows_area.top()) / crate::ui::deps::ROW) as i64)
6414                .or_default()
6415                .push((at.y, text));
6416        }
6417        assert!(
6418            rows.len() >= 4,
6419            "the panel drew {} rows of text; there is nothing to compare",
6420            rows.len()
6421        );
6422
6423        let mut widest = 0;
6424        for (which, texts) in &rows {
6425            let first = texts[0].0;
6426            widest = widest.max(texts.len());
6427            for (baseline, text) in texts {
6428                assert_eq!(
6429                    *baseline, first,
6430                    "row {which}: `{text}` sits on {baseline} and `{}` on {first}",
6431                    texts[0].1
6432                );
6433            }
6434        }
6435        // And a row really did have all three columns in it, or the fonts never differed and
6436        // the assertion above proves nothing.
6437        assert!(
6438            widest >= 3,
6439            "no row had a name, a location and a tag in it: at most {widest} texts"
6440        );
6441    }
6442
6443    /// `Ctrl+E` works with the caret in the filter box, and the filter survives the toggle.
6444    ///
6445    /// The two compose, and that is the gesture: type two letters, look at what came up, and
6446    /// want the rest of the tree. A focused text field otherwise owns the keyboard outright —
6447    /// which is right for every other shortcut and wrong for this one.
6448    #[test]
6449    fn ctrl_e_reaches_through_the_filter_box() {
6450        let mut h = Harness::new();
6451        let pane = h.app.panes[0].id;
6452        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
6453        h.app.perform(
6454            &h.ctx.clone(),
6455            Action::Navigate {
6456                pane,
6457                path: sources,
6458            },
6459        );
6460        h.settle();
6461
6462        // Ctrl+F, which is how the caret gets there without a click. Then a second frame: the
6463        // bar asks for focus on the frame it sees the shortcut, and egui grants it at the end.
6464        let held = Modifiers::COMMAND;
6465        h.modifiers = held;
6466        h.frame(vec![Event::Key {
6467            key: egui::Key::F,
6468            physical_key: None,
6469            pressed: true,
6470            repeat: false,
6471            modifiers: held,
6472        }]);
6473        h.modifiers = Modifiers::NONE;
6474        h.frame(Vec::new());
6475        assert!(
6476            h.ctx.memory(|m| m.focused()).is_some(),
6477            "Ctrl+F did not put the caret in the filter box, so this test proves nothing"
6478        );
6479
6480        // A filter typed in, so the other half of the claim can be checked: it survives.
6481        h.app.panes[0].tab_mut().filter = "sort".to_owned();
6482        h.app.panes[0].tab_mut().rebuild_order();
6483        h.frame(Vec::new());
6484        h.take_journal();
6485
6486        h.modifiers = held;
6487        h.frame(vec![Event::Key {
6488            key: egui::Key::E,
6489            physical_key: None,
6490            pressed: true,
6491            repeat: false,
6492            modifiers: held,
6493        }]);
6494        h.modifiers = Modifiers::NONE;
6495        let done = h.take_journal();
6496        assert!(
6497            done.contains(&"ToggleFlat"),
6498            "Ctrl+E did not get through the filter box, got {done:?}"
6499        );
6500        h.settle();
6501
6502        let tab = h.app.panes[0].tab();
6503        assert!(tab.flat);
6504        assert_eq!(tab.filter, "sort", "the filter went with the toggle");
6505        let names = shown_names(&h, 0);
6506        assert!(
6507            !names.is_empty() && names.iter().all(|name| name.contains("sort")),
6508            "the filter is not being applied to the flattened listing: {names:?}"
6509        );
6510        assert!(
6511            names.iter().any(|name| name.contains('\\')),
6512            "nothing from a subfolder came up, so the tree was not walked: {names:?}"
6513        );
6514    }
6515
6516    /// A flatten is a question about *this* folder, so it does not come along to the next one.
6517    ///
6518    /// Which is also what makes opening a row the way out of the view: without this, clicking a
6519    /// folder three levels down in a flattened listing would start a second tree walk on
6520    /// arrival, and there would be no gesture that ended one.
6521    #[test]
6522    fn a_flattened_view_does_not_follow_you_into_the_next_folder() {
6523        let mut h = Harness::new();
6524        let pane = h.app.panes[0].id;
6525        let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
6526        let ctx = h.ctx.clone();
6527        h.app.perform(
6528            &ctx,
6529            Action::Navigate {
6530                pane,
6531                path: sources.clone(),
6532            },
6533        );
6534        h.app.perform(&ctx, Action::ToggleFlat(pane));
6535        h.settle();
6536        assert!(h.app.panes[0].tab().flat);
6537
6538        h.app.perform(
6539            &ctx,
6540            Action::Navigate {
6541                pane,
6542                path: sources.join("ui"),
6543            },
6544        );
6545        h.settle();
6546        assert!(
6547            !h.app.panes[0].tab().flat,
6548            "the flatten followed the navigation"
6549        );
6550        assert!(
6551            shown_names(&h, 0).iter().all(|name| !name.contains('\\')),
6552            "and the listing that arrived was still a flattened one"
6553        );
6554
6555        // A *refresh* is the same question again, so that one keeps it.
6556        h.app.perform(&ctx, Action::ToggleFlat(pane));
6557        h.settle();
6558        h.app.perform(&ctx, Action::Refresh(pane));
6559        h.settle();
6560        assert!(
6561            h.app.panes[0].tab().flat,
6562            "F5 turned the flatten off, which is not what a re-read means"
6563        );
6564    }
6565
6566    #[test]
6567    fn a_folder_dropped_on_the_sidebar_is_bookmarked() {
6568        // The bookmarks group publishes itself as a drop zone, and a drop there pins
6569        // instead of copying. The zone has to *exist*: without it the drag reports "no"
6570        // to the pointer and the drop never happens at all.
6571        let mut h = Harness::new();
6572        let rect = h
6573            .app
6574            .bookmarks_rect
6575            .expect("the bookmarks group did not publish a drop zone");
6576        assert!(rect.height() > 0.0 && rect.width() > 0.0);
6577
6578        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
6579        h.app.perform(
6580            &h.ctx,
6581            Action::AddBookmark(here.join("src")),
6582        );
6583        assert!(
6584            h.app.bookmarks.contains(&here.join("src")),
6585            "pinning is what a drop on that zone performs"
6586        );
6587    }
6588
6589    #[test]
6590    fn bookmarks_reorder_to_where_they_are_dropped() {
6591        // Insertion-point arithmetic, which is where an off-by-one is invisible until a
6592        // list quietly reverses itself.
6593        let ctx = egui::Context::default();
6594        let mut app = App::opening(&ctx, Config::default(), Vec::new(), Side::Right);
6595        app.bookmarks = ["a", "b", "c", "d"].iter().map(PathBuf::from).collect();
6596        let names = |app: &App| -> Vec<String> {
6597            app.bookmarks
6598                .iter()
6599                .map(|p| p.display().to_string())
6600                .collect()
6601        };
6602
6603        // The first one to the end.
6604        app.perform(&ctx, Action::MoveBookmark { from: 0, to: 4 });
6605        assert_eq!(names(&app), ["b", "c", "d", "a"]);
6606        // The last one to the front.
6607        app.perform(&ctx, Action::MoveBookmark { from: 3, to: 0 });
6608        assert_eq!(names(&app), ["a", "b", "c", "d"]);
6609        // One place down: `to` is an insertion point in the list as it was, so 2 means
6610        // "before what is currently at 2".
6611        app.perform(&ctx, Action::MoveBookmark { from: 0, to: 2 });
6612        assert_eq!(names(&app), ["b", "a", "c", "d"]);
6613        // Nowhere, twice: onto itself, and just after itself.
6614        app.perform(&ctx, Action::MoveBookmark { from: 1, to: 1 });
6615        app.perform(&ctx, Action::MoveBookmark { from: 1, to: 2 });
6616        assert_eq!(names(&app), ["b", "a", "c", "d"]);
6617        // Out of range, from a stale drag: nothing moves and nothing panics.
6618        app.perform(&ctx, Action::MoveBookmark { from: 9, to: 0 });
6619        app.perform(&ctx, Action::MoveBookmark { from: 0, to: 9 });
6620        assert_eq!(names(&app), ["b", "a", "c", "d"]);
6621    }
6622
6623    #[test]
6624    fn dragging_a_name_picks_the_file_up_and_dragging_beside_it_bands() {
6625        let mut h = Harness::new();
6626        assert!(h.tab(0).order.len() > 4, "the crate root has rows to drag");
6627
6628        // ---- From the name: the file is picked up ------------------------
6629        let row = h.row_center(0, 0);
6630        let pane = h.pane_rect(0);
6631        // Just past the icon, which is where the first row's name starts.
6632        let on_name = pos2(pane.left() + 46.0, row.y);
6633        let done = h.drag(on_name, pos2(on_name.x + 60.0, on_name.y + 90.0));
6634        assert!(
6635            done.contains(&"DragOut"),
6636            "dragging a name has to pick the file up, got {done:?}"
6637        );
6638        // The OLE drag itself is not started here and cannot be: it needs a window, to find
6639        // the pointer gesture it is following. What is being tested is that the gesture is
6640        // read as a drag of the file, which is `DragOut` being dispatched at all.
6641        assert!(h.app.file_drag.is_none(), "and no drag left in flight");
6642
6643        // ---- From the blank space on the same row: a band ----------------
6644        //
6645        // The gap between the end of the name and the Size column, which is the widest
6646        // blank stretch of any row and the one a user reaches for. *Not* the far right: the
6647        // last column is fitted to its content, so a date long enough fills it right up to
6648        // the edge — and a press there is genuinely on the row's ink, as this test used to
6649        // discover the hard way when the glyph advances moved by a pixel.
6650        h.wait();
6651        let blank = pos2(pane.left() + pane.width() * 0.45, row.y);
6652        let done = h.drag(blank, pos2(blank.x - 40.0, blank.y + crate::pane::ROW_HEIGHT * 3.5));
6653        assert!(
6654            !done.contains(&"DragOut"),
6655            "a drag from the empty part of a row is a band, not a drag of the file: {done:?}"
6656        );
6657        assert!(
6658            h.tab(0).selected_count >= 3,
6659            "the band should have swept the rows it crossed, got {}",
6660            h.tab(0).selected_count
6661        );
6662    }
6663
6664    #[test]
6665    fn the_bare_title_bar_moves_and_maximises_the_window() {
6666        let mut h = Harness::new();
6667        // Between the application mark and the first tab there is bar and nothing else,
6668        // which is where a window is grabbed.
6669        let bar_y = crate::ui::chrome::HEIGHT * 0.5;
6670        let bare = pos2(
6671            crate::ui::chrome::content_left(crate::ui::chrome::bar_rect(Rect::from_min_size(
6672                Pos2::ZERO,
6673                h.size,
6674            ))) + 60.0,
6675            bar_y,
6676        );
6677
6678        let done = h.drag(bare, pos2(bare.x + 80.0, bare.y + 40.0));
6679        assert!(
6680            done.contains(&"Window"),
6681            "dragging the bar has to move the window, got {done:?}"
6682        );
6683
6684        let done = h.double_click_at(bare);
6685        assert!(
6686            done.contains(&"Window"),
6687            "double-clicking the bar has to maximise, got {done:?}"
6688        );
6689    }
6690
6691    #[test]
6692    fn a_pane_stacked_below_another_gets_a_reachable_strip_of_its_own() {
6693        let mut h = Harness::new();
6694        h.app.actions.push(Action::SplitFocused {
6695            side: crate::pane::Side::Bottom,
6696        });
6697        h.settle();
6698        assert_eq!(h.app.panes.len(), 2);
6699
6700        // The lower pane is the one whose top is furthest down.
6701        let lower = h
6702            .app
6703            .panes
6704            .iter()
6705            .max_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
6706            .map(|p| p.id)
6707            .expect("two panes");
6708        let slot = h
6709            .app
6710            .tab_slots
6711            .iter()
6712            .find(|s| s.pane == lower)
6713            .map(|s| s.rect)
6714            .expect("the lower pane has no tab on screen at all");
6715
6716        assert!(
6717            slot.top() > crate::ui::chrome::HEIGHT,
6718            "a pane in the row below cannot have its tabs in the title bar, but {slot:?} is"
6719        );
6720        let pane_top = h
6721            .app
6722            .panes
6723            .iter()
6724            .find(|p| p.id == lower)
6725            .map(|p| p.rect.top())
6726            .unwrap();
6727        assert!(
6728            slot.bottom() <= pane_top,
6729            "the strip has to be above the pane it governs, not inside it"
6730        );
6731
6732        // And it is a tab, not a picture of one.
6733        let index = h.app.tab_slots.iter().position(|s| s.pane == lower).unwrap();
6734        let id = Id::new(("tab", lower, h.app.tab_slots[index].tab));
6735        assert!(
6736            h.hovers(id, slot.center()),
6737            "the tab on the band is not reachable"
6738        );
6739    }
6740
6741    #[test]
6742    fn split_or_not_every_tab_is_in_the_title_bar() {
6743        let mut h = Harness::new();
6744        h.app.panes[0]
6745            .tabs
6746            .push(Tab::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")));
6747        h.settle();
6748        assert_eq!(h.app.tab_slots.len(), 2, "one pane, two tabs");
6749
6750        h.app.actions.push(Action::SplitFocused {
6751            side: crate::pane::Side::Right,
6752        });
6753        h.settle();
6754        assert_eq!(h.app.panes.len(), 2, "the split happened");
6755
6756        // Every tab in the window, whichever pane governs it, is in the title bar — and
6757        // each pane has its own group there, so the tabs are grouped by owner rather than
6758        // merged into one strip.
6759        let bar = crate::ui::chrome::HEIGHT;
6760        for slot in &h.app.tab_slots {
6761            assert!(
6762                slot.rect.top() >= 0.0 && slot.rect.bottom() <= bar,
6763                "a tab for pane {} is at {:?}, outside the title bar",
6764                slot.pane,
6765                slot.rect
6766            );
6767        }
6768        for pane in &h.app.panes {
6769            let group: Vec<&crate::ui::chrome::Slot> = h
6770                .app
6771                .tab_slots
6772                .iter()
6773                .filter(|s| s.pane == pane.id)
6774                .collect();
6775            assert_eq!(
6776                group.len(),
6777                pane.tabs.len(),
6778                "pane {} has {} tabs and {} of them are on screen",
6779                pane.id,
6780                pane.tabs.len(),
6781                group.len()
6782            );
6783        }
6784        // Groups do not interleave: every slot of the left pane is left of every slot of
6785        // the right one, which is what makes the divider between them mean anything.
6786        let left = h.app.pane_order[0];
6787        let split = h
6788            .app
6789            .tab_slots
6790            .iter()
6791            .filter(|s| s.pane == left)
6792            .map(|s| s.rect.right())
6793            .fold(f32::MIN, f32::max);
6794        assert!(
6795            h.app
6796                .tab_slots
6797                .iter()
6798                .filter(|s| s.pane != left)
6799                .all(|s| s.rect.left() >= split),
6800            "the two panes' tabs are mixed together in the bar"
6801        );
6802    }
6803
6804    // ---- Does browsing let go of what it read? --------------------------
6805    //
6806    // "The more I browse, the more memory" has two causes that look identical from Task
6807    // Manager: a cache filling up to its budget and then holding steady, and something that
6808    // never lets go. These tell them apart by measuring, across a few hundred real folders.
6809    //
6810    // Run on their own, because two of the three numbers are process-wide and every other
6811    // test allocates too:
6812    //
6813    //     cargo test --release browsing_hundreds -- --ignored --nocapture --test-threads=1
6814
6815    /// Live heap bytes: every Rust allocation, minus every free. See [`crate::counting`].
6816    fn live_heap() -> isize {
6817        crate::counting::LIVE.load(std::sync::atomic::Ordering::Relaxed)
6818    }
6819
6820    /// Directories under `from`, breadth-first, up to `want` of them.
6821    ///
6822    /// Real folders with real contents: a synthetic tree of empty directories would exercise
6823    /// none of the per-entry storage this is about.
6824    fn folders(from: &Path, want: usize) -> Vec<PathBuf> {
6825        let mut found = Vec::new();
6826        let mut queue = std::collections::VecDeque::from([from.to_path_buf()]);
6827        while let Some(dir) = queue.pop_front() {
6828            if found.len() >= want {
6829                break;
6830            }
6831            let Ok(entries) = std::fs::read_dir(&dir) else {
6832                continue;
6833            };
6834            for entry in entries.flatten() {
6835                if entry.file_type().is_ok_and(|t| t.is_dir()) {
6836                    queue.push_back(entry.path());
6837                    found.push(entry.path());
6838                }
6839            }
6840        }
6841        found
6842    }
6843
6844    #[test]
6845    #[ignore = "measures the whole process; run explicitly, single-threaded"]
6846    fn scrolling_the_same_folder_up_and_down_costs_nothing() {
6847        // The reported case, exactly: one folder, scrolled up and down, the same rows drawn
6848        // over and over. Nothing about that is new work — every name, every icon and every
6849        // formatted size has been seen before — so anything that grows here grows *per frame*
6850        // rather than per folder, which is why leaving the folder would not give it back.
6851        let dir = PathBuf::from(r"C:\Windows");
6852        if !dir.is_dir() {
6853            println!("no C:\\Windows; skipping");
6854            return;
6855        }
6856        let mut h = Harness::new();
6857        h.app.panes[0].tab_mut().navigate(dir);
6858        h.settle();
6859        let rows = h.tab(0).order.len();
6860        assert!(rows > 40, "need a folder taller than the window");
6861
6862        // One full pass first, so every icon, galley and column width is already resolved:
6863        // whatever the first sweep costs is work, not growth.
6864        let sweep = |h: &mut Harness| {
6865            for top in (0..rows).step_by(11) {
6866                h.app.panes[0].tab_mut().scroll_to = Some(top as f32 * crate::pane::ROW_HEIGHT);
6867                h.frame(Vec::new());
6868                h.app.icons.poll(&h.ctx.clone());
6869                h.app.deliver_icons();
6870            }
6871            for top in (0..rows).step_by(11).rev() {
6872                h.app.panes[0].tab_mut().scroll_to = Some(top as f32 * crate::pane::ROW_HEIGHT);
6873                h.frame(Vec::new());
6874            }
6875        };
6876        sweep(&mut h);
6877        h.settle();
6878
6879        let heap0 = live_heap();
6880        let (private0, gdi0, _) = process_memory();
6881        println!("after one pass: heap {} KB, private {} KB, gdi {gdi0}", heap0 / 1024, private0 / 1024);
6882
6883        for pass in 1..=10 {
6884            sweep(&mut h);
6885            let (private, gdi, _) = process_memory();
6886            println!(
6887                "pass {pass:>2}:  heap {:+7} KB   private {:+7} KB   gdi {gdi:>4}",
6888                (live_heap() - heap0) / 1024,
6889                (private as isize - private0 as isize) / 1024
6890            );
6891        }
6892
6893        // Ten more passes over rows that were already drawn ten times must cost nothing.
6894        // A megabyte of slack for egui's own per-frame reuse.
6895        assert!(
6896            live_heap() - heap0 < (1 << 20),
6897            "scrolling the same folder grew the heap by {:+} KB -- something allocates per \
6898             frame and keeps it",
6899            (live_heap() - heap0) / 1024
6900        );
6901    }
6902
6903    #[test]
6904    #[ignore = "measures the whole process; run explicitly, single-threaded"]
6905    fn what_a_very_large_folder_costs() {
6906        // A tab holds its listing, its display order and a selection flag per entry, and the
6907        // cache holds the listing again until it is evicted. This is what one folder of
6908        // 27,000 comes to, and it is the number to multiply by if a session keeps several
6909        // such folders open in tabs.
6910        let dir = PathBuf::from(r"C:\Windows\WinSxS");
6911        if !dir.is_dir() {
6912            println!("no WinSxS; skipping");
6913            return;
6914        }
6915        let mut h = Harness::new();
6916        h.settle();
6917        let (private0, _, _) = process_memory();
6918        let heap0 = live_heap();
6919
6920        h.app.panes[0].tab_mut().navigate(dir);
6921        h.settle();
6922        let rows = h.tab(0).order.len();
6923        let (private, _, _) = process_memory();
6924        println!(
6925            "{rows} entries:  heap {:+} KB   private {:+} KB   = {} bytes an entry (heap)",
6926            (live_heap() - heap0) / 1024,
6927            (private as isize - private0 as isize) / 1024,
6928            (live_heap() - heap0) / rows.max(1) as isize
6929        );
6930
6931        // Leave it, and see what comes back once neither the tab nor the cache holds it.
6932        h.app.panes[0].tab_mut().navigate(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
6933        h.settle();
6934        h.app.loader.invalidate(Path::new(r"C:\Windows\WinSxS"));
6935        h.settle();
6936        println!(
6937            "after leaving and dropping it from the cache: heap {:+} KB",
6938            (live_heap() - heap0) / 1024
6939        );
6940    }
6941
6942    #[test]
6943    #[ignore = "measures the whole process; run explicitly, single-threaded"]
6944    fn what_scrolling_a_folder_of_executables_costs() {
6945        // The worst case for icons, and the one a synthetic walk of small folders never
6946        // reaches: a folder of thousands of files that each carry their own icon. Every one
6947        // that comes on screen is a separate question to the shell, which opens the file and
6948        // reads its resources — so this is where "memory grows as I browse" would come from.
6949        let dir = PathBuf::from(r"C:\Windows\System32");
6950        if !dir.is_dir() {
6951            println!("no System32; skipping");
6952            return;
6953        }
6954
6955        let mut h = Harness::new();
6956        h.app.panes[0].tab_mut().navigate(dir);
6957        h.settle();
6958        let rows = h.tab(0).order.len();
6959        println!("{rows} rows");
6960
6961        let (private0, gdi0, _) = process_memory();
6962        let heap0 = live_heap();
6963        // Scroll the whole listing past, a screenful at a time, so every row is drawn once.
6964        let step = 20;
6965        for top in (0..rows).step_by(step) {
6966            h.app.panes[0].tab_mut().scroll_to = Some(top as f32 * crate::pane::ROW_HEIGHT);
6967            for _ in 0..3 {
6968                h.frame(Vec::new());
6969            }
6970            h.app.icons.poll(&h.ctx.clone());
6971            h.app.deliver_icons();
6972            if top % (step * 40) == 0 {
6973                let (private, gdi, _) = process_memory();
6974                let (kinds, paths, textures) = h.app.icons.held();
6975                println!(
6976                    "row {top:>5}:  private {:+8} KB   heap {:+7} KB   gdi {gdi:>5}   \
6977                     icons {kinds:>4}/{paths:>5}/{textures:>4}",
6978                    (private as isize - private0 as isize) / 1024,
6979                    (live_heap() - heap0) / 1024
6980                );
6981                let _ = gdi0;
6982            }
6983        }
6984        let (private, gdi, _) = process_memory();
6985        let (kinds, paths, textures) = h.app.icons.held();
6986        println!(
6987            "after the whole listing: private {:+} KB   gdi {gdi}   icons {kinds}/{paths}/{textures}",
6988            (private as isize - private0 as isize) / 1024
6989        );
6990    }
6991
6992    /// The whole chain, through the app: a right click opens a menu straight away, the
6993    /// builder thread fills in the shell's entries, and hovering a submenu fetches that.
6994    ///
6995    /// The pieces are tested where they live -- `shell::menu` for the shell, `ui::menu` for
6996    /// the drawing -- and what is only tested here is the wiring between them: `pump_menu`
6997    /// taking delivery against the right token, and draining what the menu asked for on the
6998    /// way back out. That wiring is easy to get subtly wrong and impossible to notice,
6999    /// because a menu that never fills its submenus looks exactly like a menu whose
7000    /// submenus are empty.
7001    #[test]
7002    #[cfg(windows)]
7003    fn a_right_click_opens_a_menu_now_and_fills_it_from_the_shell() {
7004        let _serialised = crate::shell::serialised();
7005        let mut h = Harness::new();
7006
7007        // What a baseline frame of this window costs, to compare the asking one against.
7008        let mut baseline = std::time::Duration::ZERO;
7009        for _ in 0..5 {
7010            let at = std::time::Instant::now();
7011            h.frame(Vec::new());
7012            baseline = baseline.max(at.elapsed());
7013        }
7014
7015        h.app.open_folder_menu(&h.ctx.clone());
7016        let at = std::time::Instant::now();
7017        h.frame(Vec::new());
7018        let asking = at.elapsed();
7019        // The cheapest `QueryContextMenu` measured on this machine was 130 ms, for a folder;
7020        // a file was 130-690. So a frame that raised the menu and came in well under a tenth
7021        // of a second did not ask the shell anything on the way, which is the whole point.
7022        // Compared against a frame of the same window rather than against a constant, because
7023        // the constant that matters is the shell's and this bound only has to be under it.
7024        assert!(
7025            asking < baseline + std::time::Duration::from_millis(100),
7026            "the frame that raised the menu took {asking:?} against a {baseline:?} baseline \
7027             -- something on it went to the shell"
7028        );
7029        // And nothing is on screen yet: a menu that appeared here would be a menu that grew
7030        // afterwards, which is what this deliberately does not do.
7031        assert!(h.app.menu.is_none(), "the menu appeared before it was ready");
7032        assert!(h.app.menu_pending(), "and it is not on its way either");
7033
7034        // It arrives whole, a moment later.
7035        let waited = std::time::Instant::now();
7036        while h.app.menu_pending() {
7037            h.frame(Vec::new());
7038            assert!(
7039                waited.elapsed() < std::time::Duration::from_secs(20),
7040                "the builder never delivered"
7041            );
7042        }
7043        let menu = h.app.menu.as_ref().expect("open by now");
7044        // Windows' menu, and nothing of ours in front of it. This program used to put half a
7045        // dozen entries of its own above the shell's — they are gone, and the only entries it
7046        // still owns anywhere are the three a right-button drop asks.
7047        assert!(
7048            menu.entries.len() > 3,
7049            "the shell should have filled the menu: {:?}",
7050            menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
7051        );
7052        let ours: Vec<&String> = menu
7053            .entries
7054            .iter()
7055            .filter(|e| {
7056                matches!(
7057                    e.kind,
7058                    crate::shell::menu::Kind::Command(crate::shell::menu::Command::Own(_))
7059                )
7060            })
7061            .map(|e| &e.label)
7062            .collect();
7063        assert!(
7064            ours.is_empty(),
7065            "a file's menu should be Windows' own, with nothing of ours in it: {ours:?}"
7066        );
7067
7068        // Every shell submenu is there and empty, which is the point of the lazy fill.
7069        let submenus: Vec<usize> = menu
7070            .entries
7071            .iter()
7072            .enumerate()
7073            .filter(|(_, e)| e.kind.unasked().is_some())
7074            .map(|(i, _)| i)
7075            .collect();
7076        assert!(
7077            !submenus.is_empty(),
7078            "a folder's menu has 7-Zip, Send To or Give access to in it: {:?}",
7079            menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
7080        );
7081
7082        // Opening each one asks for it, and what comes back has to be what the shell has in
7083        // it. Every one of them, and counting what arrived rather than merely that an answer
7084        // did: an answer of "nothing" also marks a submenu filled, so a version of this that
7085        // waited for the flag passed while every submenu in the program was empty.
7086        let mut counts: Vec<(String, usize)> = Vec::new();
7087        for index in submenus {
7088            h.app.menu.as_mut().unwrap().open = vec![index];
7089            let waited = std::time::Instant::now();
7090            loop {
7091                h.frame(Vec::new());
7092                let done = h
7093                    .app
7094                    .menu
7095                    .as_ref()
7096                    .is_some_and(|m| m.entries[index].kind.unasked().is_none());
7097                if done {
7098                    break;
7099                }
7100                assert!(
7101                    waited.elapsed() < std::time::Duration::from_secs(20),
7102                    "the submenu was never filled -- nothing carried the ask to the builder"
7103                );
7104            }
7105            let menu = h.app.menu.as_ref().unwrap();
7106            let count = match &menu.entries[index].kind {
7107                crate::shell::menu::Kind::Submenu { children, .. } => children.len(),
7108                _ => 0,
7109            };
7110            counts.push((menu.entries[index].label.clone(), count));
7111        }
7112        assert!(
7113            counts.iter().any(|(_, count)| *count > 0),
7114            "every submenu in the menu came back empty: {counts:?}"
7115        );
7116
7117        h.app.close_menu();
7118        h.frame(Vec::new());
7119    }
7120
7121    /// A menu the shell has not finished making can be given up on, and says so while it lasts.
7122    ///
7123    /// The window used to have nothing to say about a menu that was on its way, which is fine
7124    /// for the tenth of a second a folder takes. On an executable on a mapped share it is
7125    /// twenty-four seconds — measured, see [`crate::shell::menu::Builder`] — and twenty-four
7126    /// seconds of a window that shows nothing, reacts to nothing you can see, and then puts up a
7127    /// menu at a place you have long since stopped pointing at is not a wait, it is a fault.
7128    ///
7129    /// So there is a cursor for it, and Escape means never mind. What Escape cannot do is stop
7130    /// `QueryContextMenu`, so what is tested at the end is the part that matters: that the answer
7131    /// nobody wants any more does not arrive later and open a menu on its own.
7132    #[test]
7133    #[cfg(windows)]
7134    fn escape_gives_up_on_a_menu_the_shell_is_still_making() {
7135        use std::sync::atomic::Ordering;
7136
7137        let _serialised = crate::shell::serialised();
7138        let mut h = Harness::new();
7139        h.settle();
7140
7141        // Stand in for the share. Long enough to press Escape inside, short enough that the
7142        // abandoned worker is finished before the assertion that it changed nothing.
7143        const STALL: u64 = 1_500;
7144        crate::shell::menu::STALLED.store(0, Ordering::SeqCst);
7145        crate::shell::menu::STALL_MS.store(STALL, Ordering::SeqCst);
7146
7147        h.app.open_folder_menu(&h.ctx.clone());
7148        h.frame(Vec::new());
7149        // Cleared once the worker has picked it up, and not before: clearing it straight after
7150        // the frame races the thread that is about to read it.
7151        let waited = std::time::Instant::now();
7152        while crate::shell::menu::STALLED.load(Ordering::SeqCst) == 0 {
7153            h.frame(Vec::new());
7154            assert!(
7155                waited.elapsed() < std::time::Duration::from_secs(5),
7156                "the worker never started the stalled build"
7157            );
7158        }
7159        crate::shell::menu::STALL_MS.store(0, Ordering::SeqCst);
7160        assert!(h.app.menu_pending(), "the ask never went out");
7161        assert!(h.app.menu.is_none());
7162
7163        // A second frame, and the cursor says the window is working on something.
7164        h.frame(Vec::new());
7165        assert!(h.app.menu_pending(), "the stalled build answered immediately");
7166        assert_eq!(
7167            h.cursor,
7168            egui::CursorIcon::Progress,
7169            "nothing on screen says a menu is being waited for"
7170        );
7171
7172        h.frame(vec![Event::Key {
7173            key: egui::Key::Escape,
7174            physical_key: None,
7175            pressed: true,
7176            repeat: false,
7177            modifiers: Modifiers::NONE,
7178        }]);
7179        assert!(
7180            !h.app.menu_pending(),
7181            "Escape did not give up on the menu that was being built"
7182        );
7183
7184        // And the answer, when it finally comes, is nobody's: no menu appears out of the blue a
7185        // second and a half after the click that asked for it was called off.
7186        let waited = std::time::Instant::now();
7187        while waited.elapsed() < std::time::Duration::from_millis(STALL + 800) {
7188            h.frame(Vec::new());
7189            assert!(
7190                h.app.menu.is_none() && !h.app.menu_pending(),
7191                "the abandoned menu turned up {:?} after Escape",
7192                waited.elapsed()
7193            );
7194            std::thread::sleep(std::time::Duration::from_millis(10));
7195        }
7196        assert_eq!(
7197            crate::shell::menu::STALLED.load(Ordering::SeqCst),
7198            1,
7199            "the build under test never actually stalled, so nothing here was given up on"
7200        );
7201    }
7202
7203    /// `Reset window size` asks the platform for the default size, and un-maximises on the way.
7204    ///
7205    /// The order is the part worth pinning. A window that has been maximised is the one most
7206    /// likely to be sitting there when somebody reaches for this, and asking for a size while
7207    /// still maximised is asking for something a window manager is entitled to ignore. Windows
7208    /// happens not to — measured: from a maximised 2560×1392 the window comes back to 1024×600
7209    /// with the un-maximise taken out, because `SetWindowPos` restores on the way — but that is
7210    /// winit's platform behaviour rather than a promise, so the state is asked for explicitly.
7211    #[test]
7212    fn reset_window_size_asks_for_the_default_and_unmaximises_first() {
7213        use egui::ViewportCommand as Cmd;
7214
7215        let mut h = Harness::new();
7216        h.settle();
7217        let [w, hh] = crate::config::WINDOW_SIZE;
7218
7219        // From maximised: both commands, un-maximise first.
7220        h.app.maximized = true;
7221        h.commands.clear();
7222        h.app
7223            .perform(&h.ctx.clone(), Action::Window(WindowAction::ResetSize));
7224        // Checked before the next frame, not after. The harness feeds a viewport of a fixed size
7225        // for ever, so the frame that follows re-samples that and writes it back over this —
7226        // which is the right behaviour against a real window, where the frame that follows a
7227        // resize is the one that sees the new shape.
7228        assert!(!h.app.maximized, "the flag still says maximised");
7229        assert_eq!(h.app.window_size, Some([w, hh]));
7230
7231        h.frame(Vec::new());
7232        let asked: Vec<&Cmd> = h
7233            .commands
7234            .iter()
7235            .filter(|c| matches!(c, Cmd::Maximized(_) | Cmd::InnerSize(_)))
7236            .collect();
7237        assert_eq!(
7238            asked,
7239            vec![&Cmd::Maximized(false), &Cmd::InnerSize(egui::vec2(w, hh))],
7240            "from maximised, the window was asked for {asked:?}"
7241        );
7242
7243        // From a restored window there is nothing to un-maximise, so only the size is asked for.
7244        h.app.maximized = false;
7245        h.app.window_size = Some([1380.0, 840.0]);
7246        h.commands.clear();
7247        h.app
7248            .perform(&h.ctx.clone(), Action::Window(WindowAction::ResetSize));
7249        h.frame(Vec::new());
7250        let asked: Vec<&Cmd> = h
7251            .commands
7252            .iter()
7253            .filter(|c| matches!(c, Cmd::Maximized(_) | Cmd::InnerSize(_)))
7254            .collect();
7255        assert_eq!(
7256            asked,
7257            vec![&Cmd::InnerSize(egui::vec2(w, hh))],
7258            "a restored window should only be asked for a size: {asked:?}"
7259        );
7260
7261        // And the default is one value, not two: the size the window opens at on a first run is
7262        // the size this puts it back to.
7263        assert_eq!(Config::default().window.unwrap_or(crate::config::WINDOW_SIZE), [w, hh]);
7264    }
7265
7266    /// Double-clicking the sidebar splitter puts it back where it started.
7267    ///
7268    /// The gesture the column edges in the listing already answer to, and the way out of a
7269    /// sidebar dragged somewhere silly. Driven through the real splitter rather than by calling
7270    /// something, because what is easy to get wrong here is the grip's rect and its `Sense`: a
7271    /// `drag`-only splitter reports no clicks at all, and the reset would be dead code.
7272    #[test]
7273    fn double_clicking_the_sidebar_splitter_restores_its_width() {
7274        let mut h = Harness::new();
7275        h.settle();
7276
7277        let default = crate::config::SIDEBAR_WIDTH;
7278        assert_eq!(
7279            h.app.sidebar_width, default,
7280            "the harness did not start at the default width"
7281        );
7282
7283        // Somewhere silly, the way a drag would leave it.
7284        h.app.sidebar_width = 380.0;
7285        h.frame(Vec::new());
7286        let dragged = h.app.sidebar_width;
7287        assert_eq!(dragged, 380.0);
7288
7289        // Found by asking the splitter itself, rather than by re-deriving where the layout put
7290        // it: a test that computes the grip's x is a test of this test's arithmetic.
7291        let grip = egui::Id::new("sidebar-grip");
7292        let at = (0..40)
7293            .map(|step| pos2(dragged - 8.0 + step as f32 * 0.5, 300.0))
7294            .find(|&at| h.hovers(grip, at))
7295            .expect("the sidebar splitter is not reachable by the pointer at all");
7296        h.double_click_at(at);
7297        assert_eq!(
7298            h.app.sidebar_width, default,
7299            "double-clicking the splitter at {at:?} left the sidebar at {}",
7300            h.app.sidebar_width
7301        );
7302        // Not `config_dirty`: the frame after the one that sets it writes the settings and
7303        // clears it again, so by the time this can look it is already false — which is the flag
7304        // doing its job rather than a fault. That it *was* set is covered by the settings file
7305        // holding `sidebar_width=200` after a real window is dragged wider and double-clicked
7306        // back, which is a thing to check with a real window and not from here.
7307    }
7308
7309    /// The panels are square, flush, and separated by one line of the selected tab's colour.
7310    ///
7311    /// Asserted against the shapes the frame actually painted, not against the source. Three
7312    /// separate things had to agree for the old look — a corner radius, a `stroke-subtle` ring
7313    /// and a four-point gap — and each of them was written down somewhere else, which is how a
7314    /// window ends up with panels that read as loose cards without anyone having decided that.
7315    #[test]
7316    fn the_panels_are_square_and_a_single_line_apart() {
7317        let mut h = Harness::with_panes(2);
7318        h.settle();
7319
7320        let seam = crate::ui::seam(&h.app.theme);
7321        let layer = h.app.theme.bg.layer;
7322
7323        // ---- The gaps, from the layout ------------------------------------
7324        let body = Rect::from_min_max(
7325            pos2(0.0, crate::ui::chrome::bar_rect(Rect::from_min_size(Pos2::ZERO, h.size)).bottom()),
7326            Pos2::ZERO + h.size,
7327        );
7328        let (sidebar, panes_area) = App::split_body(body, h.app.sidebar_width);
7329        assert_eq!(
7330            panes_area.left() - sidebar.right(),
7331            crate::ui::SEAM,
7332            "the sidebar and the panes are {} apart",
7333            panes_area.left() - sidebar.right()
7334        );
7335
7336        let mut rects: Vec<Rect> = h.app.pane_rects.iter().map(|(_, r)| *r).collect();
7337        rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
7338        assert_eq!(rects.len(), 2, "this test wants two panes side by side");
7339        assert_eq!(
7340            rects[1].left() - rects[0].right(),
7341            crate::ui::SEAM,
7342            "two panes are {} apart",
7343            rects[1].left() - rects[0].right()
7344        );
7345
7346        // ---- The fills, from the frame ------------------------------------
7347        //
7348        // The seam is not a shape of its own: it is what the fill behind the whole block leaves
7349        // showing, so what is checked is that the block is there, in that colour, under panels
7350        // that are square and do not cover it.
7351        let (corner, fill) = h
7352            .fill_at(sidebar)
7353            .expect("nothing was painted at the sidebar's rect");
7354        assert_eq!(fill, layer, "the sidebar is not `background-layer`");
7355        assert_eq!(corner, egui::CornerRadius::ZERO, "the sidebar has rounded corners");
7356
7357        for rect in &rects {
7358            let (corner, fill) = h
7359                .fill_at(*rect)
7360                .unwrap_or_else(|| panic!("nothing was painted at the pane's rect {rect:?}"));
7361            assert_eq!(fill, layer, "a pane is not `background-layer`");
7362            assert_eq!(corner, egui::CornerRadius::ZERO, "a pane has rounded corners");
7363        }
7364
7365        let block = sidebar.union(panes_area);
7366        let (corner, fill) = h
7367            .fill_at(block)
7368            .expect("nothing is painted behind the panels for the seams to show");
7369        assert_eq!(fill, seam, "the seams are not the selected tab's colour");
7370        assert_eq!(corner, egui::CornerRadius::ZERO);
7371
7372        // ---- And nothing but the window's border outside them --------------
7373        //
7374        // The block fills the body: no canvas ring, and the panels reach three of the window's
7375        // four edges. Written as edges rather than as "no gutter" because that is the thing that
7376        // can be seen — a stripe of `background-canvas` down the side of the window.
7377        assert_eq!(block, body, "there is canvas showing around the panels");
7378        assert_eq!(sidebar.left(), 0.0, "the sidebar stops short of the window");
7379        assert_eq!(
7380            rects[1].right(),
7381            h.size.x,
7382            "the last pane stops short of the window's right edge"
7383        );
7384        for rect in &rects {
7385            assert_eq!(
7386                rect.bottom(),
7387                h.size.y,
7388                "a pane stops short of the window's bottom edge"
7389            );
7390        }
7391    }
7392
7393    /// Opening a rename leaves the name exactly where it was.
7394    ///
7395    /// It used to jump two points right and one point down — a field brings its own padding and
7396    /// its own idea of where a line sits — and on the one word you are looking at that reads as a
7397    /// flinch. Measured here the same way it was found: the position of the *text* in the frame
7398    /// before and the frame after, which is the thing the eye is complaining about. Asserting on
7399    /// the field's rect instead would only be checking this test's own arithmetic.
7400    #[test]
7401    fn opening_a_rename_does_not_move_the_name() {
7402        let mut h = Harness::new();
7403        h.settle();
7404        let pane = h.app.panes[0].id;
7405
7406        let name = {
7407            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
7408            tab.select_only(0);
7409            let dir = tab.dir.clone().expect("a listing");
7410            let entry = tab.entry_at(0).expect("a first row");
7411            dir.name(entry).to_owned()
7412        };
7413        h.frame(Vec::new());
7414
7415        let find = |h: &Harness, what: &str| {
7416            h.texts()
7417                .into_iter()
7418                .find(|(_, text)| text == &name)
7419                .map(|(at, _)| at)
7420                .unwrap_or_else(|| panic!("`{name}` was not drawn {what}"))
7421        };
7422        let before = find(&h, "in the listing");
7423
7424        h.app.perform(&h.ctx.clone(), Action::BeginRename(pane));
7425        h.frame(Vec::new());
7426        assert!(
7427            h.tab(0).renaming.is_some(),
7428            "the rename did not open, so this test proves nothing"
7429        );
7430        let after = find(&h, "in the rename field");
7431
7432        assert_eq!(
7433            after, before,
7434            "the name moved by {:?} when the rename opened",
7435            after - before
7436        );
7437    }
7438
7439    /// Arriving in a text field selects what is in it, so the next keystroke replaces it.
7440    ///
7441    /// Asserted by *typing* rather than by reading egui's cursor state, for two reasons. The
7442    /// selection lives in `TextEdit`'s state under an id the design system generates internally,
7443    /// so there is nothing to read from out here without the library handing it over. And "the
7444    /// text is selected" is not the point — "one keystroke replaces the path" is, and that is a
7445    /// claim about what happens when you type, which is a thing a test can do.
7446    #[test]
7447    fn arriving_in_a_field_selects_what_is_there() {
7448        let mut h = Harness::new();
7449        h.settle();
7450        let pane = h.app.panes[0].id;
7451
7452        // ---- The path field ------------------------------------------------
7453        {
7454            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
7455            tab.editing_path = true;
7456            tab.edit_text = r"C:\Windows\System32".to_owned();
7457        }
7458        // It asks for focus itself on the frame it opens; two frames for egui to grant it and
7459        // for the field to see it arrive.
7460        h.frame(Vec::new());
7461        h.frame(Vec::new());
7462        h.frame(vec![Event::Text("D".to_owned())]);
7463        assert_eq!(
7464            h.tab(0).edit_text,
7465            "D",
7466            "typing into a freshly opened path field appended instead of replacing"
7467        );
7468        h.app.pane_mut(pane).expect("the pane").tab_mut().editing_path = false;
7469        h.frame(Vec::new());
7470
7471        // ---- The filter, reached by clicking it ----------------------------
7472        {
7473            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
7474            tab.filter = "old".to_owned();
7475        }
7476        h.settle();
7477        // Found by sweeping the bar rather than by deriving the box's rect: a text field is what
7478        // asks for `CursorIcon::Text`, and coming in from the right edge the filter is the first
7479        // thing that does. (The empty part of the breadcrumb asks for it too, and is further
7480        // left.)
7481        //
7482        // The whole run of it, and then the middle — not the first point that answered. The
7483        // clearable ✕ sits at the right-hand end of the box and is registered *after* the text
7484        // area, so it wins the pointer there; clicking the edge of the run emptied the filter
7485        // instead of typing into it, which is a real click target doing its real job.
7486        let y = h.path_bar_y(0);
7487        let right = h.pane_rect(0).right();
7488        let mut run: Vec<f32> = Vec::new();
7489        for dx in (8..400).step_by(2) {
7490            let at = pos2(right - dx as f32, y);
7491            h.frame(vec![Event::PointerMoved(at)]);
7492            if h.cursor == egui::CursorIcon::Text {
7493                run.push(at.x);
7494            } else if !run.is_empty() {
7495                break;
7496            }
7497        }
7498        assert!(!run.is_empty(), "the filter box is not reachable by the pointer");
7499        let at = pos2((run[0] + run[run.len() - 1]) * 0.5, y);
7500        h.click_at(at);
7501        h.frame(vec![Event::Text("n".to_owned())]);
7502        assert_eq!(
7503            h.tab(0).filter,
7504            "n",
7505            "clicking into the filter and typing appended instead of replacing"
7506        );
7507    }
7508
7509    /// Every text field in the window is square, and wears the fill it is supposed to.
7510    ///
7511    /// All three at once, in one frame, because they do not come from one place: the rename field
7512    /// is egui's `TextEdit` and takes its frame from `Style::interact`, while the path field and
7513    /// the filter are Azur's `TextField` and took theirs from a hard-coded `control_radius()`
7514    /// until the design system was changed to read the installed style too. One mechanism now —
7515    /// `ui::squared` — but a test that checked only one of them would pass while the other stayed
7516    /// a bubble.
7517    ///
7518    /// Found by what a field *is* rather than by re-deriving where the layout put it — a test that
7519    /// computes a field's rect is a test of its own arithmetic. `background-control` is the fill,
7520    /// and that alone is not enough: in the dark theme it is the same `GRAY_4` as `stroke-subtle`,
7521    /// so the block behind the panels, the path bar, the selected tab, the divider on the bar and
7522    /// the drive gauges all match it too. Adding "no taller than a control, and wider than a line"
7523    /// leaves exactly the three, and the count is asserted so that stops being true loudly.
7524    #[test]
7525    fn the_text_fields_are_square_and_wear_the_right_fill() {
7526        let mut h = Harness::new();
7527        h.settle();
7528
7529        let pane = h.app.panes[0].id;
7530        let ctx = h.ctx.clone();
7531        // The filter is up whenever the pane is wide enough, which it is. The other two have to
7532        // be opened.
7533        {
7534            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
7535            tab.select_only(0);
7536            tab.filter = "r".to_owned();
7537            tab.editing_path = true;
7538        }
7539        h.app.perform(&ctx, Action::BeginRename(pane));
7540        h.frame(Vec::new());
7541
7542        // Two fills, not one: the path field and the filter are `background-control`, and the
7543        // rename field is `background-layer` on purpose — it stands in for a row and has to look
7544        // like the panel rather than like a control dropped on top of one.
7545        let fills = [h.app.theme.bg.control, h.app.theme.bg.layer];
7546        let field_shaped = |rect: &Rect| {
7547            rect.height() >= 16.0
7548                && rect.height() <= azur::tokens::control::SMALL + 0.5
7549                && rect.width() >= 40.0
7550        };
7551        let fields: Vec<(Rect, egui::CornerRadius, egui::Color32)> = h
7552            .rects()
7553            .into_iter()
7554            .filter(|(rect, _, fill)| fills.contains(fill) && field_shaped(rect))
7555            .collect();
7556        assert_eq!(
7557            fields.len(),
7558            3,
7559            "expected the rename field, the path field and the filter, found {}: {:?}",
7560            fields.len(),
7561            fields.iter().map(|(r, _, _)| *r).collect::<Vec<_>>()
7562        );
7563        // All of them, not the first: the three come from two different widgets, and a failure
7564        // that names only one leaves you guessing whether the other is square or merely earlier
7565        // in the paint order.
7566        let rounded: Vec<Rect> = fields
7567            .iter()
7568            .filter(|(_, corner, _)| *corner != egui::CornerRadius::ZERO)
7569            .map(|(rect, _, _)| *rect)
7570            .collect();
7571        assert!(
7572            rounded.is_empty(),
7573            "{} of 3 text fields have rounded corners: {rounded:?}",
7574            rounded.len()
7575        );
7576
7577        // And exactly one of them is the panel's own colour: the rename field, which stands in for
7578        // a row. `background-control` on that one drew a grey slab over the name.
7579        let on_panel = fields
7580            .iter()
7581            .filter(|(_, _, fill)| *fill == h.app.theme.bg.layer)
7582            .count();
7583        assert_eq!(
7584            on_panel, 1,
7585            "the rename field should be the only `background-layer` one of the three"
7586        );
7587    }
7588
7589    /// The window's resize band does not swallow the listing's scrollbar.
7590    ///
7591    /// This is the bill for the panels reaching the window's edges. The east band is registered
7592    /// last and so wins any click in the right-hand four points, which used to be canvas and is
7593    /// now the outer edge of a `10`-point scrollbar. Six points is enough — but "enough" is a
7594    /// claim about a click target, and the only way to know is to press the pointer down there
7595    /// and see whether the listing moves.
7596    #[test]
7597    fn the_scrollbar_survives_the_window_resize_band() {
7598        let mut h = Harness::new();
7599        // Short enough that the folder overflows and there is a bar to grab at all.
7600        h.size = egui::vec2(1024.0, 300.0);
7601        h.settle();
7602        assert_eq!(h.app.panes[0].tab().scroll_y, 0.0, "it starts at the top");
7603
7604        // As far out as the pointer can go and still be the scrollbar's: one point inside the
7605        // band, which is the pixel this test exists to defend.
7606        let x = h.size.x - crate::ui::GUTTER - 1.0;
7607        let y = h.size.y * 0.5;
7608        h.frame(vec![Event::PointerMoved(pos2(x, y))]);
7609        h.frame(vec![Event::PointerButton {
7610            pos: pos2(x, y),
7611            button: PointerButton::Primary,
7612            pressed: true,
7613            modifiers: Modifiers::NONE,
7614        }]);
7615        for step in 1..=4 {
7616            h.frame(vec![Event::PointerMoved(pos2(x, y + step as f32 * 10.0))]);
7617        }
7618        let scrolled = h.app.panes[0].tab().scroll_y;
7619        h.frame(vec![Event::PointerButton {
7620            pos: pos2(x, y + 40.0),
7621            button: PointerButton::Primary,
7622            pressed: false,
7623            modifiers: Modifiers::NONE,
7624        }]);
7625        assert!(
7626            scrolled > 0.0,
7627            "dragging the scrollbar {} points from the window's edge scrolled nothing",
7628            h.size.x - x
7629        );
7630    }
7631
7632    /// The path bar is the selected tab's colour, and the tab is the same colour it is.
7633    ///
7634    /// Both from one frame, because the point of the change is that they *match*: a test that
7635    /// checked the bar alone would pass just as well if the tab drifted, and the two are painted
7636    /// by different modules.
7637    #[test]
7638    fn the_path_bar_and_the_selected_tab_are_one_surface() {
7639        let mut h = Harness::new();
7640        h.settle();
7641
7642        let seam = crate::ui::seam(&h.app.theme);
7643        // The pane's own rect, not an inset of it: the bar reaches the seams on both sides.
7644        let pane = h.app.pane_rects[0].1;
7645        let bar = Rect::from_min_size(
7646            pane.min,
7647            egui::vec2(pane.width(), crate::ui::breadcrumb::HEIGHT),
7648        );
7649
7650        let (corner, fill) = h
7651            .fill_at(bar)
7652            .expect("nothing was painted at the path bar's rect");
7653        assert_eq!(fill, seam, "the path bar is not the selected tab's colour");
7654        assert_eq!(corner, egui::CornerRadius::ZERO);
7655
7656        // The tab above it, found among the slots this frame resolved rather than derived.
7657        let tab = h
7658            .app
7659            .tab_slots
7660            .iter()
7661            .find(|slot| slot.pane == h.app.focused && slot.tab == 0)
7662            .map(|slot| slot.rect)
7663            .expect("the focused pane's tab was not laid out");
7664        // Its fill reaches a point past its own rect, over the strip's bottom hairline. That
7665        // overhang *is* the weld — with the tab stopping at its rect, a line of `stroke-subtle`
7666        // ran between two surfaces of the same colour — so the test looks for the welded rect
7667        // and would fail if the tab went back to painting only itself.
7668        let welded = Rect::from_min_max(tab.min, pos2(tab.max.x, tab.max.y + 1.0));
7669        let filled = h
7670            .rects()
7671            .into_iter()
7672            .rev()
7673            .find(|(rect, _, _)| {
7674                rect.min.distance(welded.min) < 0.5 && rect.max.distance(welded.max) < 0.5
7675            })
7676            .expect("the focused pane's tab does not reach over the strip's bottom edge");
7677        assert_eq!(
7678            filled.2, seam,
7679            "the focused pane's selected tab is not the colour its path bar is"
7680        );
7681    }
7682
7683    /// Which items get the short menu, and that a short one admits to being short.
7684    ///
7685    /// The rule is narrow on purpose. What is slow is not the network: on the share this was
7686    /// measured against, a 41 MB `.lib` builds a full menu in 1.8 s and the folder itself in
7687    /// 0.2 s. It is an *executable* on a share, where the time is linear in the file's size
7688    /// because something reads all of it. A blanket rule for network paths would drop 7-Zip and
7689    /// Send To from every file on the share to fix a problem only executables have.
7690    #[test]
7691    #[cfg(windows)]
7692    fn only_a_network_executable_gets_the_short_menu() {
7693        use crate::shell::menu::Depth;
7694
7695        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
7696        let unc = PathBuf::from(r"\\somehost\someshare\bin");
7697
7698        for (what, folder, items, want) in [
7699            (
7700                "a local executable",
7701                here.clone(),
7702                vec![here.join("thing.exe")],
7703                Depth::Full,
7704            ),
7705            (
7706                "an executable on a share",
7707                unc.clone(),
7708                vec![unc.join("thing.exe")],
7709                Depth::Fast,
7710            ),
7711            (
7712                "a DLL on a share",
7713                unc.clone(),
7714                vec![unc.join("thing.DLL")],
7715                Depth::Fast,
7716            ),
7717            (
7718                "a text file on a share",
7719                unc.clone(),
7720                vec![unc.join("notes.txt")],
7721                Depth::Full,
7722            ),
7723            (
7724                "a big archive on a share",
7725                unc.clone(),
7726                vec![unc.join("everything.7z")],
7727                Depth::Full,
7728            ),
7729            (
7730                "the folder itself, on a share",
7731                unc.clone(),
7732                Vec::new(),
7733                Depth::Full,
7734            ),
7735            (
7736                "a mixed selection with an executable in it, on a share",
7737                unc.clone(),
7738                vec![unc.join("notes.txt"), unc.join("thing.exe")],
7739                Depth::Fast,
7740            ),
7741        ] {
7742            assert_eq!(
7743                App::menu_depth(&folder, &items),
7744                want,
7745                "{what}: wrong depth"
7746            );
7747        }
7748
7749        // A UNC path is a network path by construction; the extended-length and device forms
7750        // start the same way and are not.
7751        assert!(crate::shell::over_network(&unc));
7752        assert!(!crate::shell::over_network(&here));
7753        assert!(!crate::shell::over_network(Path::new(r"\\?\C:\Windows")));
7754        assert!(!crate::shell::over_network(Path::new(r"\\.\PhysicalDrive0")));
7755        assert!(!crate::shell::over_network(Path::new("")));
7756    }
7757
7758    /// A short menu says so, because a missing 7-Zip should not look like a broken program.
7759    #[test]
7760    #[cfg(windows)]
7761    fn a_short_menu_admits_to_being_short() {
7762        let _serialised = crate::shell::serialised();
7763        let mut h = Harness::new();
7764        h.settle();
7765
7766        // A local file, so the menu is quick; the point here is the depth it was asked at, not
7767        // where the file is.
7768        let pane = h.app.panes[0].id;
7769        let folder = h.app.pane_mut(pane).expect("the pane").tab().path.clone();
7770        let items = vec![folder.join("Cargo.toml")];
7771        h.app.asking = Some(Asking {
7772            token: h
7773                .app
7774                .menu_builder
7775                .build(&folder, &items, crate::shell::menu::Depth::Fast),
7776            pane,
7777            at: egui::pos2(200.0, 200.0),
7778            items,
7779            folder,
7780            depth: crate::shell::menu::Depth::Fast,
7781            since: h.ctx.cumulative_pass_nr(),
7782            asked: std::time::Instant::now(),
7783        });
7784
7785        let waited = std::time::Instant::now();
7786        while h.app.menu_pending() {
7787            h.frame(Vec::new());
7788            assert!(
7789                waited.elapsed() < std::time::Duration::from_secs(20),
7790                "the reduced menu never arrived"
7791            );
7792        }
7793        let menu = h.app.menu.as_ref().expect("a menu");
7794        assert!(
7795            menu.entries.len() > 3,
7796            "the reduced menu is too short to be one: {:?}",
7797            menu.entries.iter().map(|e| &e.label).collect::<Vec<_>>()
7798        );
7799        let notice = h.app.notice.clone().unwrap_or_default();
7800        assert!(
7801            notice.contains("network"),
7802            "nothing told the user why the menu is short: {notice:?}"
7803        );
7804        h.app.close_menu();
7805        h.frame(Vec::new());
7806    }
7807
7808    /// Anything on a share that is *still* slow gets asked for again with less.
7809    ///
7810    /// The extension list in [`App::menu_depth`] covers what was measured. This covers what was
7811    /// not: whatever this machine's extensions decide to read a whole file for next. Two and a
7812    /// half seconds, because the slowest *full* menu measured on that share for something that
7813    /// was not an executable was 1.8 s — so past this it is somebody inspecting the file rather
7814    /// than the share being busy.
7815    #[test]
7816    #[cfg(windows)]
7817    fn a_slow_network_menu_is_asked_for_again_with_less() {
7818        use std::sync::atomic::Ordering;
7819        use crate::shell::menu::Depth;
7820
7821        let _serialised = crate::shell::serialised();
7822        let mut h = Harness::new();
7823        h.settle();
7824
7825        // Nothing here should reach the shell: a UNC path to a host that does not exist would
7826        // spend the test's whole budget in DNS and SMB timeouts. The stall stands in for the
7827        // slow build, and is left set so the retry stalls too.
7828        crate::shell::menu::STALLED.store(0, Ordering::SeqCst);
7829        crate::shell::menu::STALL_MS.store(4_000, Ordering::SeqCst);
7830
7831        let pane = h.app.panes[0].id;
7832        let folder = PathBuf::from(r"\\somehost\someshare\bin");
7833        let items = vec![folder.join("mystery.dat")];
7834        let first = h
7835            .app
7836            .menu_builder
7837            .build(&folder, &items, Depth::Full);
7838        h.app.asking = Some(Asking {
7839            token: first,
7840            pane,
7841            at: egui::pos2(200.0, 200.0),
7842            items,
7843            folder,
7844            depth: Depth::Full,
7845            // Already past the deadline, so the test does not have to sit through it.
7846            since: h.ctx.cumulative_pass_nr(),
7847            asked: std::time::Instant::now()
7848                .checked_sub(std::time::Duration::from_secs(3))
7849                .expect("a monotonic clock three seconds old"),
7850        });
7851
7852        h.frame(Vec::new());
7853
7854        let asking = h.app.asking.as_ref().expect("still asking, with less");
7855        assert_eq!(
7856            asking.depth,
7857            Depth::Fast,
7858            "a full menu on a share was still being waited for three seconds in"
7859        );
7860        assert_ne!(
7861            asking.token, first,
7862            "the depth changed but the ask did not, so nothing was re-asked"
7863        );
7864
7865        h.app.close_menu();
7866        crate::shell::menu::STALL_MS.store(0, Ordering::SeqCst);
7867        h.frame(Vec::new());
7868    }
7869
7870    /// Ctrl+C then Ctrl+V, in the folder you are already looking at.
7871    ///
7872    /// The most ordinary thing anybody does with a clipboard, and the one case the first
7873    /// end-to-end test skipped: it copied from one folder and pasted into another, which is the
7874    /// *easy* half. Pasting into the folder the file is already in is where the shell has to be
7875    /// told not to ask, and where a paste that quietly does nothing is hardest to notice.
7876    ///
7877    /// Driven by real key events rather than by pushing actions, so the bindings are on trial
7878    /// too -- `Ctrl+C` and `Ctrl+V` being wired to the right actions is part of what is claimed.
7879    #[test]
7880    #[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
7881    #[cfg(windows)]
7882    fn copy_and_paste_in_the_same_folder_makes_a_copy() {
7883        use crate::shell::clipboard;
7884
7885        let _serialised = crate::shell::serialised();
7886        // The paste below has to be the shell's real one. Inside `target/sandbox`; see
7887        // `crate::shell::ops::FOR_REAL`.
7888        let _for_real = crate::shell::ops::for_real();
7889        crate::shell::init();
7890        clipboard::settle_for_tests();
7891
7892        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
7893            .join("target")
7894            .join("sandbox")
7895            .join("samefolder");
7896        let _ = std::fs::remove_dir_all(&root);
7897        std::fs::create_dir_all(&root).expect("sandbox");
7898        std::fs::write(root.join("one.txt"), b"one").expect("write");
7899
7900        let mut h = Harness::new();
7901        let pane = h.app.panes[0].id;
7902        h.app.perform(
7903            &h.ctx.clone(),
7904            Action::Navigate {
7905                pane,
7906                path: root.clone(),
7907            },
7908        );
7909        h.settle();
7910
7911        assert_eq!(
7912            h.app.pane_mut(pane).expect("the pane").tab().order.len(),
7913            1,
7914            "one file to start with"
7915        );
7916
7917        // Selected by *clicking* it, which is how anybody selects a file -- and which is the
7918        // difference between this test and the version that set the selection directly and
7919        // passed against a broken program. A click gives the row keyboard focus, and every
7920        // shortcut in this program was switched off while anything at all had focus.
7921        let body = h.app.panes[0].rect;
7922        let mut clicked = false;
7923        for step in 0..60 {
7924            let at = egui::pos2(body.left() + 60.0, body.top() + 40.0 + step as f32 * 4.0);
7925            if !body.contains(at) {
7926                break;
7927            }
7928            h.click_at(at);
7929            if h.app.pane_mut(pane).expect("the pane").tab().selected_count == 1 {
7930                clicked = true;
7931                break;
7932            }
7933        }
7934        assert!(clicked, "could not find the row to click");
7935        eprintln!("PROBE focused after the click: {:?}", h.ctx.memory(|m| m.focused()));
7936
7937        /// One frame carrying what pressing Ctrl+C or Ctrl+V *actually* delivers.
7938        ///
7939        /// Not `Event::Key`. `egui-winit` recognises these combinations itself and queues
7940        /// `Event::Copy` or `Event::Paste` in place of the key press, returning before the key
7941        /// event is ever added -- so a test that synthesises `Event::Key { key: C }` is testing a
7942        /// keystroke this program will never receive. This one did, and it passed against a
7943        /// program in which Ctrl+C and Ctrl+V did nothing whatsoever.
7944        ///
7945        /// The modifiers still go on the input, since `InputState::modifiers` is what the other
7946        /// shortcuts read.
7947        fn shortcut(h: &mut Harness, event: Event) {
7948            h.modifiers = Modifiers {
7949                command: true,
7950                ctrl: true,
7951                ..Modifiers::NONE
7952            };
7953            h.frame(vec![event]);
7954            h.modifiers = Modifiers::NONE;
7955        }
7956
7957        // Three times over, because once is what it managed. A file operation ends in a
7958        // re-read of the folder, the re-read used to clear the selection, and a cleared selection
7959        // is nothing to copy — so the second Ctrl+C copied nothing and everything after it was
7960        // a paste of whatever the first round had left on the clipboard.
7961        for round in 1..=3 {
7962            shortcut(&mut h, Event::Copy);
7963            assert!(
7964                clipboard::has_files(),
7965                "round {round}: Ctrl+C put nothing on the clipboard; the program said {:?}",
7966                h.app.notice
7967            );
7968            assert_eq!(
7969                h.app
7970                    .pane_mut(pane)
7971                    .expect("the pane")
7972                    .tab()
7973                    .selection_paths()
7974                    .len(),
7975                1,
7976                "round {round}: the file stopped being selected, so there was nothing to copy"
7977            );
7978
7979            // What `paste_keystroke` in `main.rs` puts back when the clipboard holds files rather
7980            // than text, which is the case that matters here.
7981            shortcut(&mut h, Event::Paste(String::new()));
7982            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
7983            while h.app.ops.in_progress().is_some() {
7984                h.frame(Vec::new());
7985                assert!(
7986                    std::time::Instant::now() < deadline,
7987                    "round {round}: the paste never finished"
7988                );
7989            }
7990            h.settle();
7991
7992            let mut names: Vec<String> = std::fs::read_dir(&root)
7993                .expect("read the folder back")
7994                .filter_map(|e| e.ok())
7995                .map(|e| e.file_name().to_string_lossy().into_owned())
7996                .collect();
7997            names.sort();
7998            assert_eq!(
7999                names.len(),
8000                round + 1,
8001                "round {round}: Ctrl+C then Ctrl+V left {names:?}; the program said {:?}",
8002                h.app.notice
8003            );
8004        }
8005
8006        clipboard::clear();
8007        let _ = std::fs::remove_dir_all(&root);
8008    }
8009
8010    /// Which action each of the clipboard events becomes, and Shift+Delete among them.
8011    ///
8012    /// On Windows `egui-winit` recognises Ctrl+X, Ctrl+C, Ctrl+V, Ctrl+Insert, Shift+Insert *and
8013    /// Shift+Delete* itself, and queues `Event::Cut`, `Event::Copy` or `Event::Paste` for all of
8014    /// them -- no key event at all. So Shift+Delete arrives as a `Cut`, indistinguishable from
8015    /// Ctrl+X except by the modifiers, and the arm that handled cuts was guarding itself with
8016    /// `!m.shift`. Shift+Delete therefore did nothing whatsoever: the cut arm refused it and the
8017    /// `Delete` key it was hoping for never came.
8018    ///
8019    /// The last case is the one worth keeping: with Ctrl held it stays a cut. Reading a stray
8020    /// Ctrl+Shift+X as "delete this for ever" would be the worst mistake this program could make,
8021    /// so ambiguity resolves to the recoverable answer.
8022    #[test]
8023    fn the_clipboard_events_map_to_the_right_actions() {
8024        let mut h = Harness::new();
8025        h.settle();
8026        let pane = h.app.panes[0].id;
8027        {
8028            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
8029            tab.select_only(0);
8030        }
8031
8032        let fired = |h: &mut Harness, mods: Modifiers, event: Event| -> Vec<&'static str> {
8033            h.app.journal = Some(Vec::new());
8034            h.modifiers = mods;
8035            h.frame(vec![event]);
8036            h.modifiers = Modifiers::NONE;
8037            h.app.journal.clone().unwrap_or_default()
8038        };
8039
8040        let ctrl = Modifiers {
8041            command: true,
8042            ctrl: true,
8043            ..Modifiers::NONE
8044        };
8045        let shift = Modifiers {
8046            shift: true,
8047            ..Modifiers::NONE
8048        };
8049        let ctrl_shift = Modifiers {
8050            command: true,
8051            ctrl: true,
8052            shift: true,
8053            ..Modifiers::NONE
8054        };
8055
8056        assert!(fired(&mut h, ctrl, Event::Copy).contains(&"Copy"));
8057        assert!(fired(&mut h, ctrl, Event::Cut).contains(&"Cut"));
8058        assert!(fired(&mut h, ctrl, Event::Paste(String::new())).contains(&"Paste"));
8059        // Shift+Delete: a permanent delete, not a cut.
8060        let shift_delete = fired(&mut h, shift, Event::Cut);
8061        assert!(
8062            shift_delete.contains(&"Delete"),
8063            "Shift+Delete produced {shift_delete:?}"
8064        );
8065        assert!(
8066            !shift_delete.contains(&"Cut"),
8067            "and it must not also cut: {shift_delete:?}"
8068        );
8069        // With Ctrl held it is a cut, whatever else is down.
8070        let both = fired(&mut h, ctrl_shift, Event::Cut);
8071        assert!(both.contains(&"Cut"), "Ctrl+Shift+X produced {both:?}");
8072        assert!(
8073            !both.contains(&"Delete"),
8074            "and it must never delete: {both:?}"
8075        );
8076    }
8077
8078    /// A right drag asks even inside the folder the files are already in.
8079    ///
8080    /// A left drag there means "move this to where it already is", which is nothing, and dropping
8081    /// a folder into itself is nothing whatever the button. But right-dragging a file onto its own
8082    /// folder is how Explorer is asked for a copy of it, and filtering that out before the question
8083    /// was asked meant a right drag inside a folder did nothing at all -- which is the most obvious
8084    /// way anybody would try the gesture.
8085    #[test]
8086    fn a_right_drag_can_land_in_the_folder_it_started_in() {
8087        let here = std::path::PathBuf::from(r"C:\Temp");
8088        let file = here.join("one.txt");
8089        let elsewhere = std::path::PathBuf::from(r"C:\Other\two.txt");
8090
8091        // A left drag inside the same folder has nothing to do.
8092        assert!(App::droppable(vec![file.clone()], &here, false).is_empty());
8093        // The same drag with the right button is a question worth asking.
8094        assert_eq!(
8095            App::droppable(vec![file.clone()], &here, true),
8096            vec![file.clone()]
8097        );
8098        // A folder dropped into itself is nothing either way.
8099        assert!(App::droppable(vec![here.clone()], &here, true).is_empty());
8100        assert!(App::droppable(vec![here.clone()], &here, false).is_empty());
8101        // And anything from somewhere else is fine with either button.
8102        assert_eq!(
8103            App::droppable(vec![elsewhere.clone()], &here, false),
8104            vec![elsewhere.clone()]
8105        );
8106        // A mixed batch keeps what it can.
8107        assert_eq!(
8108            App::droppable(vec![file, elsewhere.clone()], &here, false),
8109            vec![elsewhere]
8110        );
8111    }
8112
8113    /// The highlight marks the folder a drop would land in, and nothing that takes no drop.
8114    ///
8115    /// A drop can go into the folder being shown or into any folder row in it, and which one it
8116    /// will be is the thing worth showing. Lighting up the whole pane while the pointer sits on a
8117    /// subfolder promises the wrong destination — and so does lighting up the column header,
8118    /// which sorts, or the status line, which counts. The listing is the target.
8119    ///
8120    /// Driven by setting the hover point directly, because that is what a drag does to it from
8121    /// wherever it was started: the OLE callbacks write it and the frame reads it.
8122    #[test]
8123    fn the_drop_highlight_marks_the_row_and_not_the_pane() {
8124        let mut h = Harness::new();
8125        h.settle();
8126        let pane = h.app.panes[0].id;
8127
8128        let rows = h.app.panes[0].drop_rows.clone();
8129        let (row, _) = rows
8130            .iter()
8131            .find(|(_, path)| path.file_name().is_some_and(|n| n == "src"))
8132            .expect("`src` should be one of the folder rows");
8133        let scale = h.ctx.pixels_per_point();
8134
8135        // Over the row: the preview covers the row.
8136        let at = row.center();
8137        h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
8138        let drawn = h.app.preview_rect_for_tests(pane, scale);
8139        assert_eq!(
8140            drawn,
8141            Some(*row),
8142            "over a folder row the highlight has to be that row"
8143        );
8144
8145        // Away from any row: the listing, since that is where the drop would go.
8146        let pane_rect = h.app.panes[0].rect;
8147        let listing = h.app.panes[0].drop_area;
8148        assert!(
8149            listing.top() > pane_rect.top() && listing.bottom() < pane_rect.bottom(),
8150            "the listing has to stop short of the header above it and the status line below:              listing {listing:?} in pane {pane_rect:?}"
8151        );
8152        let below = rows.iter().map(|(r, _)| r.bottom()).fold(f32::MIN, f32::max);
8153        if below + 4.0 < listing.bottom() {
8154            let at = egui::pos2(listing.center().x, below + 2.0);
8155            h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
8156            assert_eq!(
8157                h.app.preview_rect_for_tests(pane, scale),
8158                Some(listing),
8159                "away from a row the highlight is the listing, whose folder takes the drop"
8160            );
8161        }
8162
8163        // Over the column header, which sorts rather than receives: no highlight, because
8164        // there is no drop to promise there.
8165        let at = egui::pos2(listing.center().x, listing.top() - 6.0);
8166        h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
8167        assert_eq!(
8168            h.app.preview_rect_for_tests(pane, scale),
8169            None,
8170            "the column header is not a drop target"
8171        );
8172
8173        // No drag, no highlight.
8174        h.app.drop_hover = None;
8175        assert_eq!(h.app.preview_rect_for_tests(pane, scale), None);
8176    }
8177
8178    /// A drag in flight keeps asking for frames, and ends by re-reading what a move emptied.
8179    ///
8180    /// This is the whole reason the drag runs on a thread of its own. While one is running the
8181    /// pointer belongs to OLE: not one mouse or keyboard event reaches winit, so nothing in
8182    /// egui's own event flow would ever ask for a repaint — and with no repaint there is no
8183    /// highlight of the folder the drop will land in and no sign of the selection the drag just
8184    /// made. Feedback that only appears once the gesture is over is not feedback.
8185    #[test]
8186    fn a_drag_in_flight_keeps_the_window_painting() {
8187        let mut h = Harness::new();
8188        h.settle();
8189        let pane = h.app.panes[0].id;
8190
8191        // Down to a window that has stopped asking for frames, which is the baseline the
8192        // assertion below needs: a freshly opened one is still finishing its icons and its
8193        // animations, and against *that* every frame looks like a repaint somebody wanted.
8194        assert!(h.quiesce(), "the window should settle into asking for nothing");
8195
8196        let (drag, finish) = crate::shell::dnd::Drag::pretend();
8197        h.app.file_drag = Some((pane, drag));
8198
8199        for _ in 0..3 {
8200            h.frame(Vec::new());
8201            assert!(
8202                h.ctx.has_requested_repaint(),
8203                "a drag in flight has to keep the frames coming, or nothing about it is visible"
8204            );
8205        }
8206
8207        // A move took the files out of this folder and OLE does not say which, so it is re-read.
8208        finish
8209            .send(Some(crate::shell::clipboard::Effect::Move))
8210            .unwrap();
8211        h.frame(Vec::new());
8212        assert!(h.app.file_drag.is_none(), "the drag is over and let go of");
8213        assert!(
8214            h.take_journal().contains(&"Refresh"),
8215            "and the folder the files left is re-read"
8216        );
8217    }
8218
8219    /// A folder that changed on disk is re-read without blanking what is on screen.
8220    ///
8221    /// `Tab::refresh` drops the listing, which is right for F5 — somebody asked, and a moment
8222    /// of "Reading..." is the honest answer. It is wrong for a watcher: a build writing into the
8223    /// folder would flash the pane on every file. So the old rows stay up until the new ones
8224    /// land, and the selection comes across with them.
8225    #[test]
8226    fn a_changed_folder_is_re_read_without_blanking_it() {
8227        let mut h = Harness::new();
8228        h.settle();
8229        let path = h.tab(0).path.clone();
8230        assert!(h.tab(0).order.len() > 2, "the crate root has rows");
8231        h.app.panes[0].tab_mut().select_only(1);
8232        let chosen = {
8233            let tab = h.tab(0);
8234            tab.entry_at(1)
8235                .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
8236        };
8237        assert!(chosen.is_some(), "and a row to select");
8238
8239        h.app.folder_changed(&path);
8240        assert!(
8241            h.tab(0).dir.is_some(),
8242            "the rows on screen have to stay on screen"
8243        );
8244        assert!(
8245            h.tab(0).awaiting.is_some(),
8246            "and a fresh read has to be on its way"
8247        );
8248        assert!(
8249            h.app.loader.cached(&path).is_none(),
8250            "with the cached copy dropped, or the re-read hands back what it already had \
8251             and the change is never seen"
8252        );
8253
8254        h.settle();
8255        assert!(h.tab(0).dir.is_some(), "the re-read landed");
8256        assert_eq!(
8257            h.tab(0).selected_count,
8258            1,
8259            "and what was selected is still selected"
8260        );
8261        let after = {
8262            let tab = h.tab(0);
8263            tab.cursor
8264                .and_then(|at| tab.entry_at(at))
8265                .and_then(|entry| tab.dir.as_ref().map(|dir| dir.name(entry).to_owned()))
8266        };
8267        assert_eq!(after, chosen, "and it is the same row, by name");
8268    }
8269
8270    /// The Bookmarks group lights up for a drag, and only over itself.
8271    ///
8272    /// Dropping a folder there pins it, which is a real destination and needs to look like one.
8273    #[test]
8274    fn the_bookmarks_group_shows_a_drop_target() {
8275        let mut h = Harness::new();
8276        h.settle();
8277        let rect = h
8278            .app
8279            .bookmarks_rect
8280            .expect("the Bookmarks group is on screen");
8281        let scale = h.ctx.pixels_per_point();
8282        let put = |h: &mut Harness, at: egui::Pos2| {
8283            h.app.drop_hover = Some(((at.x * scale) as i32, (at.y * scale) as i32));
8284        };
8285
8286        put(&mut h, rect.center());
8287        assert_eq!(h.app.bookmarks_preview(scale), Some(rect));
8288
8289        // Below the group, in Places: pinning is not what a drop there would mean.
8290        put(&mut h, egui::pos2(rect.center().x, rect.bottom() + 40.0));
8291        assert_eq!(h.app.bookmarks_preview(scale), None);
8292
8293        h.app.drop_hover = None;
8294        assert_eq!(h.app.bookmarks_preview(scale), None);
8295    }
8296
8297    /// A second drag picks files up, and a third, and every one after that.
8298    ///
8299    /// The release that ends a drag is consumed by `DoDragDrop`'s own loop and never reaches
8300    /// this window, so egui went on believing the button was held — and a press arriving while
8301    /// a button is already down starts no drag. One drag per window, then nothing, until some
8302    /// unrelated click happened to put the state right.
8303    ///
8304    /// The sequence below is the real one: press, travel, *no release*, the drag ends by
8305    /// itself. Every earlier test released the button, which is precisely why a suite of them
8306    /// stayed green while dragging twice did not work.
8307    #[test]
8308    fn a_second_drag_still_picks_the_files_up() {
8309        let mut h = Harness::new();
8310        h.settle();
8311        let pane = h.app.panes[0].id;
8312        let body = h.app.panes[0].rect;
8313
8314        for round in 0..3 {
8315            // Just past the icon, which is where a row's name starts and where a drag of the
8316            // file rather than a rubber band begins.
8317            let from = pos2(body.left() + 46.0, h.row_center(0, round).y);
8318            let done = h.drag_and_hold(from, pos2(from.x + 60.0, from.y + 90.0));
8319            assert!(
8320                done.contains(&"DragOut"),
8321                "round {round}: a drag of a name has to pick the file up, got {done:?}"
8322            );
8323
8324            // The drag a real window would have started, and its end. This is the only step
8325            // the platform does for us and the harness cannot.
8326            let (drag, finish) = crate::shell::dnd::Drag::pretend();
8327            h.app.file_drag = Some((pane, drag));
8328            finish.send(None).unwrap();
8329            h.frame(Vec::new());
8330            h.wait();
8331        }
8332    }
8333
8334    /// Only the two buttons that mean something drag, and the thumb buttons navigate.
8335    ///
8336    /// A middle-button or thumb-button drag over the listing used to pick files up and start an
8337    /// OLE drag, because `drag_started()` without a button is true for any of them.
8338    #[test]
8339    fn only_the_left_and_right_buttons_drag_and_the_thumbs_navigate() {
8340        use egui::PointerButton as B;
8341
8342        let mut h = Harness::new();
8343        h.settle();
8344        let pane = h.app.panes[0].id;
8345        let body = h.app.panes[0].rect;
8346
8347        // A middle-button drag across a row does nothing at all.
8348        for button in [B::Middle, B::Extra1, B::Extra2] {
8349            h.app.journal = Some(Vec::new());
8350            let from = egui::pos2(body.left() + 60.0, body.top() + 60.0);
8351            h.frame(vec![Event::PointerMoved(from)]);
8352            h.frame(vec![Event::PointerButton {
8353                pos: from,
8354                button,
8355                pressed: true,
8356                modifiers: Modifiers::NONE,
8357            }]);
8358            for step in 1..6 {
8359                h.frame(vec![Event::PointerMoved(from + vec2(0.0, step as f32 * 12.0))]);
8360            }
8361            let drawn = h.app.pane_mut(pane).expect("the pane").tab().band.is_some();
8362            let journal = h.app.journal.clone().unwrap_or_default();
8363            h.frame(vec![Event::PointerButton {
8364                pos: from,
8365                button,
8366                pressed: false,
8367                modifiers: Modifiers::NONE,
8368            }]);
8369            assert!(
8370                !drawn,
8371                "{button:?} drew a selection band; only the left and right buttons should"
8372            );
8373            assert!(
8374                !journal.contains(&"DragOut"),
8375                "{button:?} started a file drag: {journal:?}"
8376            );
8377        }
8378
8379        // The thumb buttons navigate instead.
8380        h.app.journal = Some(Vec::new());
8381        h.frame(vec![Event::PointerButton {
8382            pos: body.center(),
8383            button: B::Extra1,
8384            pressed: true,
8385            modifiers: Modifiers::NONE,
8386        }]);
8387        assert!(
8388            h.app.journal.clone().unwrap_or_default().contains(&"Back"),
8389            "the first thumb button should go back: {:?}",
8390            h.app.journal
8391        );
8392        h.app.journal = Some(Vec::new());
8393        h.frame(vec![Event::PointerButton {
8394            pos: body.center(),
8395            button: B::Extra2,
8396            pressed: true,
8397            modifiers: Modifiers::NONE,
8398        }]);
8399        assert!(
8400            h.app.journal.clone().unwrap_or_default().contains(&"Forward"),
8401            "the second thumb button should go forward: {:?}",
8402            h.app.journal
8403        );
8404    }
8405
8406    /// A new folder arrives selected, with its name open for editing.
8407    ///
8408    /// `New folder` on its own is half a gesture: nobody wants a folder called `New folder`, and
8409    /// in Explorer creating one and naming it is a single action. Which name to open is the
8410    /// shell's answer rather than this program's guess -- ask for `New folder` when one already
8411    /// exists and what appears is `New folder (2)` -- so it comes back through
8412    /// `IFileOperationProgressSink`, and this is the test that it comes back at all.
8413    #[test]
8414    #[cfg(windows)]
8415    fn a_new_folder_opens_its_name_for_editing() {
8416        let _serialised = crate::shell::serialised();
8417        // The whole point is that the *shell* picks the name, so the shell has to be asked.
8418        // Inside `target/sandbox`; see `crate::shell::ops::FOR_REAL`.
8419        let _for_real = crate::shell::ops::for_real();
8420
8421        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
8422            .join("target")
8423            .join("sandbox")
8424            .join("newfolder");
8425        let _ = std::fs::remove_dir_all(&root);
8426        std::fs::create_dir_all(&root).expect("sandbox");
8427        // One already there, so the shell has to pick a different name and this cannot pass by
8428        // guessing "New folder".
8429        std::fs::create_dir_all(root.join("New folder")).expect("sandbox");
8430
8431        let mut h = Harness::new();
8432        let pane = h.app.panes[0].id;
8433        h.app.perform(
8434            &h.ctx.clone(),
8435            Action::Navigate {
8436                pane,
8437                path: root.clone(),
8438            },
8439        );
8440        h.settle();
8441
8442        h.app.perform(&h.ctx.clone(), Action::NewFolder(pane));
8443        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
8444        loop {
8445            h.frame(Vec::new());
8446            let renaming = h
8447                .app
8448                .pane_mut(pane)
8449                .expect("the pane")
8450                .tab()
8451                .renaming
8452                .clone();
8453            if let Some((_, name)) = renaming {
8454                assert_ne!(
8455                    name, "New folder",
8456                    "the shell had to pick another name, and this is editing the old folder"
8457                );
8458                assert!(
8459                    name.starts_with("New folder"),
8460                    "the row opened for editing is `{name}`"
8461                );
8462                break;
8463            }
8464            assert!(
8465                std::time::Instant::now() < deadline,
8466                "the new folder never opened for editing; the program said {:?}",
8467                h.app.notice
8468            );
8469        }
8470
8471        let _ = std::fs::remove_dir_all(&root);
8472    }
8473
8474    /// Dropping onto a folder row means *into that folder*.
8475    ///
8476    /// The destination used to be worked out again when the drop landed, from the pane under the
8477    /// pointer, which made every drop go into the folder being shown. Dragging a file onto a
8478    /// folder is the one gesture where that is exactly wrong. The zones the application publishes
8479    /// are what the OLE callbacks answer from, so they are what this checks: a folder row has to
8480    /// be a target of its own, and it has to win over the pane it sits in.
8481    #[test]
8482    #[cfg(windows)]
8483    fn a_folder_row_is_its_own_drop_target() {
8484        use crate::shell::dnd::Onto;
8485
8486        let mut h = Harness::new();
8487        h.settle();
8488        let pane = h.app.panes[0].id;
8489
8490        // The crate's own folder, which has `src` in it.
8491        let rows = h.app.panes[0].drop_rows.clone();
8492        assert!(
8493            !rows.is_empty(),
8494            "the listing reported no folder rows at all, so nothing can be dropped onto one"
8495        );
8496        let (row, folder) = rows
8497            .iter()
8498            .find(|(_, path)| path.file_name().is_some_and(|n| n == "src"))
8499            .expect("`src` should be one of the folder rows");
8500
8501        h.app.publish_drop_targets(&h.ctx.clone());
8502        let scale = h.ctx.pixels_per_point();
8503        let at = (
8504            (row.center().x * scale) as i32,
8505            (row.center().y * scale) as i32,
8506        );
8507        let resolved = h.app.drops.resolve(at).expect("a zone under the row");
8508        assert_eq!(
8509            resolved,
8510            Onto::Folder(folder.clone()),
8511            "the row resolved to {resolved:?} rather than to the folder it shows"
8512        );
8513
8514        // And the pane's own folder is still the target away from any row: the status line at the
8515        // bottom of the pane is inside the pane and below the last row.
8516        let pane_rect = h.app.panes[0].rect;
8517        let below = rows
8518            .iter()
8519            .map(|(row, _)| row.bottom())
8520            .fold(f32::MIN, f32::max);
8521        if below + 4.0 < pane_rect.bottom() {
8522            let at = (
8523                (pane_rect.center().x * scale) as i32,
8524                ((below + 2.0) * scale) as i32,
8525            );
8526            let resolved = h.app.drops.resolve(at).expect("the pane's own zone");
8527            assert_eq!(
8528                resolved,
8529                Onto::Folder(
8530                    h.app.pane_mut(pane).expect("the pane").tab().path.clone()
8531                ),
8532                "away from a row, a drop belongs to the folder being shown"
8533            );
8534        }
8535    }
8536
8537    /// Right-clicking a folder with nothing in it has to give the folder's own menu.
8538    ///
8539    /// It gave nothing. `rows` is what wires a listing's clicks up, and a listing with nothing
8540    /// in it never reaches `rows` -- an empty folder draws one line of text over a body that
8541    /// nothing was listening to. Which is the one place `New folder` and `Paste` are most
8542    /// wanted, so it was also the least forgiving place to do nothing.
8543    ///
8544    /// Asserted on the *action*, not on the menu: whether the shell then has entries for that
8545    /// folder is `shell::menu`'s business and is tested there. What was missing here was
8546    /// anything happening at all.
8547    #[test]
8548    #[cfg(windows)]
8549    fn a_right_click_in_an_empty_folder_still_raises_a_menu() {
8550        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
8551            .join("target")
8552            .join("sandbox")
8553            .join("empty");
8554        let _ = std::fs::remove_dir_all(&root);
8555        std::fs::create_dir_all(&root).expect("sandbox");
8556
8557        let mut h = Harness::new();
8558        let pane = h.app.panes[0].id;
8559        h.app.perform(
8560            &h.ctx.clone(),
8561            Action::Navigate {
8562                pane,
8563                path: root.clone(),
8564            },
8565        );
8566        h.settle();
8567        assert_eq!(
8568            h.app.pane_mut(pane).expect("the pane").tab().order.len(),
8569            0,
8570            "the folder has to be empty, or this proves nothing"
8571        );
8572
8573        // The middle of the listing, which in an empty folder is the middle of the message.
8574        let body = h.app.panes[0].rect;
8575        let at = body.center();
8576        let raised = h.click_with(at, PointerButton::Secondary, Modifiers::NONE);
8577        assert!(
8578            raised.contains(&"ShellMenu"),
8579            "a right click in an empty folder produced {raised:?}"
8580        );
8581
8582        let _ = std::fs::remove_dir_all(&root);
8583    }
8584
8585    /// A right click in a row asks about the file if it lands on it, and about the folder if not.
8586    ///
8587    /// A row is mostly space, and the space around a name is the listing's background as much as
8588    /// the gap under the last file is — so the two halves of a row are two different questions.
8589    /// The alternative was that the only way to reach the folder's own menu, in a folder taller
8590    /// than the pane, was to find a gap that might not be there.
8591    ///
8592    /// The same rule the drag already followed, and asserted the same way: by clicking at a
8593    /// coordinate and seeing what the program did with it.
8594    #[test]
8595    #[cfg(windows)]
8596    fn a_right_click_asks_about_the_file_or_the_folder_by_where_it_lands() {
8597        let mut h = Harness::new();
8598        let pane = h.app.panes[0].id;
8599
8600        // The second row's name, and where it was drawn — a point on the name is a point on the
8601        // file, whatever the padding around it happens to be.
8602        let name = {
8603            let tab = h.tab(0);
8604            let entry = tab.entry_at(1).expect("the test folder has a second row");
8605            tab.dir.as_ref().expect("a listing").name(entry).to_owned()
8606        };
8607        let rows = h
8608            .ctx
8609            .read_response(Id::new(("rows-hit", pane)))
8610            .map(|r| r.rect)
8611            .expect("the listing takes the pointer");
8612        let y = h.row_center(0, 1).y;
8613        let ink = h
8614            .texts()
8615            .into_iter()
8616            .find(|(_, text)| *text == name)
8617            .map(|(at, _)| at)
8618            .unwrap_or_else(|| panic!("`{name}` is not drawn in the listing"));
8619
8620        // ---- On the name: that file, and the menu for it --------------------
8621        let raised = h.click_with(
8622            pos2(ink.x + 2.0, y),
8623            PointerButton::Secondary,
8624            Modifiers::NONE,
8625        );
8626        assert!(raised.contains(&"ShellMenu"), "{raised:?}");
8627        assert_eq!(
8628            h.tab(0).selected_count,
8629            1,
8630            "a right click on a name has to select it first"
8631        );
8632        let asked = h
8633            .app
8634            .asking
8635            .as_ref()
8636            .expect("the menu is built off-thread and is still on its way")
8637            .items
8638            .clone();
8639        assert_eq!(asked.len(), 1, "the menu was asked about {asked:?}");
8640        assert!(
8641            asked[0].ends_with(&name),
8642            "the menu was asked about {asked:?} rather than about `{name}`"
8643        );
8644
8645        // ---- In the space of a row that *is* selected ------------------------
8646        //
8647        // The exception, and the same one the drag makes: the files are picked out already, so
8648        // the menu is the selection's wherever in the row the click lands. Taking the selection
8649        // away because the pointer was between two columns would undo work rather than ask a
8650        // question.
8651        h.wait();
8652        // Two points into the row, which is left of the icon and so on nothing.
8653        let (beside_first, beside_second) = (
8654            pos2(rows.left() + 2.0, y),
8655            pos2(rows.left() + 2.0, h.row_center(0, 2).y),
8656        );
8657        let raised = h.click_with(beside_first, PointerButton::Secondary, Modifiers::NONE);
8658        assert!(raised.contains(&"ShellMenu"), "{raised:?}");
8659        assert_eq!(
8660            h.tab(0).selected_count,
8661            1,
8662            "a right click in a selected row's own space dropped the selection"
8663        );
8664        assert_eq!(
8665            h.app.asking.as_ref().expect("on its way").items.len(),
8666            1,
8667            "and it stopped being the selection's menu"
8668        );
8669
8670        // ---- In the space of a row that is not -------------------------------
8671        h.wait();
8672        let at = beside_second;
8673        assert!(
8674            h.hovers(Id::new(("rows-hit", pane)), at),
8675            "the point beside the icon is not in the listing at all"
8676        );
8677        let raised = h.click_with(at, PointerButton::Secondary, Modifiers::NONE);
8678        assert!(raised.contains(&"ShellMenu"), "{raised:?}");
8679        assert_eq!(
8680            h.tab(0).selected_count,
8681            0,
8682            "a right click that was not on a file left one selected"
8683        );
8684        assert!(
8685            h.app
8686                .asking
8687                .as_ref()
8688                .expect("on its way")
8689                .items
8690                .is_empty(),
8691            "the menu is not the folder's"
8692        );
8693    }
8694
8695    /// Two folders of enough files to scroll, in the crate's own `target`.
8696    #[cfg(windows)]
8697    fn tall_sandbox(name: &str, files: usize) -> PathBuf {
8698        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
8699            .join("target")
8700            .join("sandbox")
8701            .join(name);
8702        let _ = std::fs::remove_dir_all(&root);
8703        std::fs::create_dir_all(&root).expect("sandbox");
8704        for i in 0..files {
8705            std::fs::write(root.join(format!("file-{i:02}.txt")), b"x").expect("a file");
8706        }
8707        root
8708    }
8709
8710    /// Where the listing is scrolled to survives everything except going somewhere else.
8711    ///
8712    /// Two claims, and they pull in opposite directions, which is why they are one test:
8713    ///
8714    /// - **A re-read keeps its place.** Every file operation ends in one, and so does anything
8715    ///   the context menu does — so a scroll that does not survive it means acting on a file
8716    ///   two hundred rows down and being sent back to the top to find it again.
8717    /// - **A different folder starts at the top.** Nothing else makes sense: row 200 of the
8718    ///   folder you just left is not row 200 of anything.
8719    #[test]
8720    #[cfg(windows)]
8721    fn a_re_read_keeps_its_place_and_a_new_folder_does_not() {
8722        let one = tall_sandbox("scroll-a", 80);
8723        let two = tall_sandbox("scroll-b", 80);
8724
8725        let mut h = Harness::new();
8726        let pane = h.app.panes[0].id;
8727        let go = |h: &mut Harness, path: &std::path::Path| {
8728            h.app.perform(
8729                &h.ctx.clone(),
8730                Action::Navigate {
8731                    pane,
8732                    path: path.to_path_buf(),
8733                },
8734            );
8735            h.settle();
8736        };
8737        go(&mut h, &one);
8738
8739        // Down a long way, the way the wheel does it.
8740        h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(30.0 * crate::pane::ROW_HEIGHT);
8741        h.frame(Vec::new());
8742        h.frame(Vec::new());
8743        let was = h.tab(0).scroll_y;
8744        assert!(was > 100.0, "the listing did not scroll at all ({was})");
8745
8746        // A re-read: what every file operation and every context-menu action ends in.
8747        h.app.perform(&h.ctx.clone(), Action::Refresh(pane));
8748        h.settle();
8749        assert_eq!(
8750            h.tab(0).scroll_y, was,
8751            "a re-read of the same folder moved the listing"
8752        );
8753
8754        // And the right click that asks for a context menu, which is where this was reported:
8755        // the menu is what the user was looking at, and the listing behind it had gone back to
8756        // the top.
8757        let row = h.row_center(0, 4);
8758        h.click_with(row, PointerButton::Secondary, Modifiers::NONE);
8759        assert_eq!(
8760            h.tab(0).scroll_y, was,
8761            "the right click itself sent the listing back to the top"
8762        );
8763        // And once the menu is actually on screen, which is a few hundred milliseconds later:
8764        // the shell builds it off-thread, so the frames above have only asked for it.
8765        let waited = std::time::Instant::now();
8766        while h.app.menu_pending() {
8767            h.frame(Vec::new());
8768            assert!(
8769                waited.elapsed() < std::time::Duration::from_secs(20),
8770                "the builder never delivered"
8771            );
8772        }
8773        h.frame(Vec::new());
8774        assert!(h.app.menu.is_some(), "the menu is not up, so this proves nothing");
8775        assert_eq!(
8776            h.tab(0).scroll_y, was,
8777            "the menu appearing sent the listing back to the top"
8778        );
8779        h.app.close_menu();
8780        h.frame(Vec::new());
8781        assert_eq!(
8782            h.tab(0).scroll_y, was,
8783            "dismissing the menu sent the listing back to the top"
8784        );
8785
8786        // Somewhere else, and back: both start at the top.
8787        go(&mut h, &two);
8788        assert_eq!(
8789            h.tab(0).scroll_y, 0.0,
8790            "a different folder opened part-way down"
8791        );
8792        go(&mut h, &one);
8793        assert_eq!(
8794            h.tab(0).scroll_y, 0.0,
8795            "coming back to a folder opened where the last visit left it"
8796        );
8797
8798        // And each tab keeps its own place, which is the same fact from the other side: the
8799        // offset egui remembers belongs to the pane, so without this a tab coming to the front
8800        // shows wherever the tab before it had got to.
8801        h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(20.0 * crate::pane::ROW_HEIGHT);
8802        h.frame(Vec::new());
8803        h.frame(Vec::new());
8804        let deep = h.tab(0).scroll_y;
8805        assert!(deep > 100.0, "the first tab did not scroll ({deep})");
8806
8807        h.app.perform(&h.ctx.clone(), Action::NavigateNewTab { pane, path: two.clone() });
8808        h.settle();
8809        assert_eq!(h.tab(0).scroll_y, 0.0, "a new tab opened part-way down");
8810
8811        h.app.perform(&h.ctx.clone(), Action::ActivateTab { pane, tab: 0 });
8812        h.frame(Vec::new());
8813        h.frame(Vec::new());
8814        assert_eq!(
8815            h.tab(0).scroll_y, deep,
8816            "coming back to the first tab lost where it was"
8817        );
8818
8819        let _ = std::fs::remove_dir_all(&one);
8820        let _ = std::fs::remove_dir_all(&two);
8821    }
8822
8823    /// A change made by anybody re-reads the folder on its own.
8824    ///
8825    /// **This is what a context-menu action now relies on.** Invoking a shell verb used to
8826    /// re-read the folder unconditionally, because there is no way to be told what the verb did
8827    /// — so `Copy`, `Properties` and `Open with` each threw the listing away and built it again
8828    /// to discover that nothing had changed. Nothing does that any more, which is only correct
8829    /// because the folder is watched: this is the test that says the watching works, end to end,
8830    /// through a real `ReadDirectoryChangesW` handle and a real file appearing.
8831    #[test]
8832    #[cfg(windows)]
8833    fn a_change_on_disk_re_reads_the_folder_by_itself() {
8834        let root = tall_sandbox("watched", 3);
8835
8836        let mut h = Harness::new();
8837        let pane = h.app.panes[0].id;
8838        h.app.perform(
8839            &h.ctx.clone(),
8840            Action::Navigate {
8841                pane,
8842                path: root.clone(),
8843            },
8844        );
8845        h.settle();
8846        assert_eq!(h.tab(0).order.len(), 3, "the sandbox should have three files");
8847
8848        // Somebody else's change — a shell verb, Explorer, a terminal. `std::fs`, never the
8849        // shell: see `shell::ops::FOR_REAL`.
8850        std::fs::write(root.join("arrived.txt"), b"x").expect("a fourth file");
8851
8852        let waited = std::time::Instant::now();
8853        while h.tab(0).order.len() != 4 {
8854            h.frame(Vec::new());
8855            std::thread::sleep(std::time::Duration::from_millis(5));
8856            assert!(
8857                waited.elapsed() < std::time::Duration::from_secs(10),
8858                "the folder was never re-read: still {} rows",
8859                h.tab(0).order.len()
8860            );
8861        }
8862
8863        let _ = std::fs::remove_dir_all(&root);
8864    }
8865
8866    /// A folder that reads quickly says nothing about reading.
8867    ///
8868    /// `Reading…` used to be drawn the moment a folder was asked for, and a local folder comes
8869    /// back in single-digit milliseconds — so it was a word that flashed up and vanished on every
8870    /// navigation, in the exact place the listing was about to be. It now waits for
8871    /// [`crate::pane::SLOW_SCAN`], which is what the second half of this checks: silence is not
8872    /// the same thing as never saying it.
8873    #[test]
8874    #[cfg(windows)]
8875    fn a_quick_folder_never_says_it_is_reading() {
8876        let root = tall_sandbox("quick", 4);
8877        let reading = |h: &Harness| h.texts().iter().any(|(_, text)| text.contains("Reading"));
8878
8879        let mut h = Harness::new();
8880        let pane = h.app.panes[0].id;
8881        assert!(!reading(&h), "it starts by claiming to read something");
8882
8883        // Somewhere it has never been, so the scan really goes to the loader rather than coming
8884        // straight back out of its cache.
8885        h.app.perform(
8886            &h.ctx.clone(),
8887            Action::Navigate {
8888                pane,
8889                path: root.clone(),
8890            },
8891        );
8892        // Real sleeps between frames, so the scan lands in a frame or two rather than in fifty:
8893        // the clock this is asserting against is egui's, which the harness advances by a
8894        // sixtieth per frame, so a listing that took fifty frames to arrive would be half a
8895        // *second* as far as the program is concerned and would be right to say so.
8896        let started = h.time;
8897        for _ in 0..12 {
8898            h.frame(Vec::new());
8899            assert!(
8900                !reading(&h),
8901                "a folder that read in under {}s said it was reading",
8902                crate::pane::SLOW_SCAN
8903            );
8904            if h.tab(0).dir.is_some() {
8905                break;
8906            }
8907            std::thread::sleep(std::time::Duration::from_millis(40));
8908        }
8909        assert!(h.tab(0).dir.is_some(), "the listing never arrived");
8910        assert!(
8911            h.time - started < crate::pane::SLOW_SCAN,
8912            "the frames above took {:.2}s of the program's own time, which is past the \
8913             threshold — so the assertion inside the loop proved nothing",
8914            h.time - started
8915        );
8916
8917        // And a scan that *is* slow says so. Faked by putting the clock back rather than by
8918        // finding a slow disk: `awaiting` is set so the frame does not start a new scan and
8919        // stamp the time again.
8920        {
8921            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
8922            tab.refresh();
8923            tab.awaiting = Some(u64::MAX);
8924            tab.asked_at = Some(h.time - crate::pane::SLOW_SCAN - 1.0);
8925        }
8926        h.frame(Vec::new());
8927        assert!(
8928            reading(&h),
8929            "a scan a second old still has not admitted to waiting"
8930        );
8931
8932        let _ = std::fs::remove_dir_all(&root);
8933    }
8934
8935    /// The listing keeps three rows of nothing under it, in a folder of any size.
8936    ///
8937    /// The folder's own menu — `New folder`, `Paste`, `Refresh` — is what you get by
8938    /// right-clicking a part of the listing that is not a file. In a folder taller than the pane
8939    /// there was no such part: every pixel from the header to the status line was a row, and the
8940    /// gap at the end was whatever the last row happened to leave, which was frequently nothing.
8941    #[test]
8942    #[cfg(windows)]
8943    fn the_listing_keeps_room_under_it_for_the_folder() {
8944        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
8945            .join("target")
8946            .join("sandbox")
8947            .join("tall");
8948        let _ = std::fs::remove_dir_all(&root);
8949        std::fs::create_dir_all(&root).expect("sandbox");
8950        for i in 0..60 {
8951            std::fs::write(root.join(format!("file-{i:02}.txt")), b"x").expect("a file");
8952        }
8953
8954        let mut h = Harness::new();
8955        let pane = h.app.panes[0].id;
8956        h.app.perform(
8957            &h.ctx.clone(),
8958            Action::Navigate {
8959                pane,
8960                path: root.clone(),
8961            },
8962        );
8963        h.settle();
8964
8965        let body = h.app.panes[0].drop_area;
8966        let count = h.tab(0).order.len();
8967        assert!(
8968            count as f32 * crate::pane::ROW_HEIGHT > body.height(),
8969            "{count} rows fit inside the pane, so this proves nothing"
8970        );
8971
8972        // To the end of it, which is where there used to be nothing to click.
8973        h.app.pane_mut(pane).expect("the pane").tab_mut().scroll_to = Some(100_000.0);
8974        h.frame(Vec::new());
8975        h.frame(Vec::new());
8976
8977        let tail = h
8978            .ctx
8979            .read_response(Id::new(("rows-empty", pane)))
8980            .map(|r| r.rect)
8981            .expect("nothing is listening below the last row");
8982        assert!(
8983            (tail.height() - 3.0 * crate::pane::ROW_HEIGHT).abs() < 1.0,
8984            "the space under the last file is {:.0} points, not three rows",
8985            tail.height()
8986        );
8987
8988        let raised = h.click_with(tail.center(), PointerButton::Secondary, Modifiers::NONE);
8989        assert!(raised.contains(&"ShellMenu"), "{raised:?}");
8990        assert!(
8991            h.app
8992                .asking
8993                .as_ref()
8994                .expect("the menu is still on its way")
8995                .items
8996                .is_empty(),
8997            "the space under the last file gave a file's menu"
8998        );
8999
9000        let _ = std::fs::remove_dir_all(&root);
9001    }
9002
9003    /// Copy, cut, paste and delete, driven the way the keyboard drives them, on real files.
9004    ///
9005    /// The pieces are tested where they live -- `shell::clipboard` for the data object,
9006    /// `shell::ops` for the engine. What is only testable here is the sequence: that Ctrl+C
9007    /// puts the selection on the clipboard, that Ctrl+V into another folder brings it, that a
9008    /// cut leaves its sources alone until something pastes and *then* empties the clipboard,
9009    /// and that Delete goes through the shell.
9010    ///
9011    /// Only collision-free operations and a permanent delete of nothing: every case that
9012    /// raises a dialog is in `shell::ops`, behind `run_watching`, because a test with a modal
9013    /// dialog up and nobody to answer it is a test that never finishes.
9014    ///
9015    /// Ignored because it takes over the desktop's one clipboard.
9016    #[test]
9017    #[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
9018    #[cfg(windows)]
9019    fn copy_cut_paste_and_delete_end_to_end() {
9020        use crate::shell::clipboard::{self, Effect};
9021
9022        let _serialised = crate::shell::serialised();
9023        // The one test allowed to hand a job to the real shell, and it says so out loud. Everything
9024        // below happens inside `target/sandbox/keys`; `crate::shell::ops::FOR_REAL` documents what
9025        // went wrong when this was the default rather than an opt-in.
9026        let _for_real = crate::shell::ops::for_real();
9027        // This thread has to be an OLE apartment before it can own the clipboard. In the real
9028        // program `main` does it before anything else; a test harness does not, and without it
9029        // `OleSetClipboard` simply refuses and a copy puts nothing anywhere.
9030        crate::shell::init();
9031        // Another clipboard test in this process may have left a live data object on the one
9032        // clipboard the desktop has; this lets go of it and answers what comes of that.
9033        crate::shell::clipboard::settle_for_tests();
9034
9035        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
9036            .join("target")
9037            .join("sandbox")
9038            .join("keys");
9039        let _ = std::fs::remove_dir_all(&root);
9040        let from = root.join("from");
9041        let into = root.join("into");
9042        std::fs::create_dir_all(&from).expect("sandbox");
9043        std::fs::create_dir_all(&into).expect("sandbox");
9044        std::fs::write(from.join("copied.txt"), b"c").expect("write");
9045        std::fs::write(from.join("moved.txt"), b"m").expect("write");
9046        std::fs::write(from.join("binned.txt"), b"b").expect("write");
9047
9048        let mut h = Harness::new();
9049        let pane = h.app.panes[0].id;
9050
9051        /// Show a folder, and wait for its listing.
9052        fn show(h: &mut Harness, pane: PaneId, path: &std::path::Path) {
9053            h.app.perform(
9054                &h.ctx.clone(),
9055                Action::Navigate {
9056                    pane,
9057                    path: path.to_path_buf(),
9058                },
9059            );
9060            h.settle();
9061        }
9062
9063        /// Select one file by name in the shown folder.
9064        ///
9065        /// By *display position*, which is what `select_only` takes -- not by entry index.
9066        /// The two are only the same in an unsorted, unfiltered listing, and a version of this
9067        /// that passed the entry index selected the wrong row or none at all.
9068        fn select(h: &mut Harness, pane: PaneId, name: &str) {
9069            let tab = h.app.pane_mut(pane).expect("the pane").tab_mut();
9070            let dir = tab.dir.clone().expect("a listing");
9071            let position = (0..tab.order.len())
9072                .find(|p| tab.entry_at(*p).is_some_and(|e| dir.name(e) == name))
9073                .unwrap_or_else(|| panic!("`{name}` is not in the listing"));
9074            tab.select_only(position);
9075            assert_eq!(
9076                tab.selection_paths().len(),
9077                1,
9078                "`{name}` should be the one thing selected"
9079            );
9080        }
9081
9082        /// Drain this thread's message queue, which is what a real window does constantly.
9083        #[cfg(windows)]
9084        fn pump() {
9085            use windows::Win32::UI::WindowsAndMessaging::{
9086                DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
9087            };
9088            unsafe {
9089                let mut message = MSG::default();
9090                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
9091                    let _ = TranslateMessage(&message);
9092                    DispatchMessageW(&message);
9093                }
9094            }
9095        }
9096
9097        /// Run frames until every file operation has finished.
9098        fn settle_ops(h: &mut Harness) {
9099            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
9100            while h.app.ops.in_progress().is_some() {
9101                h.frame(Vec::new());
9102                assert!(
9103                    std::time::Instant::now() < deadline,
9104                    "a file operation never finished"
9105                );
9106            }
9107            h.settle();
9108        }
9109
9110        // ---- Ctrl+C, then Ctrl+V somewhere else ----
9111        show(&mut h, pane, &from);
9112        select(&mut h, pane, "copied.txt");
9113        h.app.perform(&h.ctx.clone(), Action::Copy(pane));
9114        let on_clipboard = clipboard::get().unwrap_or_else(|| {
9115            panic!(
9116                "Ctrl+C put nothing on the clipboard; the program said {:?}",
9117                h.app.notice
9118            )
9119        });
9120        assert_eq!(on_clipboard.effect, Effect::Copy);
9121
9122        show(&mut h, pane, &into);
9123        h.app.perform(&h.ctx.clone(), Action::Paste(pane));
9124        settle_ops(&mut h);
9125        assert!(into.join("copied.txt").is_file(), "the paste should have copied it; the program said {:?}", h.app.notice);
9126        assert!(from.join("copied.txt").is_file(), "and left the original");
9127        assert!(
9128            clipboard::has_files(),
9129            "a copy stays on the clipboard, so it can be pasted twice"
9130        );
9131
9132        // ---- Ctrl+X, then Ctrl+V ----
9133        show(&mut h, pane, &from);
9134        select(&mut h, pane, "moved.txt");
9135        pump();
9136        h.app.perform(&h.ctx.clone(), Action::Cut(pane));
9137        assert_eq!(
9138            clipboard::get().map(|p| p.effect),
9139            Some(Effect::Move),
9140            "a cut has to say so, or a paste would copy; the program said {:?}",
9141            h.app.notice
9142        );
9143        assert!(
9144            from.join("moved.txt").is_file(),
9145            "a cut moves nothing on its own -- that is the whole difference from a move"
9146        );
9147        assert!(
9148            !h.app.cut.is_empty(),
9149            "and the sources have to be marked, so they can be drawn as pending"
9150        );
9151
9152        show(&mut h, pane, &into);
9153        h.app.perform(&h.ctx.clone(), Action::Paste(pane));
9154        settle_ops(&mut h);
9155        assert!(into.join("moved.txt").is_file(), "the paste should have moved it; the program said {:?}", h.app.notice);
9156        assert!(!from.join("moved.txt").exists(), "and taken it out of the source");
9157        assert!(
9158            !clipboard::has_files(),
9159            "a cut that has been pasted has to leave the clipboard empty, or Ctrl+V again \
9160             would move files that are no longer where it says"
9161        );
9162        assert!(h.app.cut.is_empty(), "and nothing is pending any more");
9163
9164        // ---- Delete ----
9165        show(&mut h, pane, &from);
9166        select(&mut h, pane, "binned.txt");
9167        h.app.perform(
9168            &h.ctx.clone(),
9169            Action::Delete {
9170                pane,
9171                permanent: false,
9172            },
9173        );
9174        settle_ops(&mut h);
9175        assert!(
9176            !from.join("binned.txt").exists(),
9177            "Delete should have sent it to the Recycle Bin"
9178        );
9179
9180        clipboard::clear();
9181        let _ = std::fs::remove_dir_all(&root);
9182    }
9183
9184    #[test]
9185    #[ignore = "measures the whole process; run explicitly, single-threaded"]
9186    fn what_the_shell_menu_costs() {
9187        // Not an assertion — a measurement, and the answer to where the memory in this
9188        // process actually is. Opening a folder's context menu makes Windows load every
9189        // installed shell extension into *this* process: an archiver, a screenshot tool, a
9190        // cloud client, a rename tool, whatever else. Each is a DLL with its own heap, none
9191        // of them is ever unloaded, and none of it is visible to the Rust allocator counter.
9192        let mut h = Harness::new();
9193        h.settle();
9194        let (private_before, gdi_before, user_before) = process_memory();
9195        let heap_before = live_heap();
9196        println!(
9197            "before the menu: private {:>7} KB   heap {:>7} KB   gdi {gdi_before:>4}   \
9198             user {user_before:>4}",
9199            private_before / 1024,
9200            heap_before / 1024
9201        );
9202
9203        // Five times over, because the answer that matters is whether it *repeats*: a DLL
9204        // loads once and stays, so a one-off cost of a few megabytes is very different from
9205        // a few megabytes every time somebody right-clicks.
9206        let mut last = private_before as isize;
9207        for round in 1..=5 {
9208            h.app.open_folder_menu(&h.ctx.clone());
9209            // The shell's entries come from a thread now, and it is exactly the extensions
9210            // this test is weighing that make it slow — so wait for them rather than
9211            // measuring a menu that never got any.
9212            let waited = std::time::Instant::now();
9213            while h.app.menu.is_none() {
9214                h.frame(Vec::new());
9215                if waited.elapsed() > std::time::Duration::from_secs(20) {
9216                    panic!("the menu builder never answered");
9217                }
9218            }
9219            for _ in 0..20 {
9220                h.frame(Vec::new());
9221            }
9222            h.settle();
9223            h.app.close_menu();
9224            h.frame(Vec::new());
9225
9226            let (private, gdi, user) = process_memory();
9227            println!(
9228                "menu {round}:  private {:>7} KB  ({:+} KB this time)   gdi {gdi:>4}   user {user:>4}",
9229                private / 1024,
9230                (private as isize - last) / 1024
9231            );
9232            last = private as isize;
9233        }
9234
9235        let (private, gdi, user) = process_memory();
9236        let heap = live_heap();
9237        println!(
9238            "after the menus: private {:>7} KB   heap {:>7} KB   gdi {gdi:>4}   user {user:>4}",
9239            private / 1024,
9240            heap / 1024
9241        );
9242        println!(
9243            "the menu cost:   private {:+} KB   heap {:+} KB   gdi {:+}   user {:+}",
9244            (private as isize - private_before as isize) / 1024,
9245            (heap - heap_before) / 1024,
9246            gdi as i64 - gdi_before as i64,
9247            user as i64 - user_before as i64
9248        );
9249    }
9250
9251    #[test]
9252    #[ignore = "measures the whole process; run explicitly, single-threaded"]
9253    fn closing_a_tab_gives_its_listing_back() {
9254        // The cache has a budget; a *tab* does not. Every open tab pins its own listing
9255        // through an `Arc`, which is why the cache's own figure understates what is held —
9256        // and it is the one shape of browsing that grows without a bound: a tab per folder.
9257        //
9258        // That much is by design. What would be a leak is a tab that is closed and does not
9259        // give the memory back, so this opens a pile of them, closes them all, and looks.
9260        let dirs = folders(Path::new(env!("CARGO_MANIFEST_DIR")), 40);
9261        assert!(dirs.len() >= 20, "need folders to open tabs on");
9262
9263        let mut h = Harness::new();
9264        h.settle();
9265        let before = live_heap();
9266
9267        let pane = h.app.panes[0].id;
9268        for dir in &dirs {
9269            h.app.perform(&h.ctx.clone(), Action::NavigateNewTab {
9270                pane,
9271                path: dir.clone(),
9272            });
9273            h.settle();
9274        }
9275        let tabs = h.app.panes[0].tabs.len();
9276        let open = live_heap();
9277        println!(
9278            "{tabs} tabs open: {:+} KB  ({} KB a tab)",
9279            (open - before) / 1024,
9280            (open - before) / 1024 / tabs as isize
9281        );
9282
9283        // Close them from the back, leaving the one the window started with.
9284        while h.app.panes[0].tabs.len() > 1 {
9285            let last = h.app.panes[0].tabs.len() - 1;
9286            h.app
9287                .perform(&h.ctx.clone(), Action::CloseTab { pane, tab: last });
9288            h.settle();
9289        }
9290        // And empty the cache, which legitimately still holds what the tabs were showing.
9291        for dir in &dirs {
9292            h.app.loader.invalidate(dir);
9293        }
9294        h.settle();
9295        let closed = live_heap();
9296        println!(
9297            "after closing: {:+} KB on the baseline (was {:+} KB with {tabs} tabs open)",
9298            (closed - before) / 1024,
9299            (open - before) / 1024
9300        );
9301
9302        // A tab's listing, its display order and its selection are the whole cost, and all
9303        // three go with it. Half a megabyte of slack for the cache's own bookkeeping.
9304        assert!(
9305            closed - before < (1 << 19),
9306            "closing every tab left {:+} KB behind -- something is holding listings after \
9307             their tab is gone",
9308            (closed - before) / 1024
9309        );
9310    }
9311
9312    #[test]
9313    #[ignore = "measures the whole process; run explicitly, single-threaded"]
9314    fn browsing_hundreds_of_folders_settles_rather_than_grows() {
9315        // Somewhere else with `YAFE_WALK`, which is how the *ceiling* gets measured: the
9316        // caches are sized in entries, so a walk of this repository's small folders and a
9317        // walk of `C:\Windows` settle at very different heights.
9318        let root = std::env::var_os("YAFE_WALK")
9319            .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
9320        let dirs = folders(&root, 600);
9321        println!("walking {} ({} folders)", root.display(), dirs.len());
9322        assert!(
9323            dirs.len() > 120,
9324            "need a few hundred real folders to see a plateau, found {}",
9325            dirs.len()
9326        );
9327
9328        let mut h = Harness::new();
9329        let browse = |h: &mut Harness, dir: &Path| {
9330            h.app.panes[0].tab_mut().navigate(dir.to_path_buf());
9331            h.settle();
9332        };
9333
9334        // The first stretch is setup, not growth: the fonts, the glyph atlas, the loader's
9335        // threads and the shell's own caches are all paid for once.
9336        for dir in dirs.iter().take(20) {
9337            browse(&mut h, dir);
9338        }
9339        let (heap0, private0, gdi0, user0) = {
9340            let (p, g, u) = process_memory();
9341            (live_heap(), p, g, u)
9342        };
9343        println!(
9344            "baseline at 20 folders: heap {:>7} KB   private {:>7} KB   gdi {gdi0:>5}   user {user0:>4}",
9345            heap0 / 1024,
9346            private0 / 1024
9347        );
9348
9349        let mut samples = Vec::new();
9350        for (i, dir) in dirs.iter().enumerate().skip(20) {
9351            browse(&mut h, dir);
9352            if (i + 1) % 50 == 0 {
9353                let heap = live_heap();
9354                let (private, gdi, user) = process_memory();
9355                let (dirs, entries) = h.app.loader.held();
9356                samples.push((heap, private as isize));
9357                println!(
9358                    "{:>4} folders:  heap {:+8} KB   private {:+8} KB   gdi {gdi:>5}   \
9359                     user {user:>4}   cache {dirs:>3}/{entries:>7} = {:>4} B/entry",
9360                    i + 1,
9361                    (heap - heap0) / 1024,
9362                    (private as isize - private0 as isize) / 1024,
9363                    if entries > 0 {
9364                        (heap - heap0) / entries as isize
9365                    } else {
9366                        0
9367                    }
9368                );
9369            }
9370        }
9371
9372        // The caches have budgets and are meant to reach them. What must not happen is the
9373        // second half of the walk costing as much as the first — that is the signature of
9374        // something that never lets go.
9375        let mid = samples.len() / 2;
9376        let (heap_mid, private_mid) = samples[mid];
9377        let (heap_end, private_end) = samples[samples.len() - 1];
9378        let folders_after = (samples.len() - mid) * 50;
9379        println!(
9380            "over the last {folders_after} folders: heap {:+} KB, private {:+} KB",
9381            (heap_end - heap_mid) / 1024,
9382            (private_end - private_mid) / 1024
9383        );
9384
9385        // A megabyte of slack over hundreds of folders, which is arena reuse and allocator
9386        // fragmentation rather than anything held.
9387        let slack = 1 << 20;
9388        assert!(
9389            heap_end - heap_mid < slack,
9390            "the heap is still growing after the cache should have settled: {:+} KB over \
9391             {folders_after} folders",
9392            (heap_end - heap_mid) / 1024
9393        );
9394        assert!(
9395            private_end - private_mid < 4 * slack,
9396            "the process is still growing after the caches should have settled: {:+} KB \
9397             over {folders_after} folders -- something outside the Rust heap is being kept",
9398            (private_end - private_mid) / 1024
9399        );
9400    }
9401}
