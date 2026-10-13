//! Where the filter's words are in a row, so the row can show why it is there.
//!
//! **The filter decides which rows stay; this only marks them.** Which rows stay is
//! `azur_egui_theme::filter::Query`, asked of each row's **whole path** — see [`crate::fs::sort`] —
//! and that type keeps its terms to itself: it answers "does this match", never "where". So the words
//! are read again here, by the same rules the library documents, and asked of the same string: the
//! folder's path from [`crate::fs::sort::prefix`], then the row's name. The two can then only disagree
//! about what is *on screen*: a word found in the folder's own path is in no row's cell, and marks
//! nothing, which is right — the breadcrumb is where that part of the path is drawn.
//!
//! Asked of the whole path rather than of the cell for the anchors' sake. `^` holds the start of the
//! *path*, so in an ordinary folder `^ann` keeps nothing and should mark nothing — where asked of the
//! name it marked the start of every name beginning `Ann`, in rows the filter never kept for it. And a
//! word that crosses a separator, `lang\trans`, is one hit split across the row's context and its name.
//!
//! The rules, as `Query::parse` has them: split on whitespace; a leading `!` excludes, and an excluded
//! word is never in a kept row so there is nothing to mark; a leading `^` and a trailing `$` hold an
//! end, and a **doubled** marker is the character itself. Case is ignored, and folded per `char`
//! exactly as the library folds it — one character to one character, so a word of *n* characters
//! covers exactly *n* of the path's, and the byte offsets come from the path itself rather than from a
//! folded copy whose offsets a multi-byte letter would have moved.

use std::ops::Range;

use azur_egui_theme::contrast::{apart, SAME};
use egui::text::{LayoutJob, TextFormat};
use egui::Color32;

use crate::theme::Theme;

/// One word to look for, folded, with where it has to be.
struct Word {
    chars: Vec<char>,
    start: bool,
    end: bool,
}

/// A filter line, read once per frame and asked of each row on screen.
#[derive(Default)]
pub(crate) struct Marks {
    words: Vec<Word>,
}

