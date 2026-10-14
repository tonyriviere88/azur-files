//! The command palette: a field, and the list of everything the window can do that it narrows.
//! `Ctrl+Shift+P`.
//!
//! **What is on the list is the application's** — see `crate::app::palette`, which builds the rows
//! and turns the one picked into actions. This half draws them, reads the keys that walk them, and
//! says which was chosen; it knows nothing about what a row does.
//!
//! # Matching
//!
//! Not a fuzzy matcher. The field speaks the filter box's language — every word has to appear, in
//! any order, `!` excludes, `^` and `$` hold an end — because that is `azur_egui_theme::filter`, and
//! a second idea of what typing into a box means would be a second thing to learn. The narrowing is
//! done by the application half, against each row's label *and* its heading, so `view` finds the
//! whole View group.
//!
//! # Keys
//!
//! Read and consumed above the field, as the path field's dropdown does in
//! [`crate::ui::breadcrumb`], because every one of them already means something to a `TextEdit`:
//! `Up` and `Down` walk the rows, `PageUp` and `PageDown` a screenful, `Enter` runs the highlighted
//! one, `Alt+Enter` runs it in a new tab where that means anything, `Escape` closes. **There is always
//! a highlighted row** while there are any — the first, until something moves it — so typing two
//! words and `Enter` is the whole gesture.

use azur_egui_theme::components::{
    menu_divider, menu_frame, menu_header, menu_header_height, menu_item_height, menu_item_metrics,
    MenuItem, Size, Text, TextColor, TextField,
};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::space;
use egui::{pos2, vec2, CornerRadius, Id, Order, Rect, Sense};

use crate::theme::Theme;

/// How many rows show before the list scrolls. A whole number of them, for the reason
/// [`crate::ui::breadcrumb`]'s `OFFERS_SHOWN` gives: a list walked with the arrow keys should end
/// where a row ends.
const ROWS_SHOWN: usize = 12;

/// How wide the palette is, unless the window is narrower.
const WIDTH: f32 = 520.0;

/// How far below the top of the window it hangs: under the title bar, where the eye already is.
const DROP: f32 = 48.0;

/// The palette's shadow: halfway, **as measured on screen**, between a menu's `shadow-16` and a
/// dialog's `shadow-64`. It takes the keyboard and sits over the whole window as a dialog does, and
/// `shadow-16` left it reading as part of the listing under it; the full `shadow-64` was too heavy for
/// something this size.
///
/// Measured off a `--shot` of the dark palette, under the palette's bottom edge: `shadow-16` darkens
/// the canvas by 4 levels at the edge and is gone in 8 points, `shadow-64` by 11 and gone in 48, and
/// this by 7 and gone in about 30 — on every side. Not the tokens' midpoint as numbers (a 40-point
/// blur at alpha 132), because without the offset the same shadow is spread over four sides instead
/// of one, and that came out weaker below than `shadow-16` had been.
///
/// **No offset, unlike every token**, so it falls on all four sides. The tokens drop their shadow
/// downwards, as for something lifted off the page under a light from above; this hangs from the top
/// of the window over the listing, and a shadow only below and to the sides left its top edge and
/// its left edge flat against what is behind it.
const SHADOW: egui::Shadow = egui::Shadow {
    offset: [0, 0],
    blur: 48,
    spread: 6,
    color: egui::Color32::from_black_alpha(150),
};

/// One row of the list.
#[derive(Clone)]
pub struct Row {
    pub label: String,
    /// The heading the row is listed under. A heading is drawn wherever it changes, so rows that
    /// share one have to be next to each other — which is the application half's job.
    pub heading: &'static str,
    /// Its shortcut, printed in the menu's own shortcut column. Empty for none.
    pub shortcut: &'static str,
    /// A tick, for a switch that is on.
    pub checked: bool,
    /// The danger ink, for the two entries that cannot be undone from here.
    pub danger: bool,
    pub icon: Option<azur_icons::Icon<'static>>,
}

/// The palette while it is on screen.
pub struct State {
    /// What has been typed.
    pub text: String,
    /// The highlighted row, as an index into the rows handed to [`show`].
    pub hot: usize,
    /// The highlight moved by a key this frame, so its row is to be scrolled into view.
    follow: bool,
    /// The pointer has not moved since the keyboard last did anything, so it does not choose the
    /// highlight — see `PathComplete::quiet` in [`crate::ui::breadcrumb`], which is the same idea.
    quiet: bool,
}

