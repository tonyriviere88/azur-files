use super::*;

/// A merged chain's path split into where the row is and what it shows, and joined back for the
/// column.
///
/// [`chain_split`] is asked by the Name column and by the tooltip, and the two must not disagree
/// about which folders belong to the row — so it is one function with one test rather than the
/// same arithmetic written twice. The cases that matter are the ends: no merging at all, where it
/// has to be the split `Dir::within` and `Dir::leaf` already make; a chain that reaches the folder
/// being listed, which has nothing in front of it; and a level count larger than the path has,
/// which cannot be produced by the builder but must not be a panic if it ever is.
#[test]
fn a_chain_splits_into_where_it_is_and_what_it_shows() {
    assert_eq!(chain_split(r"x\a\b\c", 0), (r"x\a\b", "c"));
    assert_eq!(chain_split("c", 0), ("", "c"));
    assert_eq!(chain_split(r"x\a\b\c", 2), ("x", r"a\b\c"));
    assert_eq!(chain_split(r"a\b\c", 2), ("", r"a\b\c"));
    assert_eq!(chain_split(r"a\b", 9), ("", r"a\b"));

    // And the dimmed half of the cell, which is those folders and a trailing separator.
    let mut into = String::new();
    chain_ahead(chain_folders(r"a\b\c"), false, &mut into);
    assert_eq!(into, "a > b > ");
    chain_ahead(chain_folders(r"a\b\c"), true, &mut into);
    assert_eq!(into, "… > a > b > ", "and an ellipsis for what was dropped");
    chain_ahead(chain_folders("c"), false, &mut into);
    assert_eq!(into, "", "one folder has nothing in front of it");
}

/// A rename field is as wide as its text, not as wide as the column.
///
/// The Name column is whatever the other three leave, which on a narrow pane is narrower
/// than plenty of names — and a field pinned to it scrolls the name sideways under the
/// caret, so renaming a long one meant editing it through a letterbox. It grows instead,
/// over Size, Type and Modified, and stops at the row's right edge.
#[test]
fn a_rename_field_grows_past_its_column_but_not_past_the_row() {
    // A name cell from 40 to 160 in a row 600 wide.
    let (left, right, row_right) = (40.0, 160.0, 600.0);
    let column = right - left;

    // Short text: the column's width, unchanged. Nothing overlaps that does not need to.
    assert_eq!(rename_width(30.0, left, right, row_right), column);
    // Text that just fits stays put too.
    assert_eq!(rename_width(column - 12.0, left, right, row_right), column);

    // Longer than the column: wide enough for the text, and wider than the column.
    let long = rename_width(300.0, left, right, row_right);
    assert!(
        long > column,
        "a name wider than its column got a field the width of the column ({long})"
    );
    assert!(
        long >= 300.0,
        "the field is narrower than the text it has to show ({long})"
    );

    // Never past the row. A 5000px name gets the rest of the row and no more.
    let huge = rename_width(5_000.0, left, right, row_right);
    assert!(
        left + huge <= row_right,
        "the field runs {} past the right edge of the row",
        left + huge - row_right
    );
    // And on a pane too narrow for any of this, still something to type in.
    assert!(rename_width(400.0, 40.0, 44.0, 60.0) >= MIN_FIELD);
}

/// The Name cell: the name, then its context in secondary ink, and the context is what
/// gives way when the column is narrow.
///
/// Measured against the real font, because every claim here is about what fits: the widths
/// below are found by laying the text out, not assumed.
#[test]
fn a_row_says_where_it_is_after_its_name_and_gives_that_up_first() {
    let ctx = egui::Context::default();
    azur_egui_theme::fonts::install(&ctx);
    let font = egui::FontId::proportional(14.0);
    let (ink, dim) = (egui::Color32::WHITE, egui::Color32::GRAY);
    let mut checked = false;
    let _ = ctx.run_ui(Default::default(), |ui| {
        let painter = ui.painter();
        let width_of = |text: &str| {
            crate::ui::truncated(painter, text, font.clone(), ink, f32::INFINITY)
                .size()
                .x
        };
        let cell = |context: Option<&str>, width: f32| {
            name_galley(
                painter,
                "translations_fr.json",
                context,
                font.clone(),
                ink,
                dim,
                width,
            )
        };

        // No context — a file in the folder you are looking at, which is not a shortcut.
        // One section, in body ink, and nothing else drawn.
        let plain = cell(None, 400.0);
        assert_eq!(plain.text(), "translations_fr.json");
        assert_eq!(plain.job.sections.len(), 1);
        assert_eq!(plain.job.sections[0].format.color, ink);

        // With context: `name > where`, and **everything from the separator on is
        // secondary**, which is the whole of what makes the name the thing you read.
        const WHERE: &str = "PluginGeosystem\\Resources\\Lang";
        let whole = cell(Some(WHERE), 900.0);
        assert_eq!(whole.text(), format!("translations_fr.json > {WHERE}"));
        assert!(!whole.elided);
        assert_eq!(whole.job.sections.len(), 2);
        assert_eq!(whole.job.sections[0].format.color, ink);
        assert_eq!(
            whole.job.sections[1].format.color, dim,
            "the context is drawn in the same ink as the name"
        );
        let dimmed = whole.job.sections[1].byte_range.clone();
        let separator = &whole.text()[dimmed.start.0..dimmed.end.0];
        assert!(
            separator.starts_with(" > "),
            "the separator belongs to the dimmed half, and got {separator:?}"
        );

        // Narrower: the context loses its *leading* folders, not its last one. The folder
        // immediately holding the file is what identifies it.
        let tail = "translations_fr.json > …\\Lang";
        let cut = cell(Some(WHERE), width_of(tail) + 4.0);
        assert_eq!(
            cut.text(),
            tail,
            "the folders furthest from the file are what should have gone"
        );

        // Narrower still: the name survives and the context is whatever fits. The name is
        // first in the job, so it is the last thing egui cuts.
        let squeezed = cell(Some(WHERE), width_of("translations_fr.json") + 6.0);
        assert!(
            squeezed.text().starts_with("translations_fr.json"),
            "what survived was {:?}, which is not the name",
            squeezed.text()
        );
        checked = true;
    });
    assert!(checked, "the pass never ran, so nothing was checked");
}

