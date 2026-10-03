//! The panel a copy or move shows while [`crate::shell::ops::fast`] is doing it: how far it has got,
//! pause and cancel, the question when a file is already there, and what failed.
//!
//! The shell's own copy draws a dialog of its own, and with **Fast copy** off this is never drawn.
//! With it on, this is the dialog. It sits in the window's bottom-right corner rather than over the
//! middle, and nothing behind it is blocked: a copy is something that happens while you work.
//!
//! **Nothing is changed from here.** Every button pushes an [`Action::Steer`] and the job hears
//! about it after the frame, the same as anything else drawn — see [`crate::app::Action`]. The one
//! piece of state of its own is the tick on *Do this for every conflict*, which is a widget's and
//! lives in egui's memory beside the question it belongs to.

use std::time::Duration;

use azur_egui_theme::components::{Button, Checkbox, ProgressBar, ProgressColor, Size, Variant};
use azur_egui_theme::tokens::{control, radius, shadow, space};
use egui::{vec2, Align, Align2, CornerRadius, Frame, Id, Layout, Margin, Order, RichText, Stroke};

use crate::app::Action;
use crate::shell::ops::fast::{
    Choice, Clash, Confirm, Kind, Mend, Phase, Room, Short, Snag, Snapshot, Steer, Trouble,
};
use crate::theme::Theme;

/// How long a job has run before its panel appears.
///
/// A move inside one volume is a rename, and finishes long before this; a panel that came and went
/// within a frame would say nothing but that something had happened. Anything with a question or a
/// failure in it shows at once, because then there is something to say.
pub(crate) const DELAY: Duration = Duration::from_millis(400);

const WIDTH: f32 = 380.0;

/// How many failures are named in the panel. The rest are counted.
const NAMED: usize = 5;

/// What to do about a window being closed while a copy is still running. See
/// [`crate::app::App::mind_the_close`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Leave {
    /// Cancel the copies, and close once each has cleaned up after itself.
    StopAndClose,
    /// Let them finish, then close.
    WhenDone,
    /// Never mind.
    Stay,
}

/// Draw a panel for every transfer worth one, and the question about closing above them when the
/// window has been asked to close. Returns whether any is still running, so the window can keep
/// booking frames for the figures to move.
pub fn show(
    ctx: &egui::Context,
    t: &Theme,
    transfers: &[Snapshot],
    closing: bool,
    out: &mut Vec<Action>,
) -> bool {
    let shown: Vec<&Snapshot> = transfers
        .iter()
        .filter(|s| match s.phase {
            Phase::Starting | Phase::Shell => false,
            Phase::Running => {
                s.running_for >= DELAY
                    || s.question.is_some()
                    || s.short.is_some()
                    || s.snag.is_some()
                    || s.confirm.is_some()
                    || s.failed > 0
                    || closing
            }
            Phase::Finished => s.failed > 0,
        })
        .collect();
    let running = transfers
        .iter()
        .any(|s| matches!(s.phase, Phase::Starting | Phase::Running));
    if shown.is_empty() && !closing {
        return running;
    }
    // Above the status line of a pane in the bottom row, rather than over it.
    let lift = crate::ui::filelist::STATUS_HEIGHT + space::S3;
    egui::Area::new(Id::new("transfers"))
        .order(Order::Foreground)
        .anchor(Align2::RIGHT_BOTTOM, vec2(-space::S3, -lift))
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = space::S2;
            if closing {
                leaving(ui, t, out);
            }
            for snapshot in shown {
                panel(ui, t, snapshot, out);
            }
        });
    running
}

/// A row of buttons against the right edge.
///
/// **Exactly one button high**, and that is not tidiness. `with_layout(right_to_left(Center))` on
/// its own takes the whole height it is offered, and an `Area` offers what the last frame measured
/// — so a row centred in it grew the panel by its own height every frame, and the panel climbed
/// the window and took its buttons out from under the pointer before a click could land on them.
fn buttons(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), control::SMALL),
        Layout::right_to_left(Align::Center),
        add,
    );
}

