//! The video canvas: the picture, and the strip of controls under it.
//!
//! The reading half is [`crate::preview::video`], which owns the engine; this is where a click
//! becomes a seek. The one thing worth knowing about the pair is that the *canvas* is what drives
//! playback — [`crate::preview::Player::tick`] is called from here, on the frame that draws it, so
//! the frame transferred out of the decoder is the frame that reaches the screen and never one
//! behind.

use super::*;

/// The strip of controls along the bottom of the canvas.
///
/// Along the bottom, and not in the panel's bar. Everything in [`header`] is laid out right to left
/// in fixed-width slots, and a scrubber is the first control in this panel that wants *whatever
/// width is left* — dropped into that arithmetic it would either take the name's room or be two
/// centimetres of track. It is also where every video player on the machine puts it, which is worth
/// more than the consistency with the panel's other views.
pub(super) const STRIP: f32 = 30.0;

/// The scrubber's track, and the band that can be grabbed to move it.
///
/// The band is much taller than the track for the reason the panel's own splitter gives: a
/// four-point grab target is a four-point grab target. It fills the strip's height, so anywhere in
/// the horizontal middle of the strip is on the scrubber.
const TRACK: f32 = 4.0;

/// The least track worth drawing. Below this the times give way instead — see [`strip`].
const TRACK_MIN: f32 = 48.0;

/// The knob's radius.
///
/// Drawn at every size of panel and not only while hovered, because the scrubber has to say where
/// the video has got to when nothing is near it — which is most of the time.
const KNOB: f32 = 5.0;

/// The same player over the whole window: nothing else is drawn, and nothing else can be reached.
///
/// **The only difference from [`show`] is the rect and the ground.** Which is the point of it being
/// the same two functions underneath: the strip has to behave identically — the same scrubber, the
/// same mute, the same clock — or leaving fullscreen would be the moment the controls changed
/// meaning. What it does not get is the panel's bar, because the name of the file is not what
/// somebody watching it is looking for.
///
/// Answers whether it drew anything, which is `false` for a player that has failed: a complaint
/// filling a monitor is not a use of a monitor, and the caller takes the window back.
pub(super) fn theatre(
    ui: &mut Ui,
    t: &Theme,
    screen: Rect,
    pane: PaneId,
    player: &mut preview::Player,
    layout: &mut Layout,
    out: &mut Vec<Action>,
) -> bool {
    if player.failed().is_some() {
        return false;
    }
    canvas(ui, t, screen, pane, player, layout, out, true);
    true
}

/// The picture, the controls, and the repaint that keeps it moving.
pub(super) fn show(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    player: &mut preview::Player,
    layout: &mut Layout,
    out: &mut Vec<Action>,
) {
    canvas(ui, t, rect, pane, player, layout, out, false);
}

