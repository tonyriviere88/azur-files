//! Where the filter's words are in a row's name, so the row can show why it is there.
//!
//! **The filter decides which rows stay; this only marks them.** Which rows stay is
//! `azur_egui_theme::filter::Query`, asked of each row's whole relative path — see
//! [`crate::fs::sort`] — and that type keeps its terms to itself: it answers "does this match", never
//! "where". So the words are read again here, by the same rules the library documents, and the two
//! can only disagree about *marking*: a name this finds nothing in is a row the filter kept for a
//! word in its folder, which is right to leave unmarked, because the word is not in the name.
//!
//! The rules, as `Query::parse` has them: split on whitespace; a leading `!` excludes, and an excluded
//! word is never in a kept row so there is nothing to mark; a leading `^` and a trailing `$` hold an
//! end, and a **doubled** marker is the character itself. Case is ignored. Beyond ASCII the library
//! folds per `char`; here a name or a word that is not ASCII is simply not marked, rather than marked
//! at offsets a fold may have moved — a missing highlight is a cosmetic loss, and a wrong one points
//! at a letter nobody typed.

use std::ops::Range;

/// One word to look for, folded, with where it has to be.
struct Word {
    text: String,
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
            if rest.is_empty() || !rest.is_ascii() {
                continue;
            }
            words.push(Word {
                text: rest.to_ascii_lowercase(),
                start,
                end,
            });
        }
        Self { words }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Every stretch of `name` a word of the filter covers, sorted and merged, as byte ranges.
    ///
    /// Every occurrence of an unanchored word, not the first: `aa` in `aardvark_aa.txt` is two
    /// places, and marking one would say the other is not a match. Overlaps are merged, because the
    /// sections a galley is built from have to be contiguous and in order.
    pub(crate) fn find(&self, name: &str) -> Vec<Range<usize>> {
        if self.words.is_empty() || !name.is_ascii() {
            return Vec::new();
        }
        let hay = name.to_ascii_lowercase();
        let mut found: Vec<Range<usize>> = Vec::new();
        for word in &self.words {
            let len = word.text.len();
            if len > hay.len() {
                continue;
            }
            match (word.start, word.end) {
                (true, true) => {
                    if hay == word.text {
                        found.push(0..len);
                    }
                }
                (true, false) => {
                    if hay.starts_with(&word.text) {
                        found.push(0..len);
                    }
                }
                (false, true) => {
                    if hay.ends_with(&word.text) {
                        found.push(hay.len() - len..hay.len());
                    }
                }
                (false, false) => {
                    let mut from = 0;
                    while let Some(at) = hay[from..].find(&word.text) {
                        found.push(from + at..from + at + len);
                        from += at + len;
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // A word kept for its folder is not in the name, and nothing is marked.
        assert!(marks("src", "main.rs").is_empty());
    }

    #[test]
    fn beyond_ascii_nothing_is_marked_rather_than_the_wrong_letters() {
        assert!(marks("élé", "Élément.txt").is_empty());
        assert!(marks("ment", "Élément.txt").is_empty());
    }
}
