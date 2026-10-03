//! Drawing a shell's grid, and typing into it.
//!
//! The screen half of [`crate::term`]. Everything here happens at explicit rects in the design
//! system's palette, the same as the rest of the interface — none of the drawing comes from the
//! dependency that supplies the grid.
//!
//! # A cell is the font's advance, not a rounded number
//!
//! The obvious thing is to round the cell width to a whole pixel so every column lands on the pixel
//! grid. It is the wrong trade here, because of how the text is drawn: a row is painted as **runs**
//! of cells that share a colour, one galley per run rather than one per cell, which is the
//! difference between a hundred draw calls a row and three. A run only lands on the right columns if
//! the grid's column width *is* the font's advance — round it, and every run drifts a little further
//! from its cells across the row until the last character sits in the wrong column.
//!
//! So the advance is used as measured, and the backgrounds are what get squared up instead: a run's
//! fill reaches exactly to where the next run's begins, computed rather than rounded, so there is
//! neither a seam nor an overlap between them.
//!
//! # Bold is a brighter colour, not a heavier face
//!
//! What every terminal did before font fallback, and here it is deliberate rather than historical:
//! the monospace family is one face, and asking egui for a bold monospace gets a synthesised smear
//! at 14 points. `SGR 1` therefore picks the bright half of [`crate::theme::Ansi`], which is what
//! `ls` and `git` are expecting anyway. Italic is dropped for the same reason, and nothing that
//! prints to a terminal depends on it.
//!
//! # The panel owns the keyboard when it has focus
//!
//! A terminal that let the window keep its shortcuts would be a terminal you cannot type `Ctrl+C`
//! into. So a focused panel drains the event queue — every key, every character — and the window
//! sees none of it. Three combinations are taken out first and never reach the shell: the toggle
//! that closes the panel, and copy and paste, which belong to this program's clipboard rather than
//! to an escape sequence. See [`crate::term`] on why OSC 52 is off.

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as Ink, CursorShape, NamedColor};
use egui::{pos2, vec2, Color32, Rect, Sense, Ui, Vec2};

use crate::term::{keys, Size, Terminal};
use crate::theme::Theme;

/// How thick an underline or a strikeout is, in points.
const RULE: f32 = 1.0;

/// How far a beam cursor is from being a block, as a share of the cell's width.
const BEAM: f32 = 0.15;

/// What a frame of the panel wants the window to do.
///
/// The panel cannot do any of these itself: the clipboard is one apartment-bound lock this program
/// answers for as a whole (see [`crate::shell::clipboard`]), and closing a panel is the pane's
/// business rather than the terminal's.
#[derive(Default)]
pub struct Outcome {
    /// Put this on the clipboard.
    ///
    /// There is no matching `paste`: egui reads the clipboard itself and hands the text over as
    /// [`egui::Event::Paste`], so a paste is already done by the time this returns.
    pub copy: Option<String>,
    /// Close the panel.
    pub close: bool,
    /// The shell set a title, worth showing on the panel's own bar.
    pub title: Option<String>,
}

/// How much of a pane the panel takes until somebody drags it, and what a double click on its
/// splitter puts it back to.
pub const SHARE: f32 = 0.35;

/// The least either side of the splitter is allowed to be, in points.
///
/// Below this a listing has no rows and a terminal has no lines, so the splitter simply stops rather
/// than letting either side be dragged out of existence.
pub const LEAST: f32 = 64.0;

/// Divide a pane between what is above the panel and the panel itself.
///
/// Always the bottom band, and always *below* the preview panel: a terminal is about the folder the
/// pane is showing rather than about one file in it, so it spans the pane's full width and the
/// preview divides what is left above it. Mirrors [`crate::ui::preview::split`], which is applied to
/// the rect this returns.
pub fn split(body: Rect, open: bool, share: f32) -> (Rect, Option<Rect>) {
    if !open || body.height() < LEAST * 2.0 {
        return (body, None);
    }
    let take = (body.height() * share).clamp(LEAST, body.height() - LEAST);
    let seam = body.max.y - take;
    (
        Rect::from_min_max(body.min, pos2(body.max.x, seam)),
        Some(Rect::from_min_max(pos2(body.min.x, seam), body.max)),
    )
}

