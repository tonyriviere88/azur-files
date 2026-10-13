//! The batch rename panel: the rule along the top, and every item's old and new name under it.
//!
//! It takes the listing's place while it is open — the items being renamed and nothing else,
//! because the question on screen is "what will these become", and a listing of everything else
//! in the folder would bury the answer. The rule and the names are [`crate::pane::batch`]'s; this
//! only edits the one and paints the other.
//!
//! **Nothing is renamed until Rename is pressed, and Rename cannot be pressed while any row has a
//! problem.** A conflict is shown on its row, in the danger ink, with the reason on hover — so the
//! shell's own conflict dialog, which would arrive halfway through a batch, never has to.
//!
//! `Enter` renames and `Escape` closes, from any of the fields. The rows are painted at computed
//! rects like the listing's, and only the ones on screen.

use azur_egui_theme::components::{Button, Checkbox, NumberField, Size, TextField, Variant};

use super::*;
use crate::pane::batch::{BatchRename, CaseTo, Mode, Plan, TOKENS};

/// One line of controls: a small control and the air around it.
const LINE: f32 = 32.0;

/// A small control's height — `Size::Small` — which is the slot each field is given on its line.
const FIELD: f32 = 24.0;

/// The panel, over `rect` — the listing's region, header included.
pub(super) fn show(
    ui: &mut Ui,
    t: &Theme,
    zone: &LocalZone,
    rect: Rect,
    pane: PaneId,
    tab: &mut Tab,
    out: &mut Vec<Action>,
) {
    let Some(dir) = tab.dir.clone() else { return };
    let Some(batch) = tab.batch.as_deref_mut() else { return };

    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), LINE * 2.0 + space::S2));
    ui.painter()
        .rect_filled(bar, CornerRadius::ZERO, crate::ui::bar(t));
    ui.painter().line_segment(
        [bar.left_bottom(), bar.right_bottom()],
        Stroke::new(1.0, crate::ui::seam(t)),
    );
    let inner = bar.shrink2(vec2(space::S3, space::S1));
    let first = Rect::from_min_size(inner.min, vec2(inner.width(), LINE));
    let second = Rect::from_min_size(
        pos2(inner.left(), first.bottom()),
        vec2(inner.width(), LINE),
    );

    // ---- The first line: what is being renamed, how, and the words -----------
    let mut line = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(first)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    line.spacing_mut().item_spacing.x = space::S2;
    line.label(
        egui::RichText::new(format!("Rename {} items", batch.items.len()))
            .font(t.fonts.body_strong.clone())
            .color(t.text.primary),
    );
    line.add_space(space::S3);
    // A click is an action, not a write here: the app switches the panel and remembers the mode as
    // the one to open in next time, the moment it is clicked.
    let fresh = std::mem::take(&mut batch.fresh);
    let rule = &mut batch.rule;
    for mode in Mode::ALL {
        if segment(&mut line, mode.label(), rule.mode == mode).clicked() && rule.mode != mode {
            out.push(Action::SetBatchMode { pane, mode });
        }
    }
    line.add_space(space::S3);
    match rule.mode {
        Mode::Replace => {
            let find = field(&mut line, &mut rule.find, "Find", 200.0);
            if fresh {
                find.request_focus();
            }
            field(&mut line, &mut rule.replace, "Replace with", 200.0);
        }
        Mode::Pattern => {
            let pattern = field(&mut line, &mut rule.pattern, "{name}_{n:03}", 260.0);
            if fresh {
                pattern.request_focus();
            }
            line.label(caption(t, "Start"));
            line.add_sized(
                vec2(72.0, FIELD),
                NumberField::new(&mut rule.start).decimals(0).size(Size::Small),
            );
            line.label(caption(t, "Step"));
            line.add_sized(
                vec2(72.0, FIELD),
                NumberField::new(&mut rule.step).decimals(0).size(Size::Small),
            );
        }
        Mode::Case => {
            for (to, label) in [
                (CaseTo::Lower, "lower case"),
                (CaseTo::Upper, "UPPER CASE"),
                (CaseTo::Title, "Title Case"),
            ] {
                if segment(&mut line, label, rule.case_to == to).clicked() {
                    rule.case_to = to;
                }
            }
        }
    }

    // ---- The second line: the options, then what it comes to and the two buttons -----
    let mut options = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(second)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    options.spacing_mut().item_spacing.x = space::S3;
    match rule.mode {
        Mode::Replace => {
            options.add(Checkbox::new(&mut rule.match_case, "Match case").size(Size::Small));
            options.add(Checkbox::new(&mut rule.regex, "Regex").size(Size::Small));
            options.add(Checkbox::new(&mut rule.extension, "Include extension").size(Size::Small));
        }
        Mode::Pattern => {
            options.spacing_mut().item_spacing.x = space::S1;
            for (token, what) in TOKENS {
                let chip = options
                    .add(Button::new(token).variant(Variant::Subtle).size(Size::Small))
                    .on_hover_text(what);
                if chip.clicked() {
                    rule.pattern.push_str(token);
                }
            }
        }
        Mode::Case => {
            options.add(
                Checkbox::new(&mut rule.case_extension, "Lower-case the extension")
                    .size(Size::Small),
            );
        }
    }

    // The plan, after every control has had its say this frame, so what is painted is the rule as
    // it now stands. Worked out only when the rule or the listing changed — see `replan`.
    let batch: &BatchRename = batch.replan(&dir, zone);
    let plan = batch.preview();
    let ready = plan.ready();

    let mut actions = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(second)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    actions.spacing_mut().item_spacing.x = space::S2;
    let rename = actions.add_enabled(
        ready,
        Button::new("Rename").variant(Variant::Primary).size(Size::Small),
    );
    if rename.clicked() {
        out.push(Action::BatchApply(pane));
    }
    if actions
        .add(Button::new("Cancel").size(Size::Small))
        .clicked()
    {
        out.push(Action::BatchCancel(pane));
    }
    let (summary, ink) = summary(plan, t);
    actions.label(egui::RichText::new(summary).font(t.fonts.caption.clone()).color(ink));

    // `Enter` renames and `Escape` closes, wherever the keyboard is in the panel. Read rather than
    // consumed: a field has already had `Enter` by now, and taking the focus away is all it does
    // with it.
    let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    if escape {
        out.push(Action::BatchCancel(pane));
    } else if enter && ready {
        out.push(Action::BatchApply(pane));
    }

    // ---- The names ------------------------------------------------------------
    let header = Rect::from_min_size(
        pos2(rect.left(), bar.bottom()),
        vec2(rect.width(), HEADER_HEIGHT),
    );
    ui.painter()
        .rect_filled(header, CornerRadius::ZERO, t.surfaces.header);
    let split = rect.left() + rect.width() * 0.5;
    let columns = |row: Rect| {
        (
            Rect::from_min_max(
                pos2(row.left() + CELL_PAD, row.top()),
                pos2(split - CELL_PAD, row.bottom()),
            ),
            Rect::from_min_max(
                pos2(split + CELL_PAD, row.top()),
                pos2(row.right() - CELL_PAD, row.bottom()),
            ),
        )
    };
    let (old_head, new_head) = columns(header);
    for (cell, label) in [(old_head, "Name"), (new_head, "New name")] {
        let galley = truncated(
            ui.painter(),
            label,
            t.fonts.body_strong.clone(),
            t.text.secondary,
            cell.width(),
        );
        text_left(ui.painter(), cell, galley);
    }

    let body = Rect::from_min_max(pos2(rect.left(), header.bottom()), rect.max);
    if body.height() <= 0.0 {
        return;
    }
    let mut list = ui.new_child(egui::UiBuilder::new().max_rect(body));
    list.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .id_salt(("batch-rows", pane))
        .auto_shrink([false, false])
        .show_rows(&mut list, ROW_HEIGHT, batch.items.len(), |ui, range| {
            for i in range {
                let (row, response) = ui.allocate_exact_size(
                    vec2(ui.available_width(), ROW_HEIGHT),
                    Sense::hover(),
                );
                let under = row_fill(t, false, response.hovered()).unwrap_or(t.bg.layer);
                if let Some(fill) = row_fill(t, false, response.hovered()) {
                    ui.painter().rect_filled(row, CornerRadius::ZERO, fill);
                }
                let item = &batch.items[i];
                let planned = &plan.rows[i];
                let (old, new) = columns(row);
                let galley = truncated(
                    ui.painter(),
                    &item.name,
                    t.fonts.body.clone(),
                    t.text.secondary,
                    old.width(),
                );
                text_left(ui.painter(), old, galley);

                // The new name, with what changed in it marked — the stretch between what it keeps
                // of the old name's start and of its end, which is where the eye has to go.
                let ink = if planned.problem.is_some() {
                    t.status.danger
                } else if planned.changes {
                    t.text.primary
                } else {
                    t.text.tertiary
                };
                let hits = changed_span(&item.leaf, &planned.new)
                    .filter(|_| planned.changes && planned.problem.is_none());
                // The new name alone: in a flattened listing the Name column already says which
                // folder it is in, and the file stays there.
                let mark = if planned.problem.is_some() { MARK + space::S2 } else { 0.0 };
                let galley = name_galley(
                    ui.painter(),
                    &planned.new,
                    0,
                    None,
                    None,
                    t.fonts.body.clone(),
                    ink,
                    t.text.tertiary,
                    (new.width() - mark).max(0.0),
                    Marked { hits: hits.as_slice(), fill: hit_fill(t, under), ink },
                );
                let width = galley.size().x;
                text_left(ui.painter(), new, galley);
                if let Some(problem) = &planned.problem {
                    let at = icon_rect(row, (new.left() + width + space::S2).min(new.right() - MARK), MARK);
                    azur_icons::warning(ui.painter(), at, t.status.danger);
                    if response.hovered() {
                        azur_egui_theme::components::tooltip(response.clone(), problem.sentence());
                    }
                }
            }
        });
}

