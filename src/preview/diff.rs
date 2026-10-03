//! Two pictures, and where they differ: a selected pair, or one file against `HEAD`.
//!
//! The mask that comes out of here is a *measurement* — white with the alpha carrying how
//! much — because what colour "different" is painted in is [`crate::ui::preview`]'s decision.

use super::*;

/// Two pictures, and where they differ.
pub struct Diff {
    pub a: Picture,
    pub b: Picture,
    /// **Where they differ, as a mask**: white, with the alpha carrying how much. Transparent
    /// wherever the two agree.
    ///
    /// A mask and not a coloured image, deliberately. What colour "different" is painted in is a
    /// decision for [`crate::ui::preview`], where every other colour in this program is decided —
    /// so what comes off the worker is a measurement and the panel tints it.
    pub mask: Picture,
    /// The share of pixels that differ at all, 0..1. The number you actually want: "they are the
    /// same file" and "0.02% of it moved" are different answers and a picture shows neither.
    pub differing: f32,
}

/// Two pictures, and a mask of where they differ.
///
/// **Compared at the larger of the two sizes**, with a pixel that exists in only one of them
/// counted as differing. Two files of different dimensions are not the same picture, and saying so
/// by lighting up the region one of them does not reach is more useful than either refusing to
/// compare them or quietly cropping to the overlap and reporting a small difference.
pub(super) fn compare(a: &Path, b: &Path) -> Payload {
    let (Payload::Picture(a), Payload::Picture(b)) = (picture::load(a), picture::load(b)) else {
        // Whichever failed, its own complaint is the useful one — so it is read again rather than
        // guessed at. Both are page-cache warm by now.
        return match picture::load(a) {
            Payload::Failed(why) => Payload::Failed(why),
            _ => picture::load(b),
        };
    };
    difference(*a, *b)
}

/// One picture, and the same picture as the last commit has it.
///
/// **The picture alone when there is nothing to compare it with**, which is the answer for an
/// untracked file, a file outside a repository, and a `HEAD` version this build cannot decode. The
/// panel asks for this only where git has already said the file has changed, so those are the odd
/// cases rather than the common one — but every one of them has to end in a picture, because the file
/// is there and somebody asked to see it.
///
/// The blob is decoded from memory rather than written out and read back. It is already in memory —
/// git wrote it down a pipe — and a preview that left temporary files behind would be a preview that
/// left temporary files behind.
pub(super) fn against_head(path: &Path) -> Payload {
    let now = match picture::load(path) {
        Payload::Picture(now) => now,
        // The file itself will not decode: its own complaint, not a comparison's.
        other => return other,
    };
    let Some(bytes) = crate::git::blob(path) else {
        return Payload::Picture(now);
    };
    // The working file's name goes with the bytes, because a blob has none of its own — and both
    // questions the decoder asks of a name, vector art and `.cur`, are questions about this file.
    let Ok(before) = picture::decode(&bytes, path) else {
        return Payload::Picture(now);
    };
    difference(before, *now)
}

/// Two decoded pictures, and a mask of where they differ. See [`compare`] for the size rule.
fn difference(a: Picture, b: Picture) -> Payload {
    let size = [
        a.pixels.size[0].max(b.pixels.size[0]),
        a.pixels.size[1].max(b.pixels.size[1]),
    ];
    let at = |picture: &Picture, x: usize, y: usize| -> Option<egui::Color32> {
        let [w, h] = picture.pixels.size;
        (x < w && y < h).then(|| picture.pixels.pixels[y * w + x])
    };

    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    let mut differing = 0usize;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let delta = match (at(&a, x, y), at(&b, x, y)) {
                (Some(one), Some(other)) => {
                    // The largest single-channel difference, alpha included. A per-channel maximum
                    // rather than a sum, so a picture that differs in one channel by a lot is not
                    // averaged down towards one that differs in three by a little.
                    let ([p, q], [r, s]) = ([one.r(), one.g()], [other.r(), other.g()]);
                    (p.abs_diff(r))
                        .max(q.abs_diff(s))
                        .max(one.b().abs_diff(other.b()))
                        .max(one.a().abs_diff(other.a()))
                }
                // Present in one and not the other: as different as it gets.
                _ => 255,
            };
            if delta > 0 {
                differing += 1;
            }
            // **Amplified eightfold.** A one-level difference at alpha 1 is invisible, and the
            // whole job of this image is to be *findable*: past a delta of 32 it is fully opaque,
            // and below that it fades rather than vanishing. The count above is the honest
            // measurement; this is the visible one.
            pixels.push(egui::Color32::from_white_alpha(
                ((delta as u32) * 8).min(255) as u8,
            ));
        }
    }

    let total = (size[0] * size[1]).max(1);
    Payload::Diff(Box::new(Diff {
        differing: differing as f32 / total as f32,
        mask: Picture {
            pixels: egui::ColorImage {
                size,
                pixels,
                source_size: egui::vec2(size[0] as f32, size[1] as f32),
            },
            natural: [size[0] as u32, size[1] as u32],
            scaled: false,
            vector: false,
            // A measurement this program made, and the only `Picture` here that was never a file.
            shell: false,
        },
        a,
        b,
    }))
}
