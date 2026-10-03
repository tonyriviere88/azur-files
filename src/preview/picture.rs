//! Decoding a raster picture, and the two bounds that keep a photograph from costing the
//! window a quarter of a second and 160 MB. Vector art is [`super::vector`]'s.

use super::*;

/// A decoded picture, waiting to be uploaded.
pub struct Picture {
    pub pixels: egui::ColorImage,
    /// What it is on disk, which is what the panel reports — `pixels` may be smaller.
    pub natural: [u32; 2],
    /// It was larger than [`CAP`] and has been scaled down.
    pub scaled: bool,
    /// Vector art, rasterised at [`CAP`] rather than decoded at a natural size.
    pub vector: bool,
    /// Not decoded here at all: this is Windows' registered visualizer's render of a file nothing in
    /// this program can read. See [`super::visual`].
    ///
    /// The panel has to know, because `natural` then means something different — the size of the
    /// *render* rather than of the file, which has no pixel size of its own — and because a first page
    /// standing in for a document is worth saying out loud.
    pub shell: bool,
}

pub(super) fn load(path: &Path) -> Payload {
    match if is_vector(path) {
        vector::art(path, CAP)
    } else {
        raster(path)
    } {
        Ok(picture) => Payload::Picture(Box::new(picture)),
        Err(why) => Payload::Failed(why),
    }
}

/// Whether this name is vector art, which is the one thing about a picture that is decided by its
/// name rather than by its bytes.
///
/// Asked of the *name* because that is all a blob out of git has — and because `usvg` and `image` are
/// two different decoders rather than two formats one decoder sniffs between.
///
/// `pub(crate)` for [`crate::shell::thumbs`], which asks it to decide the one kind of file it draws
/// itself instead of handing to the shell. One list of vector extensions, so a tile and the panel
/// cannot come to different conclusions about the same name.
pub(crate) fn is_vector(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
}

/// A picture already in memory: the bytes git had for it.
///
/// The same two decoders [`load`] chooses between, and the same bounds — [`raster_from`] is where
/// the megapixel cap and the [`CAP`] scale-down live, so a blob cannot get past a limit a file cannot.
pub(super) fn decode(bytes: &[u8], vector: bool) -> Result<Picture, String> {
    if vector {
        vector::from_bytes(bytes, CAP)
    } else {
        raster_from(|| {
            image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .map_err(|_| "Cannot be read".to_owned())
        })
    }
}

/// Anything `image` can decode, out of a file.
fn raster(path: &Path) -> Result<Picture, String> {
    // The format from the *contents* rather than from the extension, which is how a `.jpg` that
    // is really a PNG — and there are a great many of those — still opens.
    raster_from(|| {
        image::ImageReader::open(path)
            .map_err(|_| "Cannot be opened".to_owned())?
            .with_guessed_format()
            .map_err(|_| "Cannot be read".to_owned())
    })
}

/// The decode itself, over whatever the bytes are coming from.
///
/// `open` is called **twice** — once for the dimensions and once for the pixels — which is why it is
/// a closure rather than a reader: the header has to be read before there is any decision to decode,
/// and a reader that has been read is a reader that has moved. Opening a file twice costs a handle
/// and a warm page cache; making a second `Cursor` over a slice costs nothing.
fn raster_from<R: std::io::BufRead + std::io::Seek>(
    open: impl Fn() -> Result<image::ImageReader<R>, String>,
) -> Result<Picture, String> {
    // **How big it is, before deciding to decode it.** Only the header is read for this, and it
    // is the one bound that has to come first: `image` has no streaming resize, so a decode is
    // the full size in memory however small the answer is going to be. A 40-megapixel image is
    // 160 MB while it is being scaled down, and that is as far as a preview gets to go.
    let natural = open()?
        .into_dimensions()
        .map_err(|why| short(&why.to_string()))?;
    let pixels = natural.0 as u64 * natural.1 as u64;
    if pixels > DECODE_MAX {
        return Err(format!(
            "Too large to preview: {:.0} megapixels",
            pixels as f64 / 1e6
        ));
    }

    let decoded = open()?
        .decode()
        .map_err(|why| short(&why.to_string()))?
        .into_rgba8();

    let (width, height) = (decoded.width(), decoded.height());
    let scaled = width > CAP || height > CAP;
    let decoded = if scaled {
        // `Triangle` rather than `Lanczos3`: this is a preview, the difference is invisible at
        // the size it is shown, and Lanczos over a 200-megapixel scan is seconds rather than
        // milliseconds.
        let ratio = (CAP as f32 / width.max(height) as f32).min(1.0);
        image::imageops::resize(
            &decoded,
            ((width as f32 * ratio) as u32).max(1),
            ((height as f32 * ratio) as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        decoded
    };

    Ok(Picture {
        pixels: egui::ColorImage::from_rgba_unmultiplied(
            [decoded.width() as usize, decoded.height() as usize],
            decoded.as_raw(),
        ),
        natural: [natural.0, natural.1],
        scaled,
        vector: false,
        shell: false,
    })
}
