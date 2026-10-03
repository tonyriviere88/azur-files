//! The log: every line a shell has printed, laid out and drawn.
//!
//! The expensive part of the panel, and the one with a cap on it — see [`super::State`].

use super::*;

/// What a font measures, taken off one laid-out glyph.
#[derive(Clone, Copy)]
pub(crate) struct Metrics {
    /// A row's height, rounded up to a whole **device** pixel.
    ///
    /// The rounding is not cosmetic. Rows are placed at `top + n * row`, so a fractional pitch puts
    /// every row on a different subpixel phase — and epaint rounds a galley's baseline to a whole
    /// pixel relative to the galley's own origin, so each row's text gets rounded a different way.
    /// The result is a column of sharp text that visibly does not sit on one line, which is what
    /// this panel looked like until it was measured. A whole-pixel pitch plus [`crate::ui::snap`] at
    /// each origin gives every row the same phase and therefore the same rounding.
    pub(crate) row: f32,
    /// One column's width, taken off the glyph rather than off the galley.
    ///
    /// **`Galley::size()` is rounded to whole pixels** — `round_output_to_gui` does it so that a
    /// widget measured from text lands on the grid — and a rounded advance is wrong by a fraction of
    /// a pixel *per column*. Consolas advances 8.4 and the galley reports 8, so by column twenty the
    /// arithmetic is a whole character out and by column forty it is two: the highlight stops
    /// agreeing with the glyphs, the tail of a long line cannot be reached at all, and dragging
    /// slowly moves the selection in jumps. `Glyph::advance_width` is the unrounded number.
    pub(crate) advance: f32,
    /// How far **above** the middle of a row the ink of text in it sits.
    pub(crate) lift: f32,
}

/// Measure a font.
///
/// `lift` is the one worth explaining, and it is the whole of a complaint that has been made about
/// this panel twice. A row box is ascent *plus descent*, and a digit's ink stops at the baseline —
/// so the ink of a line of text sits above the middle of the box holding it, by about half the
/// descent. A chevron centred in the same box is therefore centred on nothing the eye is looking at,
/// and reads low.
///
/// Measured rather than nudged by a constant: a laid-out glyph carries its baseline in `pos` and its
/// ink's own offset from that baseline in `uv_rect`, so this is the font's answer and not a taste.
pub(crate) fn metrics(ui: &Ui, font: &egui::FontId) -> Metrics {
    let probe = ui
        .painter()
        .layout_no_wrap("0".to_owned(), font.clone(), egui::Color32::PLACEHOLDER);
    let ppp = ui.ctx().pixels_per_point();
    let row = (probe.size().y.max(1.0) * ppp).ceil() / ppp;
    let glyph = probe
        .rows
        .first()
        .and_then(|placed| placed.row.glyphs.first().map(|glyph| (placed.pos.y, glyph)));
    let ink = glyph.map_or(row * 0.5, |(top, glyph)| {
        top + glyph.pos.y + glyph.uv_rect.offset.y + glyph.uv_rect.size.y * 0.5
    });
    Metrics {
        row,
        advance: glyph.map_or(row * 0.5, |(_, glyph)| glyph.advance_width).max(1.0),
        lift: row * 0.5 - ink,
    }
}

/// Where to put a line of text — with `Align2::LEFT_TOP` — so that its **ink** ends up in the middle
/// of `rect` rather than its box.
///
/// A galley's box is ascent plus descent, and the ink of a line of text stops at the baseline: laid
/// flush in a row, all of the slack ends up underneath it and the text reads as stuck to the top of
/// its row. Measured here, that was ten pixels of ink at the top of a sixteen-point row with six
/// empty ones below — which is exactly what it looked like.
///
/// This is [`Metrics::lift`]'s only job, and doing it once here is what lets everything *beside* the
/// text — a chevron, a note — simply centre in the row through [`crate::ui::icon_rect`] and land on
/// it. Centre the ink, and the box centre becomes the right answer for everything else.
pub(crate) fn ink_top(rect: Rect, m: &Metrics) -> f32 {
    rect.center().y - m.row * 0.5 + m.lift
}

/// How wide a string is in a font, for laying a strip out before drawing it.
pub(crate) fn width(ui: &Ui, font: &egui::FontId, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
        .size()
        .x
}

