//! The bar along the top of the panel: what the file is, what is being done to it, and the
//! buttons that do it.
//!
//! One row for every content type, which is why it is here rather than in each of them — the
//! zoom field only appears for a picture, the diff toggle only for a changed text file, but
//! the row they sit in is the same row and lays out once.

use super::*;

/// The zoom field.
///
/// Enough for `400%` and a chevron. It is a combo box rather than a label because a zoom you can
/// only reach through `+` and `−` is a zoom you cannot ask for: 100% from 874% is eight clicks.
pub(super) const ZOOM_W: f32 = 64.0;

/// The strip above each image in a comparison, holding which file it is.
pub(super) const CAPTION: f32 = 16.0;

/// What is on show, what it turned out to be, and the controls.
///
/// Laid out **right to left**, because that is the order the priorities run in: the controls take
/// what they need first, then the two details in turn, and the name gets whatever is left. See the
/// module header for the rule.
#[allow(clippy::too_many_arguments)]
pub(super) fn header(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    preview: &mut Preview,
    layout: &mut Layout,
    changed: bool,
    // Whether the selection is two pictures, which is what turns the diff button into the compare
    // button. See [`crate::app::App::can_compare`].
    can_compare: bool,
    scratch: &mut String,
    out: &mut Vec<Action>,
) {
    use std::fmt::Write as _;

    // **The bar is about the focused tile.** Everything below reads one file's content and one
    // file's find bar, and with up to `MOST` of them on show the focused one is the one the bar
    // names — see [`Preview::focus`]. Destructured rather than reached through `focused_mut`, because
    // the compare latch beside it is the panel's rather than the tile's and both are wanted at once.
    let focus = preview.focused_at();
    let Preview {
        slots, compare, ..
    } = preview;
    let slot = &mut slots[focus];

    // One baseline for the whole bar, taken from its principal font, so the title, the details and
    // the glyph beside them are on one line. `crate::ui::deps` says why at length.
    let baseline = ink_baseline(ui.painter(), &t.fonts.body, rect.top(), rect.height());
    let surface = t.bg.layer;
    let button = |right: f32| {
        Rect::from_center_size(
            pos2(right - TOOL_SIZE * 0.5, rect.center().y),
            vec2(TOOL_SIZE, TOOL_SIZE),
        )
    };

    // ---- The controls, from the right edge inwards ------------------------
    let close = button(rect.right() - PAD);
    if tool_button(
        ui,
        t,
        close,
        Id::new(("preview-close", pane)),
        &azur_icons::close,
        "Close the preview (Ctrl+P)",
        true,
        false,
        surface,
    )
    .clicked()
    {
        out.push(Action::ClosePreview(pane));
    }
    let mut right = close.left() - PAD;

    // **Which view this is, as a question**, immediately inside close and ahead of every view's own
    // controls: it is about the file rather than about how one view reads it, and it is the one
    // control that stays put as the view underneath it changes. Only for a file whose name had no
    // answer — see [`Slot::choosable`].
    if slot.choosable() {
        let at = button(right);
        if tool_button(
            ui,
            t,
            at,
            Id::new(("preview-view-as", pane)),
            &crate::icons::view_as,
            "Show this file as something else",
            true,
            slot.choosing,
            surface,
        )
        .clicked()
        {
            slot.choose();
        }
        right = at.left() - PAD;
    }

    // The view's own toggle, immediately inside the close button: showing both sources of a
    // comparison, or numbering the lines of a text file. Never both — they belong to different
    // views — so they share the slot.
    //
    // **None of them while the chooser is up**, nor find or zoom below: each acts on a canvas that is
    // not on screen, and a button whose effect cannot be seen is one that seems not to work.
    let viewing = !slot.choosing;
    match &mut slot.content {
        _ if !viewing => {}
        Content::Picture(picture) => {
            // **A picture git has moved on from gets the same toggle a text file does**, and it is
            // the same preference behind it: one answer to "show me what changed", whatever kind of
            // file is on screen. Offered only where there is something to compare with — and unlike
            // the text one, that cannot be read off the payload, because with the toggle off the
            // panel holds one picture and nothing that remembers there was another.
            //
            // **Never over a shell render**, which is the one case where there is nothing to compare
            // *with*. `crate::preview::diff` builds the older side by decoding the blob git has, and
            // a registered visualizer takes a path rather than bytes — so `selected_preview` refuses
            // an `AgainstHead` for anything but a [`crate::preview::Kind::Picture`] and this button
            // would sit there latching a preference that changed nothing on screen. A control that
            // does nothing is worse than no control; the same argument the two text toggles make.
            // **With two pictures selected this is the compare button instead**, and that is what
            // keeps the blended view reachable now that selecting two files tiles them side by side.
            // The same glyph, because it is the same question — *show me the difference* — asked of
            // two files rather than of two versions of one. It takes precedence because it is about
            // the files in front of you, where `layout.diff` is a standing preference.
            let offering = if can_compare {
                Some((*compare, "Compare the two selected pictures"))
            } else if changed && !picture.shell {
                Some((layout.diff, "Show what changed since the last commit"))
            } else {
                None
            };
            if let Some((latched, tip)) = offering {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-diff", pane)),
                    &crate::icons::diff,
                    tip,
                    true,
                    latched,
                    surface,
                )
                .clicked()
                {
                    if can_compare {
                        // Not a preference and not remembered: it dies with these two files. See
                        // [`Preview::compare`].
                        *compare = !*compare;
                    } else {
                        layout.diff = !layout.diff;
                        out.push(Action::RememberLayout);
                    }
                }
                right = at.left() - PAD;
            }
            // And inside it, the one that only means anything while there are three views to choose
            // between — exactly as the text view nests `collapse` inside `diff`.
            if picture.frames.len() == 3 {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-all", pane)),
                    &crate::icons::columns,
                    "Show both images as well as the difference",
                    true,
                    picture.all,
                    surface,
                )
                .clicked()
                {
                    picture.all = !picture.all;
                }
                right = at.left() - PAD;
            }
        }
        Content::Text(text) => {
            // Numbering a *rendered* document means nothing — its lines are not the file's
            // — so the gutter's toggle is only offered over something that has lines. That
            // is the one control in this bar that comes and goes with a preference rather
            // than with the file, and the alternative was a button that did nothing.
            if !text.renderable() || layout.markup {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-numbers", pane)),
                    &crate::icons::line_numbers,
                    "Number the lines",
                    true,
                    layout.numbers,
                    surface,
                )
                .clicked()
                {
                    layout.numbers = !layout.numbers;
                    out.push(Action::RememberLayout);
                }
                right = at.left() - PAD;
            }
            // **Only over a file git has something to say about**, which is the same argument the
            // numbers toggle makes: a control that does nothing is worse than no control. It appearing
            // is itself the news that this file has changed.
            if text.diffable() {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-diff", pane)),
                    &crate::icons::diff,
                    "Show what changed since the last commit",
                    true,
                    layout.diff,
                    surface,
                )
                .clicked()
                {
                    layout.diff = !layout.diff;
                    out.push(Action::RememberLayout);
                }
                right = at.left() - PAD;

                // And inside it, the one that only means anything while the diff is on: a file with
                // its unchanged stretches left out.
                if layout.diff {
                    let at = button(right);
                    if tool_button(
                        ui,
                        t,
                        at,
                        Id::new(("preview-collapse", pane)),
                        &crate::icons::collapse,
                        "Leave out the parts that have not changed",
                        true,
                        layout.collapse,
                        surface,
                    )
                    .clicked()
                    {
                        layout.collapse = !layout.collapse;
                        out.push(Action::RememberLayout);
                    }
                    right = at.left() - PAD;
                }
            }
            if text.renderable() {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-markup", pane)),
                    &crate::icons::markup,
                    "Show the Markdown itself",
                    true,
                    layout.markup,
                    surface,
                )
                .clicked()
                {
                    layout.markup = !layout.markup;
                    // The two views are two different strings, so every hit the find bar
                    // is holding is an offset into the wrong one. Forgotten rather than
                    // recomputed here: the search runs again on the next frame, against
                    // whichever of them is now on screen.
                    slot.find.forget();
                    out.push(Action::RememberLayout);
                }
                right = at.left() - PAD;
            }
        }
        // A binary gets one control, and it is the same question the listing's own flatten button
        // asks — hence the same glyph: *show me every one of these, in one list, instead of the tree
        // that says how each was reached*. A dependency graph and a folder tree are the same shape
        // and the same two ways of reading it.
        Content::Binary(_) => {
            let at = button(right);
            if tool_button(
                ui,
                t,
                at,
                Id::new(("preview-deps-list", pane)),
                &crate::icons::flatten,
                "List every module instead of the tree",
                true,
                layout.deps.list,
                surface,
            )
            .clicked()
            {
                layout.deps.list = !layout.deps.list;
                out.push(Action::RememberLayout);
            }
            right = at.left() - PAD;
        }
        _ => {}
    }

    // Find, immediately inside the line numbers, so the group reads `[find] [numbers] [close]`.
    // Outside the match above rather than in its `Text` arm: that one borrows the content, and this
    // one has to reach the find state beside it.
    //
    // A button and not a shortcut. `Ctrl+F` is the pane's filter box and has been since long before
    // this panel existed — a preview that took it would be taking the keyboard away from the window
    // it lives in, for a bar that only exists while one kind of file is selected.
    if viewing && matches!(slot.content, Content::Text(_)) {
        let at = button(right);
        if tool_button(
            ui,
            t,
            at,
            Id::new(("preview-find", pane)),
            &azur_icons::search,
            "Find in this file",
            true,
            slot.find.open,
            surface,
        )
        .clicked()
        {
            slot.find.open = !slot.find.open;
            // Opening it puts the caret in it: a find bar you have to click into after asking for it
            // is a find bar that wanted two clicks.
            slot.find.grab = slot.find.open;
        }
        right = at.left() - PAD;
    }

    // The zoom group: in, out, Fit, and the field — laid out right to left, so on screen it reads
    // `[field] [fit] [−] [+]`, which is the order asked for.
    //
    // Dropped whole if the bar is narrower than they are. That is the one case the priority rule
    // does not cover, and the alternative is drawing them on top of each other: a panel down the
    // side is never this narrow — `MIN_PANEL_W` is sized from `ACTIONS` for exactly this reason —
    // but a panel along the bottom is as wide as its pane, and a pane can be squeezed.
    if let Content::Picture(picture) = &mut slot.content {
        if viewing && right - rect.left() > ACTIONS {
            for (glyph, tip, step) in [
                (
                    &azur_icons::plus as azur_egui_theme::icons::Icon<'_>,
                    "Zoom in",
                    Some(ZOOM_STEP),
                ),
                (&azur_icons::minus, "Zoom out", Some(1.0 / ZOOM_STEP)),
                (&crate::icons::fit, "Fit the panel", None),
            ] {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-zoom", pane, tip)),
                    glyph,
                    tip,
                    true,
                    false,
                    surface,
                )
                .clicked()
                {
                    match step {
                        // Zoomed about the middle of the canvas, which is where the eye is when a
                        // button rather than the wheel was used.
                        Some(by) => {
                            picture.zoom = Some((picture.scale() * by).clamp(ZOOM_MIN, ZOOM_MAX));
                        }
                        None => {
                            picture.zoom = None;
                            picture.pan = Vec2::ZERO;
                        }
                    }
                }
                right = at.left() - PAD;
            }
            right = zoom_field(ui, rect, right, pane, picture) - PAD;
        }
    }

    // ---- The two details, in the order they give way ----------------------
    //
    // The comment first, then the size, and only once both are gone does the name start to crop.
    let mut comment = String::new();
    scratch.clear();
    let size = scratch;
    let mut mark = None;
    match &slot.content {
        Content::Picture(picture) => {
            // The dimensions, and both of them when two files are being compared and disagree —
            // because that *is* a difference.
            let mut dimensions = format!("{} × {}", picture.natural[0], picture.natural[1]);
            if let Some(other) = picture.other {
                let _ = write!(dimensions, " / {} × {}", other[0], other[1]);
            }
            match picture.differing {
                // **For a comparison the headline is the share that differs**, so it takes the
                // slot that survives and the dimensions take the one that goes first. The two
                // slots are priorities and not captions: "0.04% differs" is the answer somebody
                // opened the comparison for, and `300 × 200` is the nicety.
                Some(differing) => {
                    let _ = write!(size, "{:.2}% differs", differing * 100.0);
                    if differing > 0.0 {
                        mark = Some(t.status.danger);
                    }
                    comment.push_str(&dimensions);
                }
                // **And for a shell render the headline is that it is one**, which is the same
                // priority argument the comparison makes one arm up: the slot that survives a narrow
                // panel goes to the thing somebody needs to know, and the dimensions take the one
                // that gives way. What they need to know is that this is a *picture of* the file
                // produced by Windows — the first page of the PDF, a frame of the video, at the size
                // it was asked for — so `1024 × 576` is the size of that render and not of anything
                // in the file. Left in the surviving slot, those dimensions would read as the file's
                // own the moment the note beside them was dropped.
                None if picture.shell => {
                    size.push_str("Windows preview");
                    comment.push_str(&dimensions);
                }
                None => {
                    size.push_str(&dimensions);
                    if picture.vector {
                        comment.push_str("vector, no text");
                    } else if picture.scaled {
                        comment.push_str("scaled to fit memory");
                    }
                }
            }
        }
        Content::Text(text) => {
            crate::fs::fmt::size(text.body.len() as u64, size);
            if text.truncated {
                comment.push_str("first part only");
            }
        }
        // The picture's size in the slot that survives, and how long it runs in the one that gives
        // way — which is the other way round from what it looks like it should be, and is the same
        // argument the shell render makes two arms up. The running time is *already on screen*, in
        // the strip under the canvas beside a clock that says where in it you are; the pixel size is
        // written down nowhere else. Repeating the duration here at the cost of the dimensions would
        // be spending the surviving slot on the one fact the panel is already showing.
        Content::Video(player) => {
            if let Some([w, h]) = player.native() {
                let _ = write!(size, "{w} × {h}");
            }
            if let Some(duration) = player.duration() {
                comment.push_str(&crate::preview::video::clock(duration));
            }
        }
        Content::Binary(view) => {
            let (files, api_sets, missing) = view.graph().tally();
            if missing > 0 {
                let _ = write!(size, "{missing} missing");
                // The count in `text-primary` with the status hue on a **glyph** beside it, rather
                // than in `status.danger` as text: measured on this surface the danger role is
                // under the floor for 12-point text and over the one for a shape, so the mark
                // carries the colour and the number stays legible. `crate::ui::deps` has the table.
                mark = Some(t.status.danger);
            } else {
                let _ = write!(
                    size,
                    "{files} file{}, {api_sets} API set{}",
                    if files == 1 { "" } else { "s" },
                    if api_sets == 1 { "" } else { "s" },
                );
            }
            if view.graph().truncated {
                comment.push_str("stopped early");
            }
        }
        _ => {}
    }

    // ---- The title, and what the details have left it --------------------
    let mut x = rect.left() + PAD;
    let (glyph, ink): (azur_egui_theme::icons::Icon<'_>, Color32) = match &slot.content {
        Content::Picture(_) => (&crate::icons::image, t.image),
        // The same glyph and the same hue the row in the listing beside it is wearing, which is what
        // the whole table in `crate::icons::for_kind` is for.
        Content::Video(_) => (&crate::icons::video, t.video),
        Content::Text(_) => (&crate::icons::document, t.document),
        Content::Binary(_) => (&crate::icons::executable, t.executable),
        Content::Failed(_) => (&azur_icons::error, t.status.danger),
        _ => (&crate::icons::file, t.text.secondary),
    };
    let box_rect = icon_rect(rect, x, GLYPH);
    glyph(ui.painter(), box_rect, ink);
    x = box_rect.right() + PAD;

    let name = slot
        .of
        .as_ref()
        .map(Ask::title)
        .unwrap_or_else(|| "Preview".to_owned());
    // How wide the name would like to be, which is what decides whether a detail has to go: the
    // name is never cropped while a detail could have been dropped instead.
    let wanted = ui
        .painter()
        .layout_no_wrap(name.clone(), t.fonts.body.clone(), t.text.primary)
        .size()
        .x;
    let costs = |text: &str| {
        if text.is_empty() {
            0.0
        } else {
            ui.painter()
                .layout_no_wrap(text.to_owned(), t.fonts.caption.clone(), t.text.secondary)
                .size()
                .x
                + PAD * 2.0
        }
    };
    let mark_w = if mark.is_some() { MARK + PAD } else { 0.0 };
    let (show_comment, show_size) = what_fits(
        wanted,
        costs(&comment),
        costs(size) + mark_w,
        (right - x).max(0.0),
    );

    {
        let mut detail = |ui: &Ui, text: &str, with_mark: bool| {
            if text.is_empty() {
                return;
            }
            let galley = truncated(
                ui.painter(),
                text,
                t.fonts.caption.clone(),
                // The one carrying the mark is the headline, so it is the one in the readable ink.
                if with_mark {
                    t.text.primary
                } else {
                    t.text.secondary
                },
                (right - x).max(0.0),
            );
            let w = galley.size().x;
            galley_on_baseline(ui.painter(), right - w, baseline, galley);
            right -= w + PAD;
            if with_mark {
                if let Some(ink) = mark {
                    azur_icons::error(ui.painter(), icon_rect(rect, right - MARK, MARK), ink);
                    right -= MARK + PAD;
                }
            }
        };
        if show_size {
            detail(ui, size, mark.is_some());
        }
        if show_comment {
            detail(ui, &comment, false);
        }
    }

    let galley = truncated(
        ui.painter(),
        &name,
        t.fonts.body.clone(),
        t.text.primary,
        (right - PAD - x).max(0.0),
    );
    galley_on_baseline(ui.painter(), x, baseline, galley);

    // A binary's title carries the whole answer: what the walk found, how long it took, and
    // where every name was looked for — which is the one thing a location column cannot say for
    // itself, and does not fit on a bar this narrow.
    if let Content::Binary(view) = &slot.content {
        let response = ui.interact(
            Rect::from_min_max(pos2(x, rect.top()), pos2(right, rect.bottom())),
            Id::new(("preview-title", pane)),
            Sense::hover(),
        );
        if response.hovered() {
            azur_egui_theme::components::tooltip(response, view.about());
        }
    }
}

