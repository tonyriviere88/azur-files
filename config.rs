1//! Settings that outlive a session.
2//!
3//! A hand-rolled `key=value` file rather than serde and a format crate. There are a
4//! dozen settings; a derive, a dependency and a schema would all be larger than the
5//! sixty lines of parsing below, and a file a user can open and fix by hand is a
6//! feature rather than a compromise.
7//!
8//! Anything unparseable is skipped rather than fatal: a settings file is not worth
9//! refusing to start over.
10//!
11//! # What the window comes back as
12//!
13//! Where it was on the desktop, how big it was, which panes it was divided into, and
14//! which tabs were in each of them. Three of those four are single values; the panes
15//! are a tree, and [`crate::dock::Node`] writes its own one-line form of it — the
16//! `layout=` key — over pane *numbers* that the `pane=` lines below supply in the same
17//! order.
18//!
19//! ```text
20//! layout=h0.500(0,1)
21//! focus=1
22//! pane=0            ← pane 0's tabs, with the first of them in front
23//! path=C:\src
24//! path=C:\src\ui
25//! pane=0            ← pane 1's
26//! path=D:\
27//! ```
28//!
29//! A `path` before any `pane` line is a file from the version that remembered tabs and
30//! not panes, and every tab in it goes into one pane — which is what that version did.
31
32use std::path::{Path, PathBuf};
33
34use crate::ui::sidebar::Sections;
35
36/// One pane's worth of tabs, as it was left.
37#[derive(Clone, Debug, Default)]
38pub struct PaneTabs {
39    /// The folders, in the order their tabs sat in the strip.
40    pub paths: Vec<PathBuf>,
41    /// Which of them was in front.
42    pub active: usize,
43}
44
45#[derive(Clone, Debug)]
46pub struct Config {
47    /// Folders pinned in the sidebar.
48    pub bookmarks: Vec<PathBuf>,
49    /// Every tab that was open, grouped by the pane it was in — so the window reopens
50    /// divided the way it was left and not merely pointing at the same folders.
51    pub panes: Vec<PaneTabs>,
52    /// How the panes divided the window: [`crate::dock::Node::encode`]'s form, over
53    /// positions in [`Self::panes`]. `None` on a first run, and on a file that predates
54    /// panes being remembered.
55    pub layout: Option<String>,
56    /// Which pane had the keyboard, as a position in [`Self::panes`].
57    pub focus: usize,
58    pub sidebar_width: f32,
59    /// Where every pane's preview panel goes, and how much of the pane it takes.
60    ///
61    /// A preference rather than per-folder state, which is why it is here and the *open* flag is
62    /// not: "where the preview goes" is a habit worth keeping across sessions, and "is one showing
63    /// for this folder" is a thing you decide when you are looking at the folder. See
64    /// [`crate::ui::preview::Layout`].
65    pub preview: crate::ui::preview::Layout,
66    pub sections: Sections,
67    /// Window size in points, as last seen.
68    pub window: Option<[f32; 2]>,
69    /// Where the window was: the outer top-left corner, **in physical pixels**.
70    ///
71    /// Physical rather than points, which every other measurement here is in, because a
72    /// desktop of two monitors at different scale factors has no single coordinate space
73    /// in points — a logical position means something different depending on which
74    /// monitor resolves it, and the platform resolves it against the monitor the window
75    /// happens to be created on rather than the one it is being sent to. Pixels are the
76    /// one space where "1920, -8" is a place. See `main::restore_position`.
77    pub position: Option<[f32; 2]>,
78    pub maximized: bool,
79    /// Azur ships both sides of the palette; this one opens on the dark side.
80    pub dark: bool,
81}
82
83/// How wide the sidebar is until somebody drags it, and what a double click on its splitter
84/// puts it back to.
85///
86/// Enough for the longest place name and a drive's label and letter, now that nothing has to
87/// share the row with a free-space caption.
88pub const SIDEBAR_WIDTH: f32 = 200.0;
89
90/// How many tabs are worth reopening, across every pane.
91///
92/// Reopening more than this is slower than the folder the user actually wanted, so a session
93/// that ended with fifty tabs open does not become a slow start. Applied when the file is read
94/// as well as when it is written, because the cost is in the reading either way.
95const TABS: usize = 24;
96
97/// And how many panes. Nothing can produce this many by dragging tabs to edges; it is here so
98/// that a settings file which has been edited, truncated or filled with nonsense costs a
99/// bounded amount to read.
100const PANES: usize = 24;
101
102/// The window's size on a first run, and what `Reset window size` puts it back to.
103///
104/// 1024×600 fits every laptop this is likely to run on, and the layout is built to be usable at
105/// it: four columns, a sidebar and a status line all fit, which is the point of the density
106/// choices throughout. Comfortably above the 720×420 minimum the window refuses to go below.
107pub const WINDOW_SIZE: [f32; 2] = [1024.0, 600.0];
108
109impl Default for Config {
110    fn default() -> Self {
111        Self {
112            bookmarks: Vec::new(),
113            panes: Vec::new(),
114            layout: None,
115            focus: 0,
116            sidebar_width: SIDEBAR_WIDTH,
117            preview: crate::ui::preview::Layout::default(),
118            sections: Sections::default(),
119            window: None,
120            position: None,
121            maximized: false,
122            dark: true,
123        }
124    }
125}
126
127impl Config {
128    /// Read the settings, falling back to the defaults for anything missing.
129    pub fn load() -> Self {
130        let text = file()
131            .and_then(|path| std::fs::read_to_string(path).ok())
132            // Nothing under the current name: either a first run or a rename, and the two
133            // are told apart by whether the old file is there. See [`previous_file`].
134            .or_else(|| previous_file().and_then(|path| std::fs::read_to_string(path).ok()));
135        match text {
136            Some(text) => Self::parse(&text),
137            None => Self::default(),
138        }
139    }
140
141    /// The file, without the file: separated from [`Self::load`] so that what the format
142    /// means can be checked without a disk or a profile directory anywhere in it.
143    ///
144    /// Reachable from the rest of the crate for one reason — `app::layout_tests` puts a real
145    /// window's settings through this and opens a window from what comes back, which is the
146    /// only place the two halves of the layout meet: the tree's pane *numbers* here and the
147    /// pane order in [`crate::dock::Node::panes`] there.
148    pub(crate) fn parse(text: &str) -> Self {
149        let mut config = Self::default();
150        for line in text.lines() {
151            let line = line.trim();
152            if line.is_empty() || line.starts_with('#') {
153                continue;
154            }
155            let Some((key, value)) = line.split_once('=') else {
156                continue;
157            };
158            let value = value.trim();
159            match key.trim() {
160                "bookmark" => config.bookmarks.push(PathBuf::from(value)),
161                // Opens a pane, and everything down to the next one belongs to it. The
162                // value is which of its tabs was in front.
163                "pane" => {
164                    if config.panes.len() < PANES {
165                        config.panes.push(PaneTabs {
166                            paths: Vec::new(),
167                            active: value.parse().unwrap_or(0),
168                        });
169                    }
170                }
171                "path" => {
172                    // See the module header: a file from before panes were remembered has
173                    // no `pane` line at all, and one pane is what it meant.
174                    let pane = match config.panes.last_mut() {
175                        Some(pane) => pane,
176                        None => {
177                            config.panes.push(PaneTabs::default());
178                            config.panes.last_mut().expect("just pushed")
179                        }
180                    };
181                    if pane.paths.len() < TABS {
182                        pane.paths.push(PathBuf::from(value));
183                    }
184                }
185                "layout" => config.layout = Some(value.to_owned()),
186                "focus" => config.focus = value.parse().unwrap_or(0),
187                "preview" => {
188                    // `right`, `bottom` or `auto`, and then optionally the share — a word and a
189                    // number rather than two keys, since neither means much without the other.
190                    let (at, share) = value.split_once(',').unwrap_or((value, ""));
191                    if let Some(at) = crate::ui::preview::Where::parse(at.trim()) {
192                        config.preview.at = at;
193                    }
194                    if let Ok(share) = share.trim().parse::<f32>() {
195                        config.preview.share = share.clamp(0.1, 0.9);
196                    }
197                }
198                "line_numbers" => config.preview.numbers = value == "1",
199                "sidebar_width" => {
200                    if let Ok(width) = value.parse::<f32>() {
201                        config.sidebar_width = width.clamp(140.0, 520.0);
202                    }
203                }
204                "sections" => {
205                    let flags: Vec<bool> = value.split(',').map(|f| f.trim() == "1").collect();
206                    if flags.len() == 3 {
207                        config.sections = Sections {
208                            drives: flags[0],
209                            bookmarks: flags[1],
210                            places: flags[2],
211                        };
212                    }
213                }
214                "window" => {
215                    if let Some((w, h)) = value.split_once(',') {
216                        if let (Ok(w), Ok(h)) = (w.trim().parse::<f32>(), h.trim().parse::<f32>()) {
217                            // A window smaller than this would have no room for the
218                            // chrome, and a saved size can come from another monitor.
219                            if w >= 640.0 && h >= 400.0 && w < 20_000.0 && h < 20_000.0 {
220                                config.window = Some([w, h]);
221                            }
222                        }
223                    }
224                }
225                "position" => {
226                    if let Some((x, y)) = value.split_once(',') {
227                        if let (Ok(x), Ok(y)) = (x.trim().parse::<f32>(), y.trim().parse::<f32>()) {
228                            // A desktop is bounded, and a position outside these is either
229                            // corrupt or from a machine with a wall of monitors this one
230                            // does not have. Whether it is on a monitor *now* is a question
231                            // only the platform can answer — see `main::restore_position`.
232                            if x.abs() < 60_000.0 && y.abs() < 60_000.0 {
233                                config.position = Some([x, y]);
234                            }
235                        }
236                    }
237                }
238                "maximized" => config.maximized = value == "1",
239                "theme" => config.dark = !value.eq_ignore_ascii_case("light"),
240                _ => {}
241            }
242        }
243        config
244    }
245
246    /// Write the settings back, best effort.
247    ///
248    /// **Never from a test.** This is not caution, it is a bug that was shipped and found: a
249    /// test that adds a bookmark, changes the theme or drags the sidebar marks the settings
250    /// dirty, and the frame it runs in writes them — so `cargo test` replaced the real
251    /// `config.ini` with a test fixture's, and the bookmarks in it were simply gone. It happened
252    /// repeatedly and looked like nothing, because a test that passes is a test nobody examines.
253    /// A test process has no business writing a user's settings under any circumstances.
254    pub fn save(&self) {
255        if cfg!(test) {
256            return;
257        }
258        let Some(path) = file() else { return };
259        if let Some(parent) = path.parent() {
260            let _ = std::fs::create_dir_all(parent);
261        }
262        let text = self.to_text();
263
264        // One level of undo, and the reason it is here: settings are written whole, so
265        // anything wrong with the in-memory copy at the moment of a save — a launch that read
266        // no config, a second instance, a test run against the wrong profile — replaces the
267        // file with it and the bookmarks are simply gone. The previous contents cost one write.
268        if let Ok(existing) = std::fs::read(&path) {
269            if !existing.is_empty() && existing != text.as_bytes() {
270                let _ = std::fs::write(path.with_extension("ini.bak"), &existing);
271            }
272        }
273        let _ = std::fs::write(path, text);
274    }
275
276    /// What [`Self::save`] would write. Separated from it for the same reason
277    /// [`Self::parse`] is: the format is worth checking, and a test must never go near the
278    /// file.
279    pub(crate) fn to_text(&self) -> String {
280        let mut text = format!("# {}\n", crate::brand::NAME);
281        text.push_str(&format!("sidebar_width={:.0}\n", self.sidebar_width));
282        // The preview panel: where it goes and how much room it takes, as a word and a number,
283        // since neither means much without the other.
284        text.push_str(&format!(
285            "preview={},{:.3}\n",
286            self.preview.at.as_str(),
287            self.preview.share
288        ));
289        text.push_str(&format!(
290            "line_numbers={}\n",
291            flag(self.preview.numbers)
292        ));
293        text.push_str(&format!(
294            "sections={},{},{}\n",
295            flag(self.sections.drives),
296            flag(self.sections.bookmarks),
297            flag(self.sections.places),
298        ));
299        if let Some([w, h]) = self.window {
300            text.push_str(&format!("window={w:.0},{h:.0}\n"));
301        }
302        if let Some([x, y]) = self.position {
303            text.push_str(&format!("position={x:.0},{y:.0}\n"));
304        }
305        text.push_str(&format!("maximized={}\n", flag(self.maximized)));
306        text.push_str(if self.dark {
307            "theme=dark\n"
308        } else {
309            "theme=light\n"
310        });
311        for bookmark in &self.bookmarks {
312            text.push_str(&format!("bookmark={}\n", bookmark.display()));
313        }
314        // The layout, then the panes it is written over, in the order it numbers them.
315        if let Some(layout) = &self.layout {
316            text.push_str(&format!("layout={layout}\n"));
317        }
318        text.push_str(&format!("focus={}\n", self.focus));
319        // Reopening more than this is slower than the folder the user actually wanted, so a
320        // runaway session does not become a slow start. Counted across every pane, and never
321        // at the cost of a pane: the first tab of each is what the layout is a tree over, so
322        // dropping one would leave `layout=` describing a window that cannot be built.
323        let mut room = TABS;
324        for pane in self.panes.iter().take(PANES) {
325            let keep = pane.paths.len().min(room.max(1));
326            room = room.saturating_sub(keep);
327            text.push_str(&format!("pane={}\n", pane.active.min(keep.saturating_sub(1))));
328            for path in pane.paths.iter().take(keep) {
329                text.push_str(&format!("path={}\n", path.display()));
330            }
331        }
332        text
333    }
334}
335
336fn flag(value: bool) -> u8 {
337    u8::from(value)
338}
339
340/// Where the settings live.
341fn file() -> Option<PathBuf> {
342    // A folder with spaces in it is the Windows convention and a nuisance to type
343    // everywhere else, so the two platforms get the two spellings of the same name.
344    let name = if cfg!(windows) {
345        crate::brand::NAME
346    } else {
347        "azur-file-explorer"
348    };
349    base_dir().map(|base| base.join(name).join("config.ini"))
350}
351
352/// What the settings folder was called before the program had a name.
353///
354/// Read only when there is nothing under the current name, and never written: a user who
355/// had bookmarks, open tabs and a window size should not lose them to a rename, and the
356/// first save afterwards puts them in the new place. One migration, nothing remembered,
357/// and it can go once nobody is coming from that version.
358fn previous_file() -> Option<PathBuf> {
359    base_dir().map(|base| base.join("yet-another-file-explorer").join("config.ini"))
360}
361
362/// Where per-user settings go on this platform.
363///
364/// **`YAFE_PROFILE` replaces it outright.** Not a developer convenience — a safety rail. A
365/// test that drives this program with real mouse and keyboard input has to launch it for real,
366/// and a real launch writes real settings on the way out: the window size, the open tabs, the
367/// bookmarks. Point it somewhere disposable and a test run cannot touch anybody's own.
368fn base_dir() -> Option<PathBuf> {
369    if let Some(dir) = std::env::var_os("YAFE_PROFILE") {
370        return Some(PathBuf::from(dir));
371    }
372    if cfg!(windows) {
373        std::env::var_os("APPDATA").map(PathBuf::from)
374    } else {
375        std::env::var_os("XDG_CONFIG_HOME")
376            .map(PathBuf::from)
377            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
378    }
379}
380
381#[cfg(test)]
382mod tests {
383    use super::*;
384
385    fn pane(active: usize, paths: &[&str]) -> PaneTabs {
386        PaneTabs {
387            paths: paths.iter().map(PathBuf::from).collect(),
388            active,
389        }
390    }
391
392    /// Everything about the window's shape survives a write and a read.
393    ///
394    /// Round-tripped rather than compared against a fixture: the file is something a user may
395    /// edit, but what it has to *do* is bring the window back, and that is the claim worth
396    /// pinning. The position is the awkward one — it is the only value here in physical
397    /// pixels, and it can be negative, which is what a monitor to the left of the primary one
398    /// means.
399    #[test]
400    fn the_window_comes_back_the_way_it_was_left() {
401        let saved = Config {
402            panes: vec![pane(1, &[r"C:\src", r"C:\src\ui"]), pane(0, &[r"D:\"])],
403            layout: Some("h0.400(0,1)".to_owned()),
404            focus: 1,
405            window: Some([1380.0, 840.0]),
406            position: Some([-1920.0, -8.0]),
407            sidebar_width: 260.0,
408            dark: false,
409            preview: crate::ui::preview::Layout {
410                at: crate::ui::preview::Where::Bottom,
411                share: 0.615,
412                numbers: true,
413            },
414            ..Config::default()
415        };
416
417        let back = Config::parse(&saved.to_text());
418        assert_eq!(back.window, saved.window);
419        assert_eq!(back.position, saved.position);
420        assert_eq!(back.layout, saved.layout);
421        assert_eq!(back.focus, 1);
422        assert_eq!(back.sidebar_width, 260.0);
423        assert!(!back.dark);
424        // The preview panel's three preferences. Worth pinning together with the window's shape,
425        // because they are the same kind of thing — how the window comes back — and because a
426        // value that is written and not read is the failure this round trip is for: `preview=` was
427        // parsed for a while before anything wrote it, and the panel silently forgot its position
428        // every session.
429        assert_eq!(back.preview.at, crate::ui::preview::Where::Bottom);
430        assert!((back.preview.share - 0.615).abs() < 1e-3);
431        assert!(back.preview.numbers);
432
433        assert_eq!(back.panes.len(), 2, "{:?}", back.panes);
434        assert_eq!(back.panes[0].paths, saved.panes[0].paths);
435        assert_eq!(back.panes[0].active, 1, "which tab was in front is part of it");
436        assert_eq!(back.panes[1].paths, saved.panes[1].paths);
437    }
438
439    /// A settings file from the version that remembered tabs but not panes.
440    ///
441    /// Its `path` lines have no `pane` line above them, and what they meant was one pane
442    /// holding all of them. Somebody upgrading has that file and no other, so reading it as
443    /// nothing at all would lose every folder they had open.
444    #[test]
445    fn a_file_from_before_panes_opens_as_one_pane() {
446        let back = Config::parse("path=C:\\a\npath=C:\\b\ntheme=dark\n");
447        assert_eq!(back.panes.len(), 1);
448        assert_eq!(back.panes[0].paths.len(), 2);
449        assert_eq!(back.layout, None, "and no layout to try to build");
450    }
451
452    /// The caps hold, and never by dropping a pane.
453    ///
454    /// A pane with no tabs cannot be drawn, and `layout=` is a tree over exactly the panes
455    /// that follow it — so a cap that emptied one would describe a window this program then
456    /// refuses to build, and the whole layout would be thrown away over a tab limit.
457    #[test]
458    fn the_tab_cap_never_costs_a_pane() {
459        let many: Vec<String> = (0..40).map(|i| format!("C:\\{i}")).collect();
460        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
461        let saved = Config {
462            // The first pane alone wants more than the cap allows, and there are two more
463            // behind it.
464            panes: vec![pane(30, &refs), pane(0, &[r"D:\one"]), pane(0, &[r"D:\two"])],
465            layout: Some("h0.500(0,v0.500(1,2))".to_owned()),
466            ..Config::default()
467        };
468
469        let back = Config::parse(&saved.to_text());
470        assert_eq!(back.panes.len(), 3, "every pane has to be written");
471        assert!(back.panes.iter().all(|p| !p.paths.is_empty()));
472        let total: usize = back.panes.iter().map(|p| p.paths.len()).sum();
473        assert!(total <= TABS + 2, "{total} tabs got through the cap");
474        assert!(
475            back.panes[0].active < back.panes[0].paths.len(),
476            "the tab in front has to be one of the ones that survived"
477        );
478    }
479
480    /// Nonsense is skipped, not fatal — and the rest of the file still lands.
481    #[test]
482    fn a_broken_line_costs_only_itself() {
483        let back = Config::parse(
484            "# a comment\n\
485             \n\
486             sidebar_width=lots\n\
487             window=wide,tall\n\
488             position=\n\
489             focus=first\n\
490             pane=x\n\
491             path=C:\\a\n\
492             theme=light\n",
493        );
494        assert_eq!(back.sidebar_width, SIDEBAR_WIDTH);
495        assert_eq!(back.window, None);
496        assert_eq!(back.position, None);
497        assert_eq!(back.focus, 0);
498        assert_eq!(back.panes.len(), 1);
499        assert_eq!(back.panes[0].active, 0);
500        assert!(!back.dark, "and the line after the mess still applies");
501    }
502}
