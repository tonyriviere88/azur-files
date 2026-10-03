//! The picture Windows itself has for a file this program cannot decode.
//!
//! # Why there is anything here at all
//!
//! Everything else in [`super`] is a decoder: `image` for raster art, `resvg` for `.svg`, [`text`]
//! for anything with lines in it, [`crate::pe`] for a binary. Between them they cover a source tree
//! and not much else, and what a folder of *work* holds is `.pdf`, `.docx`, `.xlsx`, `.mp4`, `.heic`,
//! `.cr2`, `.psd`, `.ai`, `.dwg` — every one of which the panel used to answer with
//! "No preview for a .pdf".
//!
//! None of those is a codec this program would be right to embed. But **each of them already has one
//! installed**, registered as a thumbnail provider by whatever produced the file, and it is the same
//! provider Explorer's own views ask. So the panel asks it too: one call, through
//! [`crate::shell::thumbs::rendered`], which is where the tiles in [`crate::ui::grid`] have always got
//! their pictures. The registered visualizer is the general answer, and the four decoders above are
//! the cases where this program has a better one — see [`super::kind_of`], which is where that line is
//! drawn.
//!
//! # A render is not the file, and the panel says so
//!
//! What comes back is a *picture of* the document rather than the document: the first page of a PDF
//! and not its twenty-fourth, a poster frame and not the video, at whatever size was asked for and no
//! larger. That is worth a good deal more than the words "No preview for a .pdf" and a good deal less
//! than a decoder, so [`super::Picture::shell`] carries the fact to the bar, which puts
//! `Windows preview` where a decoded picture's dimensions would go. See `ui::preview::header`, and
//! note which of the two details it puts in the slot that survives a narrow panel.
//!
//! # No icon, ever
//!
//! [`crate::shell::thumbs::rendered`] passes `SIIGBF_THUMBNAILONLY`, so a type with no provider comes
//! back with nothing rather than with the file's icon — and the panel keeps its
//! "No preview for a .zip". That is the one deliberate difference from the tiles, which want the icon
//! because a cell has to hold something. A panel does not: an icon scaled to fill four hundred points
//! is the glyph the row beside it is already showing, enormous and soft, presented where the contents
//! of the file ought to be.
//!
//! # `IPreviewHandler` is the other answer, and it is deliberately not this one
//!
//! Worth writing down, because it is the first thing anybody will reach for on reading this file.
//! Explorer's preview *pane* — as opposed to its views — does not use the call above at all. It hosts
//! `IPreviewHandler`, which is the live article: the PDF scrolls, all twenty-four pages of it, the
//! video plays, the spreadsheet has its tabs. Against that, a thumbnail is a photograph of page one.
//!
//! It was not chosen, and the reason is that a preview handler does not draw — it is **given a window
//! to draw into**. That means a child `HWND` over this program's surface, positioned and clipped to
//! the canvas on every frame, shown and hidden as tabs change and panes split, and drawn by the
//! compositor *above* everything egui paints — so every menu, dropdown and drag overlay in this window
//! would go behind it. It also runs a stranger's DLL in-process with a message loop of its own, where
//! this call runs one on a worker that can be abandoned. Explorer can afford it because Explorer hosts
//! the thing in `prevhost.exe`, a separate process, and talks to it across a marshalled boundary.
//!
//! So the trade taken here is the one the rest of this module documents: a still picture, of everything,
//! for the cost of a function call. A live handler is a larger piece of work with a different shape —
//! its own surrogate process, or a compositor layer — and it is the kind of thing to do on purpose
//! rather than as an extension of this.

use super::*;

/// The long edge to ask the shell for.
///
/// The panel is a few hundred points wide, so this leaves room to zoom several times past fit before
/// the softness shows — which is the same argument [`CAP`] makes at 2048, arrived at from the other
/// end: 2048 asks somebody else's provider to render four megapixels, and unlike `image` we are not
/// the ones paying for it, so the number has to be a size a stranger's code will not mind producing.
///
/// Measured, by `the_shell_draws_what_this_program_cannot_and_says_so_when_it_cannot_either` and the
/// probe it grew out of:
///
/// | asked about | came back | cold | warm |
/// | --- | --- | --- | --- |
/// | a one-page PDF | 724 × 1024 | 708 ms | 64 ms |
/// | a 3000 × 2000 PNG | 1024 × 682 | 115 ms | 56 ms |
/// | a 320 × 180 PNG | **320 × 180** | 152 ms | 6 ms |
/// | a `.zip`, a `.rlib`, an extension nobody has | nothing | 12–22 ms | — |
///
/// Three things worth reading off that table. `SIIGBF_RESIZETOFIT` **fits, and does not enlarge** —
/// there is no `SIIGBF_SCALEUP` — so a small thumbnail comes back at its own size rather than
/// upsampled, which is the right answer for a preview and is why this is a cap and not a target. The
/// warm figures are the per-user thumbnail cache, shared with Explorer and outliving this process,
/// which is what makes coming back to a folder cheap. And a type with **no** provider is answered in
/// twelve milliseconds, which is what makes it affordable to ask about every file in a build folder.
///
/// 1024 is also the size `crate::shell::icons`' bitmap reader had as its sanity bound until this
/// existed, and the one place the coupling shows: see the limit in `windows::icons::read_bgra`, which
/// a render of exactly this size would otherwise have sat exactly on.
pub const SIZE: u32 = 1024;

