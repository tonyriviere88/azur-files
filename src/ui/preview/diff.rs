//! The diff view of a text file: what changed against `HEAD`, as a body of its own.
//!
//! A separate string rather than annotations over the file, because a removed line is not in
//! the file and a collapsed region means most lines are not shown. See [`Diffed`].
//!
//! Inside a changed line, the characters that changed are marked again on top of its band — see
//! [`words`] and [`refine`] — so a one-word edit reads as one word and not as two whole lines to
//! compare by eye.

use super::*;
use crate::preview::myers;

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
    /// **Where, inside a changed line, the change actually is**: ranges of `body`, in *characters*,
    /// in order, and never across a line break. See [`words`].
    ///
    /// Characters and not bytes, because what reads them is the walk over the galley's rows, and a
    /// galley counts characters — a byte range would land a highlight one glyph late for every
    /// accented letter in front of it.
    pub(super) words: Vec<std::ops::Range<usize>>,
    /// Where each line of `body` begins, in characters — recorded by [`Diffed::push`] as it appends
    /// one, for [`words`] to place its ranges.
    starts: Vec<usize>,
    /// How many characters `body` holds, which is where the next line will begin.
    chars: usize,
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

/// Where a walk over a hunk's added lines should stop for a body of `lines` lines: the exclusive end,
/// which is `lines + reach` or where the hunk's own count runs out, whichever comes first.
///
/// **A hunk header is not to be trusted for the length of a loop.** `added_count` is a number git
/// printed about the whole file, and it arrives here beside a body that is the first
/// [`crate::preview::TEXT_CAP`] of that file — so on a large file most of what the headers describe is
/// past the end of what is being shown, and walking it line by line is a million no-ops per toggle. It
/// is also a number this program did not work out for itself, which is reason enough on its own not to
/// size a loop with it.
///
/// `reach` is how far past the end of the body a line can still matter: one for marking a line as
/// added, and [`CONTEXT`] more than that for opening a window around a change, since a change just
/// beyond the cut still has context inside the body.
fn in_body(hunk: &crate::git::Hunk, lines: usize, reach: u32) -> u32 {
    hunk.added
        .saturating_add(hunk.added_count)
        .min((lines as u32).saturating_add(reach))
}