/// The splitter above the panel, which is also where the panel's top edge is drawn.
///
/// Returns the share it should be after this frame. Separated from [`show`] because the drag has to
/// be handled against the *pane's* height, which the panel's own rect no longer knows.
pub fn splitter(ui: &mut Ui, t: &Theme, id: egui::Id, panel: Rect, body: Rect, share: f32) -> f32 {
    let grip = Rect::from_min_max(
        pos2(panel.left(), panel.top() - crate::ui::SEAM),
        pos2(panel.right(), panel.top() + crate::ui::SEAM),
    );
    let response = ui.interact(grip, id, Sense::click_and_drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(panel.left(), panel.top()), pos2(panel.right(), panel.top() + 1.0)),
        egui::CornerRadius::ZERO,
        crate::ui::seam(t),
    );

    if response.double_clicked() {
        return SHARE;
    }
    if response.dragged() {
        let delta = -response.drag_delta().y;
        return (share + delta / body.height().max(1.0)).clamp(0.1, 0.9);
    }
    share
}

/// What one cell measures, which everything else is derived from.
///
/// Asked of the font rather than assumed. `M` is as good as any character in a monospace family and
/// the row height is the family's own, so a grid built from these is a grid whose cells are exactly
/// where the glyphs will land.
pub fn cell(ctx: &egui::Context, t: &Theme) -> Vec2 {
    let font = mono(t);
    ctx.fonts_mut(|fonts| {
        let advance = fonts
            .layout_no_wrap("M".to_owned(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x;
        vec2(advance.max(1.0), fonts.row_height(&font).max(1.0))
    })
}

fn mono(t: &Theme) -> egui::FontId {
    egui::FontId::new(t.fonts.body.size, egui::FontFamily::Monospace)
}

/// Draw the panel and take whatever was typed into it.
pub fn show(ui: &mut Ui, t: &Theme, id: egui::Id, term: &mut Terminal, rect: Rect) -> Outcome {
    let mut outcome = Outcome::default();
    let cell = cell(ui.ctx(), t);

    // The shell is told the shape before anything is drawn, so what is drawn is the shape it was
    // told. Both of these are cheap when nothing has moved.
    let size = Size::fitting(rect.size(), cell);
    term.resize(size, cell);
    term.set_palette(t.ansi.palette());

    let response = ui.interact(rect, id, Sense::click_and_drag());
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    let focused = response.has_focus();

    // Mouse before keys, so that a click that focuses the panel also places the caret.
    pointer(ui, term, &response, rect, cell, size);
    if focused {
        outcome = input(ui, term, size);
    }
    paint(ui, t, term, rect, cell, size, focused);

    outcome.title = term.title();
    outcome
}

/// Wheel, click and drag.
fn pointer(
    ui: &mut Ui,
    term: &mut Terminal,
    response: &egui::Response,
    rect: Rect,
    cell: Vec2,
    size: Size,
) {
    // The wheel scrolls the view and says nothing to the shell — which is what makes a panel you
    // can read a build in. Only when the pointer is over it, so a wheel meant for the listing above
    // does not go here.
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            term.scroll((scroll / cell.y).round() as i32);
        }
    }

    let at = |pos: egui::Pos2| -> (Point, Side) {
        let col = ((pos.x - rect.left()) / cell.x).floor();
        let row = ((pos.y - rect.top()) / cell.y).floor();
        let col = (col.max(0.0) as usize).min(size.cols.saturating_sub(1));
        let row = (row.max(0.0) as usize).min(size.rows.saturating_sub(1));
        // Which half of the cell, so that a selection started in the right-hand half of a character
        // does not include it — the thing that makes dragging over text feel exact.
        let side = if (pos.x - rect.left()) / cell.x - col as f32 > 0.5 {
            Side::Right
        } else {
            Side::Left
        };
        (term.point_at(row, col), side)
    };

    if let Some(pos) = response.interact_pointer_pos() {
        let (point, side) = at(pos);
        // A double click takes a word and a triple takes the line, which is what every terminal
        // does and what makes a path or a URL one gesture to copy rather than a careful drag.
        let clicks = ui.input(|i| {
            if i.pointer.button_triple_clicked(egui::PointerButton::Primary) {
                3
            } else if i.pointer.button_double_clicked(egui::PointerButton::Primary) {
                2
            } else {
                1
            }
        });
        if response.drag_started() || clicks > 1 {
            let kind = match clicks {
                3 => SelectionType::Lines,
                2 => SelectionType::Semantic,
                _ => SelectionType::Simple,
            };
            term.select(Some(Selection::new(kind, point, side)));
        } else if response.dragged() {
            term.extend_selection(point, side);
        }
    }
    // A plain click with nothing dragged clears the selection, so the highlight does not outlive
    // what it was highlighting.
    if response.clicked() && !ui.input(|i| i.pointer.is_decidedly_dragging()) {
        term.select(None);
    }
}

