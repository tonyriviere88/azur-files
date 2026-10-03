//! Rasterising vector art, at the size the panel will show it.
//!
//! `resvg`, with `default-features = false` — which is what leaves `<text>` elements
//! undrawn, and why this says so on the canvas rather than quietly leaving a hole.

use super::*;

/// SVG, rasterised to fit `cap` on its long edge.
///
/// Sized by a cap rather than by the space it is going into, because there are two callers wanting
/// two very different sizes and neither wants the *panel's*: the panel is resized and zoomed, and
/// re-rasterising on every frame that changes would be a parse and a fill per frame. It passes
/// [`CAP`], and the cost is that zooming far past fit goes soft — which is what happens to a raster
/// image too. [`crate::shell::thumbs`] passes its own cell, because a tile that asked for 2048 would
/// rasterise four megapixels to fill 96 of them.
///
/// One decoder for both, so a tile and the panel behind it cannot disagree about what a drawing looks
/// like. Reached from outside this module as `preview::vector_art`.
///
/// **Text inside the SVG is not drawn.** `usvg`'s text support means a font database, shaping and
/// system font enumeration — see the dependency's justification in `Cargo.toml` — and the honest
/// thing is to say so, which [`Picture::vector`] is for.
pub(crate) fn art(path: &Path, cap: u32) -> Result<Picture, String> {
    let bytes = std::fs::read(path).map_err(|_| "Cannot be opened".to_owned())?;
    from_bytes(&bytes, cap)
}

/// The same, from a drawing already in memory — the bytes git had for it.
pub(super) fn from_bytes(bytes: &[u8], cap: u32) -> Result<Picture, String> {
    use resvg::tiny_skia;
    use resvg::usvg;

    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default())
        .map_err(|why| short(&why.to_string()))?;

    let size = tree.size();
    if size.width() < 1.0 || size.height() < 1.0 {
        return Err("The drawing has no size".to_owned());
    }
    // Always to the cap on the long edge, up as well as down. A 16-point icon is the commonest
    // thing in a folder of SVGs and rasterising it at its nominal size would put a 16-pixel
    // square in the middle of a 400-point panel — the whole point of vector art is that there is
    // no natural size to respect. The cap is what bounds the cost, and it is the same cap every
    // other picture here is held at.
    let ratio = cap as f32 / size.width().max(size.height());
    let width = ((size.width() * ratio) as u32).max(1);
    let height = ((size.height() * ratio) as u32).max(1);

    let mut canvas =
        tiny_skia::Pixmap::new(width, height).ok_or_else(|| "Too large to draw".to_owned())?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(ratio, ratio),
        &mut canvas.as_mut(),
    );

    // `tiny_skia` hands back premultiplied RGBA, which is what egui wants — so the bytes go
    // across as they are rather than through the unmultiplied constructor, which would divide
    // the alpha back out and then multiply it in again.
    let pixels = egui::ColorImage {
        size: [width as usize, height as usize],
        pixels: canvas
            .pixels()
            .iter()
            .map(|p| {
                egui::Color32::from_rgba_premultiplied(p.red(), p.green(), p.blue(), p.alpha())
            })
            .collect(),
        source_size: egui::vec2(width as f32, height as f32),
    };
    Ok(Picture {
        pixels,
        natural: [size.width() as u32, size.height() as u32],
        scaled: false,
        vector: true,
    })
}