impl Default for State {
    /// Nothing typed, the first row lit, and the pointer not yet trusted to move the highlight.
    fn default() -> Self {
        Self {
            text: String::new(),
            hot: 0,
            follow: false,
            quiet: true,
        }
    }
}

/// What a frame of the palette came to.
pub enum Outcome {
    Open,
    /// `Escape`, or a press outside it.
    Closed,
    /// A row was picked. `aside` is `Alt+Enter`: somewhere new rather than here.
    Chose { at: usize, aside: bool },
    /// `Shift+Delete` on a row: take it off whatever list it was recalled from.
    Forget(usize),
}

/// The list's height for these rows: every heading and row, up to [`ROWS_SHOWN`] rows' worth.
///
/// Counted down the list rather than multiplied, because a heading is shorter than a row — so the
/// figure that ends on a whole row at the top of the list is the sum of what comes before it.
fn list_height(rows: &[Row]) -> f32 {
    let mut height = 0.0;
    let mut heading = "";
    for row in rows.iter().take(ROWS_SHOWN) {
        if row.heading != heading {
            heading = row.heading;
            height += menu_header_height();
        }
        height += menu_item_height();
    }
    height.max(menu_item_height())
}

/// Draw the palette over the window and read its keys.
///
/// `screen` is the window's rect, which it is centred in.
pub fn show(ctx: &egui::Context, t: &Theme, screen: Rect, state: &mut State, rows: &[Row]) -> Outcome {
    if !rows.is_empty() {
        state.hot = state.hot.min(rows.len() - 1);
    }

    // ---- The keys, before the field sees them ----------------------------
    let mut outcome = Outcome::Open;
    ctx.input_mut(|i| {
        use egui::{Key, Modifiers};
        const NONE: Modifiers = Modifiers::NONE;
        let last = rows.len().saturating_sub(1);
        let hot = state.hot;
        // `Down` from the last row goes round to the first, and `Up` from the first to the last: a
        // short list is quicker walked backwards than all the way down.
        let to = if i.consume_key(NONE, Key::ArrowDown) {
            Some(if hot >= last { 0 } else { hot + 1 })
        } else if i.consume_key(NONE, Key::ArrowUp) {
            Some(if hot == 0 { last } else { hot - 1 })
        } else if i.consume_key(NONE, Key::PageDown) {
            Some(hot + ROWS_SHOWN)
        } else if i.consume_key(NONE, Key::PageUp) {
            Some(hot.saturating_sub(ROWS_SHOWN))
        } else {
            None
        };
        if let Some(to) = to {
            state.hot = to.min(last);
            state.follow = true;
            state.quiet = true;
        }
        if !rows.is_empty() {
            if i.consume_key(Modifiers::ALT, Key::Enter) {
                outcome = Outcome::Chose { at: state.hot, aside: true };
            } else if i.consume_key(NONE, Key::Enter) {
                outcome = Outcome::Chose { at: state.hot, aside: false };
            } else if i.consume_key(Modifiers::SHIFT, Key::Delete) {
                outcome = Outcome::Forget(state.hot);
            }
        }
        if i.consume_key(NONE, Key::Escape) {
            outcome = Outcome::Closed;
        }
    });
    if !matches!(outcome, Outcome::Open) {
        return outcome;
    }

    // ---- Where it goes ----------------------------------------------------
    let width = WIDTH.min(screen.width() - space::S4 * 2.0).max(160.0);
    let at = pos2(
        (screen.center().x - width * 0.5).round(),
        (screen.top() + DROP).round(),
    );
    let wanted = list_height(rows);

    if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_pressed()) {
        state.quiet = false;
    }
    let quiet = state.quiet;
    let follow = std::mem::take(&mut state.follow);
    let mut hovered: Option<usize> = None;
    let mut clicked: Option<usize> = None;

    let area = egui::Area::new(Id::new("command-palette"))
        .order(Order::Foreground)
        .fixed_pos(at)
        .movable(false)
        // No fade, for the context menu's reason: an `Area` fades in over a tenth of a second, which
        // is right for a tooltip that appeared on its own and wrong for something a key asked for.
        // Faded, the listing's text shows through it for the first few frames.
        .fade_in(false)
        .show(ctx, |ui| {
            // A menu's surface with a heavier shadow than a menu's. See [`SHADOW`].
            menu_frame(t).shadow(SHADOW).show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;

                // Square, like every other field in the window. See [`crate::ui::squared`]. Inset by
                // the gap a menu row keeps from its frame, so the field reads as inside the palette
                // rather than as its lid.
                let inset = menu_item_metrics().padding;
                let field = egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(inset as i8, space::S1 as i8))
                    .show(ui, |ui| {
                        crate::ui::squared(ui, |ui| {
                            ui.add(
                                TextField::new(&mut state.text)
                                    .placeholder("Type a command, or a folder to go to")
                                    .prefix(&azur_icons::search)
                                    .size(Size::Small)
                                    .width(width - inset * 2.0),
                            )
                        })
                    })
                    .inner;
                // On the frame it opens, and again whenever something takes the keyboard away.
                if !field.has_focus() {
                    field.request_focus();
                }
                if field.changed() {
                    // A different list: back to its first row, and to the top of it.
                    state.hot = 0;
                    state.follow = true;
                }
                // `Tab` would hand the keyboard to the next widget in the window. Nothing in here
                // wants it, so the field keeps it.
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        field.id,
                        egui::EventFilter {
                            tab: true,
                            vertical_arrows: true,
                            escape: true,
                            ..Default::default()
                        },
                    );
                });

                menu_divider(ui);

                if rows.is_empty() {
                    ui.add_space(space::S2);
                    ui.add(Text::new("Nothing matches").color(TextColor::Tertiary));
                    ui.add_space(space::S2);
                    return;
                }

                // The room the rows need, claimed rather than asked for — see `offers_height` in
                // `crate::ui::breadcrumb` for the `Area` and `ScrollArea` behaviour that makes
                // asking get stuck at the size of the first list.
                let taken = Rect::from_min_size(ui.cursor().min, vec2(width, wanted));
                let mut list = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(taken)
                        .layout(egui::Layout::top_down_justified(egui::Align::Min)),
                );
                list.spacing_mut().item_spacing.y = 0.0;
                list.style_mut().spacing.scroll.floating = true;
                list.style_mut().spacing.scroll.fade.strength = 0.0;
                egui::ScrollArea::vertical()
                    .id_salt("command-palette-rows")
                    .max_height(wanted)
                    .show(&mut list, |ui| {
                        let mut slots: Vec<(egui::layers::ShapeIdx, Rect)> =
                            Vec::with_capacity(rows.len());
                        let mut heading = "";
                        for (index, row) in rows.iter().enumerate() {
                            if row.heading != heading {
                                heading = row.heading;
                                menu_header(ui, heading);
                            }
                            let slot = ui.painter().add(egui::Shape::Noop);
                            let mut item = MenuItem::new(row.label.as_str())
                                .shortcut(row.shortcut)
                                .selected(row.checked)
                                .danger(row.danger)
                                .gutter(true);
                            if let Some(icon) = row.icon {
                                item = item.icon(icon);
                            }
                            let response = ui.add(item);
                            if follow
                                && state.hot == index
                                && !ui.clip_rect().contains_rect(response.rect)
                            {
                                response.scroll_to_me(None);
                            }
                            if response.hovered() {
                                hovered = Some(index);
                            }
                            if response.clicked() {
                                clicked = Some(index);
                            }
                            slots.push((slot, response.rect));
                        }
                        // Behind the row it lights, in the slot kept for it: by now the label is
                        // down, and a fill over it would be a fill over the name.
                        let lit = if quiet { Some(state.hot) } else { hovered.or(Some(state.hot)) };
                        if let Some(&(slot, rect)) = lit.and_then(|at| slots.get(at)) {
                            ui.painter().set(
                                slot,
                                egui::epaint::RectShape::filled(
                                    rect,
                                    CornerRadius::ZERO,
                                    t.bg.control_hover,
                                ),
                            );
                        }
                    });
                // Over the rows while the keyboard has them, so a pointer resting where the palette
                // happened to open lights nothing up.
                if quiet {
                    ui.interact(taken, Id::new("command-palette-quiet"), Sense::click());
                }
                ui.advance_cursor_after_rect(taken);
            });
        });

    if !quiet {
        if let Some(at) = hovered {
            state.hot = at;
        }
    }
    if let Some(at) = clicked {
        return Outcome::Chose { at, aside: false };
    }
    // A press anywhere else puts it away, as it would a menu.
    let outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|pos| !area.response.rect.contains(pos))
    });
    if outside {
        return Outcome::Closed;
    }
    Outcome::Open
}
