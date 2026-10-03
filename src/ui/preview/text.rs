//! The text canvas: the body, its syntax colouring, the find bar's marks, and the gutter.
//!
//! Everything here works on *a* string and never on the file — which is what lets the diff
//! view in [`super::diff`] hand it a body that is not quite the file and change nothing else.

use super::*;

pub(super) struct Text {
    pub(super) body: String,
    pub(super) truncated: bool,
    /// Its columns mean something, so it is set in the monospace role.
    pub(super) code: bool,
    /// What language it is in, kept because the diff view has to colour a *different* string.
    pub(super) lang: syntax::Lang,
    /// `body`'s syntax colouring. Empty for prose, for an unknown language, and for a
    /// file over [`syntax::CAP`].
    pub(super) spans: Vec<syntax::Span>,
    /// The document, for a file that has one. `Some` only for Markdown.
    ///
    /// Both this and `spans` are worked out **once, when the file arrives**, and not while
    /// drawing: parsing a README on every frame the pointer moves over the panel would be
    /// the same mistake the find bar's `done` exists to avoid.
    pub(super) doc: Option<markdown::Doc>,
    /// What git says changed in it, or `None` when nothing did — see [`crate::git::Changes`].
    pub(super) changes: Option<crate::git::Changes>,
    /// The diff view, when one is being shown: a body of its own, and what each of its lines is.
    pub(super) view: Option<Diffed>,
    /// Which pair of toggles `view` was built for, so it is built again when they move and not on
    /// every frame. `None` means "no view", which is also what an unchanged file has.
    pub(super) view_for: Option<bool>,
}

impl Text {
    /// The string on screen, which is the document's own where one is being shown.
    ///
    /// **Everything about the find bar goes through here**, so a search is over what can
    /// be read rather than over the markup behind it: looking for `bold` in `**bold**`
    /// finds it, and looking for `**` finds nothing, because there are no asterisks on
    /// the screen. See [`crate::markdown`].
    pub(super) fn shown(&self, markup: bool) -> &str {
        match (&self.doc, &self.view) {
            (Some(doc), _) if !markup => &doc.text,
            (_, Some(view)) => &view.body,
            _ => &self.body,
        }
    }

    /// Whether there is a document to show, and so a markup toggle to offer.
    pub(super) fn renderable(&self) -> bool {
        self.doc.is_some()
    }

    /// Whether there is a diff to show, and so a diff toggle to offer.
    pub(super) fn diffable(&self) -> bool {
        self.changes.is_some()
    }

    /// Build, keep or drop the diff view for the toggles as they now are.
    ///
    /// Called once a frame and costs a comparison unless something moved. The build is one pass over
    /// the file plus a lex of what comes out of it — the same order of work as arriving does, and for
    /// the same reason it is not done while drawing.
    pub(super) fn follow(&mut self, on: bool, collapse: bool) -> bool {
        let want = (on && self.diffable()).then_some(collapse);
        if self.view_for == want {
            return false;
        }
        self.view_for = want;
        self.view = match (want, &self.changes) {
            (Some(collapse), Some(changes)) => {
                Some(Diffed::build(&self.body, changes, collapse, self.lang))
            }
            _ => None,
        };
        true
    }
}

/// The faces one block of a document is set in.
///
/// Held together because the interesting one is [`Faces::code`], which cannot be worked out from the
/// other two — it has to be *measured* against them. See [`Faces::of`].
pub(super) struct Faces {
    /// The block's own face: body, a heading's size, or the monospace of a table row.
    pub(super) base: egui::FontId,
    /// The semibold family, for `**bold**`.
    pub(super) strong: egui::FontFamily,
    /// The monospace face for `` `inline code` ``, at the surrounding size.
    pub(super) code: egui::FontId,
    /// And the line height that puts that face **on the surrounding text's baseline**.
    ///
    /// `None` where there is nothing to correct.
    pub(super) code_line: Option<f32>,
}

