//! Decoding a raster picture, and the two bounds that keep a photograph from costing the
//! window a quarter of a second and 160 MB. Vector art is [`super::vector`]'s.

use super::*;

/// A decoded picture, waiting to be uploaded.
pub struct Picture {
    pub pixels: egui::ColorImage,
    /// What it is on disk, which is what the panel reports — `pixels` may be smaller.
    ///
    /// **Upright, not as stored.** A camera records a portrait photograph as a landscape frame plus
    /// an EXIF tag; [`raster_from`] honours the tag, so the edges here are swapped to match what the
    /// panel is actually showing. Reporting the stored 4032 × 3024 under a picture that is plainly
    /// taller than it is wide would be the bar contradicting the canvas above it.
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
        raster(path, CAP)
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
///
/// **`name` is the working file's path, not a file these bytes are in**, and it is here for the two
/// questions a blob cannot answer for itself: whether this is vector art, and — for the one extension
/// [`named`] covers — what format it is. A blob out of git has no extension of its own.
pub(super) fn decode(bytes: &[u8], name: &Path) -> Result<Picture, String> {
    if is_vector(name) {
        vector::from_bytes(bytes, CAP)
    } else {
        raster_from(
            || {
                let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                    .with_guessed_format()
                    .map_err(|_| "Cannot be read".to_owned())?;
                Ok(sniffed(reader, name))
            },
            CAP,
        )
    }
}

/// Anything `image` can decode, out of a file, at no more than `cap` on the long edge.
///
/// **`cap` because a tile wants [`crate::shell::thumbs::CELL`] where the panel wants [`CAP`]**, which
/// is the same parameter [`vector::art`] takes and it is there for the same reason: one decoder for
/// both, so a tile and the panel behind it cannot come to different conclusions about a file. Reached
/// from outside this module as `preview::raster_art`, by the tiles of the formats Windows itself has no
/// codec for — see [`crate::shell::thumbs`], where the list of them is.
pub(crate) fn raster(path: &Path, cap: u32) -> Result<Picture, String> {
    // The format from the *contents* rather than from the extension, which is how a `.jpg` that
    // is really a PNG — and there are a great many of those — still opens.
    raster_from(
        || {
            let reader = image::ImageReader::open(path)
                .map_err(|_| "Cannot be opened".to_owned())?
                .with_guessed_format()
                .map_err(|_| "Cannot be read".to_owned())?;
            Ok(sniffed(reader, path))
        },
        cap,
    )
}

/// A reader whose format is settled: what the contents said, and [`named`] where they said nothing.
fn sniffed<R: std::io::BufRead + std::io::Seek>(
    mut reader: image::ImageReader<R>,
    name: &Path,
) -> image::ImageReader<R> {
    // Only where nothing else knew, so a name never overrules the bytes — the whole point of the
    // sniff above is that the extension is the least reliable thing about a picture.
    if reader.format().is_none() {
        if let Some(format) = named(name) {
            reader.set_format(format);
        }
    }
    reader
}

/// The format for a name `image` knows neither by its extension nor by its magic.
///
/// **One entry, and `.cur` is it.** A cursor is an icon whose directory entries hold a hotspot where an
/// icon's hold the colour planes and the bit depth — which `image`'s ICO decoder reads past and ignores,
/// so the pixels come out — but its magic is `00 00 02 00` against an icon's `00 00 01 00`, and `cur`
/// is not in `image`'s extension table either. Both of the questions above therefore answer "no idea",
/// and without this a `.cur` is the one name in [`super::PICTURES`] whose preview is a library's
/// complaint about the extension rather than a picture.
///
/// `.ani` is not here and cannot be: an animated cursor is a RIFF container with icons inside it, which
/// is a decoder rather than a name. It is not in [`super::PICTURES`], so it goes to the shell.
fn named(path: &Path) -> Option<image::ImageFormat> {
    let ext = path.extension()?.to_str()?;
    ext.eq_ignore_ascii_case("cur")
        .then_some(image::ImageFormat::Ico)
}

/// The decode itself, over whatever the bytes are coming from.
///
/// `open` is called **twice** — once for the dimensions and once for the pixels — which is why it is
/// a closure rather than a reader: the header has to be read before there is any decision to decode,
/// and a reader that has been read is a reader that has moved. Opening a file twice costs a handle
/// and a warm page cache; making a second `Cursor` over a slice costs nothing.
///
/// `cap` is the long edge anything larger is scaled down to. [`CAP`] for the panel and
/// [`crate::shell::thumbs::CELL`] for a tile; [`DECODE_MAX`] is not a parameter, because how much this
/// is allowed to *decode* on the way is the same question whoever asked.
///
/// **EXIF orientation is applied here**, which is the reason the second `open` goes through
/// `into_decoder` rather than `decode` — see the comment on it. Every caller wants that: a photograph
/// is upright in the panel, in a diff, in a git blob and on a tile, or it is upright in none of them.
fn raster_from<R: std::io::BufRead + std::io::Seek>(
    open: impl Fn() -> Result<image::ImageReader<R>, String>,
    cap: u32,
) -> Result<Picture, String> {
    // The trait behind `orientation`, `total_bytes` and `set_limits`, none of which are inherent
    // methods on a decoder — and the enum, which is matched on rather than merely passed along.
    use image::metadata::Orientation;
    use image::ImageDecoder;

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

    // **The rotation a camera recorded rather than applied.** A phone writes the sensor's own
    // landscape readout plus an EXIF `Orientation` tag saying which way up it was being held, and
    // `image` hands back the pixels exactly as stored. Without this line a portrait photograph
    // previews on its side while the *tile* for the same file, two inches away, is upright — a tile
    // comes from `IShellItemImageFactory` and the shell's codec honours the tag. Two decoders behind
    // one panel, and this is what keeps them agreeing about the same photograph.
    //
    // Which is why the second `open` produces a decoder rather than going straight to `decode`:
    // `ImageReader::decode` builds one, consumes it and throws it away, and the tag is only
    // reachable through it. In `image` 0.25 that means JPEG, PNG and WebP; a tagged `.tif` is still
    // shown as stored, which is a gap in the library rather than one here — and JPEG is the format
    // every camera on earth writes this tag into.
    let mut decoder = open()?
        .into_decoder()
        .map_err(|why| short(&why.to_string()))?;
    // The one thing `ImageReader::decode` did on the way past that `into_decoder` leaves to its
    // caller: the allocation ceiling, checked against the size before the buffer is asked for.
    // `Limits::default()` is exactly what the reader was carrying, because nothing here sets any.
    let mut limits = image::Limits::default();
    limits
        .reserve(decoder.total_bytes())
        .map_err(|why| short(&why.to_string()))?;
    decoder
        .set_limits(limits)
        .map_err(|why| short(&why.to_string()))?;
    // Absent, malformed, and "this decoder cannot read EXIF at all" are one answer here. There is
    // nothing to do differently about a picture that never said which way up it goes, and failing a
    // preview over a metadata tag would be a worse bug than the one this fixes.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|why| short(&why.to_string()))?;
    // Turned before `into_rgba8`, so a JPEG moves the three bytes a pixel it actually has rather
    // than four.
    decoded.apply_orientation(orientation);
    let decoded = decoded.into_rgba8();

    // **What the bar reports, now that the picture has been turned.** `into_dimensions` above read
    // the header, and a header describes the file as stored: a portrait photograph is 4032 × 3024
    // there and 3024 × 4032 on the screen. The bar has to say the second — and so does the zoom
    // percentage, which divides the decoded width by `natural[0]` and would otherwise read 75% at
    // 1:1 for every photograph a camera turned.
    //
    // The two bounds above are left measuring the stored size on purpose: one is a product and the
    // other a `max`, and neither cares which way round the edges are.
    let natural = match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (natural.1, natural.0),
        Orientation::NoTransforms
        | Orientation::Rotate180
        | Orientation::FlipHorizontal
        | Orientation::FlipVertical => natural,
    };

    let (width, height) = (decoded.width(), decoded.height());
    let scaled = width > cap || height > cap;
    let decoded = if scaled {
        // `Triangle` rather than `Lanczos3`: this is a preview, the difference is invisible at
        // the size it is shown, and Lanczos over a 200-megapixel scan is seconds rather than
        // milliseconds.
        //
        // Which holds at a tile's `cap` too, and for a better reason than "it is small": `resize`
        // scales the filter's support by the ratio, so a 512-pixel picture going into a 96-pixel cell
        // is averaged over the ten pixels each output one covers rather than sampled out of them.
        let ratio = (cap as f32 / width.max(height) as f32).min(1.0);
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