impl Diffed {
    /// Build the view of `body` that `changes` describes.
    pub(super) fn build(body: &str, changes: &crate::git::Changes, collapse: bool, lang: syntax::Lang) -> Self {
        // Split on `\n` and not `lines()`: a body ending in a newline has one more row in the galley
        // than it has lines of text, and this vector is indexed by *row*. One entry out of step and
        // every number below the mistake is wrong.
        let file: Vec<&str> = body.split('\n').collect();

        // Which of the file's lines the working tree gained, and where the lines it lost belong.
        //
        // **The counts are clamped to the body rather than walked** — see [`in_body`]: a hunk that added
        // a million lines past the cut used to be a million `get_mut`s that could only answer `None`.
        // `max(1)` guards the other end of the same arithmetic, since the line number below is an index:
        // git writes `+0` where a removal sits above the first line, and a count beside that nought
        // would make `line as usize - 1` a panic in a debug build.
        let mut added = vec![false; file.len()];
        for hunk in &changes.hunks {
            for line in hunk.added.max(1)..in_body(hunk, file.len(), 1) {
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
                // A changed line, and [`CONTEXT`] lines either side of *it*. Clamped for the reason
                // above, and to [`CONTEXT`] lines *past* the body rather than to the body: a change
                // just beyond the cut still opens the window at the end of what is being shown, which
                // is what the unclamped walk did.
                for line in hunk.added.max(1)..in_body(hunk, lines, CONTEXT as u32 + 1) {
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
            words: Vec::new(),
            starts: Vec::new(),
            chars: 0,
        };
        // Where each line ended up in the view, for [`words`]: the file's lines by index, `None` for
        // one collapsed away, and the first line each hunk put back.
        let mut view_of: Vec<Option<usize>> = vec![None; file.len()];
        let mut put_back: Vec<Option<usize>> = vec![None; changes.hunks.len()];

        // Which hunks put lines back in front of which line of the file, in line order.
        //
        // **This walk used to be nested inside the one below, and that was the hang.** The outer loop
        // is the previewed body — bounded, at [`crate::preview::TEXT_CAP`] a shade under fifteen
        // thousand lines — but `changes.hunks` is bounded by nothing the body knows about: it comes
        // from `git diff -U0` over the *whole* file, and `-U0` gives every isolated changed line a
        // hunk of its own. A large mostly-rewritten file is on the order of a hundred thousand hunks,
        // so the two together were 10^9 iterations — on the UI thread, inside [`Text::follow`], on the
        // frame the diff toggle was pressed.
        //
        // One ordered walk instead, against a cursor that only moves forward. Sorted rather than
        // assumed sorted, since git's own order is the file's and this is then one pass over a list
        // already in order — and **stably**, so several hunks landing in front of the same line are
        // still put back in the order git gave them.
        let mut removals: Vec<(u32, usize)> = changes
            .hunks
            .iter()
            .enumerate()
            .filter(|(_, hunk)| !hunk.removed.is_empty())
            .map(|(which, hunk)| (hunk.after.saturating_add(1), which))
            .collect();
        removals.sort_by_key(|(line, _)| *line);
        let mut next = 0;

        let mut skipped = 0u32;
        for (at, line) in file.iter().enumerate() {
            let number = at as u32 + 1;
            // The lines `HEAD` had here, in front of whatever replaced them — or after the line they
            // followed, for a hunk that only took lines away.
            while let Some(&(_, which)) = removals.get(next).filter(|(line, _)| *line == number) {
                next += 1;
                let hunk = &changes.hunks[which];
                out.flush(&mut skipped);
                put_back[which] = Some(out.lines.len());
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
            view_of[at] = Some(out.lines.len());
            out.push(line, mark, Some(number));
        }
        // A hunk that removed the end of the file has nothing after it to sit in front of.
        for (index, hunk) in changes.hunks.iter().enumerate() {
            if hunk.after as usize >= file.len() && !hunk.removed.is_empty() {
                out.flush(&mut skipped);
                put_back[index] = Some(out.lines.len());
                for (which, gone) in hunk.removed.iter().enumerate() {
                    out.push(gone, Mark::Removed, Some(hunk.removed_at + which as u32));
                }
            }
        }
        out.flush(&mut skipped);

        out.spans = syntax::spans(&out.body, lang);
        out.words = words(&out, changes, &file, &view_of, &put_back);
        out
    }

    pub(super) fn push(&mut self, text: &str, mark: Mark, number: Option<u32>) {
        if !self.lines.is_empty() {
            self.body.push('\n');
            self.chars += 1;
        }
        self.starts.push(self.chars);
        self.chars += text.chars().count();
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

/// How many tokens either side of a hunk may have before [`words`] leaves it to its line bands.
///
/// [`Diffed::build`] runs on the UI thread, on the frame the diff toggle is pressed, so the word diff
/// is bounded per hunk as the line diff is per file. A hunk past this is a block rewritten rather than a
/// line edited, and highlighting inside it would be most of it anyway.
const WORD_TOKENS: usize = 4000;

/// How many characters either side of one changed stretch may have before [`refine`] leaves it marked
/// word by word.
///
/// A stretch is what lies between two words the sides agree on, so in an edited line it is a word or
/// three; one this long is a run of text that was replaced, and its letters in common would be chance.
/// It is also what bounds the letter walk, which is on the UI thread with the rest of this.
const STRETCH_CHARS: usize = 400;

/// A run of one kind of character in a changed line, for [`words`] — or one character of one, once
/// [`refine`] has split it.
#[derive(Clone, Copy)]
struct Token<'a> {
    /// Which line of its side of the hunk.
    line: usize,
    /// Its characters within that line.
    from: usize,
    to: usize,
    text: &'a str,
}

impl Token<'_> {
    /// The characters of it that are ink, which is what "how much changed" is counted in.
    fn ink(&self) -> usize {
        if self.text.trim().is_empty() {
            0
        } else {
            self.to - self.from
        }
    }
}

/// What a run of tokens says, a piece each, for [`myers::diff`] to compare.
fn texts<'a>(side: &[Token<'a>]) -> Vec<&'a str> {
    side.iter().map(|token| token.text).collect()
}