/// What a rename starts with selected.
///
/// Explorer selects the stem, so that typing replaces the name and leaves the extension
/// alone. The awkward cases are the point: a dotfile has no extension to speak of, a
/// double extension only counts the last one, and a folder is a folder whatever is in its
/// name.
#[test]
fn a_rename_selects_the_name_and_not_the_extension() {
    assert_eq!(stem_chars("report.docx", false), "report".len());
    assert_eq!(stem_chars("archive.tar.gz", false), "archive.tar".len());
    // A dotfile is all name.
    assert_eq!(stem_chars(".gitignore", false), ".gitignore".chars().count());
    // No extension at all.
    assert_eq!(stem_chars("Makefile", false), "Makefile".len());
    // A folder called `my.folder` is not a `folder` file.
    assert_eq!(stem_chars("my.folder", true), "my.folder".chars().count());
    // Counted in characters, not bytes, or the caret lands mid-glyph: `réunion` is seven
    // characters and eight bytes, so this number is the whole point of the assertion.
    assert_eq!(stem_chars("réunion.txt", false), 7);
    assert_eq!("réunion".len(), 8, "and it would be 8 if this counted bytes");
    assert_eq!(stem_chars("", false), 0);
}

/// Every ink on the status line can be read on the status line.
///
/// The bar is `background-layer-alt`, one rung off the surface every other status mark in this
/// window sits on — so the one thing this catches is a colour chosen for its meaning and never
/// checked where it landed. It has caught three, and none of them was subtle: Azur's
/// `status.success` measures **2.37:1** on the light bar and its `status.warning` **2.23:1**, both
/// well under the floor for a shape, let alone for `13 changed`; and the scan's own figure was in
/// `text-disabled` at **2.20:1** in the dark theme, which is no way to show a number somebody is
/// invited to check. [`crate::theme::Bar`] is the answer to the first two and carries the
/// measurements.
///
/// **Nearly everything here is held to [`TEXT`]**, because nearly everything here is read rather
/// than glanced at: a count, a branch name, `13 changed`. The exception is `text-tertiary` at
/// **3.62:1** on the dark bar, which is the deliberately quietest thing on the line — the scan's
/// figure and the parenthesis saying how much is not on show — and is held to [`SHAPE`].
/// `crate::theme::Syntax` draws the same line the other way, where a screenful of diff *is* text
/// and gets two rungs of its own for it.
#[test]
fn every_ink_on_the_status_line_can_be_read() {
    use azur_egui_theme::contrast::{ratio, SHAPE, TEXT};

    for t in Theme::all() {
        let name = t.palette.key();
        // The region rather than the role it used to borrow: the light palette gives the status
        // bar a colour of its own, and measuring these inks on `background-layer-alt` would then
        // be measuring them on the popover surface instead of on the bar they are drawn in.
        let bar = t.surfaces.status;
        for (what, ink) in [
            ("the selected count", t.bar.counted),
            ("the total", t.text.secondary),
            ("an operation in progress", t.text.primary),
            ("the branch", t.bar.info),
            ("commits ahead", t.bar.success),
            ("commits behind", t.bar.danger),
            ("what has changed", t.bar.warning),
        ] {
            let got = ratio(ink, bar);
            assert!(
                got >= TEXT,
                "{name}: {what} is {got:.2}:1 on the status bar, under the {TEXT}:1 floor for text"
            );
        }
        for (what, ink) in [
            ("what is not shown", t.text.tertiary),
            ("the scan's own figure", t.text.tertiary),
        ] {
            let got = ratio(ink, bar);
            assert!(
                got >= SHAPE,
                "{name}: {what} is {got:.2}:1 on the status bar, under the {SHAPE}:1 floor"
            );
        }
    }
}