/// Both of the above. `filling` is whether this is the whole screen rather than a panel, which
/// decides two things and nothing else: what is behind the picture, and which way the fullscreen
/// button points.
#[allow(clippy::too_many_arguments)]
fn canvas(
    ui: &mut Ui,
    t: &Theme,
    canvas: Rect,
    pane: PaneId,
    player: &mut preview::Player,
    layout: &mut Layout,
    out: &mut Vec<Action>,
    filling: bool,
) {
    // **The preference is what is true, and the player is told.** Mute is one setting for the window
    // — see [`Layout::muted`] — so a player opened before it was last changed, in another pane or
    // another tab, has to be brought into line rather than keeping the value it was born with.
    if player.muted() != layout.muted {
        player.set_muted(layout.muted);
    }
    // The strip is dropped whole rather than squeezed when the panel is too short for it.
    //
    // The threshold leaves the picture 24 points, which is deliberately mean: at the *minimum* panel
    // height the canvas is 64 points, so a rule generous to the picture — half the panel, say — would
    // have taken the controls away from a panel somebody had merely dragged small rather than dragged
    // shut. For a player the controls are the more useful half of that trade.
    let bar = (canvas.height() >= STRIP + 24.0).then(|| {
        Rect::from_min_max(pos2(canvas.left(), canvas.bottom() - STRIP), canvas.max)
    });
    let screen = match bar {
        Some(bar) => Rect::from_min_max(canvas.min, pos2(canvas.right(), bar.top())),
        None => canvas,
    };

    // **Before anything is drawn**, so the frame this fetches is the frame painted below. It also
    // has to happen even when there is nothing to show yet: the engine's own news — the metadata,
    // a failure — arrives through the same call.
    if player.tick(ui.ctx(), screen.size(), ui.ctx().pixels_per_point()) {
        ui.ctx().request_repaint();
    }

    // One interaction over the picture, before it is painted, so the cursor can be set from it.
    let response = ui.interact(screen, Id::new(("preview-video", pane)), Sense::click());

    // The ground the picture sits on, which is also what shows either side of it.
    //
    // In the panel that is `bg.canvas`, the window's own backmost surface, rather than black:
    // letterbox bars are absence, and the colour this program already uses for absence is that one.
    // Filling a monitor it is **black**, because there the argument runs out — a panel belongs to the
    // window around it and there is no window around this, and every player ever written puts black
    // beside a film.
    let ground = if filling { Color32::BLACK } else { t.bg.canvas };
    ui.painter()
        .rect_filled(screen, CornerRadius::ZERO, ground);

    if let Some(why) = player.failed() {
        // The engine's complaint, in the middle, exactly where a decoder's would go. No strip: there
        // is nothing to play, so a play button would be a control that does nothing.
        note(ui, t, screen, why);
        return;
    }

    match player.frame() {
        Some((texture, pixels)) => {
            // **Fitted, and enlarged where it is smaller than the panel** — which is the one place
            // this canvas deliberately parts company with [`pictures`], where fitting is capped at
            // 1:1 because a 16-pixel icon blown up to fill a panel is a mosaic. A video is not an
            // icon: a 320 × 240 clip shown at a quarter of the panel it was opened in is not a
            // preview of it, and every player ever written scales it up.
            let scale = (screen.width() / pixels.x.max(1.0)).min(screen.height() / pixels.y.max(1.0));
            let where_ = Rect::from_center_size(screen.center(), pixels * scale);
            ui.painter().add(egui::Shape::image(
                texture.id(),
                where_,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            ));
        }
        // **A container that turned out to have no picture in it**, which is a real file and not a
        // failure: a `.mp4` holding one AAC track is what a voice memo and a ripped soundtrack look
        // like. The strip below plays it, so the canvas has to say why it is empty — otherwise this
        // is the one case that sits on `Opening…` for ever, since a frame is never coming.
        None if !player.opening() => note(ui, t, screen, "Sound only — no picture in this file"),
        // Otherwise a handful of frames, while Media Foundation resolves the file and the first
        // frame comes through. The same word the other views use for the same wait.
        None => note(ui, t, screen, "Opening…"),
    }

    // **A press tells this canvas whether it has the keyboard**, and that is the whole of the focus
    // rule: over the picture, the keys are this player's; anywhere else, they are not. Every drawn
    // canvas is told on the same press, so clicking one video takes the keys off another without
    // either knowing the other exists — see [`crate::preview::Player::keys`], and
    // `App::video_keys` for what is then done with them.
    if ui.input(|i| i.pointer.any_pressed()) {
        player.pressed_on(response.contains_pointer());
    }

    // A click on the picture plays or pauses it, and a double click fills the screen — the two
    // gestures every player has, and they are in each other's way: egui reports the first press of a
    // double click as an ordinary click and only names the second one. So the click is *held* until
    // the double-click delay has passed and a double click cancels it, which is
    // [`crate::preview::Player::clicked`] and the settle below.
    let now = ui.input(|i| i.time);
    if response.double_clicked() {
        player.double_clicked();
        out.push(Action::ToggleVideoFullscreen(pane));
    } else if response.clicked() {
        player.clicked(now);
    }
    // The delay is egui's own, so the gesture is the one the rest of the machine agrees is a double
    // click rather than a number chosen here.
    let delay = ui.ctx().options(|o| o.input_options.max_double_click_delay);
    if let Some(left) = player.settle_click(now, delay) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(left));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if let Some(bar) = bar {
        strip(ui, t, bar, pane, player, layout, out, filling);
    }
}