/// The size of the warning mark on a row with a problem.
const MARK: f32 = 14.0;

/// One button of a set of which one is chosen — the modes, and the three cases.
fn segment(ui: &mut Ui, label: &str, chosen: bool) -> egui::Response {
    ui.add(
        Button::new(label)
            .variant(if chosen { Variant::Secondary } else { Variant::Subtle })
            .size(Size::Small),
    )
}

/// A small text field on a line of controls.
///
/// **Sized, not added.** A `TextField` lays itself out in a `ui.vertical`, which in a centred row
/// takes the row's whole height and stacks from its top — so a plain `add` puts the 24-point box at
/// the top of the 32-point line. A slot of exactly the field's size is what the row centres, which
/// is how the number fields beside them already sit.
fn field(ui: &mut Ui, text: &mut String, placeholder: &str, width: f32) -> egui::Response {
    ui.add_sized(
        vec2(width, FIELD),
        TextField::new(text)
            .placeholder(placeholder)
            .size(Size::Small)
            .width(width),
    )
}

fn caption(t: &Theme, text: &str) -> egui::RichText {
    egui::RichText::new(text)
        .font(t.fonts.caption.clone())
        .color(t.text.secondary)
}

/// What the rule comes to, in a few words, and the ink to say it in.
fn summary(plan: &Plan, t: &Theme) -> (String, Color32) {
    if let Some(error) = &plan.error {
        return (error.clone(), t.status.danger);
    }
    let problems = plan.problems();
    let changing = plan.changing();
    if problems > 0 {
        return (
            format!(
                "{problems} problem{} — hover a row to see why",
                plural(problems as u32)
            ),
            t.status.danger,
        );
    }
    if changing == 0 {
        return ("Nothing would change".to_owned(), t.text.tertiary);
    }
    (
        format!("{changing} of {} will be renamed", plan.rows.len()),
        t.text.secondary,
    )
}