impl Faces {
    /// Measure the three, for a block whose prose is set in `base`.
    ///
    /// # Why inline code needs a line height of its own
    ///
    /// epaint places a glyph in a row at `face_ascent + valign × (row_height − line_height)`. Two
    /// faces in one row therefore share a baseline only where their ascents *and* their line heights
    /// happen to agree, and these two do not: at 14 points the proportional face's ascent is 15.09
    /// against the monospace face's 10.41, and its line height is 18.59 against 16.41. Left alone,
    /// inline code came out **three pixels above** the prose around it — and the tinted fill behind
    /// it stayed put, which is what made it read as an underline.
    ///
    /// No value of `valign` fixes that. `TOP` aligns the two *ascents*, which differ; `BOTTOM`
    /// aligns the two line boxes' bottoms, which differ by the descents; `Center` splits the
    /// difference. The only per-section dial that moves a baseline by an arbitrary amount is
    /// `line_height`, which `BOTTOM` subtracts — so shortening the code section's line box by the
    /// gap between the two baselines lowers it by exactly that much.
    ///
    /// **The gap is read back rather than derived**, because deriving it needs the faces' ascents
    /// and epaint's public `Fonts` exposes only `row_height`. So both faces go into one probe job —
    /// two characters, a cached layout, one hash lookup — and the difference between where epaint
    /// actually put them is the correction. That it comes back already rounded to the pixel grid is
    /// what makes this land *on* the prose's baseline rather than near it: both positions are
    /// multiples of one physical pixel, so shifting by their difference commutes with the rounding.
    pub(super) fn of(painter: &egui::Painter, base: egui::FontId, strong: egui::FontFamily) -> Self {
        let code = egui::FontId::new(base.size, egui::FontFamily::Monospace);
        let mut probe = egui::text::LayoutJob::default();
        for font in [base.clone(), code.clone()] {
            probe.append(
                "x",
                0.0,
                egui::TextFormat {
                    font_id: font,
                    color: Color32::PLACEHOLDER,
                    ..Default::default()
                },
            );
        }
        let probe = painter.layout_job(probe);
        let code_line = match probe.rows.first().map(|row| &row.glyphs[..]) {
            Some([prose, inline]) => Some(inline.line_height - (prose.pos.y - inline.pos.y)),
            // Either the probe laid out nothing, or the block is already monospace — a table row —
            // in which case the two are one face and the correction is nought anyway.
            _ => None,
        };
        Self {
            base,
            strong,
            code,
            code_line,
        }
    }
}

/// What the find bar contributes to one drawing pass: where the hits are, which of them is current,
/// and whether it still has to be brought into view.
pub(super) struct Marks<'a> {
    pub(super) hits: &'a [Range<usize>],
    pub(super) at: usize,
    pub(super) reveal: &'a mut bool,
}

/// How tall a row of the text view should be for the ink to sit in the middle of it, or `None` to
/// leave the face's own line box alone.
///
/// # Why a row is not a line box
///
/// **Every band in this view is drawn on the row**, and one of them is not this program's to move:
/// epaint paints a text selection as vertices in the row it belongs to, spanning `0..row.height`. So
/// does a search hit's background. The diff's bands could be offset by hand — and were — but that only
/// moved one of the three, and a selection band and a diff band two points apart is worse than either
/// being off on its own.
///
/// A line box is not symmetric about its ink and has no reason to be. Measured on the two faces this
/// view uses, at 14 points, `pixels_per_point` 1:
///
/// | | monospace | proportional |
/// | --- | --- | --- |
/// | line box | 16.00 | 19.00 |
/// | baseline (the face's ascent) | 10.00 | 16.00 |
/// | ink, from the top of the box | 1.00 … 13.00 | 6.00 … 19.00 |
/// | **the ink's own band, centred** | **14.00** | 25.00 |
///
/// The last row is what this returns: `top + bottom`, which is the height whose middle is the ink's
/// middle — because the baseline stays where the face puts it whatever the line height says, so
/// changing the height moves the *box* around the ink rather than the ink inside the box.
///
/// **And it only ever tightens.** Monospace comes down from 16 to 14, which centres it. Proportional
/// would have to go the other way — its ink already reaches the bottom of its box — and 25 points for
/// a 14-point paragraph is a line and a half of leading, so prose keeps its own line box and its bands
/// stay a little low. One rule, applied where it makes a row fit its text and declined where it would
/// make a paragraph fall apart.
pub(super) fn row_box(painter: &egui::Painter, font: &egui::FontId) -> Option<f32> {
    // A capital and a descender, never drawn. egui caches the layout, so this is a hash lookup after
    // the first call.
    let probe = painter.layout_no_wrap("Ay".to_owned(), font.clone(), Color32::PLACEHOLDER);
    let row = probe.rows.first()?;
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for glyph in row.glyphs.iter().filter(|g| !g.uv_rect.is_nothing()) {
        top = top.min(glyph.pos.y + glyph.uv_rect.offset.y);
        bottom = bottom.max(glyph.pos.y + glyph.uv_rect.offset.y + glyph.uv_rect.size.y);
    }
    let height = top + bottom;
    (height.is_finite() && height < row.size.y).then_some(height)
}