/// The controls: play, where it has got to, the scrubber, and mute.
///
/// Laid out from both ends inwards — the buttons take their slots, the two times take theirs, and
/// **the scrubber gets what is left**, which is the opposite of [`header`]'s rule and right for the
/// same reason header's is: there, the name is the information and the details are the nicety; here,
/// the track *is* the control and `0:07` is the nicety. So the times are what give way in a narrow
/// panel, and they go together rather than one at a time — a strip showing an elapsed time with no
/// duration beside it reads as a clock that has lost half of itself.
#[allow(clippy::too_many_arguments)]
fn strip(
    ui: &mut Ui,
    t: &Theme,
    bar: Rect,
    pane: PaneId,
    player: &mut preview::Player,
    layout: &mut Layout,
    out: &mut Vec<Action>,
    filling: bool,
) {
    let surface = t.bg.layer;
    ui.painter().rect_filled(bar, CornerRadius::ZERO, surface);

    let playing = player.playing();
    let at = Rect::from_center_size(
        pos2(bar.left() + PAD + TOOL_SIZE * 0.5, bar.center().y),
        Vec2::splat(TOOL_SIZE),
    );
    if tool_button(
        ui,
        t,
        at,
        Id::new(("preview-play", pane)),
        // Two glyphs in one slot, which is why they are drawn to the same weight — see
        // [`crate::icons::pause`].
        if playing {
            &crate::icons::pause as azur_egui_theme::icons::Icon<'_>
        } else {
            &crate::icons::play
        },
        if playing { "Pause" } else { "Play" },
        true,
        false,
        surface,
    )
    .clicked()
    {
        player.toggle();
    }
    let mut left = at.right() + PAD;
    let mut right = bar.right() - PAD;

    // Fullscreen, in the outermost slot on the right — the corner every player puts it in.
    //
    // **Always offered, unlike mute beside it.** There is no such thing as a video with nothing to
    // fill a screen with, so the argument that takes mute away for a silent clip has nothing to say
    // here. It is the same button coming and going in one slot either way, which is why the two
    // glyphs are one drawing with an argument between them — see [`crate::icons::fullscreen`].
    let at = Rect::from_center_size(
        pos2(right - TOOL_SIZE * 0.5, bar.center().y),
        Vec2::splat(TOOL_SIZE),
    );
    if tool_button(
        ui,
        t,
        at,
        Id::new(("preview-fullscreen", pane)),
        if filling {
            &crate::icons::fullscreen_exit as azur_egui_theme::icons::Icon<'_>
        } else {
            &crate::icons::fullscreen
        },
        if filling {
            "Leave fullscreen (Esc)"
        } else {
            "Fill the screen (double click)"
        },
        true,
        false,
        surface,
    )
    .clicked()
    {
        out.push(Action::ToggleVideoFullscreen(pane));
    }
    right = at.left() - PAD;

    // Mute, and **only for something with a sound track**: the same argument the text view's two
    // toggles make, that a control which does nothing is worse than no control. A silent screen
    // recording is the common case, not a rare one.
    if player.has_audio() {
        let muted = layout.muted;
        let at = Rect::from_center_size(
            pos2(right - TOOL_SIZE * 0.5, bar.center().y),
            Vec2::splat(TOOL_SIZE),
        );
        if tool_button(
            ui,
            t,
            at,
            Id::new(("preview-mute", pane)),
            if muted {
                &crate::icons::sound_off as azur_egui_theme::icons::Icon<'_>
            } else {
                &crate::icons::sound
            },
            if muted { "Unmute" } else { "Mute" },
            true,
            false,
            surface,
        )
        .clicked()
        {
            layout.muted = !muted;
            player.set_muted(layout.muted);
            out.push(Action::RememberLayout);
        }
        right = at.left() - PAD;
    }

    // The two times, measured before either is drawn: whether they fit is one question about the
    // pair.
    let baseline = ink_baseline(ui.painter(), &t.fonts.caption, bar.top(), bar.height());
    let duration = player.duration();
    let elapsed = preview::video::clock(player.at());
    let total = duration.map(preview::video::clock).unwrap_or_default();
    let width = |text: &str| {
        if text.is_empty() {
            0.0
        } else {
            ui.painter()
                .layout_no_wrap(text.to_owned(), t.fonts.caption.clone(), t.text.secondary)
                .size()
                .x
                + PAD
        }
    };
    // Widened to the longest string it will hold rather than to the one it holds now, so the track's
    // left edge does not jump sideways as `0:09` becomes `0:10`. The duration is the longest the
    // elapsed can ever get.
    let clock_w = width(&total).max(width(&elapsed));
    let show_clocks = right - left - clock_w * 2.0 >= TRACK_MIN;

    if show_clocks {
        let write = |x: f32, text: &str| {
            let galley = truncated(
                ui.painter(),
                text,
                t.fonts.caption.clone(),
                t.text.secondary,
                // Without the gap `clock_w` reserves, which is beside the text and not for it.
                clock_w - PAD,
            );
            galley_on_baseline(ui.painter(), x, baseline, galley);
        };
        write(left, &elapsed);
        if !total.is_empty() {
            write(right - clock_w + PAD, &total);
        }
        left += clock_w;
        right -= clock_w;
    }

    // And the scrubber in what is left of the middle. Nothing to scrub for a stream — see
    // [`crate::preview::Player::duration`] — so it is a bare track then, which is honest: the
    // picture is playing and there is no position in it to point at.
    if right - left >= TRACK_MIN * 0.5 {
        scrubber(
            ui,
            t,
            Rect::from_min_max(pos2(left, bar.top()), pos2(right, bar.bottom())),
            pane,
            player,
        );
    }
}

