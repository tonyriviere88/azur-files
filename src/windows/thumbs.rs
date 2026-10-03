//! `IShellItemImageFactory`: the thumbnail the shell would show, at the size asked for.
//!
//! The Windows half of [`crate::shell::thumbs`]. The cache, the queue and the atlas next door
//! are all portable; this is only the one call that produces pixels.

use super::*;

/// Ask the shell for one file's picture, at [`CELL`].
///
/// `IShellItemImageFactory::GetImage` is the same call Explorer's own views make, so what comes back
/// is what Explorer would have drawn: the thumbnail where the file has one — out of the per-user
/// cache when it is already there — and the file's large icon where it has not.
///
/// **Without `SIIGBF_BIGGERSIZEOK`.** That flag lets the shell hand back whatever size it happens to
/// have, which for a `.jpg` is often the 1024-pixel cached thumbnail: four megabytes to fill a
/// 96-pixel cell, decoded and copied across a thread for nothing. Asking for the size wanted makes
/// the shell do the scaling, in the code that is written for it.
///
/// `SIIGBF_THUMBNAILONLY` is deliberately *not* passed either — see the module header. A file with no
/// thumbnail provider still needs a picture on its tile, and its icon is that picture.
#[cfg(windows)]
pub(super) fn picture(path: &Path) -> Got {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::DeleteObject;
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT,
    };

    use std::os::windows::ffi::OsStrExt as _;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: `wide` is null-terminated and outlives the call. The factory is a COM interface
    // released by its own `Drop`; the bitmap is ours to free and is freed on both paths out.
    unsafe {
        let Ok(factory) = SHCreateItemFromParsingName::<_, _, IShellItemImageFactory>(
            PCWSTR(wide.as_ptr()),
            None,
        ) else {
            // Usually the file has been moved or deleted since the listing was read — but the shell
            // namespace refuses under load too, so this is a `Later` like everything else here and the
            // attempt count is what stops it. See [`BACKOFF`].
            return Got::Later;
        };
        let size = SIZE {
            cx: CELL as i32,
            cy: CELL as i32,
        };
        let bitmap = match factory.GetImage(size, SIIGBF_RESIZETOFIT) {
            Ok(bitmap) => bitmap,
            // **`E_PENDING` is not "no".** The thumbnail cache is per user and shared with
            // Explorer, and an extraction already running for the same file — by Explorer, or by
            // another window of this program — answers this one with "not yet" rather than with a
            // picture. Reproduced by asking from two threads at once, which is what
            // `a_picture_is_fetched_once_and_off_the_ui_thread` does while its neighbour is asking
            // about the same file. Cached as a refusal, it would be a tile that never got its
            // picture until you left the folder and came back.
            // `E_PENDING` is the *documented* "not yet", and the reason it is not special-cased is that
            // it is not the only one: under contention the shell fails with whatever it fails with. See
            // [`BACKOFF`], which is where that measurement lives.
            Err(_) => return Got::Later,
        };
        let image = crate::shell::icons::bitmap_premultiplied(bitmap);
        let _ = DeleteObject(bitmap.into());
        match image {
            Some(image) => Got::Picture(image),
            None => Got::Later,
        }
    }
}