/// Take the keyboard, and hand the shell the bytes.
fn input(ui: &mut Ui, term: &mut Terminal, size: Size) -> Outcome {
    let mut outcome = Outcome::default();

    // The one combination that stays the window's, taken before the shell can see it.
    let toggle = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Backtick);
    if ui.input_mut(|i| i.consume_shortcut(&toggle)) {
        outcome.close = true;
        return outcome;
    }

    let mode = keys::Mode {
        app_cursor: term.read(|term| term.mode().contains(TermMode::APP_CURSOR)),
    };

    // Everything else belongs to the shell. Drained rather than read, so a `Ctrl+W` typed at a
    // prompt deletes a word instead of closing the tab this panel is in.
    let events = ui.input_mut(|i| std::mem::take(&mut i.events));
    let mut bytes = Vec::new();
    for event in events {
        match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if let Some(encoded) = keys::encode(key, &modifiers, mode) {
                    bytes.extend(encoded);
                }
            }
            egui::Event::Text(text) => {
                if let Some(encoded) = keys::text(&text) {
                    bytes.extend(encoded);
                }
            }
            // **`Ctrl+C` never arrives as a key.** egui-winit turns it into `Event::Copy` before
            // any key event exists — and its test is `command && C`, which ignores shift, so
            // `Ctrl+Shift+C` produces the identical event. There is no way to tell the two apart
            // from here, which rules out the usual arrangement of copy-on-shift.
            //
            // So this is the rule Windows Terminal settled on for exactly the same reason: **copy
            // when something is selected, interrupt when nothing is.** Nothing is lost — a runaway
            // program is one `Ctrl+C` away as long as no selection is up, and the selection is
            // cleared on the way out so the next press interrupts rather than copying twice.
            egui::Event::Copy => match term.selected() {
                Some(text) => {
                    outcome.copy = Some(text);
                    term.select(None);
                }
                None => bytes.push(0x03),
            },
            // `Ctrl+V`, likewise pre-baked by egui with the clipboard already read. A newline in
            // pasted text is a carriage return here for the same reason `Enter` is: a shell reading
            // `\n` believes a line was pasted rather than entered.
            egui::Event::Paste(text) => bytes.extend(text.replace('\n', "\r").into_bytes()),
            // A page of scrollback, since the shell will not see the key.
            egui::Event::MouseWheel { delta, .. } if delta.y != 0.0 => {
                term.scroll(delta.y.signum() as i32 * size.rows as i32 / 2)
            }
            _ => {}
        }
    }
    term.send(bytes);
    outcome
}

