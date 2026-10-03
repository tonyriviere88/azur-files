//! `IShellItemImageFactory`: the picture the shell would show, at the size asked for.
//!
//! The Windows half of [`crate::shell::thumbs`], and the one place in this program that calls
//! `GetImage`. The cache, the queue and the atlas next door are all portable; this is only the one
//! call that produces pixels — and it has two callers who want different things out of it. See
//! [`Want`].

use super::*;

/// What will do as an answer.
///
/// The one thing a tile and the preview panel disagree about, and they disagree completely.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Want {
    /// A picture of the file where it has one, and the file's own icon where it has not.
    ///
    /// What [`crate::ui::grid`]'s tiles want, and what makes the grid one code path rather than
    /// two: every cell holds something, and the something for a `.zip` is its icon asked for at
    /// [`CELL`] rather than the 16-point image list cell the details view draws.
    Whatever,
    /// A render of what is *in* the file, or nothing at all.
    ///
    /// What [`crate::preview::visual`] wants, and the reason this is a parameter rather than a
    /// constant. A file's icon blown up to fill a panel four hundred points wide is not a preview
    /// of the file — it is the glyph the row beside it is already showing, enormous and soft. So
    /// nothing is the better answer here, and the panel says so in words instead.
    Rendered,
}

/// Ask the shell for one file's picture, at `size` on its long edge.
///
/// `IShellItemImageFactory::GetImage` is the same call Explorer's own views make, so what comes back
/// is what Explorer would have drawn: out of the per-user thumbnail cache when it is already there,
/// and extracted by whichever provider is registered for the type when it is not. That is the whole
/// reason this program asks rather than decoding — a `.heic`, a `.cr2`, a `.psd`, a `.pdf` and an
/// `.mp4` each have a provider registered by whatever produced them, and not one of them is a codec
/// a file manager would be right to embed.
///
/// **Without `SIIGBF_BIGGERSIZEOK`.** That flag lets the shell hand back whatever size it happens to
/// have, which for a `.jpg` is often the 1024-pixel cached thumbnail: four megabytes to fill a
/// 96-pixel cell, decoded and copied across a thread for nothing. Asking for the size wanted makes
/// the shell do the scaling, in the code that is written for it.
///
/// **`None` is not "this file has no picture"**, whichever [`Want`] was asked — the call fails under
/// contention as well as for want of a provider, and it does not label the difference reliably. Both
/// callers have to decide what to make of that, and they decide differently: see [`picture`] for the
/// tile's answer and [`crate::preview::visual`]'s `load` for the panel's.
#[cfg(windows)]
pub(crate) fn image(path: &Path, size: u32, want: Want) -> Option<ColorImage> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::DeleteObject;
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT,
        SIIGBF_THUMBNAILONLY,
    };

    let wide = crate::shell::wide(path);

    // SAFETY: `wide` is null-terminated and outlives the call. The factory is a COM interface
    // released by its own `Drop`; the bitmap is ours to free and is freed on both paths out.
    unsafe {
        // Usually a failure here means the file has been moved or deleted since the listing was
        // read — but the shell namespace refuses under load too, which is why this is not
        // distinguished from the `GetImage` failure below.
        let factory = SHCreateItemFromParsingName::<_, _, IShellItemImageFactory>(
            PCWSTR(wide.as_ptr()),
            None,
        )
        .ok()?;
        let asked = SIZE {
            cx: size as i32,
            cy: size as i32,
        };
        let flags = match want {
            Want::Whatever => SIIGBF_RESIZETOFIT,
            Want::Rendered => SIIGBF_RESIZETOFIT | SIIGBF_THUMBNAILONLY,
        };
        let bitmap = factory.GetImage(asked, flags).ok()?;
        let image = crate::shell::icons::bitmap_premultiplied(bitmap);
        let _ = DeleteObject(bitmap.into());
        image
    }
}

/// One tile's picture, at [`CELL`].
///
/// [`Want::Whatever`], so a file with no thumbnail provider still gets something to draw — which is
/// the argument the grid rests on and the reason `SIIGBF_THUMBNAILONLY` is not passed here.
///
/// **A failure is a `Later` and never a "no".** The thumbnail cache is per user and shared with
/// Explorer, and an extraction already running for the same file — by Explorer, or by another window
/// of this program — answers this one with `E_PENDING` rather than with a picture. Reproduced by
/// asking from two threads at once, which is what `a_picture_is_fetched_once_and_off_the_ui_thread`
/// does while its neighbour is asking about the same file. Cached as a refusal, it would be a tile
/// that never got its picture until you left the folder and came back.
///
/// `E_PENDING` is the *documented* "not yet", and the reason it is not special-cased is that it is
/// not the only one: under contention the shell fails with whatever it fails with. See
/// [`BACKOFF`], which is where that measurement lives and what bounds the retrying.
#[cfg(windows)]
pub(super) fn picture(path: &Path) -> Got {
    match image(path, CELL as u32, Want::Whatever) {
        Some(image) => Got::Picture(image),
        None => Got::Later,
    }
}
