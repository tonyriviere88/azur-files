//! The crumbs themselves: one segment per folder, each with a chevron that opens what it
//! points at.
//!
//! The widest thing in the bar and the one that has to give way first — see [`segments`] for
//! what is dropped, and in which order, as the pane narrows.

use super::*;

/// The pen at the right-hand end, and the room reserved for it.
///
/// Larger than [`crate::ui::TOOL_ICON`], which every other glyph on this bar is drawn at, and
/// deliberately: the others are centred in a 24-point button whose fill and hover give them
/// their presence, and this one has no button around it at all. At 14 it read as a speck. 18 is
/// still under the bar's 24 points of room, so nothing about the bar's height changes.
///
/// Its space comes out of the trail's, so a path deep enough to fill the bar stops short of the
/// pen rather than running underneath it.
pub(crate) const PEN: f32 = 18.0;

/// Where a click on the segment at `index` goes: that folder if it is still there, and otherwise
/// nowhere — unless the folder on show has gone with it, when it is the deepest folder above both
/// that is still standing, so the pane is not left showing a folder that no longer exists.
///
/// **The second line of defence.** A folder deleted from the one on show leaves the bar on its own,
/// when the watch has the listing re-read — see [`Tab::trim_trail`]. What that cannot see is a
/// folder deeper down the trail, which nothing watches: walking up two levels out of `a\b\c` and
/// deleting `c` from inside `b` changes nothing in `a`. So the click is checked here, and the
/// segments that have gone **stay on the bar, struck through in red** — [`Tab::trail_gone`] — which
/// is the answer to the click: a segment that silently left the bar would leave you wondering
/// whether the click had missed. Only [`fs::gone`]'s *not found* counts: see there for what is let
/// through.
///
/// Asked again on every click, so a folder that has come back is no longer marked.
fn standing(tab: &mut Tab, crumbs: &[(String, PathBuf)], index: usize) -> Option<PathBuf> {
    let at = (0..=index)
        .rev()
        .find(|&at| !fs::gone(&crumbs[at].1))
        .unwrap_or(0);
    let found = &crumbs[at].1;
    if at == index {
        if tab.trail_gone.as_ref().is_some_and(|gone| found.starts_with(gone)) {
            tab.trail_gone = None;
        }
        return Some(found.clone());
    }
    // Everything under a folder that has gone has gone with it, so the mark is its shallowest
    // segment and covers the rest of the trail.
    tab.trail_gone = Some(crumbs[at + 1].1.clone());
    (*found != tab.path && tab.path.starts_with(found)).then(|| found.clone())
}

