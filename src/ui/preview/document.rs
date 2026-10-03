//! Markdown, drawn as a document rather than as its source: headings, paragraphs, code
//! blocks, bullets and rules.
//!
//! The parse is [`crate::markdown`]'s and happens once, when the file arrives. This is only
//! the painting.

use super::*;

/// The air either side of a line number.
///
/// Wider than [`PAD`], and the reason is that this gap is between two runs of *text* rather than
/// between a control and its edge: at four points the number and the first character of the line
/// read as one word.
pub(super) const GUTTER_GAP: f32 = space::S3;

/// One level of list nesting, in a rendered document.
///
/// Wider than the marker box, so a nested list's bullet is clear of the text of the item above it —
/// which is the only thing that makes the nesting visible at all in a panel this narrow.
pub(super) const NEST: f32 = 16.0;

/// The bar beside a block quote, and the air between it and the text.
pub(super) const QUOTE_BAR: f32 = 2.0;

pub(super) const QUOTE_GAP: f32 = space::S3;

/// How far the fill behind inline code reaches past the glyphs, in points. See `ink`.
pub(super) const CHIP: f32 = 2.0;

/// The room a list marker gets before its text, before [`GUTTER_GAP`] is added.
///
/// Enough for `99.`, measured against rather than assumed: a wider marker takes the room it needs,
/// and only the ones narrower than this are padded, so every item in a list starts at one x.
pub(super) const MARKER: f32 = 14.0;