/// The grid, as it is now.
fn paint(
    ui: &Ui,
    t: &Theme,
    term: &Terminal,
    rect: Rect,
    cell: Vec2,
    size: Size,
    focused: bool,
) {
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, t.ansi.bg);
    let font = mono(t);

    term.read(|grid| {
        let content = grid.renderable_content();
        let offset = content.display_offset as i32;
        let selection = content.selection;

        for row in 0..size.rows {
            let line = Line(row as i32 - offset);
            let top = rect.top() + row as f32 * cell.y;
            if top > rect.bottom() {
                break;
            }

            // One pass over the row, gathering runs of cells that share an appearance. The run is
            // flushed when the appearance changes or the row ends.
            let mut run = String::new();
            let mut run_from = 0usize;
            let mut run_look: Option<(Color32, Color32, Flags)> = None;

            let mut flush = |run: &mut String, from: usize, to: usize, look: (Color32, Color32, Flags)| {
                if run.is_empty() {
                    return;
                }
                let (fg, bg, flags) = look;
                let left = rect.left() + from as f32 * cell.x;
                // To exactly where the next run starts: computed rather than rounded, so adjacent
                // fills neither overlap nor leave a seam.
                let right = rect.left() + to as f32 * cell.x;
                let box_ = Rect::from_min_max(pos2(left, top), pos2(right, top + cell.y));
                if bg != t.ansi.bg {
                    painter.rect_filled(box_, 0.0, bg);
                }
                if !flags.contains(Flags::HIDDEN) {
                    let galley = painter.layout_no_wrap(std::mem::take(run), font.clone(), fg);
                    painter.galley(box_.left_top(), galley, fg);
                }
                if flags.intersects(Flags::ALL_UNDERLINES) {
                    let y = box_.bottom() - RULE;
                    painter.hline(left..=right, y, egui::Stroke::new(RULE, fg));
                }
                if flags.contains(Flags::STRIKEOUT) {
                    let y = box_.center().y;
                    painter.hline(left..=right, y, egui::Stroke::new(RULE, fg));
                }
                run.clear();
            };

            for col in 0..size.cols {
                let cell_at = &grid.grid()[line][Column(col)];
                // The second half of a double-width character has no glyph of its own; the first
                // half already drew it, and drawing the spacer would print a space over it.
                if cell_at.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let selected = selection.is_some_and(|range| {
                    range.contains(Point::new(line, Column(col)))
                });
                let (fg, bg) = ink(t, cell_at.fg, cell_at.bg, cell_at.flags, selected);
                let look = (fg, bg, cell_at.flags);

                if run_look != Some(look) {
                    if let Some(previous) = run_look {
                        flush(&mut run, run_from, col, previous);
                    }
                    run_from = col;
                    run_look = Some(look);
                }
                run.push(cell_at.c);
            }
            if let Some(look) = run_look {
                flush(&mut run, run_from, size.cols, look);
            }
        }

        // The cursor, last, so it is over whatever it is on.
        //
        // Only while the panel has the keyboard, and hollow otherwise: two panels both showing a
        // filled block is two panels both claiming to be where typing goes.
        let cursor = content.cursor;
        if cursor.shape != CursorShape::Hidden && content.display_offset == 0 {
            let row = cursor.point.line.0 + offset;
            if row >= 0 && (row as usize) < size.rows {
                let left = rect.left() + cursor.point.column.0 as f32 * cell.x;
                let top = rect.top() + row as f32 * cell.y;
                let box_ = Rect::from_min_size(pos2(left, top), cell);
                let ink = t.ansi.fg;
                match (focused, cursor.shape) {
                    (false, _) | (_, CursorShape::HollowBlock) => {
                        painter.rect_stroke(
                            box_,
                            0.0,
                            egui::Stroke::new(RULE, ink),
                            egui::StrokeKind::Inside,
                        );
                    }
                    (true, CursorShape::Underline) => {
                        painter.rect_filled(
                            Rect::from_min_max(
                                pos2(box_.left(), box_.bottom() - RULE * 2.0),
                                box_.right_bottom(),
                            ),
                            0.0,
                            ink,
                        );
                    }
                    (true, CursorShape::Beam) => {
                        painter.rect_filled(
                            Rect::from_min_size(
                                box_.left_top(),
                                vec2((cell.x * BEAM).max(RULE), cell.y),
                            ),
                            0.0,
                            ink,
                        );
                    }
                    (true, _) => {
                        // A block, with the character it is on redrawn in the background colour so
                        // it stays legible rather than being buried.
                        painter.rect_filled(box_, 0.0, ink);
                        let under = grid.grid()[cursor.point.line][cursor.point.column].c;
                        if under != ' ' {
                            let galley = painter.layout_no_wrap(
                                under.to_string(),
                                font.clone(),
                                t.ansi.bg,
                            );
                            painter.galley(box_.left_top(), galley, t.ansi.bg);
                        }
                    }
                }
            }
        }
    });
}

