//! The diff view of a text file: what changed against `HEAD`, as a body of its own.
//!
//! A separate string rather than annotations over the file, because a removed line is not in
//! the file and a collapsed region means most lines are not shown. See [`Diffed`].

use super::*;

/// How many unchanged lines are kept either side of a change when regions are collapsed.
///
/// Three, which is git's own default for `--unified` and what every review tool shows: enough to see
/// what the changed line is *inside* — the function it is in, the block it closes — and few enough
/// that two changes twenty lines apart still collapse.
pub(super) const CONTEXT: usize = 3;

/// The diff view of a text file: a body that is not quite the file, and the facts per line that go
/// with it.
///
/// **A separate string rather than annotations over the file**, because both halves of this feature
/// need one. A removed line is not in the file and has to be put back to be shown; a collapsed region
/// means most of the file's lines are not shown at all. Everything downstream then works unchanged —
/// the layout job, the find bar, the syntax colouring — because all any of them ever sees is *a*
/// string.
pub(super) struct Diffed {
    pub(super) body: String,
    /// One per line of `body`, in order, and therefore one per row of the galley that begins a line.
    pub(super) lines: Vec<Line>,
    /// `body`'s colouring, which cannot be the file's: every offset has moved.
    ///
    /// A collapsed body is also a discontinuous one, so a string or a block comment spanning a
    /// hidden region is lexed from where the visible text resumes. That is a real limit of showing
    /// part of a file and not a bug to fix here.
    pub(super) spans: Vec<syntax::Span>,
}

/// What one line of a [`Diffed`] body is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mark {
    /// In the file and in `HEAD`: shown plainly.
    Same,
    /// In the file and not in `HEAD`.
    Added,
    /// In `HEAD` and not in the file. Put back for the view, and numbered by where it was in `HEAD`
    /// rather than by where it sits — in the danger ink, so a gutter that suddenly counts backwards
    /// cannot be read as one of the file's own lines.
    Removed,
    /// A run of unchanged lines that were left out, and how many.
    Skipped(u32),
}

/// A line of the diff view: what it is, and its number in the file if it has one.
#[derive(Clone, Copy, Debug)]
pub(super) struct Line {
    pub(super) mark: Mark,
    pub(super) number: Option<u32>,
}

impl Diffed {
    /// Build the view of `body` that `changes` describes.
    pub(super) fn build(body: &str, changes: &crate::git::Changes, collapse: bool, lang: syntax::Lang) -> Self {
        // Split on `\n` and not `lines()`: a body ending in a newline has one more row in the galley
        // than it has lines of text, and this vector is indexed by *row*. One entry out of step and
        // every number below the mistake is wrong.
        let file: Vec<&str> = body.split('\n').collect();

        // Which of the file's lines the working tree gained, and where the lines it lost belong.
        let mut added = vec![false; file.len()];
        for hunk in &changes.hunks {
            for line in hunk.added..hunk.added.saturating_add(hunk.added_count) {
                if let Some(flag) = added.get_mut(line as usize - 1) {
                    *flag = true;
                }
            }
        }

        // Which lines are worth showing at all. Everything, unless regions are being collapsed — and
        // then a change and [`CONTEXT`] lines either side of it, counting the *gap* a removal leaves
        // as a place a change happened.
        let mut keep = vec![!collapse; file.len()];
        if collapse {
            let lines = file.len();
            // Both windows in one closure, because two closures cannot borrow `keep` at once. `to` is
            // exclusive, and both bounds are indices — one less than the line number.
            let mut window = |from: usize, to: usize| {
                for flag in keep.iter_mut().take(to.min(lines)).skip(from) {
                    *flag = true;
                }
            };
            for hunk in &changes.hunks {
                // A changed line, and [`CONTEXT`] lines either side of *it*.
                for line in hunk.added..hunk.added.saturating_add(hunk.added_count) {
                    let at = line as usize;
                    window(at.saturating_sub(CONTEXT + 1), at + CONTEXT);
                }
                // A removal leaves no line to be the change: the gap sits *between* `after` and the
                // line after it, so the window is three either side of the gap and not of a line —
                // which is one line narrower at the top than treating `after` as changed, and getting
                // that wrong shows four lines of context above a deletion and three below it.
                if !hunk.removed.is_empty() {
                    let after = hunk.after as usize;
                    window(after.saturating_sub(CONTEXT), after + CONTEXT);
                }
            }
        }

        let mut out = Self {
            body: String::with_capacity(body.len() / if collapse { 4 } else { 1 }),
            lines: Vec::new(),
            spans: Vec::new(),
        };
        let mut skipped = 0u32;
        for (at, line) in file.iter().enumerate() {
            let number = at as u32 + 1;
            // The lines `HEAD` had here, in front of whatever replaced them — or after the line they
            // followed, for a hunk that only took lines away.
            for hunk in &changes.hunks {
                if hunk.after + 1 != number || hunk.removed.is_empty() {
                    continue;
                }
                out.flush(&mut skipped);
                for (which, gone) in hunk.removed.iter().enumerate() {
                    out.push(gone, Mark::Removed, Some(hunk.removed_at + which as u32));
                }
            }
            if !keep[at] {
                skipped += 1;
                continue;
            }
            out.flush(&mut skipped);
            let mark = if added[at] { Mark::Added } else { Mark::Same };
            out.push(line, mark, Some(number));
        }
        // A hunk that removed the end of the file has nothing after it to sit in front of.
        for hunk in &changes.hunks {
            if hunk.after as usize >= file.len() && !hunk.removed.is_empty() {
                out.flush(&mut skipped);
                for (which, gone) in hunk.removed.iter().enumerate() {
                    out.push(gone, Mark::Removed, Some(hunk.removed_at + which as u32));
                }
            }
        }
        out.flush(&mut skipped);

        out.spans = syntax::spans(&out.body, lang);
        out
    }

    pub(super) fn push(&mut self, text: &str, mark: Mark, number: Option<u32>) {
        if !self.lines.is_empty() {
            self.body.push('\n');
        }
        self.body.push_str(text);
        self.lines.push(Line { mark, number });
    }

    /// Close a run of hidden lines with the one row that says how many there were.
    ///
    /// An **empty** line in the body, with the count painted over it: a row that said
    /// `12 unchanged lines` in the text itself would be text — selected by a drag, copied by
    /// `Ctrl+C`, and coloured by the lexer as whatever it happened to look like.
    pub(super) fn flush(&mut self, skipped: &mut u32) {
        if *skipped > 0 {
            let count = std::mem::take(skipped);
            self.push("", Mark::Skipped(count), None);
        }
    }
}

/// The band behind a line of the diff view, or nothing for a line that is simply the file's.
///
/// **The `*_subtle` status roles**, which are the design system's translucent fills — the ones behind
/// a message bar. They compose over whatever surface they land on, which is what a band under text
/// has to do: the ink on top is the file's own colouring, syntax and search highlights included, and
/// a solid fill would have to be legible against all of it.
pub(super) fn diff_fill(t: &Theme, mark: Mark) -> Option<Color32> {
    match mark {
        Mark::Same => None,
        Mark::Added => Some(t.status.success_subtle),
        Mark::Removed => Some(t.status.danger_subtle),
        // Not a change: the seam where a stretch of the file is not being shown. `layer_alt` is the
        // same grey the status line and the find bar are, so it reads as furniture rather than as a
        // third kind of change.
        Mark::Skipped(_) => Some(t.bg.layer_alt),
    }
}