/// How far above the component's own placement a tick's label is drawn. See [`tick`].
const TICK_LIFT: f32 = 2.0;

/// A tick box and its label, remembered per transfer for as long as egui keeps the widget's memory.
///
/// **The label is drawn here rather than by `Checkbox`**, which centres it on its line box — and a
/// line box keeps room under the text for descenders a sentence like this hardly has, so the words
/// sat low against the box. Lifted by [`TICK_LIFT`] in this panel only: the component is the design
/// system's, shared with every other window, and the place to correct it for all of them is there.
/// The words still toggle the box, as the component's own label does.
fn tick(ui: &mut egui::Ui, t: &Theme, id: Id, label: &str) -> bool {
    use egui::emath::GuiRounding as _;
    let mut on = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let was = on;
    // The box takes its own row, exactly as tall as the component makes it; the words are painted
    // beside it without taking room of their own. A `horizontal` row around the two came out a
    // control's height tall rather than a line's, and grew the card by the difference.
    let boxed = ui.add(Checkbox::new(&mut on, "").size(Size::Small));
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), t.fonts.body.clone(), t.text.primary);
    // The component's own gap between the box and its label.
    let words = egui::Rect::from_min_size(
        egui::pos2(boxed.rect.right() + space::S3, boxed.rect.top()),
        vec2(galley.size().x, boxed.rect.height()),
    );
    if ui.interact(words, id.with("words"), egui::Sense::click()).clicked() {
        on = !on;
    }
    let ppp = ui.painter().pixels_per_point();
    let at = egui::pos2(
        words.left().round_to_pixels(ppp),
        (words.center().y - galley.size().y * 0.5 - TICK_LIFT).round_to_pixels(ppp),
    );
    ui.painter().galley(at, galley, t.text.primary);
    if on != was {
        ui.data_mut(|d| d.insert_temp(id, on));
    }
    on
}

/// The frame every card here is drawn in.
fn card<R>(ui: &mut egui::Ui, t: &Theme, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    Frame::new()
        .fill(t.bg.layer)
        .stroke(Stroke::new(1.0, t.stroke.subtle))
        .corner_radius(CornerRadius::same(radius::LARGE))
        .shadow(shadow::S16)
        .inner_margin(Margin::same(space::S4 as i8))
        .show(ui, |ui| {
            ui.set_width(WIDTH - 2.0 * space::S4);
            ui.spacing_mut().item_spacing.y = space::S2;
            add(ui)
        })
        .inner
}

/// Shift+Delete, before anything is touched: the shell's own *Are you sure you want to permanently
/// delete…*, which this engine has to ask for itself. Enter is Delete and Escape is Cancel, as they
/// are on the shell's dialog — see [`crate::app::App::keyboard`].
fn asking_to_delete(
    ui: &mut egui::Ui,
    t: &Theme,
    s: &Snapshot,
    confirm: &Confirm,
    out: &mut Vec<Action>,
) {
    card(ui, t, |ui| {
        let what = match &confirm.name {
            Some(name) => format!("Permanently delete {name}?"),
            None => format!("Permanently delete these {} items?", confirm.count),
        };
        ui.label(
            RichText::new(what)
                .font(t.fonts.body_strong.clone())
                .color(t.text.primary),
        );
        let said = if confirm.folders {
            "They and everything in them will not go to the Recycle Bin, and cannot be put back."
        } else {
            "They will not go to the Recycle Bin, and cannot be put back."
        };
        ui.label(
            RichText::new(said)
                .font(t.fonts.caption.clone())
                .color(t.text.secondary),
        );
        ui.add_space(space::S1);
        buttons(ui, |ui| {
            ui.spacing_mut().item_spacing.x = space::S2;
            for (label, yes, variant) in [
                ("Cancel", false, Variant::Secondary),
                ("Delete", true, Variant::Danger),
            ] {
                if ui
                    .add(Button::new(label).size(Size::Small).variant(variant))
                    .clicked()
                {
                    out.push(Action::Steer {
                        transfer: s.id,
                        steer: Steer::Confirm(yes),
                    });
                }
            }
        });
    });
}

