1//! The dependency tree: what a binary needs, and where each one came from.
2//!
3//! One of [`crate::ui::preview`]'s three views — the one a `.exe`, a `.dll`, a driver or a
4//! control panel applet gets — so everything about *where* it is drawn, what its header says and
5//! how it is opened belongs to the panel. This is the tree and nothing else.
6//!
7//! # What it is showing
8//!
9//! A tree of [`crate::pe`]'s graph, one row per module, with the folder each one was found in
10//! beside it. The graph is a graph and not a tree — `kernel32.dll` is under nearly everything —
11//! so the display is a walk of it that **stops where a module repeats along the branch it is
12//! already on**. That is what keeps a cycle (and real graphs have them) from being an endless
13//! row of the same two names, without hiding the fact that both things really do import it.
14//!
15//! # What it opens with
16//!
17//! The root expanded, and then **exactly the branches that lead to something that would stop the
18//! program from starting** — see [`crate::pe::Graph::breaks_loading`]. Everything else stays
19//! folded. A modern graph is a thousand modules; expanding it all would be a wall of `api-ms-`
20//! rows with the one line that matters somewhere in the middle of it, which is the mistake the
21//! original Dependency Walker is remembered for.
22//!
23//! # Everything in a row is on one baseline
24//!
25//! The design system's rule, and this tree is the case it is written for: **three texts at two
26//! sizes beside two painted glyphs**, on a 24-point row. Two things have to be true at once, and
27//! neither is what a rect-centred galley gives you.
28//!
29//! - The texts have to be level with the **glyphs**, which `azur::icons` centres on their own
30//!   ink. A line box reserves room under the baseline for descenders that a DLL's name does not
31//!   have, so text centred in the row hangs about a point and a half below a glyph centred in it.
32//! - The 12-point location has to be level with the 14-point **name**. Centring both boxes in the
33//!   same rect does not do that either, and the error is in the other direction: the two fonts
34//!   have different line heights *and* different ascents, so their baselines end up 1.5 points
35//!   apart and the eye sees the column step down as it crosses the row.
36//!
37//! So the row commits to one baseline — [`ink_baseline`], taken from the row's *principal* font,
38//! which is the body one the names are in — and every text in it is drawn on that baseline by
39//! [`galley_on_baseline`]. Text in the principal font lands exactly where centring would have put
40//! it once the glyph correction is applied; everything else comes into line with it.
41
42use std::collections::HashSet;
43use std::path::Path;
44use std::sync::Arc;
45
46use azur_egui_theme::components::{galley_on_baseline, ink_baseline};
47use azur_egui_theme::icons as azur_icons;
48use azur_egui_theme::tokens::space;
49use egui::{pos2, vec2, Color32, Rect, Sense, Ui};
50
51use crate::icons;
52use crate::pe::{self, Graph, State};
53use crate::theme::Theme;
54use crate::ui::{icon_rect, truncated};
55
56/// One module. The listing's row height, so a pane and the panel inside it read as one window.
57pub const ROW: f32 = crate::pane::ROW_HEIGHT;
58
59/// One level of the tree.
60///
61/// Narrow on purpose. A dependency graph is a dozen levels deep in places, and 20 points a
62/// level would put the tenth level's names off the right-hand side of the panel.
63const INDENT: f32 = 12.0;
64
65/// The expander, and the glyph after it.
66const TWISTY: f32 = 10.0;
67const GLYPH: f32 = 14.0;
68
69/// The air at each end of a row, and either side of the columns.
70const PAD: f32 = space::S2;
71
72/// Where the Location column starts, as a share of the tree's width.
73///
74/// A fixed fraction rather than the widest name, because the point of the column is that the
75/// locations line up: a walk whose modules came off four different directories is meant to be
76/// readable as four groups at a glance, and a column that moves with the longest name in view
77/// would reflow every time the tree is expanded.
78const LOCATION_AT: f32 = 0.42;
79
80/// Room at the right-hand end for the processor tag.
81const ARCH: f32 = 52.0;
82
83/// The most rows the tree will build.
84///
85/// Reached only by expanding, and expanding is a click per level — but the auto-expand in
86/// [`View::new`] opens branches on its own, and a graph of a thousand modules with a missing one
87/// under `shell32` would open a subtree of hundreds. Twenty thousand rows is more than anyone will
88/// scroll and small enough to build in a frame.
89const ROW_CAP: usize = 20_000;
90
91/// And how deep the tree goes. Real graphs are under twenty; this is a guard.
92const DEPTH_CAP: usize = 32;
93
94/// One row of the tree: which module, and where it sits in the walk.
95///
96/// Rebuilt from the graph and the expansion set whenever either changes, and not otherwise —
97/// so scrolling a thousand-module graph is arithmetic over this vector rather than a fresh
98/// traversal per frame.
99struct Row {
100    module: usize,
101    depth: u16,
102    /// Whether the reference that got here is delay-loaded.
103    delayed: bool,
104    /// It has imports, and this row is somewhere they can be shown.
105    expandable: bool,
106    open: bool,
107    /// This module is already an ancestor of this row: the branch stops here rather than
108    /// going round again.
109    cyclic: bool,
110}
111
112/// A walked graph, and how much of it is unfolded.
113pub struct View {
114    graph: Arc<Graph>,
115    /// Which modules are expanded, by index into the graph.
116    ///
117    /// By module and not by position in the tree, so expanding `shell32.dll` expands it
118    /// wherever else it appears. That is the behaviour a graph wants: the question is what a
119    /// module needs, and the answer does not depend on which of its importers you asked from.
120    expanded: HashSet<usize>,
121    rows: Vec<Row>,
122    stale: bool,
123}
124
125impl View {
126    /// Take a finished walk and open the branches worth looking at.
127    pub fn new(graph: Arc<Graph>) -> Self {
128        let mut expanded = HashSet::from([0]);
129        for chain in graph.breaks_loading() {
130            // Every module on the way down, but not the missing one itself — it has nothing
131            // under it to show.
132            for &at in chain.iter().rev().skip(1) {
133                expanded.insert(at);
134            }
135        }
136        let mut view = Self {
137            graph,
138            expanded,
139            rows: Vec::new(),
140            stale: true,
141        };
142        view.rebuild();
143        view
144    }
145
146    pub fn graph(&self) -> &Graph {
147        &self.graph
148    }
149
150    /// How many rows are on show. For the tests, which is where a fold driven by a real click
151    /// can be seen from.
152    #[cfg(test)]
153    pub fn shown(&self) -> usize {
154        self.rows.len()
155    }
156
157    /// Fold a module open or shut.
158    fn toggle(&mut self, module: usize) {
159        if !self.expanded.remove(&module) {
160            self.expanded.insert(module);
161        }
162        self.stale = true;
163    }
164
165    /// Flatten the graph into the rows on show.
166    fn rebuild(&mut self) {
167        self.stale = false;
168        self.rows.clear();
169        let graph = self.graph.clone();
170        // An explicit stack rather than recursion: the depth is bounded by the branch rule
171        // below, but that bound is the module count, and a thousand frames of a UI thread's
172        // stack is not a bound worth relying on.
173        //
174        // Each item is a row to emit and the branch it sits on, and `branch` is what makes the
175        // walk finite: a module that is already above this row is drawn and not descended into.
176        let mut stack: Vec<(usize, u16, bool, Vec<usize>)> = vec![(0, 0, false, Vec::new())];
177        while let Some((module, depth, delayed, branch)) = stack.pop() {
178            if self.rows.len() >= ROW_CAP {
179                break;
180            }
181            let cyclic = branch.contains(&module);
182            let expandable = !graph.modules[module].imports.is_empty()
183                && !cyclic
184                && (depth as usize) < DEPTH_CAP;
185            let open = expandable && self.expanded.contains(&module);
186            self.rows.push(Row {
187                module,
188                depth,
189                delayed,
190                expandable,
191                open,
192                cyclic,
193            });
194            if !open {
195                continue;
196            }
197            let mut below = branch;
198            below.push(module);
199            // Pushed in reverse, since the stack hands them back the other way round and the
200            // import table's order is the order worth showing.
201            for edge in graph.modules[module].imports.iter().rev() {
202                stack.push((edge.to, depth + 1, edge.delayed, below.clone()));
203            }
204        }
205    }
206}
207
208/// The whole answer, for the panel's title tooltip: what the walk found, how long it took, and
209/// where every name was looked for.
210///
211/// All three are here because none of them fits on a bar a few hundred points wide, and because
212/// they belong together: a location means nothing without the list of places that were tried —
213/// the interesting answer is usually "it came off the third one" — and the list means nothing
214/// without what it does *not* model, which is the last paragraph.
215///
216/// The timing is not decoration either. A program that claims to walk a thousand modules quickly
217/// should be willing to be checked, and a walk that suddenly takes two seconds is how you find out
218/// a `PATH` entry has gone.
219pub fn about(graph: &Graph) -> String {
220    /// How many of the search directories are listed. A real `PATH` has thirty entries and the
221    /// ones that matter are at the front: the binary's own folder, then `System32`, then whatever
222    /// was installed most recently.
223    const SHOWN: usize = 10;
224    use std::fmt::Write as _;
225
226    let (files, api_sets, missing) = graph.tally();
227    let mut text = format!(
228        "{files} file{}, {api_sets} API set{}, {missing} missing \u{2014} {:.0} ms{}\n\nLooked for in order:",
229        if files == 1 { "" } else { "s" },
230        if api_sets == 1 { "" } else { "s" },
231        graph.micros as f64 / 1000.0,
232        if graph.truncated {
233            "\nThe walk stopped at its budget or its patience, so this is incomplete."
234        } else {
235            ""
236        }
237    );
238    for dir in graph.search.iter().take(SHOWN) {
239        let _ = write!(text, "\n    {}", dir.display());
240    }
241    if graph.search.len() > SHOWN {
242        let _ = write!(
243            text,
244            "\n    …and {} more from PATH",
245            graph.search.len() - SHOWN
246        );
247    }
248    text.push_str(
249        "\n\nKnownDLLs, side-by-side assemblies and manifest redirection\n\
250         are not modelled, and win over all of these at load time.",
251    );
252    text
253}
254
255/// Draw the tree at `rect`, and answer the pointer.
256pub fn show(ui: &mut Ui, t: &Theme, rect: Rect, view: &mut View) {
257    if view.stale {
258        view.rebuild();
259    }
260    let graph = view.graph.clone();
261    let count = view.rows.len();
262
263    let mut child = ui.new_child(
264        egui::UiBuilder::new()
265            .max_rect(rect)
266            .layout(egui::Layout::top_down(egui::Align::Min)),
267    );
268    child.set_clip_rect(rect.intersect(ui.clip_rect()));
269    // Rows are painted at exact rects, so the scroll extent has to be the rows and nothing
270    // else — see the same line in `filelist`, and the same reason: the installed style's item
271    // spacing would make the extent taller than what is drawn and put every row a little
272    // further from where the pointer thinks it is.
273    child.spacing_mut().item_spacing = egui::Vec2::ZERO;
274
275    let mut toggled = None;
276    let scroll = egui::ScrollArea::vertical()
277        .id_salt(("deps-rows", rect.min.x as i32, rect.min.y as i32))
278        .auto_shrink([false, false]);
279    scroll.show_rows(&mut child, ROW, count, |ui, range| {
280        let first = range.start;
281        // The right-hand edge is the scroll area's, not the panel's: a `ScrollArea` keeps a
282        // few points for its bar and clips to what is left, so a row measured against the
283        // panel's own width puts its last column under that clip — which is how the processor
284        // tag came out as `x6`.
285        let right_edge = ui.clip_rect().right().min(rect.right());
286        let visible = Rect::from_min_max(
287            pos2(rect.left(), ui.min_rect().top()),
288            pos2(right_edge, ui.min_rect().top() + range.len() as f32 * ROW),
289        );
290        // One interaction for the whole block, and the row worked out from the pointer, for
291        // the reason `filelist` gives: a widget per row would be an id, a hit test and an
292        // animation slot each for a highlight that arithmetic gives away.
293        let response = ui.interact(visible, ui.id().with("deps-hit"), Sense::click());
294        let last = range.len().saturating_sub(1);
295        let hovered = response.hover_pos().and_then(|at| {
296            (visible.contains(at) && !range.is_empty())
297                .then(|| first + (((at.y - visible.top()) / ROW) as usize).min(last))
298        });
299
300        let location_x = (visible.left() + visible.width() * LOCATION_AT).round();
301        let arch_right = visible.right() - PAD;
302        let location_right = arch_right - ARCH;
303
304        for at in range.clone() {
305            let Some(row) = view.rows.get(at) else {
306                continue;
307            };
308            let module = &graph.modules[row.module];
309            let rect = Rect::from_min_size(
310                pos2(visible.left(), visible.top() + (at - first) as f32 * ROW),
311                vec2(visible.width(), ROW),
312            );
313            // The one line every text in this row sits on, whatever font or size it is in, and
314            // level with the two glyphs beside them. See the module header. Per row rather than
315            // hoisted, because it is snapped to the pixel grid and a scrolled row's top is not
316            // a whole number of points — it costs a cached layout lookup.
317            let baseline = ink_baseline(ui.painter(), &t.fonts.body, rect.top(), ROW);
318
319            if hovered == Some(at) {
320                ui.painter()
321                    .rect_filled(rect, egui::CornerRadius::ZERO, crate::ui::hover_fill(t));
322            }
323
324            let mut x = rect.left() + PAD + row.depth as f32 * INDENT;
325
326            // The expander. Only where there is something under it, and the space is kept
327            // either way so that the names of a level line up whether or not each one has
328            // children — a tree whose leaves start a few points left of its branches reads as
329            // a mistake.
330            let twisty = icon_rect(rect, x, TWISTY);
331            if row.expandable {
332                let over = hovered == Some(at);
333                let glyph: azur_egui_theme::icons::Icon<'_> = if row.open {
334                    &azur_icons::chevron_down
335                } else {
336                    &azur_icons::chevron_right
337                };
338                glyph(
339                    ui.painter(),
340                    twisty,
341                    if over { t.text.primary } else { t.text.secondary },
342                );
343            }
344            x = twisty.right() + PAD;
345
346            // The glyph, which is where the module's state is said first: a binary, or the error
347            // mark for something that is not there or cannot be read.
348            //
349            // **A missing module that is delay-loaded gets the mark in a neutral ink**, because
350            // it is not a fault: Windows ships stubs for features that are not installed, and
351            // nothing opens a delay-loaded DLL until something calls into it. See
352            // `pe::Graph::breaks_loading` for the five this program's own binary finds.
353            let (glyph, ink): (azur_egui_theme::icons::Icon<'_>, Color32) = match module.state {
354                State::Found => (&icons::executable, t.executable),
355                State::ApiSet => (&icons::executable, t.text.secondary),
356                State::Missing if row.delayed => (&azur_icons::error, t.text.secondary),
357                State::Missing | State::Unreadable(_) => (&azur_icons::error, t.status.danger),
358                State::Unvisited => (&azur_icons::ellipsis, t.text.secondary),
359            };
360            let box_rect = icon_rect(rect, x, GLYPH);
361            glyph(ui.painter(), box_rect, ink);
362            x = box_rect.right() + PAD;
363
364            // ---- Name ----
365            //
366            // A missing module's name is the one thing in the tree worth colouring, and an API
367            // set or a delay-loaded reference is one step quieter: neither is part of what has
368            // to be on disk for the program to start.
369            //
370            // Only three inks appear here, and that is measured rather than tidy — see
371            // `every_ink_in_the_tree_can_be_read`. `text-tertiary` is 1.65:1 on a hovered row
372            // in the dark theme and `text-disabled` is 1.00:1, which is to say it is the fill.
373            let name_color = match module.state {
374                State::Missing if !row.delayed => t.status.danger,
375                State::Missing | State::ApiSet | State::Unvisited => t.text.secondary,
376                _ if row.delayed => t.text.secondary,
377                _ => t.text.primary,
378            };
379            let name_right = location_x - PAD;
380            let galley = truncated(
381                ui.painter(),
382                &module.name,
383                t.fonts.body.clone(),
384                name_color,
385                (name_right - x).max(0.0),
386            );
387            galley_on_baseline(ui.painter(), x, baseline, galley);
388
389            // ---- Location ----
390            //
391            // The folder, not the whole path: the name is already in the column to the left,
392            // and repeating it costs the width that tells you the two `VCRUNTIME140.dll`s in a
393            // graph came off different directories.
394            let (where_, tint) = location_of(module, row, t);
395            let galley = truncated(
396                ui.painter(),
397                &where_,
398                t.fonts.caption.clone(),
399                tint,
400                (location_right - location_x).max(0.0),
401            );
402            galley_on_baseline(ui.painter(), location_x, baseline, galley);
403
404            // ---- The processor ----
405            //
406            // In the danger colour when it is not the root's, because Windows will not load
407            // it — which is the second most useful thing this tree can tell you.
408            if module.machine != 0 {
409                let foreign = graph.foreign(row.module);
410                let galley = truncated(
411                    ui.painter(),
412                    pe::machine_name(module.machine),
413                    t.fonts.caption.clone(),
414                    if foreign {
415                        t.status.danger
416                    } else {
417                        t.text.secondary
418                    },
419                    ARCH,
420                );
421                let width = galley.size().x;
422                galley_on_baseline(ui.painter(), arch_right - width, baseline, galley);
423            }
424        }
425
426        // Clicking a row folds it, which is the whole vocabulary this tree needs. Answered
427        // after the loop so the row that was drawn is the row that was clicked.
428        if response.clicked() {
429            if let Some(at) = hovered {
430                if let Some(row) = view.rows.get(at) {
431                    if row.expandable {
432                        toggled = Some(row.module);
433                    }
434                }
435            }
436        }
437    });
438
439    if let Some(module) = toggled {
440        view.toggle(module);
441    }
442}
443
444/// What goes in the Location column, and in what ink.
445fn location_of(module: &pe::Module, row: &Row, t: &Theme) -> (String, Color32) {
446    if row.cyclic {
447        // Already above this row: the branch stops, and saying why is better than a leaf that
448        // looks like it has nothing under it.
449        return ("(already above)".to_owned(), t.text.secondary);
450    }
451    match module.state {
452        State::Found => match module.path.as_deref().and_then(Path::parent) {
453            Some(folder) => (folder.to_string_lossy().into_owned(), t.text.secondary),
454            None => (String::new(), t.text.secondary),
455        },
456        // Not a file, and the honest answer is what resolves it rather than a path.
457        State::ApiSet => (
458            "resolved by the API set schema".to_owned(),
459            t.text.secondary,
460        ),
461        // Delay-loaded and missing is worth saying in full, because it is the difference between
462        // "this will not run" and "one feature will not work, later, somewhere else".
463        State::Missing if row.delayed => ("not found — delay-loaded".to_owned(), t.text.secondary),
464        State::Missing => ("not found".to_owned(), t.status.danger),
465        State::Unreadable(why) => (why.to_owned(), t.status.danger),
466        State::Unvisited => (
467            "not visited — the walk stopped".to_owned(),
468            t.text.secondary,
469        ),
470    }
471}
472
473#[cfg(test)]
474mod tests {
475    use super::*;
476
477    /// A graph of this program's own test binary, walked for real.
478    fn walked() -> Arc<Graph> {
479        let me = std::env::current_exe().expect("a test process has an executable");
480        Arc::new(pe::walk(&me, pe::BUDGET, pe::PATIENCE))
481    }
482
483    /// **Every ink this tree uses can be read on every surface it is drawn on**, in both themes.
484    ///
485    /// Measured rather than counted — `azur::contrast` is the ruler — and it found two real
486    /// defects on the first run, both of which are the obvious way to write this:
487    ///
488    /// | ink | on a row | on a hovered row |
489    /// | --- | --- | --- |
490    /// | `text-primary` | 16.6 / 18.1 | 7.3 / 9.7 |
491    /// | `text-secondary` | 6.9 / 7.4 | 3.0 / 4.0 |
492    /// | `status-danger` | 4.9 / 4.4 | 2.1 / 2.4 |
493    /// | ~~`text-tertiary`~~ | 3.8 / 5.8 | **1.7** / 3.1 |
494    /// | ~~`text-disabled`~~ | 2.3 / 2.5 | **1.0** / 1.3 |
495    /// | ~~`status-warning`~~ | 8.2 / **2.8** | 3.6 / **1.5** |
496    ///
497    /// `text-disabled` at 1.00:1 *is* the hover fill in the dark theme: an API set's location
498    /// line was invisible for as long as the pointer was over the row it was on. And
499    /// `status-warning` is a legitimate role that simply is not ink at 12 points on a light
500    /// surface — it was the processor tag on a mismatched module, which is the one row in a
501    /// dependency list you most need to be able to read.
502    ///
503    /// So the tree has three inks, and this is what keeps it that way. The two floors:
504    /// **4.0:1 on the surface a row is normally read on**, and **2.0:1 in the transient hovered
505    /// state** — the same argument, and a rung above the number, that `azur::desktop`'s own
506    /// `a_row_can_still_be_read_when_it_is_hovered_or_pressed` settled on at 2.5.
507    #[test]
508    fn every_ink_in_the_tree_can_be_read() {
509        use azur_egui_theme::contrast::ratio;
510
511        const RESTING: f32 = 4.0;
512        const HOVERED: f32 = 2.0;
513
514        for t in [Theme::dark(), Theme::light()] {
515            let name = if t.dark { "dark" } else { "light" };
516            let hovered = crate::ui::hover_fill(&t);
517            for (what, ink) in [
518                ("text-primary", t.text.primary),
519                ("text-secondary", t.text.secondary),
520                ("status-danger", t.status.danger),
521            ] {
522                for (surface, fill, floor) in [
523                    ("a row", t.bg.layer, RESTING),
524                    ("a hovered row", hovered, HOVERED),
525                ] {
526                    let got = ratio(ink, fill);
527                    assert!(
528                        got >= floor,
529                        "{name}: {what} on {surface} is {got:.2}:1, under {floor}"
530                    );
531                }
532            }
533            // And the glyphs, at the floor a *shape* gets rather than the one ink gets.
534            for (what, ink, fill, surface) in [
535                ("the module glyph", t.executable, t.bg.layer, "a row"),
536                ("the module glyph", t.executable, hovered, "a hovered row"),
537                ("the missing mark", t.status.danger, t.bg.layer, "a row"),
538            ] {
539                let got = ratio(ink, fill);
540                assert!(
541                    got >= azur_egui_theme::contrast::SHAPE,
542                    "{name}: {what} on {surface} is {got:.2}:1, under the shape floor"
543                );
544            }
545        }
546
547        // And the three that were taken out really are the reason: each is under the hovered
548        // floor on some surface. Across **both** themes rather than within one, which is the
549        // whole point of checking both — `status-warning` is perfectly legible on the dark side
550        // at 3.58:1 and 1.49:1 on the light one, and a rule derived from the dark theme alone is
551        // how the two light-theme defects this project has already fixed got in.
552        type Pick = fn(&Theme) -> Color32;
553        for (what, pick) in [
554            ("text-tertiary", (|t: &Theme| t.text.tertiary) as Pick),
555            ("text-disabled", |t: &Theme| t.text.disabled),
556            ("status-warning", |t: &Theme| t.status.warning),
557        ] {
558            let worst = [Theme::dark(), Theme::light()]
559                .iter()
560                .flat_map(|t| {
561                    let ink = pick(t);
562                    [t.bg.layer, crate::ui::hover_fill(t)].map(|fill| ratio(ink, fill))
563                })
564                .fold(f32::INFINITY, f32::min);
565            assert!(
566                worst < HOVERED,
567                "{what} measures {worst:.2}:1 at worst and could be used after all"
568            );
569        }
570    }
571
572    /// The tree is a walk of a graph, and the two things that make that finite: a module
573    /// already on the branch is not descended into, and expansion is what decides the rest.
574    #[test]
575    fn the_tree_folds_and_never_goes_round_a_cycle() {
576        let graph = walked();
577        let mut view = View::new(graph.clone());
578
579        assert_eq!(view.rows[0].module, 0, "the root is the first row");
580        assert_eq!(view.rows[0].depth, 0);
581        assert!(view.rows[0].open, "the root opens expanded");
582        assert_eq!(
583            view.rows.len(),
584            1 + graph.modules[0].imports.len(),
585            "a freshly walked binary shows its own imports and no more"
586        );
587
588        // Folding the root leaves one row; unfolding puts them back.
589        view.toggle(0);
590        view.rebuild();
591        assert_eq!(view.rows.len(), 1);
592        assert!(!view.rows[0].open);
593        view.toggle(0);
594        view.rebuild();
595        assert!(view.rows.len() > 1);
596
597        // Expand everything there is, which is what would run away if either rule were
598        // missing: this graph has cycles in it — `kernel32` and `kernelbase` refer to each
599        // other — so a walk with no branch rule would not come back.
600        for i in 0..graph.modules.len() {
601            view.expanded.insert(i);
602        }
603        view.rebuild();
604        assert!(view.rows.len() > graph.modules[0].imports.len());
605        assert!(view.rows.len() <= ROW_CAP);
606        for row in &view.rows {
607            assert!((row.depth as usize) <= DEPTH_CAP);
608            // Nothing is both a repeat and expanded: that is the rule, stated as an assertion.
609            assert!(!(row.cyclic && row.open));
610        }
611        // And something really was a repeat, or the rule was never exercised.
612        assert!(
613            view.rows.iter().any(|row| row.cyclic),
614            "no cycle was reached, so the branch rule proves nothing here"
615        );
616    }
617
618    /// A walk opens the branches that lead to something that would stop the program starting,
619    /// and nothing else.
620    #[test]
621    fn a_walk_opens_only_what_is_worth_looking_at() {
622        let graph = walked();
623        let view = View::new(graph.clone());
624
625        // Every module opened is either the root or on a chain to something missing.
626        let chains = graph.breaks_loading();
627        for &module in &view.expanded {
628            let on_a_chain = chains
629                .iter()
630                .any(|chain| chain[..chain.len() - 1].contains(&module));
631            assert!(
632                module == 0 || on_a_chain,
633                "{} was opened for no reason",
634                graph.modules[module].name
635            );
636        }
637        // And every chain really is open, all the way down.
638        for chain in &chains {
639            for &module in &chain[..chain.len() - 1] {
640                assert!(
641                    view.expanded.contains(&module),
642                    "the way down to a missing module is not open at {}",
643                    graph.modules[module].name
644                );
645            }
646        }
647    }
648
649    /// The search order is reported with what it does not model, because a location means
650    /// nothing without the list of places that were tried.
651    #[test]
652    fn the_whole_answer_is_on_the_title() {
653        let graph = walked();
654        let text = about(&graph);
655        let (files, _, _) = graph.tally();
656        assert!(
657            text.starts_with(&format!("{files} file")),
658            "the counts are not the first thing said: {text}"
659        );
660        assert!(text.contains(" ms"), "the timing is gone");
661        assert!(text.contains("Looked for in order:"));
662        assert!(
663            text.contains(&graph.search[0].display().to_string()),
664            "the first place looked is not in the list"
665        );
666        assert!(
667            text.contains("KnownDLLs"),
668            "the caveats are gone, and the list now reads as the whole rule"
669        );
670    }
671}
