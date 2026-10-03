1//! The path bar: history buttons, a segmented breadcrumb, refresh and the filter.
2//!
3//! Modelled on Explorer's, which is still the best version of this control:
4//!
5//! - Every **segment** is a button that goes there.
6//! - Every **chevron** between segments opens that folder's subfolders, so you can
7//!   step sideways into a sibling without going up first.
8//! - The **leading chevron** lists the drives.
9//! - When the path is too long to fit, the leading segments collapse into a `…`
10//!   that opens them as a menu — the current folder is never the part that
11//!   disappears.
12//!
13//! With one difference from Explorer, and it is the reason the bar is drawn from
14//! [`crate::pane::Tab::trail`] rather than from the current path: **going up does not trim
15//! the bar.** Walk out of `src\ui` and the bar still reads `… › src › ui`, with the folder
16//! you are now in in bold and the two you came out of still there to be clicked. Explorer
17//! cuts them off, and then the only way back is to open a chevron and read a menu to find a
18//! name that was on screen a moment ago.
19//! - **Clicking the empty space** past the last segment turns the whole thing into
20//!   an editable path field, as does `Ctrl+L`. `Enter` navigates, `Esc` puts it
21//!   back.
22
23use azur_egui_theme::components::{MenuItem, Size};
24use azur_egui_theme::icons as azur_icons;
25use azur_egui_theme::tokens::{radius, space};
26use egui::{pos2, vec2, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};
27use std::path::{Path, PathBuf};
28
29use crate::app::Action;
30use crate::fs;
31use crate::icons;
32use crate::pane::{PaneId, Tab};
33use crate::theme::Theme;
34use crate::ui::{text_left, tool_button, truncated, TOOL_SIZE};
35
36/// The bar's height: `tokens::control::MEDIUM`, so the buttons in it are Azur's
37/// small size with room to breathe.
38pub const HEIGHT: f32 = 32.0;
39
40/// A point off the top of every segment label.
41///
42/// A line of text centred in a box sits centred on its *line box*, which includes the
43/// descender space under the baseline — so beside a chevron, which is centred on its own
44/// ink, the text reads a point low. Nudging the text rather than the chevrons because the
45/// chevrons are what the eye tracks along the trail.
46const TEXT_LIFT: f32 = 1.0;
47
48/// And a point onto the chevrons between segments, for the same reason from the other
49/// side. Together the two put a `›` on the middle of the names it separates.
50const ARROW_DROP: f32 = 1.0;
51
52/// The pen at the right-hand end, and the room reserved for it.
53///
54/// Larger than [`crate::ui::TOOL_ICON`], which every other glyph on this bar is drawn at, and
55/// deliberately: the others are centred in a 24-point button whose fill and hover give them
56/// their presence, and this one has no button around it at all. At 14 it read as a speck. 18 is
57/// still under the bar's 24 points of room, so nothing about the bar's height changes.
58///
59/// Its space comes out of the trail's, so a path deep enough to fill the bar stops short of the
60/// pen rather than running underneath it.
61pub(crate) const PEN: f32 = 18.0;
62
63/// Subfolders for whichever chevron dropdown is open.
64///
65/// One at a time, so one cache is enough. Read on the frame the menu opens rather
66/// than on every frame it is showing — a popup body runs continuously, and reading
67/// `C:\Windows\System32` sixty times a second to draw the same list would be a
68/// self-inflicted stall.
69#[derive(Default)]
70pub struct CrumbMenu {
71    path: Option<PathBuf>,
72    items: Vec<(String, PathBuf)>,
73    truncated: bool,
74    /// Which of the bar's dropdowns is showing: the pane whose bar it is on, and the segment
75    /// the chevron sits in front of — or [`OVERFLOW_MENU`] for the `…`.
76    ///
77    /// **This one field is also the bar's tracking mode.** While it is set, the whole bar
78    /// behaves as one control: moving the pointer onto another segment or chevron brings the
79    /// dropdown with it, without a second click. That is what Explorer's address bar does, and
80    /// it is why the state cannot live where egui keeps a menu's open state by default — per
81    /// trigger, toggled by that trigger's own click, which is right for a button and cannot
82    /// say "the same menu, somewhere else along the bar". See
83    /// [`azur_egui_theme::components::Menu::open`].
84    ///
85    /// It leaves on its own: clicking an entry, clicking away and `Escape` all close the popup,
86    /// and the frame that notices puts this back to `None`.
87    open: Option<(PaneId, usize)>,
88}
89
90/// The `…` button's place in [`CrumbMenu::open`].
91///
92/// It is one of the bar's dropdowns and takes part in the same tracking as the chevrons — it
93/// simply has no segment of its own to be numbered after, being what stands in for the ones
94/// that did not fit.
95const OVERFLOW_MENU: usize = usize::MAX;
96
97impl CrumbMenu {
98    /// Fill the cache for `path`, unless it already holds it.
99    fn ensure(&mut self, path: &Path) {
100        if self.path.as_deref() == Some(path) {
101            return;
102        }
103        // A directory of subdirectories is the only thing this menu shows, so the
104        // listing is filtered as it is read rather than after.
105        let dir = fs::scan::scan(path);
106        let mut items: Vec<(String, PathBuf)> = Vec::new();
107        for i in 0..dir.len() {
108            let entry = &dir.entries[i];
109            if !entry.is_dir() || entry.is_hidden() {
110                continue;
111            }
112            items.push((dir.name(i).to_owned(), dir.target(i)));
113        }
114        items.sort_by(|a, b| fs::sort::natural_cmp(&a.0, &b.0));
115        // A menu is for picking one of a few; past this it is a listing, and the
116        // listing is what the pane behind it is for.
117        self.truncated = items.len() > MENU_LIMIT;
118        items.truncate(MENU_LIMIT);
119
120        self.path = Some(path.to_path_buf());
121        self.items = items;
122    }
123
124    /// What the open dropdown is listing: the folder it read, and how many subfolders it
125    /// found. For the tests, which is how "the chevron opens and has content in it" is
126    /// checked without hunting for a menu row's rect.
127    #[cfg(test)]
128    pub fn listing(&self) -> Option<(&Path, usize)> {
129        self.path.as_deref().map(|path| (path, self.items.len()))
130    }
131
132    /// Which dropdown is showing, for the tests: the pane, and the segment its chevron is in
133    /// front of. Also whether the bar is tracking, which is the same fact.
134    #[cfg(test)]
135    pub fn showing(&self) -> Option<(PaneId, usize)> {
136        self.open
137    }
138
139    /// Put the dropdown away, and with it the tracking mode it turns on.
140    ///
141    /// The popup itself closes because `open` is what the `Menu` is told to read — see
142    /// [`azur_egui_theme::components::Menu::open`] — so there is nothing else to dismiss.
143    pub fn close(&mut self) {
144        self.open = None;
145    }
146}
147
148const MENU_LIMIT: usize = 200;
149
150/// A menu entry with a tick in its icon slot when it is on.
151///
152/// The icon slot rather than a checkbox, which is what every desktop menu does with a toggle —
153/// and `MenuItem` reserves the slot for every entry, so the labels line up whether or not
154/// anything is ticked.
155fn ticked<'a>(item: MenuItem<'a>, on: bool) -> MenuItem<'a> {
156    if on {
157        item.icon(&azur_egui_theme::icons::check)
158    } else {
159        item
160    }
161}
162
163/// The preview button's own menu: whether the panel is showing, and where it goes.
164///
165/// **Sticky**, which is `azur::components::ContextMenu`'s word for "these are settings, not
166/// commands": ticking one of three positions and having the menu vanish means reopening it to see
167/// what you did. A menu of commands should close — dismissal is how a reader knows the command was
168/// taken — and this one is not.
169///
170/// Where the panel goes is the *window's* preference and not this folder's, which is why `layout`
171/// comes in from `App` while `open` comes off the tab. A radio group in a menu reads as a setting,
172/// and a setting that only applied to the folder you happened to be in when you chose it would be
173/// a setting nobody could rely on.
174fn position_menu(
175    ui: &Ui,
176    trigger: &egui::Response,
177    pane: PaneId,
178    open: bool,
179    layout: &mut crate::ui::preview::Layout,
180    out: &mut Vec<Action>,
181) {
182    use azur_egui_theme::components::{collection_label, menu_divider, ContextMenu};
183
184    ContextMenu::new(trigger)
185        .sticky(true)
186        .show(ui.ctx(), |ui| {
187            if ui
188                .add(ticked(
189                    MenuItem::new("Show preview").shortcut("Ctrl+P"),
190                    open,
191                ))
192                .clicked()
193            {
194                out.push(Action::TogglePreview(pane));
195            }
196            menu_divider(ui);
197            collection_label(ui, "Position");
198            for at in crate::ui::preview::Where::ALL {
199                if ui
200                    .add(ticked(MenuItem::new(at.label()), layout.at == at))
201                    .clicked()
202                {
203                    layout.at = at;
204                    out.push(Action::RememberLayout);
205                }
206            }
207        });
208}
209
210/// Draw the bar inside `rect`.
211#[allow(clippy::too_many_arguments)]
212pub fn show(
213    ui: &mut Ui,
214    t: &Theme,
215    rect: Rect,
216    pane: PaneId,
217    tab: &mut Tab,
218    menu: &mut CrumbMenu,
219    icons_cache: &mut crate::shell::icons::Icons,
220    layout: &mut crate::ui::preview::Layout,
221    out: &mut Vec<Action>,
222) {
223    // **The filter, applied once the typing stops.** Before anything is drawn, so the listing
224    // below is laid out from the order this settles on rather than a frame behind it.
225    //
226    // The repaint is not optional: this program is idle between events, so a keystroke's frame is
227    // the last one there will be until something else happens. Without asking for the frame that
228    // notices the deadline, a filter typed and left alone would apply whenever the pointer next
229    // moved. Asked for again on every frame that is early, because a frame that arrives for some
230    // other reason at 40 ms does not stop the clock — and egui only promises *no later than*.
231    if let Some(left) = tab.settle_filter(ui.input(|i| i.time)) {
232        ui.ctx()
233            .request_repaint_after(std::time::Duration::from_secs_f64(left));
234    }
235
236    // The bar's own surface, and the colour of a selected tab: the tab in the strip above is
237    // welded to the bar directly under it, so the two are one surface with the pane's listing
238    // hanging off it. `crate::ui::seam` is where that colour is decided, once.
239    let surface = crate::ui::seam(t);
240    ui.painter().rect_filled(rect, CornerRadius::ZERO, surface);
241
242    let center = rect.center().y;
243    let button = |x: f32| {
244        Rect::from_min_size(
245            pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
246            vec2(TOOL_SIZE, TOOL_SIZE),
247        )
248    };
249
250    // ---- Getting about ---------------------------------------------------
251    //
252    // Back, Forward, Up, Refresh. Refresh is here rather than out at the right-hand end beside
253    // the filter because it is the same kind of thing as the other three — it acts on the folder
254    // you are looking at — and because that end is the end that gets given up on a narrow pane.
255    // It is part of the group that is never dropped now, which is the point of moving it.
256    let mut x = rect.left() + space::S2;
257    for (glyph, tip, enabled, action) in [
258        (
259            &icons::arrow_left as azur_icons::Icon<'_>,
260            "Back (Alt+Left)",
261            tab.can_go_back(),
262            Action::Back(pane),
263        ),
264        (
265            &icons::arrow_right,
266            "Forward (Alt+Right)",
267            tab.can_go_forward(),
268            Action::Forward(pane),
269        ),
270        (
271            &icons::arrow_up,
272            "Up (Alt+Up)",
273            fs::parent_of(&tab.path).is_some(),
274            Action::Up(pane),
275        ),
276        (
277            &icons::refresh,
278            "Refresh (F5)",
279            true,
280            Action::Refresh(pane),
281        ),
282    ] {
283        if tool_button(
284            ui,
285            t,
286            button(x),
287            Id::new(("nav", pane, tip)),
288            glyph,
289            tip,
290            enabled,
291            false,
292            surface,
293        )
294        .clicked()
295        {
296            out.push(action);
297        }
298        x += TOOL_SIZE;
299    }
300
301    x += space::S2;
302    ui.painter().rect_filled(
303        Rect::from_min_size(pos2(x.round(), center - 8.0), vec2(1.0, 16.0)),
304        CornerRadius::ZERO,
305        t.stroke.subtle,
306    );
307    x += 1.0 + space::S2;
308
309    // ---- The right-hand end -----------------------------------------------
310    //
311    // The filter, and nothing else now. Measured before the path and given up when the pane is
312    // narrow: a filter box on top of a truncated path is worse than a path you can read. The
313    // buttons at the left are never dropped — Back with nowhere to click is the one thing a path
314    // bar cannot do without.
315    let mut right = rect.right() - space::S2;
316    /// The path needs at least this much to be worth showing at all.
317    const MIN_PATH: f32 = 96.0;
318    let room_for = |right: f32, want: f32| right - want - x >= MIN_PATH;
319
320    let filter_width = if !tab.filter.is_empty() || rect.width() > 460.0 {
321        160.0_f32.min((rect.width() - 260.0).max(0.0))
322    } else {
323        0.0
324    };
325    if filter_width >= 90.0 && room_for(right, filter_width) {
326        let field = Rect::from_min_size(
327            pos2(
328                (right - filter_width).round(),
329                (center - TOOL_SIZE * 0.5).round(),
330            ),
331            vec2(filter_width, TOOL_SIZE),
332        );
333        // Square, like the bar it sits in. See [`crate::ui::squared`].
334        let response = crate::ui::squared(ui, |ui| {
335            ui.put(
336                field,
337                azur_egui_theme::components::TextField::new(&mut tab.filter)
338                    .placeholder("Filter")
339                    .prefix(&icons::filter)
340                    .clearable(true)
341                    .size(Size::Small)
342                    .width(filter_width),
343            )
344        });
345        if response.changed() {
346            // Noted, not applied — see `Tab::settle_filter` at the top of this function, and
347            // `pane::FILTER_DELAY` for the 240 ms one pass can cost.
348            tab.filter_changed(ui.input(|i| i.time));
349        }
350        // `Ctrl+F` puts the caret here without the user having to find it.
351        if ui.input_mut(|i| {
352            i.consume_shortcut(&egui::KeyboardShortcut::new(
353                egui::Modifiers::COMMAND,
354                egui::Key::F,
355            ))
356        }) {
357            response.request_focus();
358        }
359        right = field.left() - space::S2;
360    }
361
362    // Flatten, immediately before the filter — the two are the same kind of thing, a question
363    // asked of the folder you are looking at rather than somewhere to go, and both are given up
364    // together when the pane is too narrow for the path. Latched while it is on, which is what
365    // says the listing on show is not this folder's own children.
366    //
367    // It is *not* dropped with the filter when the box itself is hidden but the room is there:
368    // a 24-point button is affordable long after a 160-point field is not, and a flatten you can
369    // turn on and not off would be a trap. The order of the two `room_for` tests below is what
370    // that comes down to.
371    if room_for(right, TOOL_SIZE) {
372        let rect = button(right - TOOL_SIZE);
373        if tool_button(
374            ui,
375            t,
376            rect,
377            Id::new(("flatten", pane)),
378            &icons::flatten,
379            "Flatten this folder's whole tree (Ctrl+E)",
380            // Nothing to flatten on This PC, whose rows are drives — see `Tab::toggle_flat`.
381            !tab.path.as_os_str().is_empty(),
382            tab.flat,
383            surface,
384        )
385        .clicked()
386        {
387            out.push(Action::ToggleFlat(pane));
388        }
389        right = rect.left() - space::S2;
390    }
391
392    // The preview toggle, before the flatten one. Both are questions asked about the folder
393    // rather than places to go, so they belong at this end — and this one is furthest from the
394    // filter because it is the least to do with it.
395    //
396    // **It carries a context menu**, which is where the panel's position lives: show or hide,
397    // and then Right, Bottom or Auto. A right click on the control that opens a thing is where
398    // people look for the settings of that thing, and it keeps three radio buttons off a path
399    // bar that has no room for them.
400    if room_for(right, TOOL_SIZE) {
401        let rect = button(right - TOOL_SIZE);
402        let response = tool_button(
403            ui,
404            t,
405            rect,
406            Id::new(("preview", pane)),
407            &icons::eye,
408            "Preview the selected file (Ctrl+P)",
409            true,
410            tab.preview.open,
411            surface,
412        );
413        if response.clicked() {
414            out.push(Action::TogglePreview(pane));
415        }
416        position_menu(ui, &response, pane, tab.preview.open, layout, out);
417        right = rect.left() - space::S2;
418    }
419
420    // Refresh used to be here, and is now in the group at the left. Nothing else is: the star
421    // that pinned the folder to Bookmarks is gone. `Ctrl+D` still does it, and so does the
422    // folder's own context menu, which is where the rest of what you can do to a folder lives —
423    // a toggle button whose two states are a hollow star and a filled one was a permanent
424    // fixture spending most of its life saying nothing.
425
426    // ---- The path itself -------------------------------------------------
427    let path_rect = Rect::from_min_max(pos2(x, rect.top()), pos2(right.max(x), rect.bottom()));
428    if tab.editing_path {
429        edit_field(ui, path_rect, pane, tab, out);
430    } else {
431        segments(ui, t, path_rect, pane, tab, menu, icons_cache, out);
432    }
433}
434
435/// The editable path field.
436fn edit_field(ui: &mut Ui, rect: Rect, pane: PaneId, tab: &mut Tab, out: &mut Vec<Action>) {
437    let field = Rect::from_min_size(
438        pos2(rect.left(), (rect.center().y - TOOL_SIZE * 0.5).round()),
439        vec2(rect.width(), TOOL_SIZE),
440    );
441    // Square, like the breadcrumb it replaces. See [`crate::ui::squared`].
442    let response = crate::ui::squared(ui, |ui| {
443        ui.put(
444            field,
445            azur_egui_theme::components::TextField::new(&mut tab.edit_text)
446                .size(Size::Small)
447                .width(rect.width()),
448        )
449    });
450    // The field is created and focused in the same frame it is opened.
451    if !response.has_focus() && !response.lost_focus() {
452        response.request_focus();
453    }
454
455    let (enter, escape) = ui.input(|i| {
456        (
457            i.key_pressed(egui::Key::Enter),
458            i.key_pressed(egui::Key::Escape),
459        )
460    });
461    if escape || response.lost_focus() && !enter {
462        tab.editing_path = false;
463    }
464    if enter {
465        tab.editing_path = false;
466        match fs::resolve_input(&tab.edit_text) {
467            Some(path) if path.is_file() => out.push(Action::Open(path)),
468            Some(path) => out.push(Action::Navigate { pane, path }),
469            // Nothing there. Leave the text as typed so it can be corrected.
470            None => tab.editing_path = true,
471        }
472    }
473}
474
475/// Which segment of the trail is the folder actually being shown.
476///
477/// Not the last one: [`crate::pane::Tab::trail`] can run deeper than the current folder,
478/// which is the whole point of it. The fallback is the end of the trail, for the case that
479/// should not arise — a path that is not on its own trail — because a bar with nothing bold
480/// on it is a worse answer than a bar with the wrong thing bold.
481pub(crate) fn active_index(crumbs: &[(String, PathBuf)], path: &Path) -> usize {
482    crumbs
483        .iter()
484        .position(|(_, crumb)| crumb == path)
485        .unwrap_or(crumbs.len() - 1)
486}
487
488/// The segmented breadcrumb.
489#[allow(clippy::too_many_arguments)]
490fn segments(
491    ui: &mut Ui,
492    t: &Theme,
493    rect: Rect,
494    pane: PaneId,
495    tab: &mut Tab,
496    menu: &mut CrumbMenu,
497    icons_cache: &mut crate::shell::icons::Icons,
498    out: &mut Vec<Action>,
499) {
500    // A segment is something you hover in order to go somewhere, like a row in the listing or in
501    // the sidebar, so it takes the same hover they do rather than a toolbar button's.
502    //
503    // The pair an open dropdown makes — a name welded to the chevron after it — wears that same
504    // grey, and so do the entries inside the dropdown. The three are one gesture: the pointer
505    // moves along the bar, into the list that opened under it, and down the folders in it, and a
506    // darker "pressed" grey under an open menu read as the bar having been dented rather than as
507    // the trail carrying on into the list.
508    let hover_fill = crate::ui::hover_fill(t);
509    // The *pressed* fill still comes from the ladder, and from `control_fills` rather than
510    // straight off the theme: the bar these sit on is `crate::ui::seam`, not `background-layer`,
511    // and in the light theme where the seam and `control-active` are the same grey a pressed
512    // segment was the same colour as the bar, so the press showed as the fill going away.
513    let (_, pressed_fill) = crate::ui::control_fills(t, crate::ui::seam(t));
514
515    const CHEVRON: f32 = 16.0;
516    const OVERFLOW: f32 = 22.0;
517
518    // The two things that are painted *behind* the segments, reserved here and filled in once
519    // the loop has worked out where they go.
520    //
521    // Both are backgrounds for something drawn later — the wash that says a click will open the
522    // path field, and the fill that welds an open chevron to the segment it belongs to — and
523    // neither can be positioned until the segments have been laid out, which is the same pass
524    // that draws their text. A reserved slot is how a painter draws in one order and composites
525    // in another; the alternative is measuring the whole bar twice.
526    let field_hint = ui.painter().add(egui::Shape::Noop);
527    let pair_hint = ui.painter().add(egui::Shape::Noop);
528    let corner = CornerRadius::same(radius::SMALL);
529
530    // The whole bar, and then the part of it the trail is laid out in: the pen at the
531    // right-hand end is reserved out of the segments' room, so a deep path runs up to it and
532    // never over it. See where the pen is drawn.
533    let bar = rect;
534    let rect = Rect::from_min_max(
535        rect.min,
536        pos2((rect.right() - PEN - space::S2).max(rect.left()), rect.bottom()),
537    );
538
539    // The trail rather than the current folder: walking up leaves the deeper part of it on
540    // the bar so it can be clicked again. See [`crate::pane::Tab::trail`].
541    let crumbs = fs::breadcrumb_segments(&tab.trail);
542    let center = rect.center().y;
543    let deepest = crumbs.len() - 1;
544    let active = active_index(&crumbs, &tab.path);
545
546    // Whether this bar had a dropdown open when the frame began, which is the same question as
547    // whether the pointer moving along it should carry that dropdown with it. Read once, before
548    // anything can change it: a click *during* this frame must not also count as a hover.
549    let tracking = menu.open.is_some_and(|(owner, _)| owner == pane);
550    // Which dropdown the pointer is asking for, whether one was clicked, and whether the one
551    // that is open was drawn — all three settled during the loop and acted on after it.
552    let mut wanted: Option<usize> = None;
553    let mut clicked = false;
554    let mut shown = false;
555
556    // Natural width of every segment, so the overflow can be decided before
557    // anything is drawn.
558    //
559    // The current folder is measured in `body-strong`, which is what it is *drawn* in —
560    // measuring it in the regular face would leave its name a few points short and
561    // truncate it with room to spare.
562    let widths: Vec<f32> = crumbs
563        .iter()
564        .enumerate()
565        .map(|(index, (label, _))| {
566            let font = if index == active {
567                t.fonts.body_strong.clone()
568            } else {
569                t.fonts.body.clone()
570            };
571            let galley =
572                ui.painter()
573                    .layout_no_wrap(label.clone(), font, egui::Color32::PLACEHOLDER);
574            // A rounded-up point of slack, so a hinted glyph advance never spills
575            // past the width it was measured at.
576            (galley.size().x + space::S3 * 2.0).ceil() + 1.0
577        })
578        .collect();
579
580    // Every segment is followed by a chevron, and the trail starts with one for the
581    // drives.
582    //
583    // Measured up to the current folder and no further: the part of the trail *past* it is
584    // a convenience and takes whatever room is left over, so a long tail can never push the
585    // folder you are actually in off the front of the bar. Whatever does not fit at the
586    // right-hand end is simply not drawn — the loop below stops when it runs out of room.
587    let cost: f32 = widths[..=active].iter().sum::<f32>() + (active + 1) as f32 * CHEVRON;
588    let mut first = 0;
589    if cost > rect.width() {
590        // Drop from the front until it fits, leaving room for the `…`.
591        let mut used = cost + OVERFLOW;
592        while first < active && used > rect.width() {
593            used -= widths[first] + CHEVRON;
594            first += 1;
595        }
596    }
597
598    let mut x = rect.left();
599
600    if rect.width() < CHEVRON + 24.0 {
601        // Not even one segment fits. Nothing is better than a chevron on its own.
602        return;
603    }
604
605    if first > 0 {
606        let overflow = Rect::from_min_size(
607            pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
608            vec2(OVERFLOW, TOOL_SIZE),
609        );
610        let response = tool_button(
611            ui,
612            t,
613            overflow,
614            Id::new(("crumb-overflow", pane)),
615            &azur_icons::ellipsis,
616            "",
617            true,
618            false,
619            crate::ui::seam(t),
620        );
621        if response.clicked() {
622            clicked = true;
623            menu.open = (menu.open != Some((pane, OVERFLOW_MENU)))
624                .then_some((pane, OVERFLOW_MENU));
625        } else if tracking && response.hovered() {
626            wanted = Some(OVERFLOW_MENU);
627        }
628        let mut open = menu.open == Some((pane, OVERFLOW_MENU));
629        if open {
630            shown = true;
631            // The same fill an open chevron wears, from behind, because `tool_button` has
632            // already painted its glyph by now. Not its `active` state, which is Azur's accent
633            // and means *latched* everywhere else in this window.
634            ui.painter().set(
635                pair_hint,
636                egui::epaint::RectShape::filled(overflow, corner, hover_fill),
637            );
638        }
639        azur_egui_theme::components::Menu::new(&response)
640            .open(&mut open)
641            .show(ui.ctx(), |ui| {
642                for (label, path) in crumbs.iter().take(first) {
643                    if ui.add(MenuItem::new(label.clone())).clicked() {
644                        out.push(Action::Navigate {
645                            pane,
646                            path: path.clone(),
647                        });
648                    }
649                }
650            });
651        if !open && menu.open == Some((pane, OVERFLOW_MENU)) {
652            menu.open = None;
653        }
654        x += OVERFLOW;
655    }
656
657    // A leading chevron for the very first visible segment, which for a full path
658    // is This PC and so lists the drives.
659    let mut pending_chevron = Some(if first == 0 {
660        PathBuf::new()
661    } else {
662        crumbs[first - 1].1.clone()
663    });
664
665    // Where the segment before the chevron about to be drawn was, so the two can be filled as
666    // one shape. `None` for the leading chevron, whose segment is in the overflow or is This PC
667    // itself — it is highlighted alone, having nothing to be welded to.
668    let mut previous: Option<Rect> = None;
669
670    for (index, (label, path)) in crumbs.iter().enumerate().skip(first) {
671        if let Some(parent) = pending_chevron.take() {
672            if x + CHEVRON > rect.right() {
673                break;
674            }
675            let chevron = Rect::from_min_size(
676                pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
677                vec2(CHEVRON, TOOL_SIZE),
678            );
679            let response = ui.interact(
680                chevron,
681                Id::new(("crumb-chevron", pane, index)),
682                Sense::click(),
683            );
684            if response.clicked() {
685                clicked = true;
686                // A click on the chevron whose dropdown is already up closes it, which is what
687                // clicking an open menu's button does everywhere.
688                menu.open = (menu.open != Some((pane, index))).then_some((pane, index));
689                // Re-read on each open: the folder may have gained a subfolder since the
690                // last time this chevron was used.
691                menu.path = None;
692            } else if tracking && response.hovered() {
693                wanted = Some(index);
694            }
695
696            let mut open = menu.open == Some((pane, index));
697            if open {
698                shown = true;
699                // **The chevron and the text before it, as one shape.** They are one control:
700                // the chevron lists that folder's subfolders, so the pair is "this folder, and
701                // what is inside it". Two fills would put a seam down the middle of it — and
702                // this has to go *behind* the segment, whose label was painted a step ago.
703                ui.painter().set(
704                    pair_hint,
705                    egui::epaint::RectShape::filled(
706                        match previous {
707                            Some(text) => text.union(chevron),
708                            None => chevron,
709                        },
710                        corner,
711                        hover_fill,
712                    ),
713                );
714            } else if response.hovered() {
715                ui.painter().rect_filled(chevron, corner, hover_fill);
716            }
717            // Down when the menu is showing, right when it is not — the same
718            // rotation Explorer uses to say "this opens".
719            //
720            // A point lower than centre, which is where it sits beside a line of text:
721            // the label's ink is above the middle of its line box, and matching the
722            // chevron to the *text* beats matching it to the box.
723            let glyph_rect = Rect::from_center_size(
724                chevron.center() + vec2(0.0, ARROW_DROP),
725                vec2(12.0, 12.0),
726            );
727            if open {
728                azur_icons::chevron_down(ui.painter(), glyph_rect, t.text.primary);
729            } else {
730                azur_icons::chevron_right(ui.painter(), glyph_rect, t.text.tertiary);
731            }
732
733            // Built every frame, open or not.
734            //
735            // `Menu` is what puts the popup up — it hangs an `egui::Popup` off the trigger
736            // response — so a menu that is only constructed once it is *already* open is a
737            // menu nothing can ever open. That is exactly what this was, and why the
738            // chevrons did nothing.
739            //
740            // Driven from [`CrumbMenu::open`] rather than from egui's own per-trigger memory,
741            // because the bar is one control while a dropdown is up. `open` comes back `false`
742            // when the popup has closed itself — an entry clicked, a click outside, `Escape` —
743            // which is how the bar learns to stop tracking.
744            //
745            // The folder is read inside the closure, which egui runs only while the popup
746            // is showing, and [`CrumbMenu`] holds the result — so the scan happens on the
747            // frame it opens rather than sixty times a second while it is up.
748            azur_egui_theme::components::Menu::new(&response)
749            .open(&mut open)
750            .show(ui.ctx(), |ui| {
751                menu.ensure(&parent);
752                if menu.items.is_empty() {
753                    ui.add(
754                        azur_egui_theme::components::Text::new("No subfolders")
755                            .color(azur_egui_theme::components::TextColor::Tertiary),
756                    );
757                }
758                for (name, target) in &menu.items {
759                    let here = *target == *path;
760                    // Windows' icon, which for the leading chevron — the one that lists
761                    // the volumes — is the difference between a row of drives and a row of
762                    // identical folders.
763                    //
764                    // A *drive* is asked about by path, because that is the only way to get
765                    // its own icon, and there are at most twenty-six of them. A folder is
766                    // asked about by kind, which is one lookup shared by every folder for
767                    // the rest of the session: this menu can hold two hundred rows, and a
768                    // per-path lookup each would be two hundred questions for the
769                    // generic folder icon two hundred times over.
770                    let icon = if target.parent().is_none() {
771                        icons_cache.place(target)
772                    } else {
773                        icons_cache.kind("", true)
774                    };
775                    let texture = icon
776                        .and_then(|icon| icons_cache.uv(ui.ctx(), icon));
777                    let paint = |painter: &egui::Painter, rect: Rect, color: egui::Color32| {
778                        match texture {
779                            Some((texture, uv)) => {
780                                painter.image(texture, rect, uv, egui::Color32::WHITE);
781                            }
782                            None => icons::folder(painter, rect, color),
783                        }
784                    };
785                    if ui
786                        .add(MenuItem::new(name.clone()).icon(&paint).selected(here))
787                        .clicked()
788                    {
789                        out.push(Action::Navigate {
790                            pane,
791                            path: target.clone(),
792                        });
793                    }
794                }
795                if menu.truncated {
796                    azur_egui_theme::components::menu_divider(ui);
797                    ui.add(
798                        azur_egui_theme::components::Text::new(format!("First {MENU_LIMIT} shown"))
799                            .color(azur_egui_theme::components::TextColor::Tertiary),
800                    );
801                }
802            });
803            // The popup has closed itself: an entry was clicked, or something outside it was, or
804            // `Escape`. That is the end of the bar's tracking mode, and this is the only place
805            // it is reported.
806            if !open && menu.open == Some((pane, index)) {
807                menu.open = None;
808            }
809            x += CHEVRON;
810        }
811
812        let width = widths[index].min((rect.right() - x).max(0.0));
813        if width <= 0.0 {
814            break;
815        }
816        let segment = Rect::from_min_size(
817            pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
818            vec2(width, TOOL_SIZE),
819        );
820        previous = Some(segment);
821        let current = index == active;
822        let response = ui.interact(
823            segment,
824            Id::new(("crumb", pane, index)),
825            Sense::click(),
826        );
827        // A click on a segment ends the tracking rather than moving it: the popup closes itself,
828        // and the hover below — the pointer is on this segment, since it was just clicked —
829        // would otherwise put the dropdown straight back up on the way out.
830        if response.clicked() || response.middle_clicked() {
831            clicked = true;
832        }
833        // Hovering a segment while the bar is tracking opens *its* chevron — the one after it,
834        // which is the one that lists this folder's subfolders. The deepest segment on the
835        // trail has no chevron after it, so there is nothing for it to open and whatever is
836        // showing stays where it is.
837        if tracking && response.hovered() && index != deepest {
838            wanted = Some(index + 1);
839        }
840        // Part of the open pair: filled as one shape with the chevron after it, from behind, so
841        // it wears nothing of its own here. See where `pair_hint` is set.
842        let paired = menu.open == Some((pane, index + 1));
843        if !paired && (response.hovered() || response.is_pointer_button_down_on()) {
844            ui.painter().rect_filled(
845                segment,
846                corner,
847                if response.is_pointer_button_down_on() {
848                    pressed_fill
849                } else {
850                    hover_fill
851                },
852            );
853        }
854        // Bold and full-contrast marks the folder being shown, which is the only cue that it
855        // is not simply the end of the trail. Everything else on the bar is a link, on both
856        // sides of it.
857        let (font, color) = if current {
858            (t.fonts.body_strong.clone(), t.text.primary)
859        } else if response.hovered() || paired {
860            (t.fonts.body.clone(), t.text.primary)
861        } else {
862            (t.fonts.body.clone(), t.text.secondary)
863        };
864        let galley = truncated(
865            ui.painter(),
866            label,
867            font,
868            color,
869            width - space::S3 * 2.0,
870        );
871        text_left(
872            ui.painter(),
873            Rect::from_min_max(
874                pos2(segment.left() + space::S3, segment.top() - TEXT_LIFT),
875                pos2(segment.right() - space::S3, segment.bottom() - TEXT_LIFT),
876            ),
877            galley,
878        );
879        if response.clicked() && !current {
880            out.push(Action::Navigate {
881                pane,
882                path: path.clone(),
883            });
884        }
885        // Middle click opens an ancestor in its own tab, as it does everywhere.
886        if response.middle_clicked() {
887            out.push(Action::NavigateNewTab {
888                pane,
889                path: path.clone(),
890            });
891        }
892
893        x += width;
894        // A chevron between every pair of segments, listing the left one's subfolders. The
895        // one after the current folder is the useful case and the reason this is keyed on
896        // the end of the trail rather than on the current folder.
897        if index != deepest {
898            pending_chevron = Some(path.clone());
899        }
900    }
901
902    // ---- Where the open dropdown goes next -------------------------------
903    //
904    // Applied here rather than where the hover was noticed, so that the frame which notices is
905    // drawn whole: switching mid-loop would leave the dropdown that is going away already
906    // painted for this frame with the one arriving painted over it. One frame of delay, and the
907    // repaint is asked for because the pointer has very likely stopped moving by now — it
908    // arrived somewhere and stayed.
909    match wanted.filter(|_| !clicked) {
910        Some(index) => {
911            if menu.open != Some((pane, index)) {
912                menu.open = Some((pane, index));
913                ui.ctx().request_repaint();
914            }
915        }
916        // A dropdown recorded as open whose chevron was not drawn at all — the pane has since
917        // been narrowed past it, or the pointer asked for one that turned out not to fit. It is
918        // showing nothing, and leaving it recorded would leave the bar tracking a menu that
919        // cannot be seen or closed.
920        None if tracking && !shown => menu.open = None,
921        None => {}
922    }
923
924    // ---- The rest of the bar: click it to type a path --------------------
925    //
926    // Including the pen's own strip, which is part of the same target: it is a drawing rather
927    // than a button, and a hint you cannot click would be a strange thing to draw.
928    let field = Rect::from_min_max(
929        pos2(bar.left(), (bar.center().y - TOOL_SIZE * 0.5).round()),
930        pos2(bar.right(), (bar.center().y + TOOL_SIZE * 0.5).round()),
931    );
932    let empty = Rect::from_min_max(pos2(x, bar.top()), bar.max);
933    let over_field = if empty.width() > 4.0 {
934        let response = ui.interact(empty, Id::new(("crumb-empty", pane)), Sense::click());
935        if response.clicked() {
936            start_editing(tab);
937        }
938        response.hovered()
939    } else {
940        false
941    };
942    if over_field {
943        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
944        // **The border and nothing else.** The one place on the bar where a click does
945        // something other than navigate says so before the click — and says it the way a field
946        // says it, with the outline a field wears under the pointer, over the whole shape the
947        // field is about to take. A fill as well made the bar change colour to announce
948        // something that is only an announcement.
949        //
950        // From behind, and this is the reason `field_hint` was reserved: the outline spans the
951        // whole bar, and by the time the empty space at the end of it is known to be hovered,
952        // every segment's label has been painted.
953        ui.painter().set(
954            field_hint,
955            egui::epaint::RectShape::stroke(field, corner, Stroke::new(1.0, pen_ink(t, true)), StrokeKind::Inside),
956        );
957    }
958
959    // ---- The pen ---------------------------------------------------------
960    //
961    // A hint, not a control: it says the bar can be typed into, which is the one thing about
962    // this bar that nothing else on it advertises. Always drawn, because a hint that only
963    // appears once the pointer is already there is not a hint — and in the same ink as the
964    // outline, brighter when the pointer is over the bar, so the two read as one cue.
965    //
966    // Its space is taken out of the trail's before the segments are laid out, so a path deep
967    // enough to fill the bar stops short of it rather than running underneath it.
968    icons::pencil(
969        ui.painter(),
970        Rect::from_center_size(
971            pos2((bar.right() - PEN * 0.5).round(), field.center().y.round()),
972            egui::vec2(PEN, PEN),
973        ),
974        pen_ink(t, over_field),
975    );
976}
977
978/// The ink the pen and the field's outline share.
979///
980/// One colour for both, because they are one hint: the outline says "this is a field" and the pen
981/// says "this is where you write". `stroke-strong` is what Azur gives a *field* under the pointer
982/// — the same border the path field itself will wear a moment later — and `stroke-control` is that
983/// border at rest, which is what the pen sits at until the pointer arrives.
984///
985/// Not `stroke-subtle`, which is the obvious name for something subtle and is invisible here:
986/// [`crate::ui::seam`] — the bar's own fill — *is* `stroke-subtle`, so a hairline in it is a
987/// hairline drawn in the colour behind it. It was, for as long as this hint was a hairline.
988pub(crate) fn pen_ink(t: &Theme, lit: bool) -> egui::Color32 {
989    if lit {
990        t.stroke.strong
991    } else {
992        t.stroke.control
993    }
994}
995
996/// Switch the bar into its editable form, prefilled with the current path.
997///
998/// The current path, not the trail: the field is for going somewhere, and what it should
999/// open showing is where you are.
1000pub fn start_editing(tab: &mut Tab) {
1001    tab.edit_text = if tab.path.as_os_str().is_empty() {
1002        "This PC".to_owned()
1003    } else {
1004        tab.path.to_string_lossy().into_owned()
1005    };
1006    tab.editing_path = true;
1007}
1008
1009#[cfg(test)]
1010mod tests {
1011    use super::*;
1012
1013    #[test]
1014    fn the_bold_segment_is_the_folder_being_shown() {
1015        let trail = fs::breadcrumb_segments(Path::new(r"C:\a\b\c"));
1016        // This PC, C:\, a, b, c.
1017        assert_eq!(trail.len(), 5, "{trail:?}");
1018
1019        // At the end of the trail it is the last segment, as it always used to be.
1020        assert_eq!(active_index(&trail, Path::new(r"C:\a\b\c")), 4);
1021        // Walked up two: the trail still shows five, and `a` is the one in bold.
1022        assert_eq!(active_index(&trail, Path::new(r"C:\a")), 2);
1023        // All the way up. This PC is a segment like any other.
1024        assert_eq!(active_index(&trail, Path::new("")), 0);
1025    }
1026
1027    #[test]
1028    fn a_path_that_is_not_on_the_trail_falls_back_to_its_end() {
1029        // Should not happen -- `Tab::go_to` keeps the two in step -- but a bar with nothing
1030        // bold on it would be worse than this.
1031        let trail = fs::breadcrumb_segments(Path::new(r"C:\a\b"));
1032        assert_eq!(active_index(&trail, Path::new(r"D:\somewhere")), trail.len() - 1);
1033    }
1034}
