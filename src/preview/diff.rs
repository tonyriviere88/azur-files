//! Two files, and where they differ: two pictures, a picture against `HEAD`, or two text files.
//!
//! The mask that comes out of here is a *measurement* — white with the alpha carrying how
//! much — because what colour "different" is painted in is [`crate::ui::preview`]'s decision.
//!
//! **Two text files come back as the second one, with the first one's lines as its changes.** The
//! panel already draws a file against `HEAD` from a [`crate::git::Changes`] — the added lines banded,
//! the removed ones put back where they were — so a pair is that same view with a different "before":
//! [`lines`] works the hunks out here, by a line diff of its own, rather than asking git, because two
//! arbitrary files are not in any repository and the machine need not have git at all.

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

/// Two text files: the second, read as any text file is, with what the first has instead as its
/// changes.
///
/// **The second is the one shown**, because it is the lower row of the listing and a diff reads
/// downwards — the same order [`Ask::Pair`] gives two pictures. Its own git changes are thrown away:
/// what the panel is answering is "what does the second have that the first does not", and a band
/// for a line the last commit lacked would be an answer to a different question.
///
/// A file that does not read as text says so by name, rather than being diffed as a page of
/// replacement characters.
pub(super) fn text_pair(a: &Path, b: &Path) -> Payload {
    let before = match as_text(a) {
        Ok(text) => text,
        Err(why) => return why,
    };
    let mut after = match as_text(b) {
        Ok(text) => text,
        Err(why) => return why,
    };
    // `Some` even when there is nothing in it: for a pair, "no changes" is the answer and not the
    // absence of one — the panel says the two are identical rather than showing a file plainly.
    after.changes = Some(lines(&before.body, &after.body));
    Payload::Text(after)
}

/// One side of a text pair, or the payload that says why it is not one.
fn as_text(path: &Path) -> Result<Text, Payload> {
    match text::sniff(path) {
        Some(true) => text::read(path),
        Some(false) => Err(Payload::Failed(format!("{} is not text", leaf(path)))),
        None => Err(Payload::Failed("Cannot be read".to_owned())),
    }
}

/// The line diff of `before` against `after`, in the shape `git diff -U0` would have given it.
///
/// [`super::myers`]' walk, after the common head and tail are taken off — which is all most
/// comparisons of two versions of a file ever need, and leaves the walk the middle where they really
/// differ. Past its budget the whole middle is one replacement: two files that differ in that many
/// lines are two different files, and one band saying so is the honest picture of them.
///
/// Both bodies are as [`text::read`] left them — tabs and `\r` already gone — so a removed line is
/// already as the panel draws it. Split by `lines`, so a file that ends in a newline and one that
/// does not are not a difference of one empty line.
pub(crate) fn lines(before: &str, after: &str) -> crate::git::Changes {
    use super::myers::{diff, runs, Op};

    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let head = old.iter().zip(&new).take_while(|(p, q)| p == q).count();
    let tail = old[head..]
        .iter()
        .rev()
        .zip(new[head..].iter().rev())
        .take_while(|(p, q)| p == q)
        .count();
    let (xs, ys) = (&old[head..old.len() - tail], &new[head..new.len() - tail]);
    let ops = diff(xs, ys).unwrap_or_else(|| {
        let mut ops = vec![Op::Remove; xs.len()];
        ops.resize(xs.len() + ys.len(), Op::Add);
        ops
    });

    let hunks = runs(&ops)
        .into_iter()
        .map(|(gone, came)| {
            let (i, j) = (head + gone.start, head + came.start);
            // git's numbering for an empty side: the line it sits *after*, not the one it starts at.
            let first = |at: usize, count: usize| at as u32 + u32::from(count > 0);
            crate::git::Hunk {
                added: first(j, came.len()),
                added_count: came.len() as u32,
                after: j as u32,
                removed: old[i..i + gone.len()].iter().map(|line| line.to_string()).collect(),
                removed_at: first(i, gone.len()),
            }
        })
        .collect();
    crate::git::Changes { hunks }
}
