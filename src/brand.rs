//! The application's name and its mark.
//!
//! The mark is one rendered image — a blue folder behind a stack of pages, with a magnifier
//! over it, on a near-black rounded plate — and it lives in this program's own
//! [`assets/app-icon/`](../../assets/app-icon/) rather than in the design system. Three forms
//! of it are wired up, and all three are the same picture at different sizes:
//!
//! | form | where | comes from |
//! | --- | --- | --- |
//! | one RGBA bitmap at the size the bar asks for | the title bar | [`mark`] |
//! | one RGBA bitmap, decoded at startup | the window's taskbar button and Alt-Tab | [`window_icon`] |
//! | a nine-size `.ico` in the executable's resources | Explorer, a pinned shortcut, Alt-Tab before launch | `build.rs` |
//!
//! Ship only the first two and Explorer shows the generic application icon; ship only the
//! third and the taskbar button turns into egui's white `e` the moment the window opens.
//!
//! # Why the raster, and why not the design system's
//!
//! `azur-egui-theme/app-icons/` ships four application marks as one geometry definition plus
//! a rasterised set each, and this program used to reference the file-explorer one in place:
//! a folder in a single stroked outline, drawn by a `Painter` in whatever colour it was
//! handed. That is the better shape for a 16px glyph, and it is the reason the title bar's
//! mark was a vector at all — an outline snapped to the pixel grid cannot go mushy.
//!
//! It is not what this application wears now. The mark is a shaded three-dimensional render,
//! which no `Painter` call reproduces, so every form of it is a bitmap and the title bar's is
//! a bitmap too. The cost is paid exactly where the vector used to earn its keep: at 16px the
//! magnifier is three grey pixels and the mark reads by silhouette rather than by detail.
//! [`LADDER`] is what keeps that as small as it can be.
//!
//! # The plate
//!
//! Near-black — measured `#191D23`, a shade off the design system's `GRAY_2` — with the art
//! in azure. The design system's note on choosing a dark plate still applies and is still
//! worth knowing: against a dark Windows 11 taskbar such a plate is about 1.1:1 and
//! effectively invisible, so the icon reads there as a blue folder floating on the bar. In
//! the title bar the same thing happens in the dark theme and does not in the light one,
//! where the plate is a dark chip. Both are legible, which is the whole reason the plate is
//! kept in the title bar instead of the art being cut out of it: the pages are near-white and
//! would vanish into a light bar without it.

/// What the window is called.
pub const NAME: &str = "Azur File Explorer";

/// The sizes the mark is rasterised at for use *inside* the window, smallest first.
///
/// One entry per size Windows' scale factors ask for at the title bar's 16pt: 16 at 100%, 20
/// at 125%, 24 at 150%, 32 at 200%, 40 at 250%, 48 at 300%. [`pick`] takes the first that
/// covers the request, so the two intermediate factors land on the size above them (175% asks
/// for 28 and gets the 32) and every whole one is an exact match drawn 1:1 in device pixels.
///
/// That is the point of a ladder rather than one large bitmap minified per frame: a shaded
/// render reduced 3:1 by the sampler is mush, and the same reduction done once by Lanczos
/// when the asset was built is not. `assets/app-icon/README.md` has the recipe.
///
/// 128 and 256 are deliberately absent — nothing in the window draws the mark that big, and
/// they would be 78 KB of the binary for it. They exist in the `.ico`, where Explorer reads
/// them, and as files for anything outside this program that wants them.
const LADDER: &[(u32, &[u8])] = &[
    (16, include_bytes!("../assets/app-icon/app-16.png")),
    (20, include_bytes!("../assets/app-icon/app-20.png")),
    (24, include_bytes!("../assets/app-icon/app-24.png")),
    (32, include_bytes!("../assets/app-icon/app-32.png")),
    (40, include_bytes!("../assets/app-icon/app-40.png")),
    (48, include_bytes!("../assets/app-icon/app-48.png")),
    (64, include_bytes!("../assets/app-icon/app-64.png")),
];

/// The smallest rung of [`LADDER`] that covers `px` device pixels, or the largest there is.
///
/// Never the rung *below* the request: scaling a bitmap up is the one artefact worth avoiding
/// outright, where scaling the next one down is a filtered reduction of about a sixth.
fn pick(px: u32) -> (u32, &'static [u8]) {
    let last = LADDER[LADDER.len() - 1];
    LADDER.iter().copied().find(|(size, _)| *size >= px).unwrap_or(last)
}