/// A word on the status line and a glyph on it come out on the same middle, at any scale and
/// wherever the pane's bottom edge happens to land.
///
/// The two failures this catches are the two halves of [`status_geometry`]. **Pairing the wrong
/// two helpers**: the line used to centre each galley's *box* while centring each glyph on its
/// ink, which is the mistake [`CELL_LIFT`] exists to correct one row up. And **a band off the
/// pixel grid**: a fractional rect is feathered at both edges, so its visible middle is not the
/// one anything was centred on — which is why the rect is rounded before the baseline is taken.
///
/// The ink's own middle is derived from `galley_baseline` and `ink_lift` rather than measured off
/// the glyphs: `azur::components` owns the measurement, and what is being checked here is that
/// this line asks it the right question.
#[test]
fn the_status_line_puts_its_words_and_its_glyphs_on_one_middle() {
    use azur_egui_theme::components::{galley_baseline, ink_lift};
    use egui::emath::GuiRounding as _;

    let t = Theme::dark();
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let ctx = egui::Context::default();
        azur_egui_theme::fonts::install(&ctx);
        ctx.set_pixels_per_point(scale);
        // Twice, because the first pass has no fonts laid out yet and `set_pixels_per_point`
        // only reaches the painter on the pass after it.
        for _ in 0..2 {
            let _ = ctx.run_ui(Default::default(), |ui| {
                let painter = ui.painter();
                // Every awkward fraction of a point a split can leave a pane's bottom edge on.
                for offset in [0.0, 0.1, 0.3, 0.5, 0.72, 0.9] {
                    let asked = Rect::from_min_size(
                        pos2(0.0, 400.0 + offset),
                        vec2(600.0, STATUS_HEIGHT),
                    );
                    let (band, line, baseline) = status_geometry(painter, &t, asked);
                    let device = 1.0 / scale;

                    // The band is on whole device pixels, and it is still the height it asked to
                    // be — rounded, not floored.
                    assert_eq!(
                        band.top(),
                        band.top().round_to_pixels(scale),
                        "the band's top is off the grid at {scale}× / {offset}"
                    );
                    assert_eq!(band.bottom(), band.bottom().round_to_pixels(scale));
                    assert!(
                        (band.height() - STATUS_HEIGHT).abs() <= device,
                        "the band came out {} tall rather than {STATUS_HEIGHT}",
                        band.height()
                    );

                    // The line the content is placed against is the band nudged up, and it is the
                    // *whole* line that moves: a glyph centred on `line` and a word on `line`'s
                    // own baseline still have to land on one middle, or [`NUDGE`] would be
                    // giving away the level the rest of this bought.
                    assert_eq!(
                        band.top() - line.top(),
                        NUDGE,
                        "the line is not {NUDGE} above the band it is painted in"
                    );
                    let glyph = icon_rect(line, 8.0, MARK).center().y;
                    let probe = painter.layout_no_wrap(
                        "Ay".to_owned(),
                        t.fonts.caption.clone(),
                        t.text.primary,
                    );
                    // `ink_lift` is the ink's middle measured from a *box's* middle, so half the
                    // ink's height above the baseline is what it and the box baseline give back.
                    let half = galley_baseline(&probe)
                        - probe.size().y * 0.5
                        - ink_lift(painter, &t.fonts.caption);
                    let middle = baseline - half;
                    assert!(
                        (middle - glyph).abs() <= 1.0_f32.max(device),
                        "at {scale}× / {offset}: the words sit {:+.2} off the glyphs",
                        middle - glyph
                    );
                }
            });
        }
    }
}

/// **A jump leaves two rows showing past the row it found**; a step does not.
///
/// The type-ahead lands on a row anywhere in the listing, and brought only to the edge of the view
/// that row sat on the very last line. With the context it stops two rows short of whichever edge
/// it came in by — and a row already on show does not move the listing at all, context or not.
#[test]
fn a_jump_leaves_room_around_the_row_it_lands_on() {
    let (row, view) = (ROW_HEIGHT, 10.0 * ROW_HEIGHT);
    // Row 30, from the top of the listing: past the bottom edge.
    let top = 30.0 * row;
    assert_eq!(nudge(0.0, view, top, row, false), top + row - view, "a step: flush with the edge");
    assert_eq!(
        nudge(0.0, view, top, row, true),
        top + row + SCROLL_CONTEXT * row - view,
        "a jump: two rows below it on show"
    );
    // Row 5, from row 20: past the top edge.
    let top = 5.0 * row;
    assert_eq!(nudge(20.0 * row, view, top, row, false), top);
    assert_eq!(nudge(20.0 * row, view, top, row, true), top - SCROLL_CONTEXT * row);
    // Already on show, and not only at an edge: nothing moves.
    assert_eq!(nudge(0.0, view, 4.0 * row, row, true), 0.0);
    // Near the top of the listing there is nothing above to show, and no negative offset.
    assert_eq!(nudge(10.0 * row, view, row, row, true), 0.0);
    // A view too short for the margin centres the row as best it can rather than hiding it.
    let short = 2.0 * row;
    let offset = nudge(0.0, short, 30.0 * row, row, true);
    assert!(offset <= 30.0 * row && offset + short >= 31.0 * row, "the row is on show: {offset}");
}