/// The window was asked to close with a copy still going. Closing it there and then would kill the
/// copy mid-file and leave a half-written file under its real name, so it asks first.
fn leaving(ui: &mut egui::Ui, t: &Theme, out: &mut Vec<Action>) {
    card(ui, t, |ui| {
        ui.label(
            RichText::new("A copy, move or delete is still running")
                .font(t.fonts.body_strong.clone())
                .color(t.text.primary),
        );
        ui.label(
            RichText::new("Closing now would stop it part way through.")
                .font(t.fonts.caption.clone())
                .color(t.text.secondary),
        );
        ui.add_space(space::S1);
        buttons(ui, |ui| {
            ui.spacing_mut().item_spacing.x = space::S2;
            for (label, leave, variant) in [
                ("Keep window", Leave::Stay, Variant::Subtle),
                ("Stop and close", Leave::StopAndClose, Variant::Secondary),
                ("Close when done", Leave::WhenDone, Variant::Primary),
            ] {
                if ui
                    .add(Button::new(label).size(Size::Small).variant(variant))
                    .clicked()
                {
                    out.push(Action::Leave(leave));
                }
            }
        });
    });
}

fn panel(ui: &mut egui::Ui, t: &Theme, s: &Snapshot, out: &mut Vec<Action>) {
    // A permanent delete not yet confirmed has nothing to show but the question: nothing has been
    // counted, and nothing will be until the answer is yes.
    if let Some(confirm) = &s.confirm {
        return asking_to_delete(ui, t, s, confirm, out);
    }
    card(ui, t, |ui| {
        let steer = |steer| Action::Steer {
            transfer: s.id,
            steer,
        };

        ui.label(
            RichText::new(headline(s))
                .font(t.fonts.body_strong.clone())
                .color(t.text.primary),
        );
        let color = if s.failed > 0 {
            ProgressColor::Danger
        } else if s.paused {
            ProgressColor::Warning
        } else {
            ProgressColor::Accent
        };
        let bar = if s.files_found == 0 {
            ProgressBar::indeterminate()
        } else {
            ProgressBar::new(s.fraction())
        };
        ui.add(bar.color(color).small(true));
        ui.label(
            RichText::new(figures(s))
                .font(t.fonts.caption.clone())
                .color(t.text.secondary),
        );
        if s.phase == Phase::Running && !s.current.is_empty() {
            ui.add(
                egui::Label::new(
                    RichText::new(&s.current)
                        .font(t.fonts.caption.clone())
                        .color(t.text.tertiary),
                )
                .truncate(),
            );
        }

        if let Some(short) = &s.short {
            no_room(ui, t, s.id, short, out);
        }
        if let Some(snag) = &s.snag {
            stuck(ui, t, s.id, &s.into, snag, out);
        }
        if let Some(clash) = &s.question {
            question(ui, t, s.id, clash, out);
        }

        if s.failed > 0 {
            for failure in s.failures.iter().take(NAMED) {
                let line = format!("{} — {}", crate::fs::display_name(&failure.path), failure.why);
                ui.add(
                    egui::Label::new(
                        RichText::new(line)
                            .font(t.fonts.caption.clone())
                            .color(t.status.danger),
                    )
                    .truncate(),
                );
            }
            let named = s.failures.len().min(NAMED) as u64;
            if s.failed > named {
                ui.label(
                    RichText::new(format!("and {} more", s.failed - named))
                        .font(t.fonts.caption.clone())
                        .color(t.text.secondary),
                );
            }
        }

        ui.add_space(space::S1);
        buttons(ui, |ui| {
            ui.spacing_mut().item_spacing.x = space::S2;
            if s.phase == Phase::Finished {
                if ui.add(Button::new("Close").size(Size::Small)).clicked() {
                    out.push(steer(Steer::Dismiss));
                }
                return;
            }
            if ui
                .add(Button::new("Cancel").size(Size::Small).variant(Variant::Subtle))
                .clicked()
            {
                out.push(steer(Steer::Cancel));
            }
            // No pause while a question is up: the job is already waiting, on the question.
            if s.question.is_none() && s.short.is_none() && !s.cancelled {
                let label = if s.paused { "Resume" } else { "Pause" };
                if ui
                    .add(Button::new(label).size(Size::Small).variant(Variant::Subtle))
                    .clicked()
                {
                    out.push(steer(Steer::Pause(!s.paused)));
                }
            }
        });
    });
}