/// The part of `new` that differs from `old`: what is left between the longest start and the
/// longest end the two share. Byte offsets into `new`, on `char` boundaries. `None` when nothing
/// is left — a name made shorter only, whose change is a deletion and has no letters to mark.
fn changed_span(old: &str, new: &str) -> Option<std::ops::Range<usize>> {
    let start = old
        .char_indices()
        .zip(new.char_indices())
        .find(|((_, a), (_, b))| a != b)
        .map_or(old.len().min(new.len()), |((_, _), (at, _))| at);
    let tail = old
        .chars()
        .rev()
        .zip(new.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(_, b)| b.len_utf8())
        .sum::<usize>();
    let end = new.len().saturating_sub(tail).max(start);
    (end > start).then_some(start..end)
}

#[cfg(test)]
mod tests {
    use super::changed_span;

    #[test]
    fn the_changed_part_is_what_the_old_name_does_not_share() {
        let span = |old: &str, new: &str| changed_span(old, new).map(|r| new[r].to_owned());
        assert_eq!(span("IMG_0412.jpg", "Holiday_0412.jpg").as_deref(), Some("Holiday"));
        assert_eq!(span("a.jpg", "a_001.jpg").as_deref(), Some("_001"));
        assert_eq!(span("photo.JPG", "photo.jpg").as_deref(), Some("jpg"));
        // Only shorter: nothing to mark.
        assert_eq!(span("draft_v2.txt", "draft.txt"), None);
        assert_eq!(span("same", "same"), None);
        // Beyond ASCII, on character boundaries.
        assert_eq!(span("été.txt", "hiver.txt").as_deref(), Some("hiver"));
    }
}
