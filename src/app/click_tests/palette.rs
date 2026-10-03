//! Every colour the window paints comes from the palette it is set to.
//!
//! One test, and it exists because of one bug: the rule under the column headers was a
//! **hard-coded `Color32::from_rgb(0x20, 0x23, 0x29)`** — `tokens::palette::GRAY_4`, which is the
//! dark theme's `stroke-subtle`. So the line that divides a header strip from the listing under it
//! drew a near-black hairline across every pane of the light palette, and had done for as long as
//! there was one.
//!
//! Nothing caught it, and it is worth being precise about why, because the reason generalises:
//!
//! - **It was correct where it was written.** In the dark theme that hex *is* the right colour, so
//!   the window it was developed against looked right.
//! - **Every contrast test measures roles, not pixels.** `theme.rs` and the measurements in
//!   `ui::` ask whether `text-primary` reads on `surfaces.header`; none of them can see a painter
//!   call that never asked the theme anything.
//! - **It is one point tall.** A hairline is exactly the size of thing an eye slides over and a
//!   screenshot review does not catch.
//!
//! So the guard is not another ratio. It is the *provenance* of the colour: paint the window in
//! each palette and check that nothing on screen is a surface belonging to a **different** one. A
//! literal borrowed from the dark theme fails this on the light side the moment it is drawn, which
//! is the only kind of check that would have caught the original.

use super::Harness;
use crate::app::Action;
use crate::theme::{Palette, Theme};
use egui::Color32;

/// The neutral surfaces a palette paints with — the ones that differ between palettes.
///
/// Deliberately **not** the accent, the status hues, the file-kind colours or the syntax palette:
/// every palette takes those from Azur's own side unchanged, so they are shared by construction
/// and a shared colour can prove nothing about which palette painted it.
fn neutrals(t: &Theme) -> Vec<Color32> {
    let mut out: Vec<Color32> = t.surfaces.each().iter().map(|(_, c)| *c).collect();
    out.extend([
        t.bg.canvas,
        t.bg.layer,
        t.bg.layer_alt,
        t.bg.card,
        t.bg.control,
        t.bg.control_hover,
        t.bg.control_active,
        t.bg.control_disabled,
        t.stroke.subtle,
        t.stroke.default,
        t.stroke.control,
        t.stroke.strong,
        t.row_alt,
        crate::ui::hover_fill(t),
        azur_egui_theme::desktop::press_fill(t.azur()),
    ]);
    out
}

/// A palette's own hover survives the desktop preset.
///
/// `azur::desktop::apply` moves both hover tokens onto its own rung, keyed on which *side* of the
/// palette this is — so it hands every light palette the same lavender-grey, whatever hue that
/// palette's own surfaces are. `Theme::of` runs it **before** the palette for that reason, and
/// this is the guard on the ordering: with the two
/// calls the other way round, as they were, a hover set by a palette is silently put back and the
/// only symptom is a band that looks slightly wrong under the pointer.
///
/// Asserted against the rung the preset *would* have handed it rather than against a hex, so
/// retuning the colour does not touch this test: what must hold is that a palette which sets its
/// own hover keeps it, and that a hovered row and a hovered card are still one colour.
#[test]
fn the_preset_does_not_overrule_the_palettes_hover() {
    // What `desktop::place_hover` gives a light theme, straight from the design system: the rung
    // the light palette would be wearing if the preset had run after it.
    let preset = azur_egui_theme::tokens::paper::SLATE_6;
    let ours = crate::ui::hover_fill(&Theme::of(Palette::Light));
    assert_ne!(
        ours, preset,
        "the light palette's hover is the preset's own rung — `desktop::apply` has run after the \
         palette again and put it back. See `Theme::of`."
    );
    for palette in Palette::ALL {
        let t = Theme::of(palette);
        assert_eq!(
            t.bg.card_hover,
            t.bg.control_hover,
            "{palette:?}: a hovered row and a hovered button are different colours"
        );
        assert_eq!(
            crate::ui::hover_fill(&t),
            t.bg.control_hover,
            "{palette:?}: `hover_fill` has stopped reading the token"
        );
    }
}

/// Nothing the window paints belongs to a palette it is not set to.
#[test]
fn every_colour_on_screen_belongs_to_the_palette_it_is_set_to() {
    for palette in Palette::ALL {
        let mut h = Harness::new();
        h.app.perform(&h.ctx.clone(), Action::SetTheme(palette));
        h.settle();
        assert_eq!(h.app.theme.palette, palette, "the window did not change palette");

        let mine = neutrals(&h.app.theme);
        // Every neutral of every *other* palette, less anything this one paints too. The
        // subtraction is what keeps the test about provenance rather than about coincidence:
        // `stroke-subtle` and the seam are one value in the dark palette, and its `control-active`
        // is a colour the light one paints nothing with.
        let foreign: Vec<(Palette, Color32)> = Palette::ALL
            .into_iter()
            .filter(|p| *p != palette)
            .flat_map(|p| {
                let theirs = neutrals(&Theme::of(p));
                theirs
                    .into_iter()
                    .filter(|c| !mine.contains(c))
                    .map(move |c| (p, c))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            !foreign.is_empty(),
            "{:?}: no colour distinguishes this palette from the others, so this test proves \
             nothing — it needs a different discriminator",
            palette
        );

        // Both kinds of paint: a filled rect is how a surface is laid down, and a stroke is how a
        // hairline is. The header rule that started this was a `rect_filled` one point tall, and
        // `ui::rule_below` draws the same idea as a segment — so a guard that looked at only one
        // of the two would have caught the bug and not its neighbour.
        let painted = h
            .rects()
            .into_iter()
            .map(|(rect, _, fill)| (format!("a fill at {:?}", rect), fill))
            .chain(
                h.segments()
                    .into_iter()
                    .map(|(ends, colour)| (format!("a line at {:?}", ends), colour)),
            );

        for (what, colour) in painted {
            // Only opaque paint. A translucent wash — a drop target, a heat tint, a scrim — is a
            // colour composited at draw time and is not claiming to be a surface.
            if colour.a() != 255 {
                continue;
            }
            if let Some((owner, _)) = foreign.iter().find(|(_, c)| *c == colour) {
                panic!(
                    "{palette:?}: {what} is {colour:?}, which is {owner:?}'s surface and not this \
                     palette's — a colour written as a literal instead of read from the theme. \
                     See the header rule in `ui::filelist::columns`, which is why this test exists."
                );
            }
        }
    }
}
