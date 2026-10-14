//! The prompt row: the shell picker, the field, and what a keystroke in it means.

use super::*;

/// The prompt strip: the shell button, the line, and Run.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prompt(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    state: &mut State,
    hot: bool,
    running: bool,
    out: &mut Outcome,
) {
    let font = t.fonts.mono.clone();
    let line_metrics = metrics(ui, &font);
    let advance = line_metrics.advance;

    // Sized for the widest label, so the line does not shift when the shell changes.
    let label = t.fonts.caption.clone();
    let label_metrics = metrics(ui, &label);
    let widest = Kind::ALL
        .iter()
        .map(|kind| width(ui, &label, kind.label()))
        .fold(0.0_f32, f32::max);
    // Padding, the label, a gap, the chevron, padding — spelled out, because the version that
    // guessed one number for the whole lot printed `bash^` with the caret against the `h`.
    let shell = Rect::from_min_size(
        pos2(rect.left() + PAD, rect.center().y - control::SMALL * 0.5),
        vec2(
            space::S3 + widest + space::S2 + FOLD + space::S3,
            control::SMALL,
        ),
    );
    let run = Rect::from_min_size(
        pos2(
            rect.right() - PAD - control::SMALL,
            rect.center().y - control::SMALL * 0.5,
        ),
        Vec2::splat(control::SMALL),
    );
    let field = Rect::from_min_max(
        pos2(shell.right() + space::S3, rect.top()),
        pos2(run.left() - space::S3, rect.bottom()),
    );

    // -- the shell button --------------------------------------------------
    let picker = ui.interact(shell, Id::new(("console-shell", pane)), Sense::click());
    if picker.clicked() {
        state.picking = !state.picking;
    }
    if picker.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    // The shortcut in brackets comes out in `text-secondary` on its own: the design system's tooltip
    // recognises a trailing `(…)` and sets it as an aside, which is the same two colours every other
    // tooltip in this window says it in.
    azur_egui_theme::components::tooltip(picker.clone(), "Change shell (Shift+Tab)");
    // **Revealed on hover, like Run beside it.** No resting fill: the strip is one surface with two
    // controls sitting on it, and a filled box around the shell name made it the loudest thing in a
    // panel whose whole job is the text above it. The name alone says which shell; the box only has
    // to appear when the pointer is looking for something to press.
    // An open menu holds the button down, so it wears the press for as long as the menu is up.
    let fill = if state.picking {
        Some(control_fills(t).1)
    } else {
        crate::ui::control_look(t, &picker, true, false).0
    };
    if let Some(fill) = fill {
        ui.painter()
            .rect_filled(shell, CornerRadius::same(radius::SMALL), fill);
    }
    ui.painter().text(
        pos2(shell.left() + space::S3, ink_top(shell, &label_metrics)),
        Align2::LEFT_TOP,
        state.kind.label(),
        label.clone(),
        t.text.primary,
    );
    azur_egui_theme::icons::chevron_up(
        ui.painter(),
        crate::ui::icon_rect(shell, shell.right() - space::S3 - FOLD, FOLD),
        t.text.tertiary,
    );

    // -- Run, which is Stop while something is going -----------------------
    let button = ui.interact(run, Id::new(("console-run", pane)), Sense::click());
    if button.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    if button.clicked() {
        if running {
            out.stop = true;
        } else {
            // Never feeding: this arm is only reached when nothing is running, and the button is Stop
            // whenever something is. Typing at a running command is `Enter`, which is where you
            // already are.
            state.run(false, out);
        }
    }
    if let Some(fill) = crate::ui::control_look(t, &button, true, false).0 {
        ui.painter()
            .rect_filled(run, CornerRadius::same(radius::SMALL), fill);
    }
    azur_egui_theme::components::tooltip(
        button.clone(),
        if running {
            "Stop the command (Ctrl+C)"
        } else {
            "Launch command (Enter)"
        },
    );
    let glyph = if running {
        crate::icons::stop
    } else {
        crate::icons::play
    };
    // **The glyph has to lift when the fill arrives under it.** An empty line greys it to
    // `text-disabled`, and `text-disabled` on `background-control-hover` is two rungs of the same
    // neutral ramp — so hovering Run made Run disappear, which is the opposite of what a hover is for.
    let ink = if running {
        t.status.danger
    } else if state.line.is_empty() {
        if button.hovered() {
            t.text.secondary
        } else {
            t.text.disabled
        }
    } else {
        t.accent.default
    };
    glyph(
        ui.painter(),
        crate::ui::icon_rect(run, run.left(), control::SMALL),
        ink,
    );

    // -- the line ----------------------------------------------------------
    //
    // Not a text field: the same surface as the log above it, and a border only under the pointer.
    // A box drawn around it would make the panel two things stacked up rather than one.
    let strip = ui.interact(field, Id::new(("console-field", pane)), Sense::click_and_drag());
    // A text cursor and nothing else. The border that used to appear here made the line look like a
    // field that had grown out of the panel; the caret and the cursor already say it is somewhere you
    // can type, and the strip is one surface with the log above it.
    if strip.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    let text_x = field.left() + space::S2;
    if let Some(from) = crate::ui::held_from(ui, true) {
        if field.contains(from) {
            let col = |at: Pos2| ((at.x - text_x) / advance).round().max(0.0) as usize;
            let to = ui.input(|i| i.pointer.latest_pos()).unwrap_or(from);
            state.aim = Aim::Prompt;
            state.edited = ui.input(|i| i.time);
            if ui.input(|i| i.pointer.primary_pressed()) {
                state.line.seek(col(from), false);
            } else {
                state.line.seek(col(to), true);
            }
        }
    }

    let caret_x = text_x + state.line.col() as f32 * advance;
    if let Some(span) = state.line.span() {
        let from = state.line.text()[..span.start].chars().count() as f32;
        let to = state.line.text()[..span.end].chars().count() as f32;
        ui.painter().rect_filled(
            Rect::from_min_max(
                pos2(text_x + from * advance, field.top() + space::S1),
                pos2(text_x + to * advance, field.bottom() - space::S1),
            ),
            CornerRadius::ZERO,
            ui.visuals().selection.bg_fill,
        );
    }
    ui.painter().text(
        crate::ui::snap(ui.painter(), pos2(text_x, ink_top(field, &line_metrics))),
        Align2::LEFT_TOP,
        state.line.text(),
        font,
        t.text.primary,
    );
    if hot {
        // **egui's own caret**, colour, width, blink and all — `visuals.text_cursor` is where a text
        // field's caret comes from, and this window has one caret whatever is drawing the text under
        // it. It also schedules the repaint the blink needs, so the panel is still asleep between
        // them rather than running at the refresh rate.
        //
        // The phase is measured from the last edit, not from the epoch: a caret that carries on
        // blinking through what you are typing is the tell of one that is on a timer rather than on
        // the text.
        let since = ui.input(|i| i.time) - state.edited;
        egui::text_selection::visuals::paint_text_cursor(
            ui,
            ui.painter(),
            Rect::from_center_size(
                pos2(caret_x.round(), field.center().y),
                vec2(0.0, CARET),
            ),
            since,
        );
    }

    if state.picking {
        shells(ui, t, shell, pane, state, out);
    }
}