/// A rendered document: one block at a time, down the canvas.
///
/// **Nothing is one galley here**, which is the difference from [`text_canvas`] and the reason the
/// two are separate functions. A code block has a fill behind it and a quote has a bar beside it; a
/// list item hangs its text off a marker; a heading is a different size from the paragraph under it
/// and wants air above it. None of that is expressible as a format over one run of text, so each
/// block is laid out and drawn on its own.
///
/// What that costs is line numbers — a rendered document has no line numbers, and the bar's toggle
/// hides itself accordingly — and what it buys is that the *find bar keeps working unchanged*: the
/// hits are offsets into [`markdown::Doc::text`], each block owns a slice of it, and [`overlay`]
/// takes the slice. See [`crate::markdown`] for why the document is one string.
pub(super) fn draw(
    ui: &mut Ui,
    t: &Theme,
    canvas: Rect,
    spot: Spot,
    doc: &markdown::Doc,
    find: &mut Find,
) {
    use markdown::Kind;

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(canvas.shrink2(vec2(PAD * 2.0, 0.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(canvas.intersect(ui.clip_rect()));
    let output = egui::ScrollArea::vertical()
        .id_salt(("preview-doc", spot))
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let marks = &mut find.marks();
            // The semibold family, taken from the role that already has it rather than named here:
            // a heading is `body-strong` at a heading's size, and the family behind that role is
            // the design system's to choose.
            let strong = t.fonts.body_strong.family.clone();
            let mut last: Option<&Kind> = None;
            for block in &doc.blocks {
                let air = air_before(last, &block.kind);
                if air > 0.0 {
                    ui.add_space(air);
                }
                last = Some(&block.kind);

                let base = match &block.kind {
                    Kind::Heading(level) => egui::FontId::new(heading(*level), strong.clone()),
                    Kind::Code(_) | Kind::Row { .. } => t.fonts.mono.clone(),
                    _ => t.fonts.body.clone(),
                };
                // Per block, because a heading's inline code has to come into line with a heading.
                let faces = Faces::of(ui.painter(), base, strong.clone());
                let step = QUOTE_BAR + QUOTE_GAP;
                let left = block.quote as f32 * step + block.indent as f32 * NEST;

                let row = ui
                    .horizontal_top(|ui| {
                        ui.add_space(left);
                        match &block.kind {
                            Kind::Rule => rule_across(ui, t),
                            Kind::Code(_) => code_block(ui, t, doc, block, &faces.base, marks),
                            Kind::Item(marker) => {
                                bullet(ui, t, marker.as_ref(), block.indent, &faces.base);
                                paragraph(ui, t, doc, block, &faces, marks);
                            }
                            _ => paragraph(ui, t, doc, block, &faces, marks),
                        }
                    })
                    .response
                    .rect;

                // The quote bars, one per level, in the gutter the indent left for them.
                for level in 0..block.quote {
                    let x = row.left() + level as f32 * step;
                    ui.painter().rect_filled(
                        Rect::from_min_size(
                            pos2(x.round(), row.top()),
                            vec2(QUOTE_BAR, row.height()),
                        ),
                        CornerRadius::ZERO,
                        t.stroke.strong,
                    );
                }
                // A rule under the two headings that carry one, and under a table's header row.
                // Both are the same gesture — *what follows belongs to this* — and both are the
                // hairline every other divider in this window uses.
                let ruled = matches!(
                    block.kind,
                    Kind::Heading(1 | 2) | Kind::Row { header: true }
                );
                if ruled {
                    ui.add_space(space::S1);
                    rule_across(ui, t);
                }
            }
            // Cleared here rather than in each branch: a hit that is somehow in no block at all must
            // not leave the next frame trying to scroll to it again.
            *marks.reveal = false;
            ui.add_space(space::S3);
        });
    // As [`text_canvas`], and for the same reason: a document that carries on past the edge of the
    // panel has to look like it does.
    azur_egui_theme::components::scroll_fades_of(
        &child.painter_at(output.inner_rect),
        &output,
        t.bg.layer,
    );
}

/// One block of prose — a heading, a paragraph, a table row, or a list item's text.
pub(super) fn paragraph(
    ui: &mut Ui,
    t: &Theme,
    doc: &markdown::Doc,
    block: &markdown::Block,
    faces: &Faces,
    marks: &mut Marks<'_>,
) {
    let quoted = block.quote > 0;
    let mut parts: Vec<(Range<usize>, egui::TextFormat)> = Vec::with_capacity(block.runs.len());
    let mut cut = block.at.start;
    for run in &block.runs {
        // A gap between runs cannot happen — `inline` covers the block end to end — but a covering
        // is what `overlay` needs and the cost of insisting on it here is one comparison.
        if run.at.start > cut {
            parts.push((cut..run.at.start, ink(t, faces, Default::default(), quoted)));
        }
        parts.push((run.at.clone(), ink(t, faces, run.style, quoted)));
        cut = run.at.end;
    }
    if cut < block.at.end {
        parts.push((cut..block.at.end, ink(t, faces, Default::default(), quoted)));
    }
    let mut job = overlay(&doc.text, &block.at, &parts, marks.hits, marks.at, t);
    job.wrap = egui::text::TextWrapping {
        max_width: ui.available_width().max(16.0),
        ..Default::default()
    };
    let galley = ui.painter().layout_job(job);
    let shown = ui.add(egui::Label::new(galley.clone()).selectable(true));
    reveal(ui, marks, &block.at, &galley, shown.rect.min, &doc.text);
}

/// A code block: the monospace face on a recessed fill, coloured by [`crate::syntax`].
pub(super) fn code_block(
    ui: &mut Ui,
    t: &Theme,
    doc: &markdown::Doc,
    block: &markdown::Block,
    base: &egui::FontId,
    marks: &mut Marks<'_>,
) {
    let plain = egui::TextFormat {
        font_id: base.clone(),
        color: t.text.primary,
        ..Default::default()
    };
    // The fence's own language, already worked out — see `markdown::Doc::spans`. Sliced rather than
    // recomputed, and by the same walk `overlay` does over the hits.
    let mut parts = Vec::new();
    let mut cut = block.at.start;
    for span in &doc.spans {
        if span.at.end <= block.at.start {
            continue;
        }
        if span.at.start >= block.at.end {
            break;
        }
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
    if cut < block.at.end {
        parts.push((cut..block.at.end, plain.clone()));
    }

    egui::Frame::new()
        .fill(t.syntax.fill)
        .corner_radius(CornerRadius::same(radius::SMALL))
        .inner_margin(egui::Margin::symmetric(space::S3 as i8, space::S2 as i8))
        .show(ui, |ui| {
            // Full width whatever the code is: a band that stops at the longest line makes a
            // three-line block look like three separate ones.
            ui.set_min_width(ui.available_width());
            let mut job = overlay(&doc.text, &block.at, &parts, marks.hits, marks.at, t);
            job.wrap = egui::text::TextWrapping {
                max_width: ui.available_width().max(16.0),
                ..Default::default()
            };
            let galley = ui.painter().layout_job(job);
            let shown = ui.add(egui::Label::new(galley.clone()).selectable(true));
            reveal(ui, marks, &block.at, &galley, shown.rect.min, &doc.text);
        });
}

/// A list item's marker, in the hanging indent before its text.
///
/// Three bullets down the levels, which is what every document that nests a list does — a filled
/// disc, then a ring, then a square — because "one level in" has to be visible without counting the
/// indent. A number gets its own text and a full stop.
pub(super) fn bullet(ui: &mut Ui, t: &Theme, marker: Option<&u64>, depth: u8, base: &egui::FontId) {
    let text = match marker {
        Some(n) => format!("{n}."),
        None => match depth % 3 {
            0 => "\u{2022}".to_owned(),
            1 => "\u{25E6}".to_owned(),
            _ => "\u{25AA}".to_owned(),
        },
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text, base.clone(), t.text.secondary);
    // Right-aligned in a fixed box, so `9.` and `10.` put their text at the same place. The width is
    // the box or the marker, whichever is wider: a hundredth item must not push into its own text.
    let width = galley.size().x.max(MARKER) + GUTTER_GAP;
    let (rect, _) = ui.allocate_exact_size(vec2(width, galley.size().y), Sense::hover());
    ui.painter().galley(
        pos2(rect.right() - GUTTER_GAP - galley.size().x, rect.top()),
        galley,
        Color32::PLACEHOLDER,
    );
}

/// A hairline across whatever room is left.
pub(super) fn rule_across(ui: &mut Ui, t: &Theme) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, t.stroke.default);
}

