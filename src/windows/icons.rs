//! The system image list: the same icons Explorer draws, by index.
//!
//! The Windows half of [`crate::shell::icons`]. `SHGetFileInfoW` for an *index* rather than for
//! a type name — the index is a cheap lookup and the type name is the millisecond-per-file call
//! [`crate::fs::scan`] refuses to make.

use super::*;

/// Ask the shell for an image-list index.
///
/// `use_attributes` is the difference between a microsecond and a millisecond: with
/// it the shell answers from the name and the attribute word alone.
#[cfg(windows)]
pub(crate) fn index_of(path: &Path, is_dir: bool, use_attributes: bool) -> Option<i32> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    };
    use windows::Win32::UI::Shell::{
        SHGetFileInfoW, SHFILEINFOW, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
    };

    let wide: Vec<u16> = wide(path);
    let attributes = if is_dir {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    let mut flags = SHGFI_SYSICONINDEX;
    if use_attributes {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }

    let mut info = SHFILEINFOW::default();
    // SAFETY: `wide` is null-terminated and outlives the call; `info` is sized by
    // `size_of`. A zero return means the shell declined, which is not an error here.
    let ok = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            attributes,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    (ok != 0).then_some(info.iIcon)
}

/// The image-list index for a place: a real folder, or a namespace item.
///
/// An empty path is This PC, and anything starting with `shell:` or `::{` is a moniker
/// for something that is not a file — both of which have to be parsed into a PIDL first,
/// because `SHGetFileInfoW` on the text would go looking for a file of that name.
/// A real folder goes by name with *no* `SHGFI_USEFILEATTRIBUTES`, which is what lets the
/// shell read its `desktop.ini` and hand back the Downloads icon rather than a folder.
#[cfg(windows)]
pub(crate) fn index_of_place(path: &Path) -> Option<i32> {
    let text = path.to_string_lossy();
    let moniker = if path.as_os_str().is_empty() {
        Some(std::borrow::Cow::Borrowed("shell:MyComputerFolder"))
    } else if text.starts_with("shell:") || text.starts_with("::{") {
        Some(text.clone())
    } else {
        None
    };

    match moniker {
        Some(moniker) => index_of_moniker(&moniker),
        None => index_of(path, true, false),
    }
}

/// The icon of something named by a shell moniker rather than by a path.
#[cfg(windows)]
pub(super) fn index_of_moniker(moniker: &str) -> Option<i32> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        ILFree, SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW, SHGFI_PIDL, SHGFI_SYSICONINDEX,
    };

    let wide: Vec<u16> = moniker.encode_utf16().chain(std::iter::once(0)).collect();
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    // SAFETY: `wide` is null-terminated and outlives the call. `pidl` is written only on
    // success and freed on both paths out below.
    unsafe {
        SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).ok()?;
    }
    if pidl.is_null() {
        return None;
    }

    let mut info = SHFILEINFOW::default();
    // SAFETY: with `SHGFI_PIDL` the first argument is a PIDL cast to `PCWSTR`, which is
    // the shape this API has always had. `info` is sized by `size_of`.
    let ok = unsafe {
        SHGetFileInfoW(
            PCWSTR(pidl as *const u16),
            Default::default(),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_PIDL,
        )
    };
    // SAFETY: allocated by `SHParseDisplayName`, freed exactly once.
    unsafe { ILFree(Some(pidl)) };
    (ok != 0).then_some(info.iIcon)
}

/// Pull one icon out of the system small image list as RGBA.
#[cfg(windows)]
pub(crate) fn bitmap(index: i32) -> Option<ColorImage> {
    use windows::Win32::Graphics::Gdi::DeleteObject;
    use windows::Win32::UI::Shell::{SHGetImageList, SHIL_SMALL};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
    use windows::Win32::UI::Controls::IImageList;

    // SAFETY: every raw call below is checked, and every handle it hands back is
    // released on all paths out.
    unsafe {
        // The shell's own list, so these are the exact bitmaps Explorer draws.
        let list: IImageList = SHGetImageList(SHIL_SMALL as i32).ok()?;
        let icon: HICON = list.GetIcon(index, 0u32).ok()?;

        let mut info = ICONINFO::default();
        if GetIconInfo(icon, &mut info).is_err() {
            let _ = DestroyIcon(icon);
            return None;
        }
        let colour = info.hbmColor;
        let mask = info.hbmMask;

        let result = read_bgra(colour, mask, false);

        if !colour.is_invalid() {
            let _ = DeleteObject(colour.into());
        }
        if !mask.is_invalid() {
            let _ = DeleteObject(mask.into());
        }
        let _ = DestroyIcon(icon);
        result
    }
}