/// The scrollable log.
#[allow(clippy::too_many_arguments)]
pub(crate) fn log(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    state: &mut State,
    blocks: &mut [Block],
    gone: bool,
    // The resize grip's band, which overlaps the first row. A press that started there is the border
    // being dragged, not text being selected — and it stays excluded for the whole drag, because the
    // press origin does not move even after the pointer has travelled down into the log.
    band: Rect,
) {
    let font = t.fonts.mono.clone();
    // All three off the *same* galley on purpose: the height a row is placed at and the height the
    // text inside it is centred in have to be one number, and asking two questions is how they come
    // to differ by a rounding. `0` rather than a space, because a space is the one glyph a font is
    // allowed to give a different advance to.
    let m = metrics(ui, &font);
    let (row_h, advance) = (m.row, m.advance);
    let count = state.rows();
    // The wrap width, decided in `show` before the index was built. `None` means lines run off to the
    // right instead.
    let cols = state.cols();
    // What a scrollbar takes: the design system's width plus its margins, asked of the style rather
    // than named here so the gutter is the one egui is actually going to reserve.
    let bar = ui.spacing().scroll.allocated_width().max(1.0);

    if count == 0 {
        let (text, ink) = if gone {
            ("the shell has gone — Shift+Tab to start another", t.status.danger)
        } else {
            ("type a command", t.text.tertiary)
        };
        crate::ui::text_center(ui.painter(), rect, font, ink, text);
        return;
    }

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    // See the module header: `show_rows` reserves `row_height + item_spacing.y` per row, and rows
    // painted at exact rects need the spacing to be nothing or they drift out of their own boxes.
    child.spacing_mut().item_spacing = Vec2::ZERO;

    let mut scroll = egui::ScrollArea::both()
        .id_salt(("console-log", pane))
        .auto_shrink([false, false]);
    // Wrapping has just been turned on, and a log left scrolled sideways when there is no longer
    // anywhere sideways to be looks empty.
    if std::mem::take(&mut state.pan) {
        scroll = scroll.horizontal_scroll_offset(0.0);
    }
    // **What the foot of the log is held back for, which while lines wrap is nothing.**
    //
    // Unwrapped, a horizontal scrollbar can arrive at any moment — the moment a long line does — so a
    // bar's height is kept clear at the bottom whether or not one is showing, and the newest line
    // stays above where the bar goes rather than under it. Wrapped, there is no such moment: the
    // content is exactly as wide as the page and a horizontal bar can never appear. So the log reaches
    // the panel's bottom edge, and so does the fade that says there is more below it.
    let foot = if cols.is_some() { 0.0 } else { bar };
    let view = rect.height() - foot;
    let reach = (count as f32 * row_h - view).max(0.0);
    if let Some(row) = state.reveal.take() {
        // Nudged into view rather than centred, and against last frame's offset because
        // `ScrollArea` takes an absolute one — the same discipline as the listing's cursor.
        let top = row as f32 * row_h;
        let mut offset = state.offset;
        if top < offset {
            offset = top;
        } else if top + row_h > offset + view {
            offset = top + row_h - view;
        }
        scroll = scroll.vertical_scroll_offset(offset.clamp(0.0, reach));
    } else if state.tail {
        scroll = scroll.vertical_scroll_offset(reach);
    }

    // **The rows that exist, and one more of nothing while a bar can still arrive.** The slack is what
    // lets the last line of output be scrolled clear of a horizontal scrollbar — without it the bar
    // appears with the long line that caused it and sits on top of the very line you scrolled down to
    // read. Wrapped, no bar can appear, so there is nothing to be clear of and the extra row would only
    // be a strip of nothing under the last line. The listing keeps slack for its own reason; see its
    // `TAIL_ROWS`.
    let slack = usize::from(cols.is_none());
    let output = scroll.show_rows(&mut child, row_h, count + slack, |ui, range| {
        let range = range.start.min(count)..range.end.min(count);
        let first = range.start;
        // The panned origin: `show_rows` hands out a `Ui` already translated by the scroll offset,
        // so this is where row `first` column `0` actually lands.
        let origin = ui.min_rect().min;
        let text_x = origin.x + PAD + GUTTER;

        // The visible rows, and their text, taken once: the same strings are measured, hit-tested
        // and painted, and the borrow of `blocks` has to end before a click can fold one.
        let seen: Vec<(Row, String)> = range
            .clone()
            .filter_map(|at| {
                state
                    .row(blocks, at)
                    .map(|row| (row, shown(blocks, row, cols).into_owned()))
            })
            .collect();
        let widest = seen
            .iter()
            .map(|(_, text)| text.chars().count())
            .max()
            .unwrap_or(0);
        // What the horizontal scrollbar reaches: the widest line *on screen*, which grows as one
        // comes into view and is therefore always enough to read what is showing. Wrapped, there is
        // nowhere sideways to go and the content is exactly the page.
        let content = match cols {
            Some(cols) => cols,
            None => widest,
        };
        ui.set_min_width(PAD * 2.0 + GUTTER + content as f32 * advance);

        let visible = Rect::from_min_max(
            pos2(rect.left(), origin.y),
            pos2(rect.right(), origin.y + seen.len() as f32 * row_h),
        );
        // **The page: what is actually readable, scrollbars excluded.**
        //
        // `ui.clip_rect()` inside `show_rows` is the scroll area's *viewport* — the rect left after
        // egui reserved a gutter for whichever bars it is showing. Painting and hit-testing against
        // the panel's own rect instead is what made the bars behave like text: the pointer over one
        // showed a caret, and dragging one dragged a selection along with the scroll.
        //
        // And a gutter is reserved on whichever axis egui did *not* reserve one, so the width of the
        // text does not change when a bar appears — a bar arriving is a long line arriving, which is
        // the worst moment to reflow everything. Sideways, always: a vertical bar comes and goes with
        // the length of the log. Downwards, only while lines run off to the right — see [`foot`], which
        // is nothing while they wrap, and then the page is the panel down to its bottom edge.
        let page = {
            let seen = ui.clip_rect().intersect(rect);
            Rect::from_min_max(
                seen.min,
                pos2(
                    if seen.right() > rect.right() - bar * 0.5 {
                        seen.right() - bar
                    } else {
                        seen.right()
                    },
                    if seen.bottom() > rect.bottom() - foot * 0.5 {
                        seen.bottom() - foot
                    } else {
                        seen.bottom()
                    },
                ),
            )
        };
        // **Clipped to the page, because `visible` is not.** `show_rows` places the first laid-out row
        // wherever the scroll offset puts it, which is up to one row *above* the panel — so the rows'
        // rect reaches into the listing overhead, and a press there was taken for a press on a row.
        // That is how dragging the resize border came to select text again after the border had moved
        // out from under the pointer: the origin was no longer in the grip's band and still in this.
        let hit = visible.intersect(page);
        // One interaction for the whole visible block, and the row worked out from the pointer —
        // the listing's argument, and the same saving: no id, hit-test or hover slot per row. The
        // response itself is not read — every gesture here comes off the raw pointer, because the
        // ones this panel needs are reported a frame later than it needs them.
        let _rows = ui.interact(hit, Id::new(("console-rows", pane)), Sense::click_and_drag());
        if !state.resizing
            && ui
                .input(|i| i.pointer.latest_pos())
                .is_some_and(|at| hit.contains(at) && !band.contains(at))
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }

        let spot = |at: Pos2| -> Spot {
            let slot = (((at.y - origin.y) / row_h).floor().max(0.0) as usize)
                .min(seen.len().saturating_sub(1));
            let col = ((at.x - text_x) / advance).round().max(0.0) as usize;
            let len = seen.get(slot).map_or(0, |(_, text)| text.chars().count());
            Spot {
                row: first + slot,
                col: col.min(len),
            }
        };
        // The run of like characters a spot falls in, for a word-mode drag.
        let word = |spot: Spot| -> std::ops::Range<usize> {
            let slot = spot.row.saturating_sub(first);
            let chars: Vec<char> = seen
                .get(slot)
                .map(|(_, text)| text.chars().collect())
                .unwrap_or_default();
            word_at(&chars, spot.col)
        };

        // **Where the button went down, for as long as it is down.** `press_origin` rather than
        // `drag_started`, which is the bug this replaces: egui only calls a press a drag once it
        // has *travelled*, and by then the pointer is a row or two along — so the selection
        // anchored on the wrong row, or never anchored at all on a short drag.
        let held = ui.input(|i| {
            i.pointer
                .primary_down()
                .then(|| i.pointer.press_origin())
                .flatten()
        });
        let held = held.filter(|_| !state.resizing);
        if let Some(from) = held.filter(|at| hit.contains(*at) && !band.contains(*at)) {
            if from.x < origin.x + PAD + FOLD + space::S1 {
                // The gutter: a fold, on the press rather than on the release, and only for a
                // header that has something to hide.
                if ui.input(|i| i.pointer.primary_pressed()) {
                    let at = spot(from).row;
                    if let Some(Row::Head(which, _)) = state.row(blocks, at) {
                        let block = &mut blocks[which];
                        if !block.lines.is_empty() || block.dropped > 0 {
                            block.collapsed = !block.collapsed;
                        }
                        state.aim = Aim::Block(block.id);
                    }
                }
            } else {
                // A double click puts the drag in word mode for as long as the button is held, and a
                // double click on its own therefore selects exactly one word. Decided on the press:
                // `Response::double_clicked` is reported on the release, which is a frame after the
                // drag it would have to govern has already started.
                if ui.input(|i| i.pointer.primary_pressed()) {
                    let now = ui.input(|i| i.time);
                    let delay = ui
                        .ctx()
                        .options(|o| o.input_options.max_double_click_delay);
                    let again = state
                        .last_press
                        .is_some_and(|(when, at)| now - when <= delay && at.distance(from) < 8.0);
                    state.grab = if again { Grab::Word } else { Grab::Char };
                    state.last_press = Some((now, from));
                }
                let to = ui.input(|i| i.pointer.latest_pos()).map_or(from, |at| at);
                let (mut a, mut b) = (spot(from), spot(to));
                if state.grab == Grab::Word {
                    // Both ends out to their own word's edges, in whichever direction the drag is
                    // going — which is what makes a double click alone select exactly one word.
                    let (near, far) = if a <= b { (&mut a, &mut b) } else { (&mut b, &mut a) };
                    near.col = word(*near).start;
                    far.col = word(*far).end;
                }
                state.aim = Aim::Text(Span::new(a, b));
            }
        }

        let picked = match state.aim {
            Aim::Block(id) => Some(id),
            _ => None,
        };
        let span = match state.aim {
            Aim::Text(span) if !span.empty() => Some(span),
            _ => None,
        };
        let fill = t.bg.layer;
        let wash = crate::ui::row_fill(t, true, false);
        let ink_selected = ui.visuals().selection.bg_fill;
        // Clipped to the page, so a long line stops where the vertical scrollbar starts rather than
        // running the width of the viewport, which is a whole bar wider.
        let paint = ui.painter_at(page);

        for (slot, (row, text)) in seen.iter().enumerate() {
            let at = first + slot;
            let box_ = Rect::from_min_size(
                pos2(page.left(), origin.y + slot as f32 * row_h),
                vec2(page.width(), row_h),
            );
            // **The blank row above a block is not part of it to look at.** It carries the block's
            // index so the index arithmetic has one owner for every row, but a picked block's fill
            // starts at its header — a selection that begins one row early reads as a gap in the
            // selection rather than as air between blocks.
            let air = matches!(row, Row::Gap(_));
            let mine = !air && picked.is_some_and(|id| blocks[row.block()].id == id);

            // A header reads as a band, which is what gives the log its blocks without a rule or a
            // colour per command; a picked block wears the listing's own selected fill instead.
            match (matches!(row, Row::Head(..)), mine) {
                (_, true) => {
                    if let Some(wash) = wash {
                        paint.rect_filled(box_, CornerRadius::ZERO, wash);
                    }
                }
                (true, false) => {
                    paint.rect_filled(box_, CornerRadius::ZERO, fill);
                }
                _ => {}
            }
            if mine {
                crate::ui::selection_bar(&paint, box_, t);
            }

            let chars = text.chars().count();
            if let Some(cut) = span.and_then(|span| span.cut(at, chars)) {
                let from = text_x + cut.start as f32 * advance;
                let to = text_x + cut.end as f32 * advance;
                paint.rect_filled(
                    Rect::from_min_max(pos2(from, box_.top()), pos2(to, box_.bottom())),
                    CornerRadius::ZERO,
                    ink_selected,
                );
            }

            if let Row::Head(which, _) = row {
                let block = &blocks[*which];
                // **Always on a header, and now it is the only mark there is.** With the accent `>`
                // gone it is what says "this row is a command", so a block with nothing to fold keeps
                // it too — drawn in `text-disabled`, which is the same thing every tree says about a
                // twisty with no children.
                let folds = !block.lines.is_empty() || block.dropped > 0;
                // Centred in the row, plainly: the text in it has its own ink centred there, so this
                // is centred on the text as well.
                let chevron = crate::ui::icon_rect(box_, origin.x + PAD, FOLD);
                let glyph = if block.collapsed || !folds {
                    azur_egui_theme::icons::chevron_right
                } else {
                    azur_egui_theme::icons::chevron_down
                };
                let quiet = match (folds, mine) {
                    (false, _) => t.text.disabled,
                    (true, true) => t.text.primary,
                    (true, false) => t.text.tertiary,
                };
                glyph(&paint, chevron, quiet);
            }

            // Only two things are marked at all — a command that is still going and one that
            // failed — so the two worth noticing are the only marks on the panel.
            let ink = match row {
                Row::Head(which, _) if blocks[*which].failed() => t.status.danger,
                Row::Cut(_) => t.text.tertiary,
                // **Standard error is quieter, not red.** It was `status.danger`, on the reading that
                // stderr is the error stream. It is not: it is the unbuffered, diagnostic,
                // not-my-output stream, and the programs that use it properly are the ones that
                // suffered. Measured — a *successful* `cargo build` writes 0 bytes to stdout and all
                // 159 of `Compiling`/`Finished` to stderr, so every green build was painted as a
                // failure; `python -i` puts its banner and its `>>>` prompts there too.
                //
                // Failure is said by the exit code instead, which is a thing this panel actually
                // knows: the header above carries `status.danger` and the code itself when the command
                // failed. Two streams still tell apart, because the separation is worth keeping — it
                // is the one place this beats a terminal — but the one that means "went wrong" is the
                // one that went wrong.
                Row::Text(which, line, _) if blocks[*which].lines[*line].err => t.text.secondary,
                // **Output is `text-primary`, the same as the command above it.** It is the thing
                // being read; greying it to tell it apart from its header is the wrong way round,
                // and the header has a band, a mark and a chevron already.
                Row::Gap(_) | Row::Head(..) | Row::Text(..) => t.text.primary,
            };
            // **Snapped, and placed by its ink.** Two separate corrections in one line, both of
            // which this panel has been wrong about:
            //
            // - `Align2::LEFT_CENTER` at the row's middle put each origin half a fractional row
            //   height along, and epaint rounds a baseline to a whole pixel relative to the galley's
            //   own origin — so every row got rounded a different way and the column of text
            //   visibly did not sit on one line. A whole-pixel pitch and a snapped origin give every
            //   row the same phase and therefore the same rounding.
            // - Flush at the row's top left all of the slack under the text. See [`ink_top`].
            let at = crate::ui::snap(&paint, pos2(text_x, ink_top(box_, &m)));
            paint.text(at, Align2::LEFT_TOP, text, font.clone(), ink);

            if let Row::Head(which, _) = row {
                let block = &blocks[*which];
                let note = if block.running() {
                    Some(("running".to_owned(), t.accent.default))
                } else {
                    block
                        .code
                        .filter(|code| *code != 0)
                        .map(|code| (format!("exit {code}"), t.status.danger))
                };
                if let Some((note, ink)) = note {
                    let caption = t.fonts.caption.clone();
                    let cm = metrics(ui, &caption);
                    paint.text(
                        pos2(page.right() - PAD, ink_top(box_, &cm)),
                        Align2::RIGHT_TOP,
                        note,
                        caption,
                        ink,
                    );
                }
            }
        }
        // Handed back out, because the fades are drawn after the scroll area has closed and they have
        // to sit inside the same page the rows did.
        page
    });

    state.offset = output.state.offset.y;
    // Stuck to the bottom, and only the scroll position says so: a command sends it back down, and a
    // drag of the scrollbar takes it off again, with no flag to get out of step. Against `reach`
    // rather than against the content, because unwrapped the content carries a row of slack the log
    // never scrolls into.
    state.tail = output.state.offset.y >= reach - row_h * 0.5;

    // **There is more above, and there is more below** — the design system's rule for the edge of any
    // scrolling collection, and the log is one. See `azur_egui_theme::components::scroll_fades`, which
    // is where the fade itself, its depth and the argument for it live now.
    //
    // Its own two figures rather than the `ScrollArea`'s: unwrapped, this log reserves a row of trailing
    // slack past its last line, and measuring the bottom off the content would fade for slack nobody
    // put anything in. `output.inner` is the *page* — the viewport with the scrollbars taken off and,
    // while lines run off to the right, a bar's height held back at the foot for one that has not
    // arrived yet — worked out inside the closure, and not `inner_rect`, which knows about neither.
    let above = output.state.offset.y;
    azur_egui_theme::components::scroll_fades(
        &ui.painter_at(output.inner),
        output.inner,
        t.bg.canvas,
        above,
        (reach - above).max(0.0),
    );
}