impl Marks {
    /// Read a filter line. Nothing to mark for an empty one, which costs nothing per row.
    pub(crate) fn parse(filter: &str) -> Self {
        let mut words = Vec::new();
        for raw in filter.split_whitespace() {
            let (negated, rest) = marker(raw, '!');
            if negated {
                continue;
            }
            let (start, rest) = marker(rest, '^');
            let (end, rest) = end_marker(rest);
            if rest.is_empty() {
                continue;
            }
            words.push(Word {
                chars: rest.chars().map(fold).collect(),
                start,
                end,
            });
        }
        Self { words }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Every stretch of `hay` a word of the filter covers, sorted and merged, as byte ranges.
    ///
    /// Every occurrence of an unanchored word, not the first: `aa` in `aardvark_aa.txt` is two
    /// places, and marking one would say the other is not a match. Overlaps are merged, because the
    /// sections a galley is built from have to be contiguous and in order.
    pub(crate) fn find(&self, hay: &str) -> Vec<Range<usize>> {
        if self.words.is_empty() {
            return Vec::new();
        }
        // Each character's byte offset and its fold, plus the end — about forty rows a frame, over a
        // path each, so one small vector per row is not worth a byte path of its own.
        let folded: Vec<(usize, char)> = hay.char_indices().map(|(at, c)| (at, fold(c))).collect();
        let n = folded.len();
        let byte = |i: usize| folded.get(i).map_or(hay.len(), |&(at, _)| at);
        let same = |at: usize, word: &[char]| {
            folded[at..at + word.len()]
                .iter()
                .zip(word)
                .all(|(&(_, h), w)| h == *w)
        };
        let mut found: Vec<Range<usize>> = Vec::new();
        for word in &self.words {
            let m = word.chars.len();
            if m > n {
                continue;
            }
            let mut hit = |at: usize| found.push(byte(at)..byte(at + m));
            match (word.start, word.end) {
                (true, true) => {
                    if n == m && same(0, &word.chars) {
                        hit(0);
                    }
                }
                (true, false) => {
                    if same(0, &word.chars) {
                        hit(0);
                    }
                }
                (false, true) => {
                    if same(n - m, &word.chars) {
                        hit(n - m);
                    }
                }
                (false, false) => {
                    let mut at = 0;
                    while at + m <= n {
                        if same(at, &word.chars) {
                            hit(at);
                            at += m;
                        } else {
                            at += 1;
                        }
                    }
                }
            }
        }
        found.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(found.len());
        for range in found {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        merged
    }

    /// The hits in one row's `name`, as byte ranges **of the name**.
    ///
    /// `path` holds the folder's own path up to `base` — [`crate::fs::sort::prefix`], written once a
    /// frame — and is truncated back to it here, the same one-buffer arrangement the filter's own pass
    /// uses. A hit that starts in the folder's path and runs into the name keeps the part in the name.
    pub(crate) fn in_row(&self, path: &mut String, base: usize, name: &str) -> Vec<Range<usize>> {
        if self.words.is_empty() {
            return Vec::new();
        }
        path.truncate(base);
        path.push_str(name);
        self.find(path)
            .into_iter()
            .filter(|hit| hit.end > base)
            .map(|hit| hit.start.max(base) - base..hit.end - base)
            .collect()
    }
}

/// The fill a hit wears, on a row whose own surface is `under`.
///
/// `accent.subtle_active`, which clears [`SAME`] against every surface a row can wear in every
/// palette but one: hovered in Onyx, where it is 1.8 ΔL* off the hover grey and `subtle_hover` takes
/// over. It was `accent.subtle`, the find bar's fill for its other hits, which is right over text in a
/// document and wrong here — in the dark palettes it is 1.2 ΔL* off the listing's panel, so a search's
/// hits were all but invisible on the rows that had not been selected, and on a selected row they
/// vanished outright: `accent.subtle` *is* the quiet selection in the dark theme and the selection in
/// the light one. See `every_hit_can_be_seen_on_every_row_and_read`.
pub(crate) fn hit_fill(t: &Theme, under: Color32) -> Color32 {
    [t.accent.subtle_active, t.accent.subtle_hover]
        .into_iter()
        .find(|&fill| apart(fill, under) >= SAME)
        .unwrap_or(t.accent.subtle_active)
}

/// The hits in one row, and how to draw them, for the galley builders to share.
///
/// `hits` are byte ranges of a *source* string — a row's name, or the new name in a batch rename —
/// and every piece of text that is drawn says where in the source it came from, so that one list of
/// hits can be spread over a name, its context and the folders of a chain without each of them
/// working out its own share.
#[derive(Clone, Copy)]
pub(crate) struct Marked<'a> {
    pub hits: &'a [Range<usize>],
    pub fill: Color32,
    /// What a hit's text is drawn in: the row's own ink, wherever the hit is. So where the text around
    /// it is dimmed — the folders of a chain, the context after a name — the word that kept the row
    /// is not the faintest thing on it.
    pub ink: Color32,
}

impl Marked<'_> {
    /// Nothing to mark, for a test that is about the rest of a cell.
    #[cfg(test)]
    pub(crate) const NONE: Marked<'static> = Marked {
        hits: &[],
        fill: Color32::TRANSPARENT,
        ink: Color32::TRANSPARENT,
    };

    pub(crate) fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// Append `text`, which is the source's bytes from `at`, in `plain` — and the stretches of it
    /// the hits cover on the fill.
    ///
    /// `at` is `None` for text that is not from the source at all: a separator, an ellipsis, a
    /// shortcut's target. Nothing in it is marked.
    pub(crate) fn append(&self, job: &mut LayoutJob, text: &str, at: Option<usize>, plain: &TextFormat) {
        let Some(at) = at.filter(|_| !self.hits.is_empty()) else {
            job.append(text, 0.0, plain.clone());
            return;
        };
        let marked = TextFormat {
            background: self.fill,
            color: self.ink,
            ..plain.clone()
        };
        let end = at + text.len();
        let mut from = at;
        for hit in self.hits.iter().filter(|hit| hit.end > at && hit.start < end) {
            let (start, stop) = (hit.start.max(at), hit.end.min(end));
            if start > from {
                job.append(&text[from - at..start - at], 0.0, plain.clone());
            }
            job.append(&text[start - at..stop - at], 0.0, marked.clone());
            from = stop;
        }
        if from < end {
            job.append(&text[from - at..], 0.0, plain.clone());
        }
    }
}

/// A leading marker, and the word without it. Doubled, it is the character itself.
fn marker(word: &str, mark: char) -> (bool, &str) {
    match word.strip_prefix(mark) {
        Some(rest) if rest.starts_with(mark) => (false, rest),
        Some(rest) => (true, rest),
        None => (false, word),
    }
}

/// The trailing `$`, likewise.
fn end_marker(word: &str) -> (bool, &str) {
    match word.strip_suffix('$') {
        Some(rest) if rest.ends_with('$') => (false, rest),
        Some(rest) => (true, rest),
        None => (false, word),
    }
}