/// The ink of a run of tokens. See [`Token::ink`].
fn ink(side: &[Token<'_>]) -> usize {
    side.iter().map(Token::ink).sum()
}

/// `lines` as words, runs of blanks and single symbols, with a `\n` token between lines.
///
/// **A word is the first unit**, and the letters inside one only the second — see [`refine`], for why
/// not letters from the start. A symbol is a word of its own, so `f(a)` becoming `f(a, b)` is `, b` and
/// not a changed `a)`. Words, blanks and symbols are the console's double-click classes, so a word here
/// is a word there. The line break is a token so that two lines joined, or one split, are found as the
/// break moving rather than as both lines changing.
fn tokens<'a>(lines: &[&'a str]) -> Vec<Token<'a>> {
    use crate::ui::console::editor::{class, Class};

    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push(Token { line: index, from: 0, to: 0, text: "\n" });
        }
        let mut chars = line.char_indices().enumerate().peekable();
        while let Some((at, (byte, c))) = chars.next() {
            let kind = class(c);
            let (mut to, mut end) = (at + 1, byte + c.len_utf8());
            if kind != Class::Mark {
                while let Some(&(next, (next_byte, d))) = chars.peek() {
                    if class(d) != kind {
                        break;
                    }
                    (to, end) = (next + 1, next_byte + d.len_utf8());
                    chars.next();
                }
            }
            out.push(Token { line: index, from: at, to, text: &line[byte..end] });
        }
    }
    out
}