/// The air above a block, given what came before it.
///
/// A table of pairs rather than a margin per kind, because vertical rhythm is a fact about a
/// *boundary*: two list items want nothing between them and the same item under a paragraph wants
/// air, and neither of those is a property of an item.
pub(super) fn air_before(last: Option<&markdown::Kind>, next: &markdown::Kind) -> f32 {
    use markdown::Kind::*;
    let Some(last) = last else {
        // The top of the document. A little, so the first line is not welded to the bar.
        return space::S2;
    };
    match (last, next) {
        // Rows and items pack: a table with air between its rows is not a table.
        (Row { .. }, Row { .. }) => 0.0,
        (Item(_), Item(_)) => space::S1,
        // A heading is about what follows it, so it sits closer to that than to what it left.
        (Heading(_), _) => space::S2,
        (_, Heading(1 | 2)) => space::S5,
        (_, Heading(_)) => space::S4,
        (_, Rule) | (Rule, _) => space::S3,
        _ => space::S2,
    }
}

/// A heading's size, by level.
///
/// The design system's ramp as far as it goes — `title`, `title-small`, and `body-strong` for
/// anything past the third level, on the grounds that a `#####` in a README is a label rather than a
/// heading. The one number that is not a role is the third: there is no 16pt semibold in the ramp
/// (`subtitle` is 16 regular), and a document with three heading levels needs three sizes.
pub(super) fn heading(level: u8) -> f32 {
    match level {
        1 => 20.0,
        2 => 17.5,
        3 => 16.0,
        _ => 14.0,
    }
}