/// Not enough room where the copy is going: how much short, and whether to look again or go on.
fn no_room(ui: &mut egui::Ui, t: &Theme, id: u64, short: &Short, out: &mut Vec<Action>) {
    ui.add_space(space::S1);
    ui.label(
        RichText::new(format!(
            "There is not enough room on {}",
            crate::fs::display_name(&short.into)
        ))
        .font(t.fonts.body.clone())
        .color(t.text.primary),
    );
    let mut line = String::from("Needs ");
    crate::fs::fmt::size(short.needed, &mut line);
    line.push_str(" so far, and ");
    crate::fs::fmt::size(short.free, &mut line);
    line.push_str(" is free");
    ui.label(
        RichText::new(line)
            .font(t.fonts.caption.clone())
            .color(t.text.secondary),
    );
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S2;
        for (label, room, variant) in [
            ("Try again", Room::TryAgain, Variant::Primary),
            ("Copy anyway", Room::Anyway, Variant::Secondary),
        ] {
            if ui
                .add(Button::new(label).size(Size::Small).variant(variant))
                .clicked()
            {
                out.push(Action::Steer {
                    transfer: id,
                    steer: Steer::Room(room),
                });
            }
        }
    });
}

/// A file that cannot be got at: open in another program, or encrypted where the destination cannot
/// keep it so. Asked rather than failed, as the shell asks — see [`Trouble`].
fn stuck(
    ui: &mut egui::Ui,
    t: &Theme,
    id: u64,
    into: &std::path::Path,
    snag: &Snag,
    out: &mut Vec<Action>,
) {
    ui.add_space(space::S1);
    let name = crate::fs::display_name(&snag.path);
    let (said, fix) = match snag.trouble {
        Trouble::InUse => (
            format!("{name} is open in another program"),
            ("Try again", Mend::TryAgain),
        ),
        Trouble::Encrypted => (
            format!(
                "{name} is encrypted, and {} cannot keep it encrypted",
                crate::fs::display_name(into)
            ),
            ("Copy without encryption", Mend::Decrypt),
        ),
    };
    ui.label(
        RichText::new(said)
            .font(t.fonts.body.clone())
            .color(t.text.primary),
    );
    ui.add(
        egui::Label::new(
            RichText::new(&snag.why)
                .font(t.fonts.caption.clone())
                .color(t.text.secondary),
        )
        .truncate(),
    );
    // Its own tick, apart from the conflict's: the two are different questions and a standing
    // answer to one is no answer to the other.
    let every = tick(
        ui,
        t,
        Id::new(("transfer-snag-every", id)),
        "Do this for every file like it",
    );
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S2;
        for (label, mend, variant) in [
            (fix.0, fix.1, Variant::Primary),
            ("Skip", Mend::Skip, Variant::Secondary),
        ] {
            if ui
                .add(Button::new(label).size(Size::Small).variant(variant))
                .clicked()
            {
                out.push(Action::Steer {
                    transfer: id,
                    steer: Steer::Mend { mend, every },
                });
            }
        }
    });
}