/// **The characters that changed inside the changed lines** — what VS Code highlights on top of a
/// changed line's band, so the eye goes to the one renamed argument rather than reading two whole lines
/// to find it.
///
/// Only a hunk that both took lines away and put lines in can have one: a line that was only added has
/// nothing to be compared with. The hunk's removed lines and its added ones are each read as one run of
/// [`tokens`], and the same walk that diffs the lines diffs those — so a change that moved a word onto
/// the next line is still found as one word. Each stretch the words disagree over is then diffed by
/// letter, in [`refine`], so a word with two letters added is marked at those two.
///
/// **A hunk with less than a third of its shorter side's words in common gets none**: it is a rewrite,
/// not an edit, and its marks would be nearly all of it, broken up by every `;` and `(` the two happen to
/// share — saying what the line band already says, louder. Words and not letters, because two lines
/// sharing no word are a rewrite whatever letters `beta` and `delta` share. A line whose every word
/// changed inside a hunk that *was* edited is marked like any other.
///
/// Blanks between two changed pieces are part of the change, so `a b` becoming `x y` is one
/// highlight and not two with a gap between them. Blanks at either end are not.
///
/// `view_of` and `put_back` say where [`Diffed::build`] put each line: the file's, by index, and the
/// first removed line of each hunk.
fn words(
    view: &Diffed,
    changes: &crate::git::Changes,
    file: &[&str],
    view_of: &[Option<usize>],
    put_back: &[Option<usize>],
) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    for (index, hunk) in changes.hunks.iter().enumerate() {
        let Some(first_removed) = put_back[index] else {
            continue;
        };
        // The added lines that made it into the view. Stopped at the first that did not — past the
        // end of the body, which is the only way an added line is missing: collapsing never hides one.
        let mut added = Vec::new();
        let mut added_at = Vec::new();
        for line in hunk.added.max(1)..in_body(hunk, file.len(), 1) {
            let at = line as usize - 1;
            match (file.get(at), view_of.get(at).copied().flatten()) {
                (Some(text), Some(view)) => {
                    added.push(*text);
                    added_at.push(view);
                }
                _ => break,
            }
        }
        if added.is_empty() {
            continue;
        }
        let removed: Vec<&str> = hunk.removed.iter().map(String::as_str).collect();
        let removed_at: Vec<usize> = (first_removed..first_removed + removed.len()).collect();
        let (old, new) = (tokens(&removed), tokens(&added));
        if old.len() > WORD_TOKENS || new.len() > WORD_TOKENS {
            continue;
        }
        let Some(ops) = myers::diff(&texts(&old), &texts(&new)) else {
            continue;
        };
        // Decided before any letter is looked at: a rewrite costs the word walk and nothing more.
        let runs = myers::runs(&ops);
        let gone: usize = runs.iter().map(|(gone, _)| ink(&old[gone.clone()])).sum();
        let common = ink(&old) - gone;
        if common * 3 < ink(&old).min(ink(&new)) {
            continue;
        }

        let (mut was, mut is) = (Vec::new(), Vec::new());
        let (mut i, mut j) = (0, 0);
        for (gone, came) in runs {
            was.extend(old[i..gone.start].iter().map(|&token| (token, false)));
            is.extend(new[j..came.start].iter().map(|&token| (token, false)));
            refine(&old[gone.clone()], &new[came.clone()], &mut was, &mut is);
            (i, j) = (gone.end, came.end);
        }
        was.extend(old[i..].iter().map(|&token| (token, false)));
        is.extend(new[j..].iter().map(|&token| (token, false)));
        highlight(&was, &removed_at, &view.starts, &mut out);
        highlight(&is, &added_at, &view.starts, &mut out);
    }
    // In view order. A hunk's own lines already are; two hunks need not be, since a removal is put back
    // by where it sat and the hunks are in git's order.
    out.sort_by_key(|range| range.start);
    out
}

/// One side of a hunk, in pieces — words, or letters where [`refine`] split one — and whether each
/// changed.
type Marked<'a> = Vec<(Token<'a>, bool)>;

/// **One stretch the word diff found changed, diffed again letter by letter**, into `was` and `is`.
///
/// What makes the marks exact: `count` becoming `counter` is `er` added, and `println` becoming
/// `eprintln` is one `e` — rather than both whole words, which says *that* the word changed and leaves
/// the eye to find what. The words come first because letters alone align by chance: two different
/// words share an `a` or an `e` as often as not, and a stretch marked as `g` `mm` around an unchanged
/// `a` is noise.
///
/// So the stretch stays marked **whole** wherever letters would be guessing — see [`by_letter`].
fn refine<'a>(old: &[Token<'a>], new: &[Token<'a>], was: &mut Marked<'a>, is: &mut Marked<'a>) {
    match by_letter(old, new) {
        Some((letters_was, letters_is)) => {
            was.extend(letters_was);
            is.extend(letters_is);
        }
        None => {
            was.extend(old.iter().map(|&token| (token, true)));
            is.extend(new.iter().map(|&token| (token, true)));
        }
    }
}