/// Which of the two details survive: the comment, and the size.
///
/// **The rule the bar is built around**, and the only surprising thing about it: the name is never
/// cropped while a detail could have been dropped instead. So the comment goes first, then the
/// size, and the name starts losing characters only once there is nothing else left to give.
///
/// The consequence worth knowing is that a *long name* takes the details away even in a wide panel.
/// That is the right way round — the name is what identifies the file and `300 × 200` is a nicety —
/// but it does mean the details come and go as you arrow down a folder of mixed names, which is a
/// thing somebody reading this will otherwise take for a bug.
pub(super) fn what_fits(name: f32, comment: f32, size: f32, room: f32) -> (bool, bool) {
    if name + comment + size <= room {
        (true, true)
    } else if name + size <= room {
        (false, true)
    } else {
        (false, false)
    }
}

/// The zoom field: a subtle, editable combo box. Returns its left edge.
///
/// **Editable and not a menu**, because a zoom you can only reach through `+` and `−` is a zoom you
/// cannot ask for — 100% from 874% is eight clicks. The presets are what people actually want and
/// the field is there for the time they want 137%.
///
/// `Variant::Subtle` gives up the fill and the border until the pointer is over it, which is the
/// right treatment on a bar this crowded: a field's border round every control is more lines than
/// there is information, and the affordance arrives when the pointer does.
pub(super) fn zoom_field(ui: &mut Ui, bar: Rect, right: f32, pane: PaneId, picture: &mut Picture) -> f32 {
    use azur_egui_theme::components::{ComboBox, Size, Variant};

    /// What the list offers. `Fit` first, because it is where the panel starts and what a
    /// double click puts it back to.
    const PRESETS: [&str; 10] = [
        "Fit", "25%", "33%", "50%", "75%", "100%", "150%", "200%", "300%", "400%",
    ];

    let field = Rect::from_center_size(
        pos2(right - ZOOM_W * 0.5, bar.center().y),
        vec2(ZOOM_W, TOOL_SIZE),
    );
    let was = picture.zoom_pick;
    // A child `Ui` with an id of its own, rather than `Ui::put`: the widget takes its id from the
    // `Ui`'s auto-id counter, so two panes drawing their bars in the same frame would otherwise
    // depend on the order they were drawn in for their fields to stay apart.
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("preview-zoom-field", pane))
            .max_rect(field)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let response = child.add(
        ComboBox::new(&mut picture.zoom_text, &mut picture.zoom_pick)
            .options(PRESETS)
            // **Not a filtering combo box.** The field's text is the current zoom, so filtering the
            // list by it narrows ten presets down to the one you are already on — and a value
            // nobody offered, `137%`, narrows it to nothing and the list stops opening at all.
            // See `azur::ComboBox`, where the distinction is written down.
            .filter(false)
            // **And nothing can go in the leading column, so there is no leading column.** Ten
            // percentages cannot carry an icon, and a tick beside the current one would say what
            // the field above it already says — in a place you have to open the list to read.
            // Without this every entry is indented 24 points for something that never appears.
            .icons(false)
            .variant(Variant::Subtle)
            .size(Size::Small)
            .width(ZOOM_W),
    );

    let typing = response.has_focus();
    let (entered, escaped) = child.input(|i| {
        (
            typing && i.key_pressed(egui::Key::Enter),
            typing && i.key_pressed(egui::Key::Escape),
        )
    });
    if picture.zoom_pick != was {
        // Taken from the list.
        if let Some(picked) = picture.zoom_pick.and_then(|at| PRESETS.get(at)) {
            apply_zoom(picture, picked);
        }
        picture.zoom_pick = None;
    } else if entered || response.lost_focus() {
        let text = picture.zoom_text.clone();
        apply_zoom(picture, &text);
    } else if !typing {
        // Not being edited: the field reports what the canvas is actually showing, which the wheel
        // and the buttons and a resize all change without going through here.
        let now = format!("{:.0}%", picture.percent());
        if picture.zoom_text != now {
            picture.zoom_text = now;
        }
    }

    // **And then it lets the keyboard go.**
    //
    // A text field that keeps focus after you have finished with it takes every shortcut in the
    // window with it — `Ctrl+P`, `F5`, the arrow keys, the type-ahead — because `App::keyboard`
    // stands down whenever anything has focus, which is the right rule and the reason this matters.
    // egui hands focus back when a click lands on another *focusable* widget, and nothing else in
    // this panel is one: the canvas and the listing are bare `interact` rects.
    //
    // So: `Enter` and `Escape` are done with it, and so is a press anywhere but the field itself. A
    // press on the list is one of those — the pick has already been taken by the time this runs, so
    // letting go is right there too.
    if typing {
        let elsewhere = child.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten()
                .is_some_and(|at| !field.contains(at))
        });
        if entered || escaped || elsewhere {
            response.surrender_focus();
        }
    }
    field.left()
}

/// Read a percentage — or the word `Fit` — and go there.
///
/// Anything unparseable is ignored rather than reset to something: the field is rewritten from the
/// canvas on the next frame it does not have focus, so a typo puts the old value back by itself.
pub(super) fn apply_zoom(picture: &mut Picture, text: &str) {
    let text = text.trim();
    if text.eq_ignore_ascii_case("fit") {
        picture.zoom = None;
        picture.pan = Vec2::ZERO;
        return;
    }
    let number: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if let Ok(percent) = number.parse::<f32>() {
        if percent > 0.0 {
            let scale = picture.scale_for(percent);
            picture.zoom = Some(scale.clamp(ZOOM_MIN, ZOOM_MAX));
        }
    }
}

/// What a zoom step multiplies by.
///
/// A quarter, which takes four steps to double. Photo viewers use anything from 1.1 to 2; a
/// quarter is fine enough to land near what you wanted and coarse enough that getting from fit to
/// 4:1 is not a dozen clicks.
pub(super) const ZOOM_STEP: f32 = 1.25;

/// How far a picture may be zoomed, as a scale on the texture rather than a percentage of the file.
pub(super) const ZOOM_MIN: f32 = 0.01;

pub(super) const ZOOM_MAX: f32 = 32.0;
