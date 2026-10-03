1//! The left panel: drives, bookmarks, places.
2//!
3//! Three collapsible groups of rows, in the order they are worth scanning: the
4//! volumes on the machine, the folders you chose, and the ones the shell provides.
5//!
6//! Every row navigates the focused pane on a left click, opens in a new tab on a
7//! middle click, and offers "open in a new pane" from its context menu — so the
8//! split layout is reachable from here as well as by dragging a tab.
9//!
10//! The icons are the shell's, per *place* rather than per type — see
11//! [`crate::shell::icons::Icons::place`], which is what makes Downloads look like Downloads
12//! rather than like a folder.
13//!
14//! Bookmarks are a list the user arranges: dragging one reorders it, and dragging a folder
15//! in from a listing pins it. The second is an OLE drop rather than a gesture of ours — the
16//! group publishes itself as a drop zone, and [`crate::app::App`] turns a drop there into
17//! a bookmark instead of a file operation.
18
19use azur_egui_theme::components::{ContextMenu, MenuItem};
20use azur_egui_theme::icons as azur_icons;
21use azur_egui_theme::tokens::{radius, space, typography};
22use egui::{pos2, vec2, CornerRadius, Id, Rect, Sense, Ui};
23use std::path::{Path, PathBuf};
24
25use crate::app::Action;
26use crate::fs::drives::Drive;
27use crate::fs::places::Place;
28use crate::fs::{display_name, fmt};
29use crate::icons;
30use crate::pane::{PaneId, Side};
31use crate::theme::Theme;
32use crate::ui::{icon_rect, row_fill, section_label, text_left, truncated};
33
34/// A row: one body line and a point of air either side.
35///
36/// Dense on purpose. The panel is a list of names to scan rather than a set of controls
37/// to aim at, and a 14-point line does not need 26 points of row to be legible — 22 puts
38/// every drive, bookmark and place on screen at once in a 600-point window, which is the
39/// default this opens at.
40const PAD_Y: f32 = 1.0;
41const ROW: f32 = typography::LINE_BODY + PAD_Y * 2.0;
42const GAUGE_HEIGHT: f32 = 3.0;
43/// The gap between the text and the capacity bar under it.
44const GAUGE_GAP: f32 = 1.0;
45/// A drive row: the name, and the capacity bar under it. The numbers are a tooltip.
46const DRIVE_ROW: f32 = PAD_Y + typography::LINE_BODY + GAUGE_GAP + GAUGE_HEIGHT + PAD_Y;
47const HEADER: f32 = 20.0;
48const GLYPH: f32 = 14.0;
49/// Left inset for a row's glyph. The 2px selection bar lands inside it.
50const INDENT: f32 = space::S3;
51/// Left edge to where a row's label starts: the indent, the space a group's chevron
52/// occupies, and the glyph. Known without a rect, which is what lets a drive row decide
53/// how tall it is before it is allocated.
54const TEXT_INSET: f32 = INDENT + 12.0 + space::S2 + GLYPH + space::S3;
55
56/// Which groups are open. Persisted, because collapsing one is a decision about
57/// how you work rather than about this session.
58#[derive(Clone, Copy, Debug)]
59pub struct Sections {
60    pub drives: bool,
61    pub bookmarks: bool,
62    pub places: bool,
63}
64
65impl Default for Sections {
66    fn default() -> Self {
67        Self {
68            drives: true,
69            bookmarks: true,
70            places: true,
71        }
72    }
73}
74
75/// Shell icons waiting to be drawn in one run.
76///
77/// egui starts a new draw call whenever the texture changes, and a shell icon is the only
78/// thing in a sidebar row that does not come out of the font atlas — so drawing one inside
79/// the row loop splits the frame's primitive stream at every row. Under `glow` that is not
80/// merely slow, it leaks: see the note in [`crate::ui::filelist`] and `examples/spin.rs`.
81type IconQueue = Vec<(egui::TextureId, Rect, Rect)>;
82
83/// Draw the sidebar's contents into the current `Ui`, which the panel has already
84/// sized.
85#[allow(clippy::too_many_arguments)]
86pub fn show(
87    ui: &mut Ui,
88    t: &Theme,
89    drives: &[Drive],
90    bookmarks: &[PathBuf],
91    places: &[Place],
92    current: &Path,
93    focused: PaneId,
94    sections: &mut Sections,
95    icons_cache: &mut crate::shell::icons::Icons,
96    reorder: &mut Option<usize>,
97    scratch: &mut String,
98    out: &mut Vec<Action>,
99) -> Option<Rect> {
100    let mut bookmarks_rect = None;
101    let mut queue: IconQueue = Vec::new();
102    egui::ScrollArea::vertical()
103        .id_salt("sidebar")
104        .auto_shrink([false, false])
105        .show(ui, |ui| {
106            let width = ui.available_width();
107            // Rows are contiguous. The design system's 8-point item spacing is right for
108            // controls and wrong for a list of names: the hover fill *is* the row, and a
109            // gap between rows turns a list into a column of buttons and costs a third of
110            // the panel. The groups get their air explicitly instead.
111            ui.spacing_mut().item_spacing.y = 0.0;
112            ui.add_space(space::S1);
113
114            // ---- Drives ---------------------------------------------------
115            if group_header(ui, t, width, "Drives", &mut sections.drives) {
116                for drive in drives {
117                    let shell = shell_icon(ui, icons_cache, &drive.path);
118                    drive_row(
119                        ui, t, width, drive, shell, &mut queue, current, focused, scratch,
120                        out,
121                    );
122                }
123            }
124
125            // ---- Bookmarks ------------------------------------------------
126            ui.add_space(space::S2);
127            let header = ui.cursor().min;
128            if group_header(ui, t, width, "Bookmarks", &mut sections.bookmarks) {
129                if bookmarks.is_empty() {
130                    hint(
131                        ui,
132                        t,
133                        width,
134                        "Ctrl+D, or drag a folder here",
135                    );
136                }
137                // Row rects as they are drawn, so the caret between them can be worked out
138                // afterwards without laying anything out twice.
139                let mut rows: Vec<Rect> = Vec::with_capacity(bookmarks.len());
140                for (index, path) in bookmarks.iter().enumerate() {
141                    let shell = shell_icon(ui, icons_cache, path);
142                    let dragging = *reorder == Some(index);
143                    let response = row(
144                        ui,
145                        t,
146                        width,
147                        ROW,
148                        &icons::star_filled,
149                        t.status.warning,
150                        shell,
151                        &mut queue,
152                        &display_name(path),
153                        path == current,
154                        Id::new(("bookmark", path)),
155                        // Only bookmarks can be dragged, and only to reorder themselves.
156                        Sense::click_and_drag(),
157                        dragging,
158                    );
159                    rows.push(response.rect);
160                    if response.drag_started() {
161                        *reorder = Some(index);
162                    }
163                    navigate_on(&response, focused, path, out);
164                    ContextMenu::new(&response).show(ui.ctx(), |ui| {
165                        menu_targets(ui, focused, path, out);
166                        azur_egui_theme::components::menu_divider(ui);
167                        if ui
168                            .add(MenuItem::new("Remove bookmark").danger(true))
169                            .clicked()
170                        {
171                            out.push(Action::RemoveBookmark(path.clone()));
172                        }
173                    });
174                }
175                reordering(ui, t, reorder, &rows, out);
176                bookmarks_rect = Some(Rect::from_min_max(
177                    pos2(header.x, header.y),
178                    pos2(header.x + width, ui.cursor().min.y),
179                ));
180            }
181
182            // ---- Places ---------------------------------------------------
183            ui.add_space(space::S2);
184            if group_header(ui, t, width, "Places", &mut sections.places) {
185                for place in places {
186                    let glyph = icons::for_place(place.icon);
187                    let color = match crate::fs::places::kind_of(place.icon) {
188                        fmt::Kind::Folder => t.folder,
189                        _ => t.text.secondary,
190                    };
191                    let shell = shell_icon(ui, icons_cache, &place.path);
192                    let response = row(
193                        ui,
194                        t,
195                        width,
196                        ROW,
197                        glyph,
198                        color,
199                        shell,
200                        &mut queue,
201                        &place.label,
202                        !place.shell_only && place.path == current,
203                        Id::new(("place", &place.label)),
204                        Sense::click(),
205                        false,
206                    );
207                    if place.shell_only {
208                        // Not a directory this program can list — hand it over.
209                        if response.clicked() {
210                            out.push(Action::Reveal(place.path.clone()));
211                        }
212                    } else {
213                        navigate_on(&response, focused, &place.path, out);
214                        ContextMenu::new(&response).show(ui.ctx(), |ui| {
215                            menu_targets(ui, focused, &place.path, out);
216                            azur_egui_theme::components::menu_divider(ui);
217                            if ui
218                                .add(MenuItem::new("Add to bookmarks").icon(&icons::star))
219                                .clicked()
220                            {
221                                out.push(Action::AddBookmark(place.path.clone()));
222                            }
223                        });
224                    }
225                }
226            }
227
228            ui.add_space(space::S3);
229            // Every row's shell icon, in one run — see [`IconQueue`]. Inside the scroll area,
230            // so they are clipped with the rows they belong to.
231            flush_icons(ui, &queue);
232        });
233    bookmarks_rect
234}
235
236/// A bookmark being dragged to a new position: the caret while it is held, and the move
237/// when it is let go.
238///
239/// The list is short and always fully drawn, so the insertion point is simply the first row
240/// whose middle the pointer is above — no hit-testing and no scrolling to account for.
241fn reordering(
242    ui: &mut Ui,
243    t: &Theme,
244    reorder: &mut Option<usize>,
245    rows: &[Rect],
246    out: &mut Vec<Action>,
247) {
248    let Some(from) = *reorder else { return };
249    if from >= rows.len() {
250        *reorder = None;
251        return;
252    }
253
254    let Some(pointer) = ui.ctx().pointer_interact_pos() else {
255        *reorder = None;
256        return;
257    };
258    let mut to = rows.len();
259    for (index, rect) in rows.iter().enumerate() {
260        if pointer.y < rect.center().y {
261            to = index;
262            break;
263        }
264    }
265
266    if ui.input(|i| i.pointer.any_down()) {
267        // The caret goes between rows: at the top of the row it would push down, or under
268        // the last one when it is going to the end.
269        let y = rows
270            .get(to)
271            .map(|rect| rect.top())
272            .unwrap_or_else(|| rows[rows.len() - 1].bottom());
273        let band = rows[0];
274        ui.painter().rect_filled(
275            Rect::from_min_max(
276                pos2(band.left() + INDENT, y - 1.0),
277                pos2(band.right() - space::S3, y + 1.0),
278            ),
279            CornerRadius::same(radius::CIRCULAR),
280            t.accent.default,
281        );
282        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
283        return;
284    }
285
286    // Dropping a row on itself, or immediately below itself, means nothing.
287    if to != from && to != from + 1 {
288        out.push(Action::MoveBookmark { from, to });
289    }
290    *reorder = None;
291}
292
293/// A group heading that folds its rows away. Returns whether they should be drawn.
294fn group_header(ui: &mut Ui, t: &Theme, width: f32, label: &str, open: &mut bool) -> bool {
295    let (rect, _) = ui.allocate_exact_size(vec2(width, HEADER), Sense::hover());
296    let response = ui.interact(rect, Id::new(("sidebar-group", label)), Sense::click());
297    if response.clicked() {
298        *open = !*open;
299    }
300
301    let chevron = icon_rect(rect, rect.left() + INDENT, 12.0);
302    let color = if response.hovered() {
303        t.text.secondary
304    } else {
305        t.text.tertiary
306    };
307    if *open {
308        azur_icons::chevron_down(ui.painter(), chevron, color);
309    } else {
310        azur_icons::chevron_right(ui.painter(), chevron, color);
311    }
312    section_label(
313        ui.painter(),
314        Rect::from_min_max(
315            pos2(chevron.right() + space::S2, rect.top()),
316            pos2(rect.right() - space::S3, rect.bottom()),
317        ),
318        t,
319        label,
320    );
321    *open
322}
323
324/// One row. Returns its response so the caller can wire clicks and a menu.
325#[allow(clippy::too_many_arguments)]
326fn row(
327    ui: &mut Ui,
328    t: &Theme,
329    width: f32,
330    height: f32,
331    glyph: azur_icons::Icon<'_>,
332    glyph_color: egui::Color32,
333    shell: Option<(egui::TextureId, Rect)>,
334    queue: &mut IconQueue,
335    label: &str,
336    current: bool,
337    id: Id,
338    sense: Sense,
339    dragging: bool,
340) -> egui::Response {
341    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
342    let response = ui.interact(rect, id, sense);
343
344    // **The pointer's own highlight and nothing else: the left panel does not mark the folder
345    // that is open.** A band down a sidebar row was competing with the listing's selection for
346    // the same colour and the same meaning, and only one of the two is something the user
347    // selected. Where you are is still said — the label goes `text-primary`, and the breadcrumb
348    // above the listing is the bar whose entire job that is.
349    if let Some(fill) = row_fill(t, false, response.hovered()) {
350        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
351    }
352    // The row being dragged stays where it is and goes quiet, so the list does not reflow
353    // under the pointer mid-gesture — the same treatment a tab gets.
354    if dragging {
355        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg.control_active);
356    }
357
358    let glyph_rect = icon_rect(rect, rect.left() + INDENT + 12.0 + space::S2, GLYPH);
359    draw_icon(ui, glyph_rect, glyph, glyph_color, shell, queue);
360
361    let text_left_x = glyph_rect.right() + space::S3;
362    // Lifted two points, the same as every cell in the listing — see `filelist::CELL_LIFT`.
363    // A line of text centred in its row sits a shade low against the icon beside it, because the
364    // font's ascent and descent are not symmetrical about the middle.
365    let text_rect = Rect::from_min_max(
366        pos2(text_left_x, rect.top() - crate::ui::filelist::CELL_LIFT),
367        pos2(rect.right() - space::S3, rect.bottom() - crate::ui::filelist::CELL_LIFT),
368    );
369    let color = if current || response.hovered() {
370        t.text.primary
371    } else {
372        t.text.secondary
373    };
374
375    let galley = truncated(
376        ui.painter(),
377        label,
378        t.fonts.body.clone(),
379        color,
380        text_rect.width(),
381    );
382    text_left(ui.painter(), text_rect, galley);
383    response
384}
385
386/// Windows' own icon for a row, once the shell has answered.
387///
388/// The sidebar shows the same icons Explorer's navigation pane does — the Downloads arrow,
389/// the Pictures thumbnail, the Recycle Bin, a network volume's plug, a drive with a custom
390/// `autorun.inf` icon. Every one of those is the shell's answer about *that place*, which
391/// no amount of drawing here could reproduce.
392///
393/// `None` until it arrives, which is a frame or two after the first sighting and never
394/// again for the rest of the session; the painted glyph stands in until then. See
395/// [`crate::shell::icons::Icons::place`].
396fn shell_icon(
397    ui: &Ui,
398    icons_cache: &mut crate::shell::icons::Icons,
399    path: &Path,
400) -> Option<(egui::TextureId, Rect)> {
401    let icon = icons_cache.place(path)?;
402    icons_cache.uv(ui.ctx(), icon)
403}
404
405/// A row's icon: the shell's bitmap if there is one, and the painted glyph if not.
406fn draw_icon(
407    ui: &Ui,
408    rect: Rect,
409    glyph: azur_icons::Icon<'_>,
410    color: egui::Color32,
411    shell: Option<(egui::TextureId, Rect)>,
412    queue: &mut IconQueue,
413) {
414    match shell {
415        // Queued rather than drawn — see [`IconQueue`]. The painted fallback below is drawn
416        // where it stands, because it comes out of the same atlas as the text beside it.
417        Some((texture, uv)) => queue.push((texture, rect, uv)),
418        None => glyph(ui.painter(), rect, color),
419    }
420}
421
422/// Draw everything the rows queued, in one run.
423fn flush_icons(ui: &Ui, queue: &IconQueue) {
424    let painter = ui.painter();
425    for (texture, rect, uv) in queue {
426        painter.image(*texture, *rect, *uv, egui::Color32::WHITE);
427    }
428}
429
430/// The name a drive row shows: its label and its letter.
431fn drive_name(drive: &Drive) -> String {
432    format!("{} ({})", drive.label, drive.letter)
433}
434
435/// What a drive's tooltip says: its name, and the numbers behind the bar.
436///
437/// Both numbers, since nothing has to fit beside anything here — and the name as well,
438/// because in a narrow panel the row's own label is the part that got truncated.
439fn drive_tooltip(drive: &Drive, out: &mut String) {
440    out.clear();
441    out.push_str(&drive_name(drive));
442    out.push_str(" — ");
443    match drive.used_fraction() {
444        Some(_) => {
445            fmt::size(drive.free, out);
446            out.push_str(" free of ");
447            fmt::size(drive.total, out);
448        }
449        // Nothing in the slot, or a volume it would have meant waking hardware to
450        // measure. Saying so beats a tooltip that says nothing.
451        None => out.push_str("not measured"),
452    }
453}
454
455/// A drive: its name, how much room is left, and a bar showing it.
456///
457/// Painted here rather than through [`row`] because the elements have to clear each other
458/// exactly — a capacity bar drawn a few points too high strikes through the caption above
459/// it, which is the kind of thing a "roughly two lines" helper cannot promise.
460///
461/// The free-space **numbers are a tooltip**, not a caption. Inline they charged every drive
462/// row for a second piece of text that had to fit beside the name — which in a panel this
463/// narrow it often did not, so the caption came and went as the panel was dragged, and the
464/// name it belonged to lost width to it whenever it stayed. The bar under the name already
465/// carries the answer at a glance; the digits are what you go looking for, and going
466/// looking is what a hover is.
467#[allow(clippy::too_many_arguments)]
468fn drive_row(
469    ui: &mut Ui,
470    t: &Theme,
471    width: f32,
472    drive: &Drive,
473    shell: Option<(egui::TextureId, Rect)>,
474    queue: &mut IconQueue,
475    current: &Path,
476    focused: PaneId,
477    scratch: &mut String,
478    out: &mut Vec<Action>,
479) {
480    let name = drive_name(drive);
481    let text_width = (width - TEXT_INSET - space::S3).max(0.0);
482
483    let (rect, _) = ui.allocate_exact_size(vec2(width, DRIVE_ROW), Sense::hover());
484    let response = ui.interact(rect, Id::new(("drive", &drive.letter)), Sense::click());
485    let here = drive.path == *current;
486
487    // As on every other row here: the pointer's highlight only, never a mark for the folder that
488    // happens to be open. See `row`.
489    if let Some(fill) = row_fill(t, false, response.hovered()) {
490        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
491    }
492
493    let glyph = icon_rect(rect, rect.left() + INDENT + 12.0 + space::S2, GLYPH);
494    draw_icon(
495        ui,
496        glyph,
497        icons::for_drive(drive.kind),
498        t.text.secondary,
499        shell,
500        queue,
501    );
502
503    let left = glyph.right() + space::S3;
504    let right = rect.right() - space::S3;
505
506    // The name, then the bar, placed from the measured height of the name rather than at a
507    // guessed offset — a capacity bar a few points too high strikes through the text.
508    let name = truncated(
509        ui.painter(),
510        &name,
511        t.fonts.body.clone(),
512        if here || response.hovered() {
513            t.text.primary
514        } else {
515            t.text.secondary
516        },
517        text_width,
518    );
519    let mut y = rect.top() + PAD_Y;
520    let name_height = name.size().y;
521    ui.painter()
522        .galley(pos2(left, y.round()), name, egui::Color32::PLACEHOLDER);
523    y += name_height + GAUGE_GAP;
524
525    if let Some(used) = drive.used_fraction() {
526        let bar = Rect::from_min_size(pos2(left, y.round()), vec2((right - left).max(0.0), GAUGE_HEIGHT));
527        if bar.width() > 8.0 {
528            let corner = CornerRadius::same(radius::CIRCULAR);
529            ui.painter().rect_filled(bar, corner, t.gauge_track);
530            ui.painter().rect_filled(
531                Rect::from_min_size(bar.min, vec2((bar.width() * used).max(2.0), bar.height())),
532                corner,
533                t.gauge(used),
534            );
535        }
536    }
537
538    // The numbers, on hover. Guarded rather than left to the tooltip's own hover test,
539    // because the formatting is the cost and there is no sense paying it per drive per
540    // frame for text nobody is looking at.
541    if response.hovered() {
542        drive_tooltip(drive, scratch);
543        azur_egui_theme::components::tooltip(response.clone(), scratch);
544    }
545
546    navigate_on(&response, focused, &drive.path, out);
547    ContextMenu::new(&response).show(ui.ctx(), |ui| {
548        menu_targets(ui, focused, &drive.path, out);
549        azur_egui_theme::components::menu_divider(ui);
550        if ui.add(MenuItem::new("Open terminal here")).clicked() {
551            out.push(Action::OpenTerminal(drive.path.clone()));
552        }
553    });
554}
555
556/// Left click navigates, middle click opens a tab.
557fn navigate_on(response: &egui::Response, focused: PaneId, path: &Path, out: &mut Vec<Action>) {
558    if response.clicked() {
559        out.push(Action::Navigate {
560            pane: focused,
561            path: path.to_path_buf(),
562        });
563    }
564    if response.middle_clicked() {
565        out.push(Action::NavigateNewTab {
566            pane: focused,
567            path: path.to_path_buf(),
568        });
569    }
570}
571
572/// The three ways to open a place, shared by every context menu here.
573fn menu_targets(ui: &mut Ui, focused: PaneId, path: &Path, out: &mut Vec<Action>) {
574    if ui.add(MenuItem::new("Open").icon(&icons::folder_open)).clicked() {
575        out.push(Action::Navigate {
576            pane: focused,
577            path: path.to_path_buf(),
578        });
579    }
580    if ui.add(MenuItem::new("Open in new tab")).clicked() {
581        out.push(Action::NavigateNewTab {
582            pane: focused,
583            path: path.to_path_buf(),
584        });
585    }
586    if ui
587        .add(MenuItem::new("Open in a pane to the right").icon(&icons::split_side))
588        .clicked()
589    {
590        out.push(Action::OpenInSplit {
591            pane: focused,
592            path: path.to_path_buf(),
593            side: Side::Right,
594        });
595    }
596    if ui
597        .add(MenuItem::new("Open in a pane below").icon(&icons::split_down))
598        .clicked()
599    {
600        out.push(Action::OpenInSplit {
601            pane: focused,
602            path: path.to_path_buf(),
603            side: Side::Bottom,
604        });
605    }
606}
607
608/// A one-line note where a group has nothing in it.
609fn hint(ui: &mut Ui, t: &Theme, width: f32, text: &str) {
610    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
611    let inner = Rect::from_min_max(
612        pos2(rect.left() + INDENT + 12.0 + space::S2, rect.top()),
613        pos2(rect.right() - space::S3, rect.bottom()),
614    );
615    let galley = truncated(
616        ui.painter(),
617        text,
618        t.fonts.caption.clone(),
619        t.text.disabled,
620        inner.width(),
621    );
622    text_left(ui.painter(), inner, galley);
623}
624
625#[cfg(test)]
626mod tests {
627    use super::*;
628    use crate::fs::drives::DriveKind;
629
630    fn drive(total: u64, free: u64) -> Drive {
631        Drive {
632            path: PathBuf::from("D:\\"),
633            letter: "D:".to_owned(),
634            label: "Data".to_owned(),
635            kind: DriveKind::Fixed,
636            total,
637            free,
638            described: true,
639        }
640    }
641
642    #[test]
643    fn the_tooltip_names_the_drive_and_both_numbers() {
644        let mut out = String::new();
645        drive_tooltip(&drive(1_000_000_000_000, 713_000_000_000), &mut out);
646        // The name too: in a narrow panel it is the row's label that got truncated, and
647        // this is the only other place it is written.
648        assert!(out.starts_with("Data (D:) — "), "{out}");
649        assert!(out.contains(" free of "), "{out}");
650        // Both numbers, since nothing has to fit beside anything in a tooltip.
651        assert_eq!(
652            out.matches("GB").count() + out.matches("TB").count(),
653            2,
654            "{out}"
655        );
656    }
657
658    #[test]
659    fn an_unmeasured_drive_still_says_something() {
660        // An empty card reader, or a share that has not answered yet. A tooltip that came
661        // up blank would read as a bug in the tooltip.
662        let mut out = String::new();
663        drive_tooltip(&drive(0, 0), &mut out);
664        assert_eq!(out, "Data (D:) — not measured");
665    }
666}