/// A changed stretch's letters, and which of them changed — or `None` where the letters would be
/// guessing: one side empty (a word added, not edited), a line break inside it, past
/// [`STRETCH_CHARS`], or less than half the shorter side's ink in common, which is `alpha` against
/// `gamma`.
fn by_letter<'a>(old: &[Token<'a>], new: &[Token<'a>]) -> Option<(Marked<'a>, Marked<'a>)> {
    let length = |side: &[Token<'_>]| side.iter().map(|token| token.to - token.from).sum::<usize>();
    let breaks = |side: &[Token<'_>]| side.iter().any(|token| token.text == "\n");
    if old.is_empty() || new.is_empty() || breaks(old) || breaks(new) {
        return None;
    }
    if length(old) > STRETCH_CHARS || length(new) > STRETCH_CHARS {
        return None;
    }
    let (a, b) = (letters(old), letters(new));
    let ops = myers::diff(&texts(&a), &texts(&b))?;
    let mut was: Marked<'a> = a.into_iter().map(|letter| (letter, true)).collect();
    let mut is: Marked<'a> = b.into_iter().map(|letter| (letter, true)).collect();
    let (mut i, mut j, mut common) = (0, 0, 0);
    for op in ops {
        match op {
            myers::Op::Keep => {
                (was[i].1, is[j].1) = (false, false);
                common += was[i].0.ink();
                (i, j) = (i + 1, j + 1);
            }
            myers::Op::Remove => i += 1,
            myers::Op::Add => j += 1,
        }
    }
    // A word's ink is its letters', so the stretch's own tokens say how much there was.
    (common * 2 >= ink(old).min(ink(new))).then_some((was, is))
}

/// `tokens`, a character each.
fn letters<'a>(tokens: &[Token<'a>]) -> Vec<Token<'a>> {
    tokens
        .iter()
        .flat_map(|token| {
            token.text.char_indices().enumerate().map(move |(n, (byte, c))| Token {
                line: token.line,
                from: token.from + n,
                to: token.from + n + 1,
                text: &token.text[byte..byte + c.len_utf8()],
            })
        })
        .collect()
}

/// One side of a hunk's diff, as ranges of the view: `side` is its pieces and which of them differ,
/// `view` which line of the view each of the side's lines is, and `starts` where each view line begins.
fn highlight(
    side: &Marked<'_>,
    view: &[usize],
    starts: &[usize],
    out: &mut Vec<std::ops::Range<usize>>,
) {
    let mut emit = |line: usize, run: std::ops::Range<usize>| {
        if let Some(&start) = view.get(line).and_then(|&at| starts.get(at)) {
            out.push(start + run.start..start + run.end);
        }
    };
    let mut open: Option<(usize, std::ops::Range<usize>)> = None;
    for &(token, changed) in side {
        if token.text == "\n" {
            if let Some((line, run)) = open.take() {
                emit(line, run);
            }
        } else if changed {
            match &mut open {
                // Through any blanks since the last changed piece: they sit between two changes.
                Some((line, run)) if *line == token.line => run.end = token.to,
                _ => open = Some((token.line, token.from..token.to)),
            }
        } else if token.ink() > 0 {
            // Unchanged ink ends a run; unchanged blanks only might.
            if let Some((line, run)) = open.take() {
                emit(line, run);
            }
        }
    }
    if let Some((line, run)) = open.take() {
        emit(line, run);
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

/// How much of the status hue a changed word adds on top of its line's band.
///
/// Measured against the dark palette, where it matters: the `*_subtle` band laid a second time was
/// tried first and reads as no change at all at the size the preview is drawn — the word has to be
/// the thing the eye lands on, and a quarter of the solid hue over the band is what VS Code's own
/// pair of fills comes to.
const WORD_WEIGHT: f32 = 0.25;

/// The fill over a changed word, on top of its line's [`diff_fill`]: the same hue, deeper.
///
/// From the status role and not a literal, so it follows the palette — the status hues are each
/// palette's own, which is what lets a light palette's darker green carry the same weight.
pub(super) fn word_fill(t: &Theme, mark: Mark) -> Option<Color32> {
    match mark {
        Mark::Added => Some(t.status.success.gamma_multiply(WORD_WEIGHT)),
        Mark::Removed => Some(t.status.danger.gamma_multiply(WORD_WEIGHT)),
        Mark::Same | Mark::Skipped(_) => None,
    }
}