/// The mark as an icon inside the window, drawn to fill `rect`.
///
/// Takes no colour, unlike every other icon in this window and unlike the outline this
/// replaced: the mark is a full-colour image and tinting it would only dirty it. The title
/// bar's hover fill goes behind it, which is what makes it read as a button.
///
/// The texture is built once per size and kept in the context's own cache — one bitmap that
/// never changes, where uploading per frame would be uploading per frame. Keyed by the
/// chosen size rather than by the request, so a window dragged between monitors of different
/// scale reuses whichever rungs it has already decoded.
pub fn mark(painter: &egui::Painter, rect: egui::Rect) {
    let ctx = painter.ctx();
    // Device pixels, from the shorter side: the rect is square everywhere this is called
    // from, and asking for the smaller of the two can only round down into an exact rung.
    let want = (rect.size().min_elem() * ctx.pixels_per_point()).round().max(1.0) as u32;
    let (size, png) = pick(want);

    let id = egui::Id::new(("brand-mark", size));
    let texture = match ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        Some(cached) => cached,
        None => {
            let image = decode(png, size);
            let handle = ctx.load_texture(
                format!("brand-mark-{size}"),
                egui::ColorImage::from_rgba_unmultiplied(
                    [size as usize, size as usize],
                    image.as_raw(),
                ),
                // Linear, because the exact rung is only guaranteed at a whole scale factor;
                // at 175% this is a 32 drawn into 28 and nearest would tear the plate's edge.
                egui::TextureOptions::LINEAR,
            );
            ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
            handle
        }
    };

    painter.image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

/// The mark as the bitmap the window shows in the taskbar and in Alt-Tab.
///
/// 64px of the nine sizes that exist. There is only one slot — `IconData` is a single bitmap,
/// and Windows scales it for the taskbar button (32 at 100% scaling), Alt-Tab, and the
/// window's own small icon (16). 64 halves exactly into both of those, where the 256 would be
/// an eighth-scale reduction into the smallest of them.
///
/// The hand-sized 16 and 20 cannot be used here, because nothing in the window-icon path
/// takes more than one size. They are what Explorer reads, out of the `.ico`.
pub fn window_icon() -> egui::IconData {
    let (size, png) = pick(64);
    debug_assert_eq!(size, 64, "the 64px rung is gone from the ladder");
    let image = decode(png, size);
    egui::IconData {
        width: size,
        height: size,
        rgba: image.as_raw().to_vec(),
    }
}

/// One rung of the ladder, decoded.
///
/// `expect` rather than a fallback: the bytes are `include_bytes!` of a file in this
/// repository, so a failure here is a broken build rather than anything a user can cause,
/// and the tests below decode every rung.
fn decode(png: &[u8], size: u32) -> image::RgbaImage {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .unwrap_or_else(|e| panic!("assets/app-icon/app-{size}.png did not decode as a PNG: {e}"))
        .to_rgba8();
    debug_assert_eq!(
        (image.width(), image.height()),
        (size, size),
        "app-{size}.png is not {size}x{size}"
    );
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This program's icon set, as a filesystem path.
    const ICONS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/app-icon");

    #[test]
    fn the_window_icon_is_the_64px_rung() {
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

        // And it is the *mark* rather than some other image: the three things it is made of,
        // counted loosely. Measured at 1625 plate, 1698 azure and 357 near-white; the bounds
        // are a third of each, which no re-render of the same artwork will cross and a
        // wrong or empty file cannot reach.
        let opaque = |p: &&[u8; 4]| p[3] == 255;
        let plate = pixels.iter().filter(opaque).filter(|p| p[0].max(p[1]).max(p[2]) < 60).count();
        // Widened, because `p[0] + 60` overflows a `u8` on the near-white of the pages.
        let azure = pixels
            .iter()
            .filter(opaque)
            .filter(|p| p[2] > 120 && u16::from(p[2]) > u16::from(p[0]) + 60)
            .count();
        let pages = pixels.iter().filter(opaque).filter(|p| p[0].min(p[1]).min(p[2]) > 200).count();
        assert!(plate > 500, "only {plate} pixels are the dark plate");
        assert!(azure > 500, "only {azure} pixels are the azure of the folder");
        assert!(pages > 100, "only {pages} pixels are the near-white of the pages");
    }

    #[test]
    fn every_rung_decodes_at_the_size_it_claims() {
        // The ladder's sizes are what `pick` matches against and what the texture is drawn
        // at, so a mislabelled rung would silently scale. `decode`'s own `debug_assert`
        // catches it too, but only in a debug build and only for the rung being drawn.
        for (size, png) in LADDER {
            let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
                .unwrap_or_else(|e| panic!("app-{size}.png did not decode: {e}"))
                .to_rgba8();
            assert_eq!((image.width(), image.height()), (*size, *size), "app-{size}.png");
        }
    }

    #[test]
    fn the_ladder_covers_every_scale_factor_windows_offers() {
        // 16pt in the title bar, at the scale factors Windows' display settings expose.
        // Each has to land on a rung at least as large, or the mark is a bitmap scaled up.
        for (percent, want) in [
            (100, 16),
            (125, 20),
            (150, 24),
            (175, 28),
            (200, 32),
            (225, 36),
            (250, 40),
            (300, 48),
        ] {
            let (size, _) = pick(want);
            assert!(size >= want, "{percent}% scaling wants {want}px and the ladder gave {size}");
        }
        // Above the top rung it saturates rather than wrapping round to the smallest.
        assert_eq!(pick(1000).0, 64);
        assert_eq!(pick(1).0, 16);
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
