use super::*;

fn flags(bits: Flags) -> Flags {
    bits
}

/// The two colours a plain cell is drawn in are the panel's own, and a program's choice wins.
#[test]
fn a_cell_with_no_colour_of_its_own_uses_the_panels() {
    let t = Theme::dark();
    let (fg, bg) = ink(
        &t,
        Ink::Named(NamedColor::Foreground),
        Ink::Named(NamedColor::Background),
        Flags::empty(),
        false,
    );
    assert_eq!((fg, bg), (t.ansi.fg, t.ansi.bg));

    // `ls` colouring a directory blue asks for index 4 and must get the agreed blue.
    let (fg, _) = ink(&t, Ink::Named(NamedColor::Blue), Ink::Named(NamedColor::Background), Flags::empty(), false);
    assert_eq!(fg, t.ansi.normal[4]);
    // And a 24-bit colour is used exactly as given.
    let (fg, _) = ink(
        &t,
        Ink::Spec(crate::term::Rgb { r: 1, g: 2, b: 3 }),
        Ink::Named(NamedColor::Background),
        Flags::empty(),
        false,
    );
    assert_eq!(fg, Color32::from_rgb(1, 2, 3));
}

/// Bold is the bright half of the palette, because there is no bold monospace face to use.
#[test]
fn bold_brightens_rather_than_thickens() {
    let t = Theme::dark();
    let plain = ink(&t, Ink::Named(NamedColor::Green), Ink::Named(NamedColor::Background), Flags::empty(), false);
    let bold = ink(
        &t,
        Ink::Named(NamedColor::Green),
        Ink::Named(NamedColor::Background),
        flags(Flags::BOLD),
        false,
    );
    assert_eq!(plain.0, t.ansi.normal[2]);
    assert_eq!(bold.0, t.ansi.bright[2]);
    assert_ne!(plain.0, bold.0);
}

/// Inverse swaps the cell's own two colours, and a selection takes only the background.
///
/// Both halves matter. `SGR 7` is a program asking for *its* colours swapped, so it has to
/// happen before the selection is applied. And the selection must leave the foreground alone —
/// forcing an ink as well would flatten a selected `git diff` into one colour, which is the
/// opposite of what somebody selecting a diff is trying to read.
#[test]
fn inverse_swaps_and_a_selection_keeps_the_text_its_own_colour() {
    let t = Theme::dark();
    let (fg, bg) = ink(
        &t,
        Ink::Named(NamedColor::Foreground),
        Ink::Named(NamedColor::Background),
        flags(Flags::INVERSE),
        false,
    );
    assert_eq!((fg, bg), (t.ansi.bg, t.ansi.fg), "inverse did not swap");

    // A red line of a diff, selected: still red.
    let (fg, bg) = ink(
        &t,
        Ink::Named(NamedColor::Red),
        Ink::Named(NamedColor::Background),
        Flags::empty(),
        true,
    );
    assert_eq!(fg, t.ansi.normal[1], "the selection flattened the output's colour");
    assert_eq!(bg, selection(&t));
    assert_ne!(bg, t.ansi.bg, "a selection that changes nothing is not a selection");
}

/// A hidden cell draws its text in its own background — which is what makes a typed password
/// invisible rather than merely dark.
#[test]
fn hidden_text_is_the_colour_of_what_is_behind_it() {
    let t = Theme::dark();
    let (fg, bg) = ink(
        &t,
        Ink::Named(NamedColor::Red),
        Ink::Named(NamedColor::Background),
        flags(Flags::HIDDEN),
        false,
    );
    assert_eq!(fg, bg);
}

/// The 256-colour cube's arithmetic, at the three places it is easy to get wrong.
#[test]
fn the_colour_cube_is_indexed_the_way_xterm_numbers_it() {
    let palette = Theme::dark().ansi.palette();
    // 16 is the cube's black corner and 231 its white one.
    assert_eq!(palette.at(16), crate::term::Rgb { r: 0, g: 0, b: 0 });
    assert_eq!(palette.at(231), crate::term::Rgb { r: 0xff, g: 0xff, b: 0xff });
    // 196 is pure red: (196-16) = 180 = 5*36, so r is the last step and g and b the first.
    assert_eq!(palette.at(196), crate::term::Rgb { r: 0xff, g: 0, b: 0 });
    // The greyscale ramp starts at 8 and climbs by 10.
    assert_eq!(palette.at(232), crate::term::Rgb { r: 8, g: 8, b: 8 });
    assert_eq!(palette.at(255), crate::term::Rgb { r: 238, g: 238, b: 238 });
    // And the two above the cube are the panel's own pair.
    assert_eq!(palette.at(256), palette.fg);
    assert_eq!(palette.at(257), palette.bg);
}

/// The one pair held to a contrast ratio, and the reason the other sixteen are not.
///
/// Everything a shell prints without asking for a colour is this pair, so it is the pair that
/// has to be readable. The sixteen are a protocol — `git` marks a deletion with colour 1 and is
/// not asking for the red that suits this window — and colour 0 on the dark background is
/// unreadable *by design*.
#[test]
fn the_default_pair_is_readable_in_both_themes() {
    use azur_egui_theme::contrast::{ratio, TEXT};
    for t in [Theme::dark(), Theme::light()] {
        let measured = ratio(t.ansi.fg, t.ansi.bg);
        assert!(
            measured >= TEXT,
            "{} on the terminal background is {measured:.2}:1",
            if t.dark { "dark" } else { "light" }
        );
    }
}

/// Bold has to be visible, and in the light theme it is only visible for half the palette.
///
/// Which is not a transcription error — it is VS Code's own Light+ palette, and the reason is
/// the one this program keeps running into: brightening a hue on paper walks it towards white
/// and out of contrast, so red, blue, magenta and cyan have no brighter version that stays
/// readable and repeat themselves instead. Pinned rather than papered over, because inventing
/// brighter ones would put `ls` in colours no other terminal uses.
#[test]
fn bold_is_visible_in_the_dark_palette_and_half_the_light_one() {
    let dark = Theme::dark().ansi;
    for slot in 0..8 {
        assert_ne!(dark.normal[slot], dark.bright[slot], "dark slot {slot}");
    }

    let light = Theme::light().ansi;
    let moved: Vec<usize> = (0..8)
        .filter(|slot| light.normal[*slot] != light.bright[*slot])
        .collect();
    assert_eq!(
        moved,
        vec![0, 2, 3, 7],
        "black, green, yellow and white are the four that can brighten on paper"
    );
}