/// The conflict: which file, the two of them side by side, and the three answers.
fn question(ui: &mut egui::Ui, t: &Theme, id: u64, clash: &Clash, out: &mut Vec<Action>) {
    ui.add_space(space::S1);
    ui.label(
        RichText::new(format!(
            "{} is already in {}",
            clash.name,
            crate::fs::display_name(&clash.into)
        ))
        .font(t.fonts.body.clone())
        .color(t.text.primary),
    );
    let zone = crate::fs::time::LocalZone::current();
    for (which, facts) in [("This one", clash.incoming), ("The one there", clash.existing)] {
        let mut line = format!("{which}: ");
        crate::fs::fmt::size(facts.size, &mut line);
        if facts.modified != 0 {
            line.push_str(", ");
            crate::fs::fmt::modified(facts.modified, &zone, &mut line);
        }
        ui.label(
            RichText::new(line)
                .font(t.fonts.caption.clone())
                .color(t.text.secondary),
        );
    }
    let every = tick(
        ui,
        t,
        Id::new(("transfer-every", id)),
        "Do this for every conflict",
    );
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S2;
        for (label, choice, variant) in [
            ("Replace", Choice::Replace, Variant::Primary),
            ("Keep both", Choice::KeepBoth, Variant::Secondary),
            ("Skip", Choice::Skip, Variant::Secondary),
        ] {
            if ui
                .add(Button::new(label).size(Size::Small).variant(variant))
                .clicked()
            {
                out.push(Action::Steer {
                    transfer: id,
                    steer: Steer::Answer { choice, every },
                });
            }
        }
    });
}

/// `Copying 1,204 files to Photos`, and the past tense once it has stopped.
fn headline(s: &Snapshot) -> String {
    let into = crate::fs::display_name(&s.into);
    let files = match s.files_found {
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    };
    // A delete is *from* the folder its items were in; a copy and a move are *to* one.
    let (doing, done, towards) = match s.kind {
        Kind::Copy => ("Copying", "Copied", "to"),
        Kind::Move => ("Moving", "Moved", "to"),
        Kind::Delete => ("Deleting", "Deleted", "from"),
    };
    if s.phase == Phase::Finished {
        let failed = match s.failed {
            1 => "1 failed".to_owned(),
            n => format!("{n} failed"),
        };
        let stopped = if s.cancelled { ", then stopped" } else { "" };
        return format!("{done} {towards} {into}{stopped} — {failed}");
    }
    let verb = if s.cancelled {
        "Stopping"
    } else if s.paused {
        "Paused"
    } else {
        doing
    };
    if s.files_found == 0 {
        return format!("{verb} {towards} {into}…");
    }
    let so_far = if s.counted { "" } else { " so far" };
    format!("{verb} {files}{so_far} {towards} {into}")
}

/// `1.42 GB of 4.80 GB · 212 MB/s · about 16 s left`.
fn figures(s: &Snapshot) -> String {
    let mut line = String::new();
    if s.bytes_found > 0 {
        crate::fs::fmt::size(s.bytes_done.min(s.bytes_found), &mut line);
        line.push_str(" of ");
        crate::fs::fmt::size(s.bytes_found, &mut line);
    } else {
        line.push_str(&format!("{} of {}", s.files_done, s.files_found));
    }
    if s.phase == Phase::Running && !s.paused {
        if let Some(rate) = s.rate() {
            line.push_str(" · ");
            crate::fs::fmt::size(rate as u64, &mut line);
            line.push_str("/s");
        }
        if let Some(left) = s.remaining() {
            line.push_str(" · ");
            line.push_str(&about(left));
        }
    }
    line
}

/// A time left, rounded to what is worth reading: seconds under a minute, minutes under an hour.
fn about(left: Duration) -> String {
    let secs = left.as_secs();
    if secs < 5 {
        "a few seconds left".to_owned()
    } else if secs < 60 {
        format!("about {} s left", secs.div_ceil(5) * 5)
    } else if secs < 3600 {
        format!("about {} min left", secs.div_ceil(60))
    } else {
        format!("about {} h {} min left", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_is_rounded_to_what_is_worth_reading() {
        assert_eq!(about(Duration::from_secs(2)), "a few seconds left");
        assert_eq!(about(Duration::from_secs(41)), "about 45 s left");
        assert_eq!(about(Duration::from_secs(61)), "about 2 min left");
        assert_eq!(about(Duration::from_secs(3 * 3600 + 120)), "about 3 h 2 min left");
    }
}