/// How many times the shell is asked before the panel settles for saying there is no preview.
///
/// **One retry, not four.** The failure worth retrying is contention: two `GetImage` calls in flight
/// from one process answer one of them with a failure, on files that draw perfectly a moment later —
/// which `shell::thumbs`' own tests measured, and which happens here whenever a grid of tiles is
/// asking at the same time as a panel. One extra attempt is what that costs to cover, because what was
/// measured is a *collision* and not a busy period.
///
/// `crate::shell::thumbs`' `BACKOFF` answers the same fact with four attempts over seven seconds, and
/// the difference is what the two are protecting. A tile that gives up wrongly keeps a painted glyph
/// for as long as the folder is open and there is nothing the user can do about it, so it is worth
/// waiting a long time to avoid. A panel that gives up wrongly says "No preview for a .pdf" until the
/// keyboard moves off the row and back, which is one keystroke — and the price of the waiting is paid
/// by **every** file the shell genuinely has nothing for, which in a build folder is all of them.
///
/// That price is what settles it. A type with no provider is refused in twelve milliseconds — see the
/// table on [`SIZE`] — so one retry turns a `.rlib` into a 162 ms `Reading…` and four would make it
/// seven seconds. Against a *real* render at 708 ms cold, 162 ms is inside the noise; seven seconds
/// over a build folder would be the worse bug by a wide margin.
#[cfg_attr(not(windows), allow(dead_code))]
const TRIES: u32 = 2;

/// How long to wait before the second attempt.
///
/// Longer than a frame on purpose, for the reason [`crate::shell::thumbs`]'s `BACKOFF` gives at length:
/// two attempts back to back are both inside the same busy window that caused the first failure, so
/// they are one attempt with extra steps. It is also the whole cost of this module being wrong about a
/// file — see [`TRIES`] — so it is as short as it can be and still outlast a collision.
#[cfg_attr(not(windows), allow(dead_code))]
const WAIT: std::time::Duration = std::time::Duration::from_millis(150);

/// The shell's render of a file, as a picture the panel can show — or nothing, with the panel left to
/// say so.
///
/// # On a worker, with an apartment of its own
///
/// `SHCreateItemFromParsingName` goes through the shell namespace and needs COM, and this runs on the
/// one-shot thread [`Previews::request`] spawned — which has none, since every other kind of preview is
/// a decoder that wants nothing from the shell. So the apartment is entered here rather than in
/// `request`: a `.png` and a `.rs` should not pay `OleInitialize` for a call they never make.
///
/// **And it is never left**, which is a deliberate choice and not an oversight. The matching
/// `CoUninitialize` would run as this thread finished, and the *last* one in a process frees shell state
/// other threads are still using — the hazard `shell::icons` sets out at length. Skipping it leaves the
/// apartment to the thread's own rundown instead, which is the trade `shell::thumbs`' worker makes too;
/// there it is free, because that thread outlives every request, and here it is a human-paced handful of
/// threads over a session rather than something in a loop.
///
/// # Caught, because the thing being called is other people's code
///
/// `GetImage` loads whichever provider is registered for the file and runs it in this process, and
/// everything this program does with the result — reading the bitmap's header, walking its rows — is
/// working from figures that code supplied. Unguarded, a panic anywhere in that ends the worker
/// silently: nothing is ever sent back, and the panel sits on `Reading…` for that file until the
/// selection moves. Guarded, it is one file answered "no preview". The same reasoning, and the same
/// `catch_unwind`, as `shell::thumbs::Thumbs::worker`.
///
/// What is deliberately *not* guarded is a provider that **hangs**, because there is nothing to guard
/// with: `GetImage` is a synchronous in-proc call and a deadline would mean abandoning the thread
/// mid-call. What it costs here is one leaked worker thread — the panel itself has already moved on,
/// since an answer to a token nobody holds is dropped — where the same hang on `shell::thumbs`' single
/// worker stalls every tile behind it. That is the one respect in which the panel has the easier job,
/// and it is the whole reason a per-request thread is the right shape for this.
pub(super) fn load(path: &Path) -> Payload {
    match rendered(path) {
        Some(pixels) => {
            let size = pixels.size;
            Payload::Picture(Box::new(Picture {
                pixels,
                // The render's own size, because there is no other honest answer: what the shell
                // hands back is a picture of the document and the document has no pixel size of its
                // own. Which is exactly why `shell` is set — the bar reports these dimensions as the
                // *preview's* rather than as the file's, and only because of that flag.
                natural: [size[0] as u32, size[1] as u32],
                // Not `scaled`: that word means "this program decoded it and then threw pixels away
                // to fit `CAP`", and the comment it produces would be a claim about memory. The
                // shell rendered straight to the size asked for.
                scaled: false,
                vector: false,
                shell: true,
            }))
        }
        // **Not `Failed`.** Nothing went wrong — Windows has no visualizer for this type, which is a
        // fact about the file and not an error, and the panel's word for it is the same one it used
        // before any of this existed.
        None => Payload::Unsupported,
    }
}

/// The call itself, retried once. See [`TRIES`].
#[cfg(windows)]
fn rendered(path: &Path) -> Option<egui::ColorImage> {
    crate::shell::init();
    for attempt in 0..TRIES {
        if attempt > 0 {
            std::thread::sleep(WAIT);
        }
        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::shell::thumbs::rendered(path, SIZE)
        }));
        match got {
            Ok(Some(image)) => return Some(image),
            Ok(None) => continue,
            // A provider that panicked is not one that will answer differently in 150 ms, and
            // running it again is running the same fault again.
            Err(_) => return None,
        }
    }
    None
}

/// Unreachable in the window: [`super::kind_of`] answers `None` off Windows, so nothing is ever
/// classified as something to ask the shell about. Here so that the module still compiles there, and
/// so that a caller reaching past `kind_of` gets "no preview" rather than a build error.
#[cfg(not(windows))]
fn rendered(_path: &Path) -> Option<egui::ColorImage> {
    None
}