/// The track, the part already played, the knob — and the drag that moves it.
///
/// **The seek happens when the drag ends, not while it runs.** A drag across a four-hundred-point
/// panel is four hundred pointer positions, and asking the engine for each of them is four hundred
/// decodes of frames nobody will see; what moves during the drag is
/// [`crate::preview::Player::scrub_to`], which only changes what this draws. A *click* on the track
/// seeks at once, because there is one position in a click.
fn scrubber(ui: &mut Ui, t: &Theme, band: Rect, pane: PaneId, player: &mut preview::Player) {
    let response = ui.interact(
        band,
        Id::new(("preview-scrub", pane)),
        Sense::click_and_drag(),
    );
    let duration = player.duration();
    // Inset by the knob's radius at both ends, so the knob at 0% and at 100% is inside the track
    // rather than hanging off it — which is also what makes the mapping below reversible.
    let run = Rect::from_min_max(
        pos2(band.left() + KNOB, band.center().y - TRACK * 0.5),
        pos2(band.right() - KNOB, band.center().y + TRACK * 0.5),
    );
    let radius = CornerRadius::same((TRACK * 0.5) as u8);
    ui.painter().rect_filled(run, radius, t.gauge_track);

    if let Some(duration) = duration {
        let seconds_at = |x: f32| {
            let across = ((x - run.left()) / run.width().max(1.0)).clamp(0.0, 1.0);
            across as f64 * duration
        };
        if response.dragged() {
            if let Some(at) = response.interact_pointer_pos() {
                player.scrub_to(seconds_at(at.x));
            }
        }
        if response.drag_stopped() {
            player.scrub_done();
        }
        // A click is its own gesture and not the end of a drag: egui reports `clicked` for a press
        // and release that did not move, and `scrub_done` would have nothing to commit.
        if response.clicked() {
            if let Some(at) = response.interact_pointer_pos() {
                player.seek(seconds_at(at.x));
            }
        }

        let across = (player.at() / duration).clamp(0.0, 1.0) as f32;
        let x = run.left() + run.width() * across;
        if x > run.left() {
            ui.painter().rect_filled(
                Rect::from_min_max(run.min, pos2(x, run.bottom())),
                radius,
                t.accent.mark,
            );
        }
        // The knob last, over both, so it is not half-covered by the fill it terminates.
        ui.painter()
            .circle_filled(pos2(x, run.center().y), KNOB, t.accent.mark);
    }

    if response.hovered() && duration.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
}