/// One GDI bitmap as RGBA, for the icons the shell puts on its menu items.
///
/// No mask: a menu bitmap is 32-bit with real alpha, unlike the paired colour-and-mask
/// pair an `HICON` is built from.
#[cfg(windows)]
pub fn bitmap_of(bitmap: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<ColorImage> {
    // SAFETY: the handle belongs to the caller and is only read; nothing is freed here.
    unsafe { read_bgra(bitmap, windows::Win32::Graphics::Gdi::HBITMAP::default(), false) }
}

/// The same, **treating the alpha as already multiplied into the colour**.
///
/// Which is what `IShellItemImageFactory::GetImage` hands back — see [`crate::shell::thumbs`] — and
/// the difference is visible at the size that service draws at. `ColorImage::from_rgba_unmultiplied`
/// multiplies the alpha *in*, so premultiplied pixels go through it twice: a 50%-opaque edge comes
/// out at 25% of its colour, which reads as a dark fringe round every icon and a grey halo round
/// every thumbnail with a soft edge. At sixteen points that is a pixel nobody sees, which is why the
/// small path above has always been able to ignore it; at ninety-six it is the edge of the picture.
#[cfg(windows)]
pub fn bitmap_premultiplied(
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
) -> Option<ColorImage> {
    // SAFETY: the handle belongs to the caller and is only read; nothing is freed here.
    unsafe { read_bgra(bitmap, windows::Win32::Graphics::Gdi::HBITMAP::default(), true) }
}

/// Read a 32-bit icon bitmap into an egui image, using the mask for anything that
/// has no alpha channel of its own.
///
/// Monochrome and 24-bit icons still exist in the wild — a lot of shell extensions
/// ship them — and without the mask they come out as opaque black rectangles.
///
/// `premultiplied` says which of the two ways the alpha in the bitmap is meant: see
/// [`bitmap_premultiplied`], which is the one caller that says yes.
#[cfg(windows)]
unsafe fn read_bgra(
    colour: windows::Win32::Graphics::Gdi::HBITMAP,
    mask: windows::Win32::Graphics::Gdi::HBITMAP,
    premultiplied: bool,
) -> Option<ColorImage> {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS,
    };

    if colour.is_invalid() {
        return None;
    }

    let mut header = BITMAP::default();
    let read = GetObjectW(
        colour.into(),
        std::mem::size_of::<BITMAP>() as i32,
        Some((&mut header) as *mut BITMAP as *mut std::ffi::c_void),
    );
    if read == 0 || header.bmWidth <= 0 || header.bmHeight <= 0 {
        return None;
    }
    let (w, h) = (header.bmWidth as usize, header.bmHeight as usize);
    // A sanity bound and nothing more, against the case this is here for: a handle that is not the
    // bitmap it claims to be, where the header's figures are whatever happened to be in memory.
    //
    // Three real sizes come through here and the bound has to clear the largest with room to spare: an
    // image list cell is 16 or 48 square, a tile's thumbnail is [`crate::shell::thumbs::CELL`], and the
    // preview panel's shell render is [`crate::preview::visual::SIZE`] — 1024, which is why this is no
    // longer 1024 itself. A bound a legitimate caller sits exactly on is a bound that fails the day
    // somebody asks for one pixel more.
    if w > 2048 || h > 2048 {
        return None;
    }

    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        return None;
    }

    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            // Negative, so the rows come back top-down and no flip is needed.
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut pixels = vec![0u8; w * h * 4];
    let lines = GetDIBits(
        dc,
        colour,
        0,
        h as u32,
        Some(pixels.as_mut_ptr().cast()),
        &mut info,
        DIB_RGB_COLORS,
    );

    // Does this icon carry real transparency, or does it need the mask?
    let opaque = pixels.chunks_exact(4).all(|p| p[3] == 0);
    if lines != 0 && opaque && !mask.is_invalid() {
        let mut mask_bits = vec![0u8; w * h * 4];
        let read = GetDIBits(
            dc,
            mask,
            0,
            h as u32,
            Some(mask_bits.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        if read != 0 {
            // In an icon mask, white means "transparent here".
            for (pixel, m) in pixels.chunks_exact_mut(4).zip(mask_bits.chunks_exact(4)) {
                pixel[3] = if m[0] > 127 { 0 } else { 255 };
            }
        }
    } else if lines != 0 && opaque {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }

    let _ = DeleteDC(dc);
    if lines == 0 {
        return None;
    }

    // GDI hands back BGRA; egui wants RGBA.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    if premultiplied {
        // Straight across, because that is already the form epaint holds a colour in — see
        // [`bitmap_premultiplied`] for what putting it through the other constructor costs.
        return Some(ColorImage {
            size: [w, h],
            pixels: pixels
                .chunks_exact(4)
                .map(|p| egui::Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
                .collect(),
            source_size: egui::vec2(w as f32, h as f32),
        });
    }
    Some(ColorImage::from_rgba_unmultiplied([w, h], &pixels))
}

/// A path as a null-terminated wide string.
#[cfg(windows)]
pub(super) fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