/// Text, wrapped and scrolled, optionally numbered.
///
/// **Monospace only where the columns mean something** — see [`crate::preview::is_code`], which
/// asks a question with an answer rather than a matter of taste: does moving a character sideways
/// change what the file means? In a log, a table, a diff or any source file it does. A `.md` or a
/// `.txt` is paragraphs, and paragraphs are what the proportional face is for.
///
/// Wrapped rather than scrolled sideways, which is what was asked for and also the only thing that
/// works in a panel this narrow. Which makes the **line numbers** the interesting part: a wrapped
/// paragraph is several visual rows of one logical line, so the gutter cannot simply count rows.
/// It walks the galley and numbers the rows that *begin* a line, which is a fact only the finished
/// layout knows — and the reason the galley is laid out here by hand and then handed to a `Label`
/// rather than left to the widget.
pub(super) fn text_canvas(
    ui: &mut Ui,
    t: &Theme,
    canvas: Rect,
    spot: Spot,
    text: &Text,
    numbers: bool,
    find: &mut Find,
) {
    let font = if text.code {
        t.fonts.mono.clone()
    } else {
        t.fonts.body.clone()
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(canvas.shrink2(vec2(PAD, 0.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(canvas.intersect(ui.clip_rect()));
    let output = egui::ScrollArea::vertical()
        .id_salt(("preview-text", spot))
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            // **What is actually on screen**: the file, or the diff's own body when one is being
            // shown. Everything below reads these two rather than the file, so the diff view costs
            // this line and no branching further down.
            let body = match &text.view {
                Some(view) => &view.body,
                None => &text.body,
            };
            let spans = match &text.view {
                Some(view) => &view.spans,
                None => &text.spans,
            };

            // The gutter, wide enough for the last line's number. Measured from the count rather
            // than guessed, so a 12,000-line file does not have its numbers clipped and a 12-line
            // one does not carry a gutter for four digits it will never use. From the *file's* count
            // either way: a collapsed diff shows fewer lines but the same numbers.
            let lines = text.body.lines().count().max(1);
            let gutter = if numbers {
                ui.painter()
                    .layout_no_wrap(
                        "0".repeat(lines.to_string().len()),
                        font.clone(),
                        t.text.secondary,
                    )
                    .size()
                    .x
                    + GUTTER_GAP * 2.0
            } else {
                0.0
            };

            // One galley for the whole thing, laid out once and cached by egui on the job. The
            // body is capped at `preview::TEXT_CAP` for exactly this reason: egui lays out every
            // line whether or not it is on screen.
            let plain = egui::TextFormat {
                font_id: font.clone(),
                color: t.text.primary,
                ..Default::default()
            };
            let hits = find.showing();
            let mut job = if spans.is_empty() && hits.is_empty() {
                // The common case, and worth keeping: a plain body is one section rather than a
                // vector of them.
                egui::text::LayoutJob::single_section(body.clone(), plain)
            } else {
                overlay(
                    body,
                    &(0..body.len()),
                    &coloured(body.len(), spans, &plain, t),
                    hits,
                    find.at,
                    t,
                )
            };
            job.wrap = egui::text::TextWrapping {
                max_width: (ui.available_width() - gutter).max(16.0),
                ..Default::default()
            };
            // **Every row of this view is as tall as the ink it holds.** See [`row_box`] — this is
            // the one line that decides where a band lands on a line of text, because *every* band
            // here is drawn on the row: the diff's, a search hit's, and the one egui paints behind a
            // text selection, which is not this program's to move.
            if let Some(height) = row_box(ui.painter(), &font) {
                for section in &mut job.sections {
                    section.format.line_height = Some(height);
                }
            }
            let galley = ui.painter().layout_job(job);
            // A slot in the paint list, taken *before* the text goes in it, so the diff's bands end up
            // underneath: a band drawn after the label would be a band over the line it is about.
            // `Shape::Noop` costs nothing when there is no diff to fill it with.
            let bands = ui.painter().add(egui::Shape::Noop);
            let shown = ui
                .horizontal(|ui| {
                    ui.add_space(gutter);
                    ui.add(egui::Label::new(galley.clone()).selectable(true))
                })
                .inner;

            // The current hit, brought into view.
            //
            // **Nudged rather than centred**, which is the same rule the listing follows for the
            // keyboard cursor and for the same reason: a view that recentres on every press of the
            // next-match arrow is a view you cannot read while you step through it. A hit already on
            // screen does not move the text at all.
            if find.reveal {
                // **Counted over `body` and not over the file**, because that is the string the
                // search ran over — see [`Text::shown`], which hands the find bar the diff's own body
                // whenever a diff is up. That body is *longer* than the file wherever a hunk took a
                // line away, since [`super::diff`] puts the removed lines back into it: a hit in the
                // last third of a diffed file is therefore a byte index past the end of the file, and
                // slicing the file with it panicked with "byte index out of bounds" — or with "not a
                // char boundary" as soon as anything above the hit was not ASCII.
                //
                // `get` rather than a slice, and no scroll at all when it answers `None`: a byte
                // index that came from another string must not be able to panic the panel, whatever
                // else goes out of step upstream.
                if let Some(front) = find.hits.get(find.at).and_then(|hit| body.get(..hit.start)) {
                    // A `CCursor` counts characters and a hit is a range of bytes, because a layout
                    // section is a range of bytes. Counted here rather than carried: it is one pass
                    // over the front of the file when the current hit changes, against a second
                    // vector the length of the hits on every keystroke.
                    let chars = front.chars().count();
                    let at = galley.pos_from_cursor(egui::text::CCursor::new(chars));
                    let mut want = at.translate(shown.rect.min.to_vec2());
                    // **Grown upwards by the height of the bar**, because the bar is floating over
                    // the top of this view: without it, "bring the hit into the viewport" is
                    // satisfied by a hit sitting exactly where the bar is, and stepping through the
                    // matches at the top of a file scrolls to each one and hides it.
                    want.min.y -= FIND_H + PAD * 2.0;
                    ui.scroll_to_rect(want, None);
                }
                find.reveal = false;
            }

            // **One walk over the galley's rows**, which is the only place the layout's own shape is
            // known: a wrapped paragraph is several rows of one logical line, so neither the numbers
            // nor the diff's bands can count rows.
            //
            // Everything here is limited to the rows on screen. A 15,000-line file is 15,000 numbers
            // and 15,000 candidate bands, and laying out or filling the ones nobody can see would
            // undo the whole reason the body is one galley.
            if numbers || text.view.is_some() {
                let origin = shown.rect.min;
                let clip = ui.clip_rect();
                let full = Rect::from_min_max(
                    pos2(canvas.left(), clip.top()),
                    pos2(canvas.right(), clip.bottom()),
                );
                // **The band is the row, and the row is the ink** — see [`row_box`], which is where
                // that is arranged. It used to be offset from the row by hand to get it onto the ink,
                // and the trouble with that was the two bands this code does *not* draw: a text
                // selection's and a search hit's are epaint's, and they are the row exactly.
                let mut painted: Vec<egui::Shape> = Vec::new();
                let mut line = 0usize;
                let mut starts = true;
                // The logical line the row being looked at belongs to, which is not the same thing as
                // the row: a wrapped line is several rows, and all of them are that line's. Carried
                // across the continuation rows so a change that wraps is banded to its last row —
                // banding only the row that starts it left the rest of an added paragraph bare.
                let mut fact: Option<Line> = None;
                for row in &galley.rows {
                    // Whether this row begins a line of the file, and what the *next* row will be.
                    // Both settled before anything is drawn, so no branch below can leave the walk out
                    // of step with the document — which is what a `continue` past the update did.
                    let first = starts;
                    starts = row.ends_with_newline;

                    let y = origin.y + row.pos.y;
                    let on_screen = y + row.row.size.y >= clip.top() && y <= clip.bottom();
                    if first {
                        // What this line *is*, which for a diff is the view's own answer and otherwise
                        // simply the next line of the file.
                        fact = match &text.view {
                            Some(view) => view.lines.get(line).copied(),
                            None => Some(Line {
                                mark: Mark::Same,
                                number: Some(line as u32 + 1),
                            }),
                        };
                        line += 1;
                    }
                    let Some(fact) = fact.filter(|_| on_screen) else {
                        continue;
                    };
                    // The band, across the whole panel rather than the text's own width: a diff line
                    // is the line, and a fill that stopped at the last glyph would leave the
                    // indentation of an indented line uncoloured — which is exactly the part that says
                    // how deep the change is.
                    let band = Rect::from_min_max(
                        pos2(full.left(), y),
                        pos2(full.right(), y + row.row.size.y),
                    );
                    if let Some(fill) = diff_fill(t, fact.mark) {
                        painted.push(egui::Shape::rect_filled(band, CornerRadius::ZERO, fill));
                    }
                    // The rest belongs to the *line* rather than to each of its rows, so a wrapped
                    // one is numbered once, at the top, the way an editor's gutter numbers it.
                    if !first {
                        continue;
                    }
                    // A collapsed region says how much it is holding back, painted over the blank line
                    // that stands for it — see [`Diffed::flush`].
                    if let Mark::Skipped(count) = fact.mark {
                        let label = ui.painter().layout_no_wrap(
                            // `·` and not `⋯`: the interpuncts are in the installed faces — the
                            // status line separates with one — and the three-dot leader is not, so
                            // it came out as a hollow box.
                            format!(
                                "· · ·   {count} unchanged line{}",
                                if count == 1 { "" } else { "s" }
                            ),
                            t.fonts.caption.clone(),
                            t.text.tertiary,
                        );
                        // On the band's own ink baseline, which is what puts a caption-sized label in
                        // the middle of a band sized for the body face. Centring its *box* in the row
                        // is what the first version did, and a smaller font's box is a different shape
                        // from the one the band was measured against.
                        galley_on_baseline(
                            ui.painter(),
                            origin.x,
                            ink_baseline(ui.painter(), &t.fonts.caption, band.top(), band.height()),
                            label,
                        );
                    }
                    if numbers {
                        if let Some(number) = fact.number {
                            let ink = if fact.mark == Mark::Removed {
                                t.status.danger
                            } else {
                                t.text.secondary
                            };
                            let galley = ui.painter().layout_no_wrap(
                                number.to_string(),
                                // The **same font as the body**, which is what guarantees the two
                                // share a baseline: a caption-sized number beside a body-sized line
                                // would sit a point above it, and a gutter that does not line up is
                                // worse than no gutter. It is placed at the row's own `y` for the
                                // same reason — the *line box*, not the shifted band, because it is
                                // text coming level with text.
                                font.clone(),
                                ink,
                            );
                            ui.painter().galley(
                                pos2(origin.x - GUTTER_GAP - galley.size().x, y),
                                galley,
                                Color32::PLACEHOLDER,
                            );
                        }
                    }
                    starts = row.ends_with_newline;
                }
                if !painted.is_empty() {
                    ui.painter().set(bands, egui::Shape::Vec(painted));
                }
            }

            if text.truncated {
                ui.add_space(space::S2);
                let mut how_much = String::new();
                crate::fs::fmt::size(preview::TEXT_CAP as u64, &mut how_much);
                ui.label(
                    egui::RichText::new(format!("— the first {how_much} of a longer file —"))
                        .font(t.fonts.caption.clone())
                        .color(t.text.secondary),
                );
            }
            ui.add_space(space::S3);
        });
    // A file longer than the panel says so at the edge, the same way the listing and the console do.
    // Measured off the `ScrollArea`, since the content here is all there is — see
    // `azur_egui_theme::components::scroll_fades` for the rule.
    //
    // Painted on `child` rather than inside the closure: in there the `Ui` is translated by the scroll
    // offset, and the fade belongs to the panel's edge and not to the document.
    azur_egui_theme::components::scroll_fades_of(
        &child.painter_at(output.inner_rect),
        &output,
        t.bg.layer,
    );
}

/// Bring the current hit into view, if it is in this block.
///
/// The same rule as [`text_canvas`]'s — **nudged rather than centred**, and grown upwards by the
/// height of the find bar so that a hit at the top of the document does not end up underneath it.
pub(super) fn reveal(
    ui: &Ui,
    marks: &Marks<'_>,
    block: &Range<usize>,
    galley: &egui::Galley,
    origin: egui::Pos2,
    text: &str,
) {
    if !*marks.reveal {
        return;
    }
    let Some(hit) = marks.hits.get(marks.at) else {
        return;
    };
    if hit.start < block.start || hit.start >= block.end {
        return;
    }
    // A `CCursor` counts characters, and it counts them from the start of *this galley* — which is
    // why the block's own slice is what is measured and not the document up to here.
    let chars = text[block.start..hit.start].chars().count();
    let mut want = galley
        .pos_from_cursor(egui::text::CCursor::new(chars))
        .translate(origin.to_vec2());
    want.min.y -= FIND_H + PAD * 2.0;
    ui.scroll_to_rect(want, None);
}

/// How a run of a document is set: the face, the colour, and the three decorations.
pub(super) fn ink(t: &Theme, faces: &Faces, style: markdown::Style, quoted: bool) -> egui::TextFormat {
    let font = if style.code {
        // The monospace face at the surrounding size, not at the body size: inline code inside a
        // heading has to be the heading's size or the line grows a step where the code is.
        faces.code.clone()
    } else if style.bold {
        egui::FontId::new(faces.base.size, faces.strong.clone())
    } else {
        faces.base.clone()
    };
    // A link is the one run whose colour is about what it *does*; a quote's is about where it is.
    let color = if style.link {
        t.text.link
    } else if quoted {
        t.text.secondary
    } else {
        t.text.primary
    };
    egui::TextFormat {
        font_id: font,
        color,
        italics: style.italic,
        // The one thing here that is not a colour or a decoration, and [`Faces::of`] is the note:
        // it is what keeps inline code on the same baseline as the words either side of it.
        line_height: style.code.then_some(faces.code_line).flatten(),
        background: if style.code {
            t.syntax.fill
        } else {
            Color32::TRANSPARENT
        },
        // **Two points rather than epaint's one**, and the reason is the line height above: the
        // fill behind a section is its *logical* box, so shortening that box to fix the baseline
        // took the same amount off the bottom of it and left a `g`'s tail hanging below the chip.
        // Two points puts most of it back, and what it buys either side is a chip with a little air
        // in it rather than a rectangle clamped to the glyphs — which is what an inline code span
        // looks like everywhere else it appears.
        expand_bg: if style.code { CHIP } else { 1.0 },
        underline: if style.link {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        strikethrough: if style.strike {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    }
}

/// A slice of `text` as a layout job: the formats it is set in, with the find's hits laid over the
/// top of them.
///
/// **Two layers, and the order is the point.** The lower one is what the text *is* — the body face,
/// or a syntax colour, or a markdown run's emphasis — and it covers `span` end to end. The upper one
/// is what is being *looked for*, and it wins wherever the two meet, because a highlight that a
/// keyword's colour showed through would be a highlight you could not see.
///
/// Two roles doing exactly what they are described as doing: `accent.subtle` is "an accent-tinted
/// fill quiet enough to put text on", which is every other hit, and `accent.default` with
/// `text.on_accent` is the one you are standing on. `every_ink_in_the_find_bar_can_be_read` measures
/// both, in both themes — a highlight you cannot read the text through is worse than no highlight,
/// because it hides the thing it is pointing at.
///
/// The sections have to be **contiguous and in order**, which epaint asserts rather than tolerates,
/// so the empty ones are dropped instead of pushed: `base` can hand this a zero-width part, and a
/// hit can start exactly where the one before it ended — `\b` and a pattern together do that, where
/// a literal search cannot.
///
/// `span` is where in `text` this job starts, which is how one document's blocks each get their own
/// galley while the hits stay offsets into the whole of it.
pub(super) fn overlay(
    text: &str,
    span: &Range<usize>,
    base: &[(Range<usize>, egui::TextFormat)],
    hits: &[Range<usize>],
    current: usize,
    t: &Theme,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        text: text[span.clone()].to_owned(),
        ..Default::default()
    };
    let mut push = |range: Range<usize>, format: egui::TextFormat| {
        if range.start >= range.end {
            return;
        }
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: egui::text::ByteIndex(range.start - span.start)
                ..egui::text::ByteIndex(range.end - span.start),
            format,
        });
    };
    // A cursor into the hits that only ever moves forward. Both lists are sorted, so the two are
    // walked together rather than the hits being scanned once per part — which over a coloured
    // megabyte would be thirty thousand parts against four thousand hits.
    let mut next = 0;
    for (part, format) in base {
        while next < hits.len() && hits[next].end <= part.start {
            next += 1;
        }
        let mut cut = part.start;
        for (i, hit) in hits.iter().enumerate().skip(next) {
            if hit.start >= part.end {
                break;
            }
            let (from, to) = (hit.start.max(part.start), hit.end.min(part.end));
            push(cut..from, format.clone());
            let (fill, ink) = if i == current {
                (t.accent.default, t.text.on_accent)
            } else {
                (t.accent.subtle, t.text.primary)
            };
            push(
                from..to,
                egui::TextFormat {
                    background: fill,
                    color: ink,
                    ..format.clone()
                },
            );
            cut = to;
        }
        push(cut..part.end, format.clone());
    }
    job
}

/// The body's own formats: `plain` everywhere, and a syntax colour where there is a span.
///
/// The gaps are filled rather than left out, because [`overlay`] needs a covering: a section that is
/// not there is not text set in the default face, it is text egui will refuse to lay out.
pub(super) fn coloured(
    len: usize,
    spans: &[syntax::Span],
    plain: &egui::TextFormat,
    t: &Theme,
) -> Vec<(Range<usize>, egui::TextFormat)> {
    let mut parts = Vec::with_capacity(spans.len() * 2 + 1);
    let mut cut = 0;
    for span in spans {
        if span.at.start > cut {
            parts.push((cut..span.at.start, plain.clone()));
        }
        parts.push((
            span.at.clone(),
            egui::TextFormat {
                color: t.tok(span.tok),
                ..plain.clone()
            },
        ));
        cut = span.at.end;
    }
    if cut < len {
        parts.push((cut..len, plain.clone()));
    }
    parts
}