/// What one cell is drawn in: its two colours, after the flags have had their say.
fn ink(t: &Theme, fg: Ink, bg: Ink, flags: Flags, selected: bool) -> (Color32, Color32) {
    // `SGR 1` picks the bright half rather than a heavier face — see the module header.
    let bright = flags.contains(Flags::BOLD);
    let mut fore = resolve(t, fg, bright, t.ansi.fg);
    let mut back = resolve(t, bg, false, t.ansi.bg);

    if flags.contains(Flags::DIM) {
        // Halfway to the background, which is what dim means and is stable whichever theme it is:
        // towards the surface rather than towards black.
        fore = mix(fore, back, 0.45);
    }
    // `SGR 7`, and it has to happen after dim and before the selection — a program that inverts is
    // asking for its own colours swapped, not for the selection's.
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fore, &mut back);
    }
    if flags.contains(Flags::HIDDEN) {
        fore = back;
    }
    if selected {
        // **Only the background.** Forcing a foreground as well is the obvious thing and it throws
        // away the output's own colouring — selecting a `git diff` would turn its reds and greens
        // into one flat ink, which is the opposite of what you select a diff in order to read.
        back = selection(t);
    }
    (fore, back)
}

/// The fill behind selected cells.
///
/// The window's accent, most of the way back towards the panel's own background. It has to sit
/// under text whose colour this program does not choose — anything from bright yellow to dim blue —
/// so it cannot be the solid accent a selected row in the listing gets, and it cannot be neutral
/// either or a selection would be invisible against a cell that already has a background.
fn selection(t: &Theme) -> Color32 {
    mix(t.accent.default, t.ansi.bg, 0.62)
}

/// One of the terminal's colour spellings, as a colour.
fn resolve(t: &Theme, ink: Ink, bright: bool, fallback: Color32) -> Color32 {
    let ansi = |index: usize| -> Color32 {
        match index {
            0..=7 if bright => t.ansi.bright[index],
            0..=7 => t.ansi.normal[index],
            8..=15 => t.ansi.bright[index - 8],
            _ => fallback,
        }
    };
    match ink {
        Ink::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        Ink::Indexed(index) => {
            if index < 16 {
                ansi(index as usize)
            } else {
                let rgb = t.ansi.palette().at(index as usize);
                Color32::from_rgb(rgb.r, rgb.g, rgb.b)
            }
        }
        Ink::Named(named) => match named {
            NamedColor::Foreground => t.ansi.fg,
            NamedColor::Background => t.ansi.bg,
            NamedColor::Cursor => t.ansi.fg,
            other => {
                let index = other as usize;
                if index < 16 {
                    ansi(index)
                } else {
                    // The dim family, which `vte` numbers above the named sixteen. Halfway to the
                    // background, the same as the `DIM` flag.
                    let base = index.checked_sub(NamedColor::DimBlack as usize);
                    match base.filter(|base| *base < 8) {
                        Some(base) => mix(t.ansi.normal[base], t.ansi.bg, 0.45),
                        None => fallback,
                    }
                }
            }
        },
    }
}

/// `amount` of the way from `from` to `to`.
fn mix(from: Color32, to: Color32, amount: f32) -> Color32 {
    let blend = |a: u8, b: u8| -> u8 {
        (a as f32 + (b as f32 - a as f32) * amount).round().clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(
        blend(from.r(), to.r()),
        blend(from.g(), to.g()),
        blend(from.b(), to.b()),
    )
}

#[cfg(test)]
mod tests;