/// The shell dropdown, opening upwards out of its button.
///
/// The design system's menu — `menu_frame` around `MenuItem`s, the chosen shell wearing the same
/// tick as every other choice in the window — placed by hand rather than by `Menu`, for the one
/// thing `Menu` does not do: open *upwards*. The prompt sits at the foot of its pane, and in a
/// stacked layout that is the middle of the window, where a menu dropping down would cover the pane
/// below instead of the log it belongs to.
pub(crate) fn shells(ui: &mut Ui, t: &Theme, anchor: Rect, pane: PaneId, state: &mut State, out: &mut Outcome) {
    use azur_egui_theme::components::{menu_frame, MenuItem};

    let area = egui::Area::new(Id::new(("console-shells", pane)))
        .order(egui::Order::Foreground)
        // Hung by its bottom edge just above the button, so it grows upwards by however tall the
        // frame turns out to be rather than by a height worked out here and kept in step by hand.
        .pivot(Align2::LEFT_BOTTOM)
        .fixed_pos(pos2(anchor.left(), anchor.top() - space::S1))
        .show(ui.ctx(), |ui| {
            menu_frame(t)
                .show(ui, |ui| {
                    ui.set_width(anchor.width().max(96.0));
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for kind in Kind::ALL {
                        let item = MenuItem::new(kind.label()).selected(kind == state.kind);
                        if ui.add(item).clicked() {
                            state.picking = false;
                            if kind != state.kind {
                                state.kind = kind;
                                out.swap = Some(kind);
                            }
                        }
                    }
                })
                .response
                .rect
        });

    // Dismissed by a press anywhere else — including on the button, which is what makes a second
    // click on it close the menu rather than reopen it. The press that *opened* it is inside the
    // button, so it is excluded here and the menu survives its own opening click.
    if let Some(at) = crate::ui::pressed_at(ui) {
        if !area.inner.contains(at) && !anchor.contains(at) {
            state.picking = false;
        }
    }
}
