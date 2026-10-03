//! The application's name and its mark.
//!
//! The mark is not this program's. `azur-egui-theme/app-icons/` ships four application
//! marks — a task manager, this, an audio mixer, a program launcher — as one geometry
//! definition plus a rasterised set per application, and everything below is a reference to
//! the file-explorer one rather than a copy of it.
//!
//! | form | where | comes from |
//! | --- | --- | --- |
//! | a painted outline, 16px, one theme colour | the title bar | [`mark`] |
//! | one RGBA bitmap, decoded at startup | the window's taskbar button and Alt-Tab | [`window_icon`] |
//! | a nine-size `.ico` in the executable's resources | Explorer, a pinned shortcut, Alt-Tab before launch | `build.rs` |
//!
//! Ship only the first two and Explorer shows the generic application icon; ship only the
//! third and the taskbar button turns into egui's white `e` the moment the window opens.
//!
//! # Why by reference
//!
//! `app-icons/README.md` says to copy `marks.rs` into the application. Referencing it in
//! place does the same job without the copy, and the relative path is the one
//! `Cargo.toml` already depends on to find the theme at all. The point of the design
//! system holding the geometry is that four applications cannot drift apart; a vendored
//! copy is exactly how they would.
//!
//! What this program keeps is the *name*, and the wiring.
//!
//! # The mark
//!
//! A folder, in one stroked outline, on a dark plate: `GRAY_2` with the mark in `AZURE_70`,
//! which is the "dark chrome" row of `docs/icons.md` §15 rather than the white-on-azure
//! default. The design system's own note on that is worth knowing — against a dark Windows
//! 11 taskbar the plate is 1.1:1 and effectively invisible, so the icon reads there as a
//! blue folder floating on the bar. That is a decision recorded in
//! `app-icons/README.md`, and `PLATE` in the theme's `examples/app_icons.rs` is the one
//! place to revisit it.
//!
//! Inside the window the mark takes whatever colour it is handed, as any icon does.

/// The design system's mark definitions, included where they live rather than copied here.
///
/// `#[path]` reaches out of the crate, which is unusual and deliberate: see the note above.
/// The other three marks come with it and belong to other applications, hence the `allow` —
/// they cost nothing in the binary, being `const` data nothing refers to.
#[path = "../../azur-egui-theme/app-icons/marks.rs"]
#[allow(dead_code)]
mod marks;

/// What the window is called.
pub const NAME: &str = "Azur File Explorer";

/// The mark as an icon inside the window: an outline, in one colour, on the design system's
/// grid and at the stroke weight `Pen` takes from the box it is given.
///
/// A thin wrapper on [`marks::file_explorer`] so that the rest of this program refers to
/// "the brand's mark" rather than to a name in the design system's icon set — the title bar
/// should not have to know which of the four this application is.
pub fn mark(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    marks::file_explorer(painter, rect, color);
}

/// The mark as the bitmap the window shows in the taskbar and in Alt-Tab.
///
/// 64px of the nine the design system ships. There is only one slot — `IconData` is a single
/// bitmap, and Windows scales it for the taskbar button (32 at 100% scaling), Alt-Tab, and
/// the window's own small icon (16). 64 halves exactly into both of those, where the 256 the
/// design system's README reaches for would be an eighth-scale reduction into the smallest
/// of them, which is the "grey mush" `docs/icons.md` §13 warns about.
///
/// The hand-snapped 16 and 20 in the `.ico` cannot be used here, because nothing in the
/// window-icon path takes more than one size. They are what Explorer reads.
pub fn window_icon() -> egui::IconData {
    // One of the two places this program names a file in the design system's tree; the other
    // is `build.rs`, which hands `app.ico` to the linker.
    const PNG: &[u8] = include_bytes!("../../azur-egui-theme/app-icons/file-explorer/app-64.png");

    let image = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .expect("the design system's app-64.png did not decode as a PNG")
        .to_rgba8();
    egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The design system's icon set for this application, as a filesystem path.
    const ICONS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../azur-egui-theme/app-icons/file-explorer"
    );

    #[test]
    fn the_window_icon_is_the_design_system_s_bitmap() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (64, 64));
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);

        let pixels: Vec<[u8; 4]> = icon.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
        // A plate fills the square, so most of it is opaque, and its corners are rounded, so
        // something is not. Between them they catch a blank or a wrongly decoded file.
        let opaque = pixels.iter().filter(|p| p[3] == 255).count();
        let clear = pixels.iter().filter(|p| p[3] == 0).count();
        assert!(opaque * 100 / pixels.len() > 60, "only {opaque} of 4096 pixels are opaque");
        assert!(clear > 0, "nothing is transparent, so the plate has square corners");

        // And it is painted in the two palette colours `app-icons/README.md` names, rather
        // than in something merely near them.
        let plate = azur_egui_theme::tokens::palette::GRAY_2;
        let ink = azur_egui_theme::tokens::palette::AZURE_70;
        for (name, want) in [("plate", plate), ("mark", ink)] {
            let found = pixels
                .iter()
                .filter(|p| p[3] == 255 && [p[0], p[1], p[2]] == [want.r(), want.g(), want.b()])
                .count();
            assert!(
                found > 40,
                "only {found} pixels are the {name} colour #{:02X}{:02X}{:02X}",
                want.r(),
                want.g(),
                want.b()
            );
        }
    }

    #[test]
    fn the_executable_s_icon_carries_every_size() {
        // What `build.rs` hands the linker, checked here because a missing or truncated
        // `.ico` is otherwise only a `cargo:warning` nobody reads, and the result is an
        // executable wearing the generic icon.
        let path = std::path::Path::new(ICONS).join("app.ico");
        let ico = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

        assert_eq!(&ico[0..4], &[0, 0, 1, 0], "not an ICO header");
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;

        // Each directory entry is 16 bytes; the first is the width, with 0 meaning 256.
        let sizes: Vec<u32> = (0..count)
            .map(|i| match ico[6 + i * 16] {
                0 => 256,
                w => u32::from(w),
            })
            .collect();
        assert_eq!(
            sizes,
            vec![16, 20, 24, 32, 40, 48, 64, 128, 256],
            "the sizes Windows asks for are not all in the icon"
        );
    }
}