/// One character folded for comparison, exactly as `azur_egui_theme::filter` folds it: ASCII
/// directly, otherwise the first character of its lowercase. One to one, which is what lets a hit be
/// measured in the path's own characters.
fn fold(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use azur_egui_theme::contrast::{ratio, TEXT};

    fn marks(filter: &str, name: &str) -> Vec<String> {
        Marks::parse(filter)
            .find(name)
            .into_iter()
            .map(|r| name[r].to_owned())
            .collect()
    }

    #[test]
    fn every_word_is_marked_wherever_it_is_and_case_is_ignored() {
        assert_eq!(marks("report", "Annual Report.pdf"), ["Report"]);
        assert_eq!(marks("aa", "aardvark_aa.txt"), ["aa", "aa"]);
        // Two words, in either order, each marked.
        assert_eq!(marks("pdf ann", "Annual Report.pdf"), ["Ann", "pdf"]);
        // Overlaps become one stretch, because a galley's sections must not overlap.
        assert_eq!(marks("repo port", "report"), ["report"]);
    }

    #[test]
    fn anchors_hold_an_end_and_exclusions_mark_nothing() {
        assert_eq!(marks("^ann", "Annual Report.pdf"), ["Ann"]);
        assert!(marks("^report", "Annual Report.pdf").is_empty());
        assert_eq!(marks(".pdf$", "Annual Report.pdf"), [".pdf"]);
        assert_eq!(marks("^main.rs$", "main.rs"), ["main.rs"]);
        assert!(marks("!tmp", "tmp.txt").is_empty());
        // Doubled, the marker is the character.
        assert_eq!(marks("!!x", "!x.txt"), ["!x"]);
    }

    #[test]
    fn beyond_ascii_the_letters_typed_are_the_letters_marked() {
        assert_eq!(marks("élé", "Élément.txt"), ["Élé"]);
        assert_eq!(marks("ment", "Élément.txt"), ["ment"]);
        assert_eq!(marks("ÉLÉMENT$", "un élément"), ["élément"]);
        // ASCII word, non-ASCII name: the offsets are the name's own, not a folded copy's.
        assert_eq!(marks("txt", "Été.txt"), ["txt"]);
        // And a word that does not fold to its own length is not found — as the filter does not.
        assert!(marks("ss", "Straße").is_empty());
    }

    /// What a row's cells see: the hits of the whole path, as ranges of the row's own name.
    #[test]
    fn a_row_is_marked_by_its_whole_path_and_only_its_name_is_drawn() {
        let row = |filter: &str, name: &str| {
            let mut path = String::from(r"D:\Sources\azur\");
            let base = path.len();
            Marks::parse(filter)
                .in_row(&mut path, base, name)
                .into_iter()
                .map(|r| name[r].to_owned())
                .collect::<Vec<_>>()
        };
        // In the folder's own path and nowhere in the name: kept, and nothing to mark.
        assert!(row("sources", "main.rs").is_empty());
        // `^` holds the path's start, which no name in an ordinary folder is at.
        assert!(row("^main", "main.rs").is_empty());
        assert_eq!(row("^d:", "main.rs"), Vec::<String>::new());
        // `$` holds the end, which is the name's.
        assert_eq!(row(".rs$", "main.rs"), [".rs"]);
        // A flattened row's name is a relative path, and a word may cross its separator.
        assert_eq!(row(r"lang\trans", r"Res\Lang\translations.json"), [r"Lang\trans"]);
        // And one that starts in the folder's path keeps the part in the name.
        assert_eq!(row(r"azur\src", r"src\main.rs"), ["src"]);
    }

    /// A hit split across the pieces a cell is built from, each piece saying where it came from.
    #[test]
    fn hits_spread_over_the_pieces_of_a_cell() {
        let plain = TextFormat::simple(egui::FontId::proportional(14.0), Color32::GRAY);
        let marked = Marked {
            hits: &[2..6, 7..8],
            fill: Color32::from_rgb(1, 2, 3),
            ink: Color32::WHITE,
        };
        let mut job = LayoutJob::default();
        marked.append(&mut job, "abcd", Some(0), &plain);
        marked.append(&mut job, " > ", None, &plain);
        marked.append(&mut job, "efgh", Some(4), &plain);
        let runs: Vec<(&str, bool, Color32)> = job
            .sections
            .iter()
            .map(|s| {
                let text = &job.text[s.byte_range.start.0..s.byte_range.end.0];
                (text, s.format.background == marked.fill, s.format.color)
            })
            .collect();
        assert_eq!(
            runs,
            [
                ("ab", false, Color32::GRAY),
                ("cd", true, Color32::WHITE),
                (" > ", false, Color32::GRAY),
                ("ef", true, Color32::WHITE),
                ("g", false, Color32::GRAY),
                ("h", true, Color32::WHITE),
            ]
        );
    }

    /// The hit fill against every surface a row can wear: far enough off it to be seen, and the
    /// name's ink still readable on it. A row's surface is the panel, the hover grey, the two
    /// selected fills, and the quiet selection of a pane without the keyboard.
    #[test]
    fn every_hit_can_be_seen_on_every_row_and_read() {
        for t in Theme::all() {
            let name = t.palette.key();
            for (what, under) in [
                ("the panel", t.bg.layer),
                ("a hovered row", crate::ui::row_fill(&t, false, true).unwrap()),
                ("a selected row", crate::ui::row_fill(&t, true, false).unwrap()),
                ("a hovered selected row", crate::ui::row_fill(&t, true, true).unwrap()),
                ("a quiet selection", crate::ui::row_fill_quiet(&t)),
            ] {
                let fill = hit_fill(&t, under);
                let seen = apart(fill, under);
                assert!(seen >= SAME, "{name}: a hit on {what} is {seen:.1} ΔL* off it");
                let read = ratio(t.text.primary, fill);
                assert!(read >= TEXT, "{name}: a hit's ink on {what} is {read:.2}:1");
            }
        }
    }
}
