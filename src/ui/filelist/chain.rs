//! The relative path in front of a name, for a flattened listing.
//!
//! `sub\deep\file.txt` is one name as far as the rest of the program is concerned — see
//! [`crate::fs::scan::scan_deep`] — and this is what makes the folders in front of it read as
//! context rather than as part of the file's own name.

use super::*;

/// The separator between a row's name and the context after it, and between the folders of a
/// merged chain — which is the same mark meaning the same thing, one path step.
///
/// Spaces either side, and both of them belong to the *dimmed* half: what the eye should find
/// first is where the name ends, and a gap in body-coloured text before a gap in secondary
/// makes that edge fuzzy. In a chain the last separator is the one before the row's own folder,
/// and it is dim for exactly that reason.
pub(crate) const CONTEXT: &str = " > ";

/// Split a flattened row's relative path into **where the row is** and **what it shows**.
///
/// `merged` is [`crate::pane::Tab::row_merged`]: how many folders above this one are drawn as part
/// of it. So `x\a\b\c` with two merged is `("x", "a\b\c")` — the row is in `x`, and it shows the
/// chain `a > b > c`. With none merged it is the split [`Dir::within`] and [`Dir::leaf`] already
/// make, and this answers exactly the same pair.
pub(crate) fn chain_split(name: &str, merged: usize) -> (&str, &str) {
    let mut start = name.len();
    for _ in 0..=merged {
        match name[..start].rfind(['\\', '/']) {
            Some(at) => start = at,
            // The chain reaches the folder being listed: all of the path is the row's own.
            None => return ("", name),
        }
    }
    (&name[..start], name[start..].trim_start_matches(['\\', '/']))
}

/// The folders of a chain without the row's own: `a\b\c` is `a\b`, and `c` alone is nothing.
pub(crate) fn chain_folders(chain: &str) -> &str {
    chain
        .rsplit_once(['\\', '/'])
        .map_or("", |(folders, _)| folders)
}

/// Those folders joined for display, with a trailing separator: `a\b` is `a > b > `. `elided` puts a
/// `…` in place of any that were dropped.
///
/// Written into a buffer the caller owns, because this is per row per frame and the answer is a
/// handful of bytes: a `String` returned here would be an allocation a listing does not need.
pub(crate) fn chain_ahead(folders: &str, elided: bool, into: &mut String) {
    into.clear();
    if elided {
        into.push('…');
        into.push_str(CONTEXT);
    }
    for folder in folders.split(['\\', '/']).filter(|part| !part.is_empty()) {
        into.push_str(folder);
        into.push_str(CONTEXT);
    }
}

/// A merged chain's Name cell, from the row's stored name and how many folders are merged into it.
///
/// The two halves of that — [`chain_split`] then [`chain_galley`] — as one call, so
/// [`crate::ui::grid`] cannot get the division wrong in its own copy of it. A tree's folder rows are
/// drawn there exactly as they are here, chains included.
///
/// `marked` is the filter's hits as ranges of `name`, which the chain is the tail of.
#[allow(clippy::too_many_arguments)]
pub(crate) fn chain_cell(
    painter: &egui::Painter,
    name: &str,
    merged: usize,
    font: egui::FontId,
    color: egui::Color32,
    dim: egui::Color32,
    width: f32,
    marked: Marked<'_>,
) -> std::sync::Arc<egui::Galley> {
    let (_, chain) = chain_split(name, merged);
    chain_galley(painter, chain, name.len() - chain.len(), font, color, dim, width, marked)
}

/// The Name cell of a merged chain: `src > main > java > com`, cut from the **front** when it will
/// not fit.
///
/// Which is [`name_galley`]'s problem the other way round and takes the same answer. There, the row's
/// own name comes first and the context after it, so truncating at the end eats the context — the
/// right thing to lose. Here the part that must survive is at the *end*: the row is the folder the
/// chain arrives at, and `src > main > java > …` would have elided the only word that says what the
/// row is. So a folder is dropped off the front, an `…` says so, and it tries again.
///
/// The folders in front are dimmed and the last one is not, for the reason the dimmed half of an
/// ordinary flattened row is dimmed: they are where the row *is* rather than what it is.
///
/// `chain_at` is where `chain` starts in the row's stored name, which is what `marked`'s ranges are
/// of. Each folder is appended at its own offset in it, so a hit in a folder in front is marked
/// there — in the row's ink, on the fill — however many folders the elision has dropped.
#[allow(clippy::too_many_arguments)]
pub(crate) fn chain_galley(
    painter: &egui::Painter,
    chain: &str,
    chain_at: usize,
    font: egui::FontId,
    color: egui::Color32,
    dim: egui::Color32,
    width: f32,
    marked: Marked<'_>,
) -> std::sync::Arc<egui::Galley> {
    // No separator in it is no chain: one folder, drawn the way any other row's name is.
    let Some((folders, leaf)) = chain.rsplit_once(['\\', '/']) else {
        return name_galley(painter, chain, chain_at, None, None, font, color, dim, width, marked);
    };
    let leaf_at = chain_at + chain.len() - leaf.len();
    let plain = egui::TextFormat::simple(font.clone(), color);
    let dimmed = egui::TextFormat::simple(font.clone(), dim);

    // `from` is a tail of `folders`: what is left in front once the elision has dropped the rest.
    let lay = |from: &str| {
        let mut job = egui::text::LayoutJob::default();
        if from.len() < folders.len() {
            marked.append(&mut job, "…", None, &dimmed);
            marked.append(&mut job, CONTEXT, None, &dimmed);
        }
        let mut at = chain_at + folders.len() - from.len();
        for folder in from.split(['\\', '/']) {
            if !folder.is_empty() {
                marked.append(&mut job, folder, Some(at), &dimmed);
                marked.append(&mut job, CONTEXT, None, &dimmed);
            }
            at += folder.len() + 1;
        }
        marked.append(&mut job, leaf, Some(leaf_at), &plain);
        job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
        painter.layout_job(job)
    };

    let mut from = folders;
    loop {
        let galley = lay(from);
        if !galley.elided {
            return galley;
        }
        match from.split_once(['\\', '/']) {
            // One fewer folder in front, and an ellipsis where it was.
            Some((_, rest)) => from = rest,
            // Down to the last one: try the row's own folder with nothing but the ellipsis before it.
            None if !from.is_empty() => from = "",
            // And not even that fits. The leaf is still first in the *elision*, so what is on screen
            // is as much of the row's own name as there was room for — which is the right way round.
            None => return galley,
        }
    }
}
