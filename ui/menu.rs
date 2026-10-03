1//! Drawing the context menu.
2//!
3//! The content is the shell's — see [`crate::shell::menu`] — but the menu itself is
4//! this program's: Azur's `MenuItem` on Azur's popover surface, in the same palette as
5//! everything else, with the same focus ring and the same row states. A native
6//! `TrackPopupMenu` would have been less work and would have looked like a different
7//! application had opened.
8//!
9//! # Position
10//!
11//! At the pointer, and flipped rather than clipped when it would run off an edge —
12//! which is why the size is computed before anything is drawn rather than measured
13//! after. An `egui::Area` sizes itself to its content, so a menu that discovered its
14//! own height a frame late would appear in the wrong place and then jump.
15//!
16//! # Submenus
17//!
18//! One [`egui::Area`] per open level, each anchored to the item that opened it, opening
19//! to the right unless there is no room. `open` is the chain of indices currently
20//! showing, so the whole state of an arbitrarily deep menu is one `Vec<usize>`.
21//!
22//! A shell submenu arrives empty — filling it means asking an extension, which is slow
23//! enough to be worth not doing until somebody opens it. So opening one puts its id in
24//! [`Open::fills`], and the level appears when the answer does. See
25//! [`crate::shell::menu`].
26//!
27//! # Nothing here reflows
28//!
29//! The shell's half of the menu takes between a sixth of a second and most of a second to
30//! arrive, and none of that waiting happens on screen: [`Open`] is not built until the
31//! entries are all there, so the menu appears once, at its final size, and never grows
32//! under the pointer. An earlier version opened immediately with this program's own entries
33//! and a `Loading…` row and let the shell's land underneath — which kept the window
34//! responsive and did not read as a context menu at all.
35
36use std::collections::HashMap;
37
38use azur_egui_theme::components::{menu_divider, popover_frame, MenuItem};
39use azur_egui_theme::tokens::{radius, space};
40use egui::{pos2, vec2, Color32, Id, Order, Pos2, Rect, TextureHandle, Ui, Vec2};
41
42use crate::shell::menu::{Command, Entry, Kind};
43use crate::theme::Theme;
44
45/// A menu on screen.
46pub struct Open {
47    /// The pane it was raised from.
48    pub pane: crate::pane::PaneId,
49    /// Where the pointer was, in points.
50    pub at: Pos2,
51    /// What it is for. Empty means the folder's own menu.
52    pub items: Vec<std::path::PathBuf>,
53    pub folder: std::path::PathBuf,
54    pub entries: Vec<Entry>,
55    /// Which submenu chain is showing, as indices from the root.
56    pub open: Vec<usize>,
57    /// The keyboard highlight, as a path from the root.
58    pub cursor: Option<Vec<usize>>,
59    /// Which build of the shell's menu this is; see [`crate::shell::menu::Builder`].
60    pub token: u64,
61    /// Submenus the shell should be asked to fill, for the caller to drain and send on.
62    pub fills: Vec<u32>,
63    /// Submenus already asked about, so a hover asks once rather than once a frame.
64    asked: std::collections::HashSet<u32>,
65    /// Item bitmaps, uploaded once and keyed by the path to their entry.
66    textures: HashMap<Vec<usize>, TextureHandle>,
67    /// Set on the frame it opens, so the click that opened it does not also close it.
68    fresh: bool,
69    /// What the root level actually took on screen, last frame.
70    ///
71    /// Kept because it is the one thing worth asserting about a menu that lays itself out
72    /// by arithmetic: it has to come out the size it said it would. See the test.
73    pub drawn: Vec2,
74}
75
76impl Open {
77    pub fn new(
78        pane: crate::pane::PaneId,
79        at: Pos2,
80        items: Vec<std::path::PathBuf>,
81        folder: std::path::PathBuf,
82        entries: Vec<Entry>,
83        token: u64,
84    ) -> Self {
85        Self {
86            pane,
87            at,
88            items,
89            folder,
90            entries,
91            open: Vec::new(),
92            cursor: None,
93            token,
94            fills: Vec::new(),
95            asked: std::collections::HashSet::new(),
96            textures: HashMap::new(),
97            fresh: true,
98            drawn: Vec2::ZERO,
99        }
100    }
101
102    /// The entry at a path, if there is one.
103    fn entry(&self, path: &[usize]) -> Option<&Entry> {
104        let mut level = &self.entries;
105        for (depth, index) in path.iter().enumerate() {
106            let entry = level.get(*index)?;
107            if depth + 1 == path.len() {
108                return Some(entry);
109            }
110            match &entry.kind {
111                Kind::Submenu { children, .. } => level = children,
112                _ => return None,
113            }
114        }
115        None
116    }
117
118    /// The entries at a level.
119    fn level(&self, path: &[usize]) -> Option<&Vec<Entry>> {
120        if path.is_empty() {
121            return Some(&self.entries);
122        }
123        match self.entry(path)?.kind {
124            Kind::Submenu { ref children, .. } => Some(children),
125            _ => None,
126        }
127    }
128
129    /// One submenu's contents have arrived.
130    ///
131    /// An empty answer is a real answer — the extension had nothing — so the row stays where
132    /// it is and stops being usable, rather than vanishing from under the pointer.
133    pub fn filled(&mut self, id: u32, children: Vec<Entry>) {
134        fn put(entries: &mut [Entry], id: u32, children: &mut Option<Vec<Entry>>) {
135            for entry in entries.iter_mut() {
136                if entry.kind.unasked() == Some(id) {
137                    if let Some(children) = children.take() {
138                        entry.enabled = !children.is_empty();
139                        entry.kind = Kind::complete(children);
140                    }
141                    return;
142                }
143                if let Kind::Submenu { children: deeper, .. } = &mut entry.kind {
144                    put(deeper, id, children);
145                    if children.is_none() {
146                        return;
147                    }
148                }
149            }
150        }
151        put(&mut self.entries, id, &mut Some(children));
152    }
153
154    /// Ask for the open submenu's contents, once.
155    fn ask(&mut self) {
156        if self.open.is_empty() {
157            return;
158        }
159        let Some(id) = self.entry(&self.open.clone()).and_then(|e| e.kind.unasked()) else {
160            return;
161        };
162        if self.asked.insert(id) {
163            self.fills.push(id);
164        }
165    }
166}
167
168/// What happened to the menu this frame.
169pub enum Outcome {
170    /// Still open.
171    Open,
172    /// Dismissed without choosing anything.
173    Closed,
174    /// This was chosen.
175    Chose(Command),
176}
177
178/// One row's height, from the design system rather than from arithmetic repeated here.
179///
180/// It has to be the real number: [`measure`] adds these up to place the menu *before*
181/// anything is drawn, so a value that disagrees with what `MenuItem` allocates puts the
182/// menu in the wrong place — the taller the menu, the further out.
183fn row_height() -> f32 {
184    azur_egui_theme::components::menu_item_height()
185}
186
187/// A separator's, asked for rather than derived — the same reason [`row_height`] is.
188///
189/// This one was `space::S2 * 2.0 + 1.0`, which is what the divider allocated when it was
190/// written; the design system has since tightened it to `space-1` either side, and every menu
191/// with a divider in it was being placed four points out per divider. The test at the bottom
192/// of this file is what caught it.
193fn separator_height() -> f32 {
194    azur_egui_theme::components::menu_divider_height()
195}
196
197/// Draw the menu and every open submenu.
198pub fn show(ui: &mut Ui, t: &Theme, menu: &mut Open) -> Outcome {
199    let ctx = ui.ctx().clone();
200
201    // ---- Keyboard ------------------------------------------------------
202    if let Some(outcome) = keyboard(&ctx, menu) {
203        return outcome;
204    }
205
206    // Textures for the shell's item bitmaps, uploaded once each.
207    upload_icons(&ctx, menu, &mut Vec::new());
208
209    // A submenu is empty until the extension that owns it is asked, and asking is the
210    // expensive part — so it happens on the hover that opens it.
211    menu.ask();
212
213    let screen = ctx.viewport_rect();
214    let mut chosen = None;
215    let mut hovered_any = false;
216    // The chain the pointer is currently over, which becomes the open chain so that
217    // moving off a submenu and onto a sibling closes the old one.
218    let mut wants_open: Option<Vec<usize>> = None;
219
220    // Levels are drawn root-first, each anchored to the item that opened it.
221    let depth = menu.open.len();
222    let mut anchor = Rect::from_min_size(menu.at, Vec2::ZERO);
223    for level_depth in 0..=depth {
224        let path: Vec<usize> = menu.open[..level_depth].to_vec();
225        let Some(entries) = menu.level(&path).cloned() else {
226            break;
227        };
228        // A level with nothing in it is either a submenu still being filled or one that
229        // turned out to be empty; either way there is nothing to hang an area on.
230        if entries.is_empty() {
231            break;
232        }
233
234        let size = measure(&ctx, t, &entries, screen);
235        let origin = place(anchor, size, screen, level_depth == 0);
236
237        let response = egui::Area::new(Id::new(("shell-menu", level_depth)))
238            .order(Order::Foreground)
239            .fixed_pos(origin)
240            // Never movable: an `Area` that can be dragged is an `Area` that will be,
241            // and a menu that slides is a menu that is broken.
242            .movable(false)
243            // No fade. An `Area` fades in over a tenth of a second by default, which is
244            // right for a tooltip that appeared on its own and wrong for a menu the user
245            // asked for and is already reading.
246            .fade_in(false)
247            .show(&ctx, |ui| {
248                popover_frame(t.azur())
249                    .show(ui, |ui| {
250                        // An explicit rect for the rows, rather than whatever the `Ui`
251                        // says is available.
252                        //
253                        // The `Ui` an `Area` hands over reports an available height
254                        // derived from the size the area had *last* frame, so a scroll
255                        // area that sizes itself from it feeds its own output back in and
256                        // settles at whatever height it first happened to be — measured
257                        // 584, drawn 400, every frame. It is the same trap as the one
258                        // documented at length on [`crate::ui::chrome::resize_borders`],
259                        // and the same answer: use the rect this code already computed.
260                        let inner = vec2(
261                            size.x - space::S2 * 2.0,
262                            size.y - space::S2 * 2.0,
263                        );
264                        let mut rows = ui.new_child(
265                            egui::UiBuilder::new()
266                                .max_rect(Rect::from_min_size(ui.max_rect().min, inner))
267                                .layout(egui::Layout::top_down(egui::Align::Min)),
268                        );
269                        // Rows touch. A gap between menu entries is a gap the pointer can
270                        // cross into nothing, and over twenty entries — which a shell menu
271                        // with a few extensions installed easily reaches — it is a whole
272                        // screen of menu that did not need to exist.
273                        rows.spacing_mut().item_spacing.y = 0.0;
274                        // A menu with several extensions installed is taller than a
275                        // 600-point window, and the entries past the edge would simply be
276                        // unreachable. `auto_shrink` vertically, so a short menu is still
277                        // its own height rather than the whole screen.
278                        let drawn = egui::ScrollArea::vertical()
279                            .id_salt(("shell-menu-rows", level_depth))
280                            .max_height(inner.y)
281                            .auto_shrink([false, true])
282                            .show(&mut rows, |ui| {
283                                draw_level(
284                                    ui,
285                                    t,
286                                    menu,
287                                    &path,
288                                    &entries,
289                                    &mut chosen,
290                                    &mut wants_open,
291                                )
292                            })
293                            .inner;
294                        // What the child used, so the frame wraps the rows rather than
295                        // collapsing to nothing behind them.
296                        ui.advance_cursor_after_rect(rows.min_rect());
297                        drawn
298                    })
299                    .inner
300            });
301        if level_depth == 0 {
302            menu.drawn = response.response.rect.size();
303        }
304        if response.response.contains_pointer() {
305            hovered_any = true;
306        }
307
308        // The next level hangs off whichever item is open at this one.
309        if level_depth < depth {
310            let index = menu.open[level_depth];
311            anchor = response
312                .inner
313                .get(&index)
314                .copied()
315                .unwrap_or(response.response.rect);
316        }
317    }
318
319    if let Some(command) = chosen {
320        return Outcome::Chose(command);
321    }
322
323    // Opening and closing submenus follows the pointer: hovering a submenu opens it,
324    // hovering a sibling closes whatever was open beside it.
325    if let Some(path) = wants_open {
326        if menu.open != path {
327            menu.open = path;
328        }
329    }
330
331    // ---- Dismissal -----------------------------------------------------
332    let clicked = ctx.input(|i| i.pointer.any_click());
333    if menu.fresh {
334        // The release of the right click that opened it arrives on the next frame.
335        menu.fresh = false;
336    } else if clicked && !hovered_any {
337        return Outcome::Closed;
338    }
339    Outcome::Open
340}
341
342/// One level's rows. Returns where each row ended up, so a submenu can be anchored.
343fn draw_level(
344    ui: &mut Ui,
345    t: &Theme,
346    menu: &Open,
347    path: &[usize],
348    entries: &[Entry],
349    chosen: &mut Option<Command>,
350    wants_open: &mut Option<Vec<usize>>,
351) -> HashMap<usize, Rect> {
352    let mut rects = HashMap::new();
353
354    for (index, entry) in entries.iter().enumerate() {
355        if matches!(entry.kind, Kind::Separator) {
356            menu_divider(ui);
357            continue;
358        }
359
360        let mut here = path.to_vec();
361        here.push(index);
362        let is_submenu = matches!(entry.kind, Kind::Submenu { .. });
363        let highlighted = menu.cursor.as_deref() == Some(here.as_slice());
364
365        // The shell's own bitmap for the row, drawn through Azur's icon slot so the
366        // layout is the component's rather than something invented here.
367        let texture = menu.textures.get(&here).map(|handle| handle.id());
368        let paint_icon = move |painter: &egui::Painter, rect: Rect, _tint: Color32| {
369            if let Some(texture) = texture {
370                painter.image(
371                    texture,
372                    rect,
373                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
374                    Color32::WHITE,
375                );
376            }
377        };
378
379        // The arrow-key cursor, painted under the row rather than through `MenuItem`:
380        // the component fills for a hover and for keyboard focus, neither of which this
381        // is, and `selected` is not it either — that is the tick a checked entry gets,
382        // and using it would both show a false tick and shift the label sideways as the
383        // cursor moved. The rect is the one the row is about to take, which is knowable
384        // because the rows are a top-down column with no spacing between them.
385        if highlighted && entry.enabled {
386            let rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), row_height()));
387            ui.painter().rect_filled(
388                rect,
389                egui::CornerRadius::same(radius::SMALL),
390                t.bg.control_hover,
391            );
392        }
393
394        let mut item = MenuItem::new(entry.label.clone())
395            .shortcut(entry.shortcut.clone())
396            .submenu(is_submenu)
397            .selected(entry.checked);
398        if texture.is_some() {
399            item = item.icon(&paint_icon);
400        }
401
402        let response = ui.add_enabled(entry.enabled, item);
403        rects.insert(index, response.rect);
404        if highlighted {
405            // The arrow keys can walk past the bottom of a scrolled level, and a cursor
406            // you cannot see is a cursor you have lost.
407            response.scroll_to_me(None);
408        }
409
410        // The bold entry — what a double click would have done. Azur's `MenuItem` has no
411        // weight of its own, so the mark is a 2px accent bar, the same one a selected row
412        // gets, which is the vocabulary already in use.
413        if entry.default && entry.enabled {
414            ui.painter().rect_filled(
415                Rect::from_min_size(response.rect.min, vec2(2.0, response.rect.height())),
416                egui::CornerRadius::same(radius::CIRCULAR),
417                t.accent.default,
418            );
419        }
420
421        if response.hovered() {
422            *wants_open = Some(if is_submenu {
423                here.clone()
424            } else {
425                path.to_vec()
426            });
427        }
428        if response.clicked() && entry.enabled {
429            match &entry.kind {
430                Kind::Command(command) => *chosen = Some(command.clone()),
431                // Clicking a submenu row opens it rather than doing nothing, which is
432                // what a pointer expects even though hovering already did it.
433                Kind::Submenu { .. } => *wants_open = Some(here.clone()),
434                Kind::Separator => {}
435            }
436        }
437    }
438    rects
439}
440
441/// How big a level will be, before anything is drawn.
442///
443/// Computed rather than measured because the position depends on it: a menu that
444/// learned its own height a frame late would appear in the wrong place and jump.
445///
446/// Capped at the screen, which is also what makes the level scroll: the rows go in a
447/// scroll area of exactly this height, so a menu longer than the window keeps its last
448/// entry reachable instead of drawing it past the edge.
449fn measure(ctx: &egui::Context, t: &Theme, entries: &[Entry], screen: Rect) -> Vec2 {
450    // `Menu { min-width: 180px }`, and a cap so one long "Open with" entry does not
451    // stretch the menu across the window.
452    const MIN: f32 = 180.0;
453    const MAX: f32 = 420.0;
454
455    // What a row puts around its text, asked for rather than repeated here. This was
456    // repeated here, and it was wrong in two places at once: the leading gutter was added
457    // only for the entries that had an icon, when `MenuItem` reserves it for every entry,
458    // and the gap before a shortcut was `space-5` where the component uses `space-3`. The
459    // two errors cancelled out often enough to go unnoticed, because a shell menu always
460    // has one long "Restore previous versions" in it that pushes the width to `MAX`
461    // anyway. Then this program's own six entries were shown on their own, while the shell
462    // was still being asked, and the menu came up 16 points short with `Copy pa…` in it.
463    let row = azur_egui_theme::components::menu_item_metrics();
464
465    let width = ctx.fonts_mut(|fonts| {
466        let mut widest: f32 = 0.0;
467        for entry in entries {
468            if matches!(entry.kind, Kind::Separator) {
469                continue;
470            }
471            let label = fonts
472                .layout_no_wrap(entry.label.clone(), t.fonts.body.clone(), Color32::WHITE)
473                .size()
474                .x;
475            let shortcut = if entry.shortcut.is_empty() {
476                0.0
477            } else {
478                fonts
479                    .layout_no_wrap(
480                        entry.shortcut.clone(),
481                        t.fonts.caption.clone(),
482                        Color32::WHITE,
483                    )
484                    .size()
485                    .x
486            };
487            let submenu = matches!(entry.kind, Kind::Submenu { .. });
488            widest = widest.max(row.width(label, shortcut, submenu));
489        }
490        widest
491    });
492
493    // Plus the frame's own `space-2` either side.
494    //
495    // Rounded up for the same reason the design system rounds its own: handing the widest
496    // entry exactly its galley width leaves it a fraction short, and it ellipsizes.
497    let width = (width + space::S2 * 2.0).ceil().clamp(MIN, MAX);
498
499    let height: f32 = entries
500        .iter()
501        .map(|entry| {
502            if matches!(entry.kind, Kind::Separator) {
503                separator_height()
504            } else {
505                row_height()
506            }
507        })
508        .sum();
509    let height = (height + space::S2 * 2.0).min(screen.height() - space::S3 * 2.0);
510    vec2(width, height)
511}
512
513/// Where a level goes: at the anchor, flipped rather than clipped.
514fn place(anchor: Rect, size: Vec2, screen: Rect, root: bool) -> Pos2 {
515    // A root menu hangs from the pointer; a submenu from the right edge of its row, with
516    // a small overlap so the pointer does not cross a gap on the way in.
517    let (mut x, mut y) = if root {
518        (anchor.left(), anchor.top())
519    } else {
520        (anchor.right() - space::S1, anchor.top() - space::S2)
521    };
522
523    if x + size.x > screen.right() {
524        // Flip to the other side of the anchor rather than merely sliding left, which is
525        // what would put a submenu on top of its own parent.
526        x = if root {
527            anchor.left() - size.x
528        } else {
529            anchor.left() - size.x + space::S1
530        };
531    }
532    if y + size.y > screen.bottom() {
533        y = if root {
534            anchor.top() - size.y
535        } else {
536            screen.bottom() - size.y
537        };
538    }
539    pos2(
540        x.clamp(screen.left(), (screen.right() - size.x).max(screen.left())),
541        y.clamp(screen.top(), (screen.bottom() - size.y).max(screen.top())),
542    )
543}
544
545/// Upload the shell's item bitmaps, once each.
546fn upload_icons(ctx: &egui::Context, menu: &mut Open, path: &mut Vec<usize>) {
547    // Walked by index rather than by reference so the tree can be read while the texture
548    // map is written.
549    let count = menu.level(path).map(Vec::len).unwrap_or(0);
550    for index in 0..count {
551        path.push(index);
552        let image = menu
553            .entry(path)
554            .and_then(|entry| entry.icon.clone())
555            .filter(|_| !menu.textures.contains_key(path));
556        if let Some(image) = image {
557            let handle = ctx.load_texture(
558                format!("menu-icon-{path:?}"),
559                image,
560                egui::TextureOptions::LINEAR,
561            );
562            menu.textures.insert(path.clone(), handle);
563        }
564        if matches!(menu.entry(path).map(|e| &e.kind), Some(Kind::Submenu { .. })) {
565            upload_icons(ctx, menu, path);
566        }
567        path.pop();
568    }
569}
570
571/// Arrow keys, Enter and Escape.
572fn keyboard(ctx: &egui::Context, menu: &mut Open) -> Option<Outcome> {
573    use egui::Key;
574
575    let (up, down, left, right, enter, escape) = ctx.input(|i| {
576        (
577            i.key_pressed(Key::ArrowUp),
578            i.key_pressed(Key::ArrowDown),
579            i.key_pressed(Key::ArrowLeft),
580            i.key_pressed(Key::ArrowRight),
581            i.key_pressed(Key::Enter),
582            i.key_pressed(Key::Escape),
583        )
584    });
585    if escape {
586        // Escape closes one level at a time, and the whole menu from the root.
587        return if menu.open.is_empty() {
588            Some(Outcome::Closed)
589        } else {
590            menu.open.pop();
591            menu.cursor = None;
592            Some(Outcome::Open)
593        };
594    }
595
596    if up || down {
597        let level_path: Vec<usize> = menu.open.clone();
598        let count = menu.level(&level_path).map(Vec::len).unwrap_or(0);
599        if count == 0 {
600            return None;
601        }
602        let current = menu
603            .cursor
604            .as_ref()
605            .filter(|c| c.len() == level_path.len() + 1 && c.starts_with(&level_path))
606            .and_then(|c| c.last().copied());
607        let step: isize = if down { 1 } else { -1 };
608        let mut next = match current {
609            Some(index) => index as isize + step,
610            None if down => 0,
611            None => count as isize - 1,
612        };
613        // Skip separators and anything disabled, and wrap.
614        for _ in 0..count * 2 {
615            let wrapped = next.rem_euclid(count as isize) as usize;
616            let mut candidate = level_path.clone();
617            candidate.push(wrapped);
618            let usable = menu
619                .entry(&candidate)
620                .is_some_and(|e| e.enabled && !matches!(e.kind, Kind::Separator));
621            if usable {
622                menu.cursor = Some(candidate);
623                return Some(Outcome::Open);
624            }
625            next += step;
626        }
627        return Some(Outcome::Open);
628    }
629
630    if right {
631        if let Some(cursor) = menu.cursor.clone() {
632            if matches!(menu.entry(&cursor).map(|e| &e.kind), Some(Kind::Submenu { .. })) {
633                menu.open = cursor;
634                menu.cursor = None;
635                return Some(Outcome::Open);
636            }
637        }
638    }
639    if left && !menu.open.is_empty() {
640        menu.cursor = Some(menu.open.clone());
641        menu.open.pop();
642        return Some(Outcome::Open);
643    }
644    if enter {
645        if let Some(cursor) = menu.cursor.clone() {
646            match menu.entry(&cursor).map(|e| e.kind.clone()) {
647                Some(Kind::Command(command)) => return Some(Outcome::Chose(command)),
648                Some(Kind::Submenu { .. }) => {
649                    menu.open = cursor;
650                    menu.cursor = None;
651                    return Some(Outcome::Open);
652                }
653                _ => {}
654            }
655        }
656    }
657    None
658}
659
660#[cfg(test)]
661mod tests {
662    use super::*;
663    use crate::shell::menu::Own;
664
665    fn entry(label: &str) -> Entry {
666        Entry {
667            label: label.to_owned(),
668            shortcut: String::new(),
669            kind: Kind::Command(Command::Own(Own::CopyHere)),
670            enabled: true,
671            checked: false,
672            default: false,
673            icon: None,
674        }
675    }
676
677    fn submenu(label: &str, children: Vec<Entry>) -> Entry {
678        Entry {
679            kind: Kind::complete(children),
680            ..entry(label)
681        }
682    }
683
684    /// A submenu row as the shell hands it over: known to be one, not yet asked about.
685    fn unfilled(label: &str, source: u32) -> Entry {
686        Entry {
687            kind: Kind::unfilled(source),
688            ..entry(label)
689        }
690    }
691
692    fn menu(entries: Vec<Entry>) -> Open {
693        Open::new(
694            1,
695            pos2(100.0, 100.0),
696            Vec::new(),
697            std::path::PathBuf::from(r"C:\x"),
698            entries,
699            1,
700        )
701    }
702
703    /// A pass over a menu, for the tests that need one drawn.
704    fn pass(open: &mut Open, screen: Rect, times: usize) -> egui::Context {
705        let ctx = egui::Context::default();
706        let theme = Theme::dark();
707        let mut input = egui::RawInput {
708            screen_rect: Some(screen),
709            ..Default::default()
710        };
711        input.viewports.entry(egui::ViewportId::ROOT).or_default().inner_rect = Some(screen);
712        for _ in 0..times {
713            let _ = ctx.run_ui(input.clone(), |ctx| {
714                egui::CentralPanel::default().show(ctx, |ui| {
715                    let _ = show(ui, &theme, open);
716                });
717            });
718        }
719        ctx
720    }
721
722    /// Opening an unfilled submenu asks for it once, by the id the shell handed over, and
723    /// the level appears when the answer does -- not before, and not by asking again every
724    /// frame.
725    ///
726    /// The id is the part worth holding down. It was a path of entry indices, and that broke
727    /// the moment this program's own entries went above the shell's: the menu asked about
728    /// index 8 for a submenu the shell had filed under index 1, every lookup missed, and
729    /// every submenu in the program came back empty. Hence `unfilled("Send to", 41)` --
730    /// deliberately not 1, so a version that went back to computing the index from the tree
731    /// cannot pass.
732    #[test]
733    fn an_unfilled_submenu_is_asked_for_once_by_id_and_drawn_when_it_arrives() {
734        let screen = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
735        let mut open = menu(vec![entry("Open"), unfilled("Send to", 41)]);
736
737        // Nothing is asked for until it is opened.
738        pass(&mut open, screen, 2);
739        assert!(open.fills.is_empty(), "an unopened submenu was asked about");
740
741        open.open = vec![1];
742        pass(&mut open, screen, 3);
743        assert_eq!(
744            open.fills,
745            vec![41],
746            "opening it should have asked once, by the shell's id, across three frames"
747        );
748        // The caller sends it on; nothing more should accumulate.
749        open.fills.clear();
750        pass(&mut open, screen, 3);
751        assert!(open.fills.is_empty(), "it was asked for twice");
752
753        // The answer arrives and the level becomes drawable.
754        assert!(open.level(&[1]).is_some_and(Vec::is_empty));
755        open.filled(41, vec![entry("Desktop"), entry("Mail recipient")]);
756        assert_eq!(open.level(&[1]).map(Vec::len), Some(2));
757        assert!(open.entries[1].enabled);
758    }
759
760    /// An answer for a submenu nested inside another one finds its way in, and an id nobody
761    /// is holding changes nothing.
762    #[test]
763    fn a_fill_finds_its_submenu_at_any_depth() {
764        let mut open = menu(vec![
765            entry("Open"),
766            submenu("More", vec![entry("Here"), unfilled("Deeper", 9)]),
767        ]);
768        open.filled(9, vec![entry("Bottom")]);
769        assert_eq!(open.level(&[1, 1]).map(Vec::len), Some(1));
770
771        // An id from a menu that has already gone is not going to match anything, and must
772        // not overwrite whatever is holding a different one.
773        open.filled(1234, vec![entry("Wrong")]);
774        assert_eq!(open.level(&[1, 1]).map(Vec::len), Some(1));
775        assert_eq!(open.entries.len(), 2);
776    }
777
778    /// An extension that really has nothing leaves the row there and inert, rather than
779    /// deleting it from under the pointer that is on it.
780    #[test]
781    fn a_submenu_that_fills_to_nothing_stops_being_usable() {
782        let mut open = menu(vec![entry("Open"), unfilled("Nothing here", 3)]);
783        open.filled(3, Vec::new());
784        assert_eq!(open.entries.len(), 2, "the row stayed");
785        assert!(!open.entries[1].enabled, "and stopped being usable");
786        // Asked and answered: it must not be asked again.
787        open.open = vec![1];
788        pass(&mut open, Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), 2);
789        assert!(open.fills.is_empty());
790    }
791
792    #[test]
793    fn the_measured_row_heights_are_the_ones_the_components_allocate() {
794        // `measure` adds these up to place the menu before a single row is drawn, so a
795        // number that drifts from what the design system allocates puts the menu in the
796        // wrong place — and the taller the menu, the further out. It drifted once already,
797        // when the design system dropped its menu density from 36 points to 28, so this
798        // asks the components rather than trusting either arithmetic.
799        let ctx = egui::Context::default();
800        let mut taken = 0.0;
801        let _ = ctx.run_ui(Default::default(), |ctx| {
802            egui::Area::new(Id::new("probe")).show(ctx, |ui| {
803                ui.set_width(240.0);
804                ui.spacing_mut().item_spacing.y = 0.0;
805                let top = ui.cursor().top();
806                ui.add(MenuItem::new("one"));
807                ui.add(MenuItem::new("two"));
808                menu_divider(ui);
809                taken = ui.cursor().top() - top;
810            });
811        });
812        assert_eq!(taken, row_height() * 2.0 + separator_height());
813    }
814
815    #[test]
816    fn a_menu_longer_than_the_screen_is_capped_and_drawn_the_size_it_measured() {
817        // What this holds down: a menu with more entries than the window has room for is
818        // capped to the screen — without which the entries past the edge are simply
819        // unreachable — and what gets drawn is the height that was measured, since the
820        // position was computed from it.
821        //
822        // It is not proof against the whole class of failure. The real one — an `Area`
823        // whose `Ui` reports an available height derived from the area's own size last
824        // frame, so that a scroll area sizing itself from it settled at 400 points against
825        // a measured 584 — reproduces in a real window and not in a pass driven from here;
826        // it was found by capturing the window and comparing. The fix is at the call site,
827        // where the rows are given an explicit rect.
828        let ctx = egui::Context::default();
829        let theme = Theme::dark();
830        let mut open = menu((0..12).map(|i| entry(&format!("entry {i}"))).collect());
831
832        let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0));
833        let mut input = egui::RawInput {
834            screen_rect: Some(screen),
835            ..Default::default()
836        };
837        input.viewports.entry(egui::ViewportId::ROOT).or_default().inner_rect = Some(screen);
838
839        for _ in 0..4 {
840            let _ = ctx.run_ui(input.clone(), |ctx| {
841                egui::CentralPanel::default().show(ctx, |ui| {
842                    let _ = show(ui, &theme, &mut open);
843                });
844            });
845        }
846
847        let expected = measure(&ctx, &theme, &open.entries, screen);
848        assert!(
849            expected.y < row_height() * 12.0,
850            "twelve rows should not fit in a 200-point window, or this proves nothing"
851        );
852        // Within the frame's own stroke.
853        assert!(
854            (open.drawn.y - expected.y).abs() <= 2.0,
855            "measured {} and drew {}",
856            expected.y,
857            open.drawn.y
858        );
859        assert!(open.drawn.y <= screen.height(), "and it stays on screen");
860    }
861
862    #[test]
863    fn levels_and_entries_resolve_by_path() {
864        let m = menu(vec![
865            entry("one"),
866            submenu("more", vec![entry("deep"), submenu("deeper", vec![entry("bottom")])]),
867        ]);
868        assert_eq!(m.entry(&[0]).unwrap().label, "one");
869        assert_eq!(m.entry(&[1]).unwrap().label, "more");
870        assert_eq!(m.entry(&[1, 0]).unwrap().label, "deep");
871        assert_eq!(m.entry(&[1, 1, 0]).unwrap().label, "bottom");
872        assert!(m.entry(&[9]).is_none());
873        // A path through a leaf is not a path.
874        assert!(m.entry(&[0, 0]).is_none());
875
876        assert_eq!(m.level(&[]).unwrap().len(), 2);
877        assert_eq!(m.level(&[1]).unwrap().len(), 2);
878        assert!(m.level(&[0]).is_none());
879    }
880
881    #[test]
882    fn a_menu_that_would_run_off_the_right_flips() {
883        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
884        let size = vec2(200.0, 300.0);
885
886        // Room to the right: it hangs from the pointer.
887        let anchor = Rect::from_min_size(pos2(100.0, 100.0), Vec2::ZERO);
888        assert_eq!(place(anchor, size, screen, true), pos2(100.0, 100.0));
889
890        // No room: it flips to the other side rather than sliding, so the pointer is not
891        // left inside the menu it just opened.
892        let anchor = Rect::from_min_size(pos2(950.0, 100.0), Vec2::ZERO);
893        assert_eq!(place(anchor, size, screen, true), pos2(750.0, 100.0));
894    }
895
896    #[test]
897    fn a_menu_that_would_run_off_the_bottom_flips_up() {
898        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
899        let size = vec2(200.0, 300.0);
900        let anchor = Rect::from_min_size(pos2(100.0, 700.0), Vec2::ZERO);
901        assert_eq!(place(anchor, size, screen, true), pos2(100.0, 400.0));
902    }
903
904    #[test]
905    fn a_menu_taller_than_the_screen_still_starts_on_it() {
906        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 400.0));
907        let size = vec2(200.0, 900.0);
908        let anchor = Rect::from_min_size(pos2(100.0, 300.0), Vec2::ZERO);
909        let at = place(anchor, size, screen, true);
910        assert_eq!(at, pos2(100.0, 0.0), "clamped to the top rather than off it");
911    }
912
913    #[test]
914    fn a_submenu_hangs_off_the_right_of_its_row() {
915        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
916        let row = Rect::from_min_size(pos2(100.0, 200.0), vec2(180.0, 36.0));
917        let at = place(row, vec2(200.0, 100.0), screen, false);
918        assert!(at.x > row.left(), "to the right of the row it came from");
919        assert!(at.x <= row.right(), "with a small overlap so the pointer can cross");
920    }
921}