/// The segmented breadcrumb.
#[allow(clippy::too_many_arguments)]
pub(crate) fn segments(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &mut Tab,
    menu: &mut CrumbMenu,
    // What the chevrons' dropdowns read their folder from. Never [`crate::fs::scan::scan`], which
    // is a worker's call and can take 22 seconds on a bare `\\server` — see [`CrumbMenu`].
    loader: &mut crate::loader::Loader,
    icons_cache: &mut crate::shell::icons::Icons,
    slashes: bool,
    out: &mut Vec<Action>,
) {
    // A segment is something you hover in order to go somewhere, like a row in the listing or in
    // the sidebar, so it takes the same hover they do rather than a toolbar button's.
    //
    // The pair an open dropdown makes — a name welded to the chevron after it — wears that same
    // grey, and so do the entries inside the dropdown. The three are one gesture: the pointer
    // moves along the bar, into the list that opened under it, and down the folders in it, and a
    // darker "pressed" grey under an open menu read as the bar having been dented rather than as
    // the trail carrying on into the list.
    let hover_fill = crate::ui::hover_fill(t);
    // The *pressed* fill still comes from the ladder, and from `control_fills` rather than
    // straight off the theme: the bar these sit on is `crate::ui::seam`, not `background-layer`,
    // and in the light theme where the seam and `control-active` are the same grey a pressed
    // segment was the same colour as the bar, so the press showed as the fill going away.
    let (_, pressed_fill) = crate::ui::control_fills(t);

    const CHEVRON: f32 = 16.0;
    const OVERFLOW: f32 = 22.0;

    // The two things that are painted *behind* the segments, reserved here and filled in once
    // the loop has worked out where they go.
    //
    // Both are backgrounds for something drawn later — the wash that says a click will open the
    // path field, and the fill that welds an open chevron to the segment it belongs to — and
    // neither can be positioned until the segments have been laid out, which is the same pass
    // that draws their text. A reserved slot is how a painter draws in one order and composites
    // in another; the alternative is measuring the whole bar twice.
    let field_hint = ui.painter().add(egui::Shape::Noop);
    let pair_hint = ui.painter().add(egui::Shape::Noop);
    let corner = CornerRadius::same(radius::SMALL);

    // The whole bar, and then the part of it the trail is laid out in: the pen at the
    // right-hand end is reserved out of the segments' room, so a deep path runs up to it and
    // never over it. See where the pen is drawn.
    let bar = rect;
    let rect = Rect::from_min_max(
        rect.min,
        pos2((rect.right() - PEN - space::S2).max(rect.left()), rect.bottom()),
    );

    // The trail rather than the current folder: walking up leaves the deeper part of it on
    // the bar so it can be clicked again. See [`crate::pane::Tab::trail`].
    let crumbs = fs::breadcrumb_segments(&tab.trail);
    let center = rect.center().y;
    let deepest = crumbs.len() - 1;
    let active = active_index(&crumbs, &tab.path);
    // **Where the trail starts being drawn: past This PC, unless This PC is where we are.**
    //
    // Every path on this machine begins with it, so a segment saying so spends the front of the
    // bar on the one fact that is true of every folder — and the chevron in front of the first
    // segment already lists the volumes, which is the whole of what clicking This PC was for. A
    // drive letter says which machine it is on by being a drive letter.
    //
    // Kept when it *is* the folder on show, because the bar always names that one: This PC with
    // an empty trail would otherwise be a bar with nothing on it, and This PC reached by walking
    // up a deeper trail still has to have somewhere to put the bold.
    //
    // Only the drawing changes. [`fs::breadcrumb_segments`] still starts at This PC, because
    // [`crate::pane::Tab::go_to`] reads the same walk to decide which child to reveal on
    // arrival — and arriving at This PC has to reveal the drive you came out of.
    let start = usize::from(active > 0);

    // Whether this bar had a dropdown open when the frame began, which is the same question as
    // whether the pointer moving along it should carry that dropdown with it. Read once, before
    // anything can change it: a click *during* this frame must not also count as a hover.
    let tracking = menu.open.is_some_and(|(owner, _)| owner == pane);
    // Which dropdown the pointer is asking for, whether one was clicked, and whether the one
    // that is open was drawn — all three settled during the loop and acted on after it.
    let mut wanted: Option<usize> = None;
    let mut clicked = false;
    let mut shown = false;

    // Natural width of every segment, so the overflow can be decided before
    // anything is drawn.
    //
    // The current folder is measured in `body-strong`, which is what it is *drawn* in —
    // measuring it in the regular face would leave its name a few points short and
    // truncate it with room to spare.
    let widths: Vec<f32> = crumbs
        .iter()
        .enumerate()
        .map(|(index, (label, _))| {
            let font = if index == active {
                t.fonts.body_strong.clone()
            } else {
                t.fonts.body.clone()
            };
            let galley =
                ui.painter()
                    .layout_no_wrap(label.clone(), font, egui::Color32::PLACEHOLDER);
            // A rounded-up point of slack, so a hinted glyph advance never spills
            // past the width it was measured at.
            (galley.size().x + space::S3 * 2.0).ceil() + 1.0
        })
        .collect();

    // Every segment is followed by a chevron, and the trail starts with one for the
    // drives.
    //
    // Measured up to the current folder and no further: the part of the trail *past* it is
    // a convenience and takes whatever room is left over, so a long tail can never push the
    // folder you are actually in off the front of the bar. Whatever does not fit at the
    // right-hand end is simply not drawn — the loop below stops when it runs out of room.
    //
    // From `start`, so the segment that is not drawn is not paid for either.
    let cost: f32 =
        widths[start..=active].iter().sum::<f32>() + (active - start + 1) as f32 * CHEVRON;
    let mut first = start;
    if cost > rect.width() {
        // Drop from the front until it fits, leaving room for the `…`.
        let mut used = cost + OVERFLOW;
        while first < active && used > rect.width() {
            used -= widths[first] + CHEVRON;
            first += 1;
        }
    }

    let mut x = rect.left();

    if rect.width() < CHEVRON + 24.0 {
        // Not even one segment fits. Nothing is better than a chevron on its own.
        return;
    }

    if first > start {
        let overflow = Rect::from_min_size(
            pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
            vec2(OVERFLOW, TOOL_SIZE),
        );
        let response = tool_button(
            ui,
            t,
            overflow,
            Id::new(("crumb-overflow", pane)),
            &azur_icons::ellipsis,
            "",
            true,
            false,
        );
        if response.clicked() {
            clicked = true;
            menu.toggle((pane, OVERFLOW_MENU));
        } else if tracking && response.hovered() {
            wanted = Some(OVERFLOW_MENU);
        }
        let mut open = menu.open == Some((pane, OVERFLOW_MENU));
        if open {
            shown = true;
            // The same fill an open chevron wears, from behind, because `tool_button` has
            // already painted its glyph by now. Not its `active` state, which is Azur's accent
            // and means *latched* everywhere else in this window.
            ui.painter().set(
                pair_hint,
                egui::epaint::RectShape::filled(overflow, corner, hover_fill),
            );
        }
        azur_egui_theme::components::Menu::new(&response)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                // `start` rather than nothing: what the `…` holds is the part of the *trail*
                // that did not fit, and This PC is not on the bar to be collapsed into it.
                for (index, (label, path)) in crumbs.iter().enumerate().take(first).skip(start) {
                    // Red for one that has gone, as on the bar. A menu entry has no strikethrough.
                    let gone = tab.trail_gone.as_ref().is_some_and(|gone| path.starts_with(gone));
                    if ui.add(MenuItem::new(label.clone()).danger(gone)).clicked() {
                        if let Some(to) = standing(tab, &crumbs, index) {
                            out.push(Action::Navigate { pane, path: to });
                        }
                    }
                }
            });
        menu.closed_itself((pane, OVERFLOW_MENU), open);
        x += OVERFLOW;
    }

    // A leading chevron for the very first visible segment. For a full path that is the drive,
    // and the chevron in front of it belongs to This PC — so it lists the volumes, which is what
    // the front of the bar is for now that the segment naming them is gone.
    let mut pending_chevron = Some(if first == 0 {
        PathBuf::new()
    } else {
        crumbs[first - 1].1.clone()
    });

    // Where the segment before the chevron about to be drawn was, so the two can be filled as
    // one shape. `None` for the leading chevron, whose segment is in the overflow, is This PC
    // itself, or was never drawn — it is highlighted alone, having nothing to be welded to.
    let mut previous: Option<Rect> = None;

    for (index, (label, path)) in crumbs.iter().enumerate().skip(first) {
        if let Some(parent) = pending_chevron.take() {
            if x + CHEVRON > rect.right() {
                break;
            }
            let chevron = Rect::from_min_size(
                pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
                vec2(CHEVRON, TOOL_SIZE),
            );
            let response = ui.interact(
                chevron,
                Id::new(("crumb-chevron", pane, index)),
                Sense::click(),
            );
            if response.clicked() {
                clicked = true;
                // A click on the chevron whose dropdown is already up closes it, which is what
                // clicking an open menu's button does everywhere.
                menu.toggle((pane, index));
                // Ask again on each open: the folder may have gained a subfolder since the
                // last time this chevron was used. See [`CrumbMenu::reopen`] for what that
                // means now that the answer comes from the loader.
                menu.reopen();
            } else if tracking && response.hovered() {
                wanted = Some(index);
            }

            let mut open = menu.open == Some((pane, index));
            if open {
                shown = true;
                // **The chevron and the text before it, as one shape.** They are one control:
                // the chevron lists that folder's subfolders, so the pair is "this folder, and
                // what is inside it". Two fills would put a seam down the middle of it — and
                // this has to go *behind* the segment, whose label was painted a step ago.
                ui.painter().set(
                    pair_hint,
                    egui::epaint::RectShape::filled(
                        match previous {
                            Some(text) => text.union(chevron),
                            None => chevron,
                        },
                        corner,
                        hover_fill,
                    ),
                );
            } else if response.hovered() {
                ui.painter().rect_filled(chevron, corner, hover_fill);
            }
            // Down when the menu is showing, right when it is not — the same
            // rotation Explorer uses to say "this opens".
            //
            // A point lower than centre, which is where it sits beside a line of text:
            // the label's ink is above the middle of its line box, and matching the
            // chevron to the *text* beats matching it to the box.
            let glyph_rect = Rect::from_center_size(
                chevron.center() + vec2(0.0, ARROW_DROP),
                vec2(12.0, 12.0),
            );
            if open {
                azur_icons::chevron_down(ui.painter(), glyph_rect, t.text.primary);
            } else {
                azur_icons::chevron_right(ui.painter(), glyph_rect, t.text.tertiary);
            }

            // Built every frame, open or not.
            //
            // `Menu` is what puts the popup up — it hangs an `egui::Popup` off the trigger
            // response — so a menu that is only constructed once it is *already* open is a
            // menu nothing can ever open. That is exactly what this was, and why the
            // chevrons did nothing.
            //
            // Driven from [`CrumbMenu::open`] rather than from egui's own per-trigger memory,
            // because the bar is one control while a dropdown is up. `open` comes back `false`
            // when the popup has closed itself — an entry clicked, a click outside, `Escape` —
            // which is how the bar learns to stop tracking.
            //
            // The folder is asked for inside the closure, which egui runs only while the popup
            // is showing, and [`CrumbMenu`] holds the answer — so the loader is asked on the
            // frame it opens rather than sixty times a second while it is up.
            azur_egui_theme::components::Menu::new(&response)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let listed = menu.ensure(&parent, loader);
                // **`Reading…` while the answer is out, and it is not the same word as
                // `No subfolders`.** An empty menu is a claim — that the folder has nothing in
                // it — and a dropdown that made it before the folder had been read would be
                // wrong about every share and every sleeping drive, which are the two cases the
                // wait exists for.
                //
                // Said straight away rather than after [`crate::pane::SLOW_SCAN`], which is the
                // opposite of what the *listing* does, because what the delay buys there is not
                // available here. A pane already fills the room the rows will take, so its
                // alternative to silence is a word flashing in the middle of it on every
                // navigation; a popup is only as big as what is in it, so the alternative here is
                // a frame with nothing in it at all — which is `No subfolders` drawn in
                // whitespace. A folder in the loader's cache, which is most of them on the trail,
                // shows its rows on this same frame and never reaches this line.
                //
                // The word stays if the read *fails*: the reason is on the answer, and the answer
                // goes to whoever holds its token, which is a tab and never this menu. Clicking
                // the segment itself is what puts the folder — and its error — in the pane, in
                // the one place with room to word it.
                if !listed {
                    menu_note(ui, "Reading…");
                } else if menu.items.is_empty() {
                    menu_note(ui, "No subfolders");
                }
                for (name, target) in &menu.items {
                    let here = *target == *path;
                    // Windows' icon, which for the leading chevron — the one that lists
                    // the volumes — is the difference between a row of drives and a row of
                    // identical folders. See [`crumb_icon`] for why a drive is asked about
                    // by path and a folder by kind.
                    let paint = texture_or_folder(crumb_icon(icons_cache, target));
                    if ui
                        .add(MenuItem::new(name.clone()).icon(&paint).selected(here))
                        .clicked()
                    {
                        out.push(Action::Navigate {
                            pane,
                            path: target.clone(),
                        });
                    }
                }
                if menu.truncated {
                    truncated_note(ui);
                }
            });
            // The popup may have closed itself, which ends the bar's tracking mode. See
            // [`CrumbMenu::closed_itself`].
            menu.closed_itself((pane, index), open);
            x += CHEVRON;
        }

        let width = widths[index].min((rect.right() - x).max(0.0));
        if width <= 0.0 {
            break;
        }
        let segment = Rect::from_min_size(
            pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
            vec2(width, TOOL_SIZE),
        );
        previous = Some(segment);
        let current = index == active;
        let response = ui.interact(
            segment,
            Id::new(("crumb", pane, index)),
            Sense::click(),
        );
        // A click on a segment ends the tracking rather than moving it: the popup closes itself,
        // and the hover below — the pointer is on this segment, since it was just clicked —
        // would otherwise put the dropdown straight back up on the way out.
        if response.clicked() || response.middle_clicked() {
            clicked = true;
        }
        // Hovering a segment while the bar is tracking opens *its* chevron — the one after it,
        // which is the one that lists this folder's subfolders. The deepest segment on the
        // trail has no chevron after it, so there is nothing for it to open and whatever is
        // showing stays where it is.
        if tracking && response.hovered() && index != deepest {
            wanted = Some(index + 1);
        }
        // Part of the open pair: filled as one shape with the chevron after it, from behind, so
        // it wears nothing of its own here. See where `pair_hint` is set.
        let paired = menu.open == Some((pane, index + 1));
        if !paired && (response.hovered() || response.is_pointer_button_down_on()) {
            ui.painter().rect_filled(
                segment,
                corner,
                if response.is_pointer_button_down_on() {
                    pressed_fill
                } else {
                    hover_fill
                },
            );
        }
        // Bold and full-contrast marks the folder being shown, which is the only cue that it
        // is not simply the end of the trail. Everything else on the bar is a link, on both
        // sides of it.
        let (font, color) = if current {
            (t.fonts.body_strong.clone(), t.text.primary)
        } else if response.hovered() || paired {
            (t.fonts.body.clone(), t.text.primary)
        } else {
            (t.fonts.body.clone(), t.text.secondary)
        };
        // A folder found deleted when it was clicked: red and struck through, in place, so the bar
        // says what became of it. See [`standing`].
        let gone = tab.trail_gone.as_ref().is_some_and(|gone| path.starts_with(gone));
        let galley = if gone {
            let mut format = egui::TextFormat::simple(font, t.status.danger);
            format.strikethrough = egui::Stroke::new(1.0, t.status.danger);
            let mut job = egui::text::LayoutJob::single_section(label.clone(), format);
            job.wrap = egui::text::TextWrapping::truncate_at_width((width - space::S3 * 2.0).max(0.0));
            ui.painter().layout_job(job)
        } else {
            truncated(ui.painter(), label, font, color, width - space::S3 * 2.0)
        };
        text_left(
            ui.painter(),
            Rect::from_min_max(
                pos2(segment.left() + space::S3, segment.top() - TEXT_LIFT),
                pos2(segment.right() - space::S3, segment.bottom() - TEXT_LIFT),
            ),
            galley,
        );
        if response.clicked() && !current {
            if let Some(to) = standing(tab, &crumbs, index) {
                out.push(Action::Navigate { pane, path: to });
            }
        }
        // Middle click opens an ancestor in its own tab, as it does everywhere — but only the one
        // clicked: a tab of some other folder is not what was asked for.
        if response.middle_clicked() && standing(tab, &crumbs, index).as_ref() == Some(path) {
            out.push(Action::NavigateNewTab {
                pane,
                path: path.clone(),
            });
        }

        x += width;
        // A chevron between every pair of segments, listing the left one's subfolders. The
        // one after the current folder is the useful case and the reason this is keyed on
        // the end of the trail rather than on the current folder.
        if index != deepest {
            pending_chevron = Some(path.clone());
        }
    }

    // ---- Where the open dropdown goes next -------------------------------
    //
    // Applied here rather than where the hover was noticed, so that the frame which notices is
    // drawn whole: switching mid-loop would leave the dropdown that is going away already
    // painted for this frame with the one arriving painted over it. One frame of delay, and the
    // repaint is asked for because the pointer has very likely stopped moving by now — it
    // arrived somewhere and stayed.
    match wanted.filter(|_| !clicked) {
        Some(index) => {
            if menu.open != Some((pane, index)) {
                menu.open = Some((pane, index));
                ui.ctx().request_repaint();
            }
        }
        // A dropdown recorded as open whose chevron was not drawn at all — the pane has since
        // been narrowed past it, or the pointer asked for one that turned out not to fit. It is
        // showing nothing, and leaving it recorded would leave the bar tracking a menu that
        // cannot be seen or closed.
        None if tracking && !shown => menu.open = None,
        None => {}
    }

    // ---- The rest of the bar: click it to type a path --------------------
    //
    // Including the pen's own strip, which is part of the same target: it is a drawing rather
    // than a button, and a hint you cannot click would be a strange thing to draw.
    let field = Rect::from_min_max(
        pos2(bar.left(), (bar.center().y - TOOL_SIZE * 0.5).round()),
        pos2(bar.right(), (bar.center().y + TOOL_SIZE * 0.5).round()),
    );
    let empty = Rect::from_min_max(pos2(x, bar.top()), bar.max);
    let over_field = if empty.width() > 4.0 {
        let response = ui.interact(empty, Id::new(("crumb-empty", pane)), Sense::click());
        if response.clicked() {
            start_editing(tab, slashes);
        }
        response.hovered()
    } else {
        false
    };
    if over_field {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        // **The border and nothing else.** The one place on the bar where a click does
        // something other than navigate says so before the click — and says it the way a field
        // says it, with the outline a field wears under the pointer, over the whole shape the
        // field is about to take. A fill as well made the bar change colour to announce
        // something that is only an announcement.
        //
        // From behind, and this is the reason `field_hint` was reserved: the outline spans the
        // whole bar, and by the time the empty space at the end of it is known to be hovered,
        // every segment's label has been painted.
        //
        // Square, because the field it is a preview of is square — see [`crate::ui::squared`] and
        // the path field in [`super::complete`]. A rounded hint promising a field with sharp
        // corners is the outline moving as the click lands, in the one place on the bar where the
        // hint and the thing it announces occupy the very same rect.
        ui.painter().set(
            field_hint,
            egui::epaint::RectShape::stroke(field, CornerRadius::ZERO, Stroke::new(1.0, pen_ink(t, true)), StrokeKind::Inside),
        );
    }

    // ---- The pen ---------------------------------------------------------
    //
    // A hint, not a control: it says the bar can be typed into, which is the one thing about
    // this bar that nothing else on it advertises. Always drawn, because a hint that only
    // appears once the pointer is already there is not a hint — and in the same ink as the
    // outline, brighter when the pointer is over the bar, so the two read as one cue.
    //
    // Its space is taken out of the trail's before the segments are laid out, so a path deep
    // enough to fill the bar stops short of it rather than running underneath it.
    icons::pencil(
        ui.painter(),
        Rect::from_center_size(
            pos2((bar.right() - PEN * 0.5).round(), field.center().y.round()),
            egui::vec2(PEN, PEN),
        ),
        pen_ink(t, over_field),
    );
}

/// The ink the pen and the field's outline share.
///
/// One colour for both, because they are one hint: the outline says "this is a field" and the pen
/// says "this is where you write". `stroke-strong` is what Azur gives a *field* under the pointer
/// — the same border the path field itself will wear a moment later — and `stroke-control` is that
/// border at rest, which is what the pen sits at until the pointer arrives.
///
/// Not `stroke-subtle`, which is the obvious name for something subtle and is invisible here:
/// [`crate::ui::seam`] — the bar's own fill — *is* `stroke-subtle`, so a hairline in it is a
/// hairline drawn in the colour behind it. It was, for as long as this hint was a hairline.
pub(crate) fn pen_ink(t: &Theme, lit: bool) -> egui::Color32 {
    if lit {
        t.stroke.strong
    } else {
        t.stroke.control
    }
}
