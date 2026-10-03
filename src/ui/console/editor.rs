//! The one-line editor behind the prompt: the caret, the selection, and word motion.
//!
//! Its own thing rather than egui's `TextEdit` because a shell prompt needs history, tab
//! completion and a caret that survives a frame in which the panel was not focused.

/// The command being typed, and the caret in it.
///
/// Byte offsets, kept on character boundaries by only ever moving through [`prev`] and [`next`].
/// `caret` is where typing happens and `anchor` is the far end of the selection, equal to it when
/// there is none — which is the same shape every text editor uses, and the reason `Shift+Left`
/// extends rather than jumps.
#[derive(Clone, Debug, Default)]
pub struct Editor {
    pub(crate) text: String,
    pub(crate) caret: usize,
    pub(crate) anchor: usize,
}

/// The character boundary at or before `at`.
pub(crate) fn prev(text: &str, at: usize) -> usize {
    text[..at].chars().next_back().map_or(0, |c| at - c.len_utf8())
}

/// The character boundary after `at`.
pub(crate) fn next(text: &str, at: usize) -> usize {
    text[at..].chars().next().map_or(at, |c| at + c.len_utf8())
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replace the whole line, caret at the end — what recalling a command does.
    pub(crate) fn set(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.caret = self.text.len();
        self.anchor = self.caret;
    }

    pub(crate) fn clear(&mut self) {
        self.set("");
    }

    pub(crate) fn span(&self) -> Option<std::ops::Range<usize>> {
        let (from, to) = (self.caret.min(self.anchor), self.caret.max(self.anchor));
        (from < to).then_some(from..to)
    }

    pub(crate) fn selected(&self) -> &str {
        self.span().map_or("", |at| &self.text[at])
    }

    /// Take the selection out, and say whether there was one.
    pub(crate) fn erase(&mut self) -> bool {
        let Some(at) = self.span() else { return false };
        self.text.replace_range(at.clone(), "");
        self.caret = at.start;
        self.anchor = self.caret;
        true
    }

    pub(crate) fn insert(&mut self, text: &str) {
        self.erase();
        self.text.insert_str(self.caret, text);
        self.caret += text.len();
        self.anchor = self.caret;
    }

    pub(crate) fn backspace(&mut self) {
        if self.erase() {
            return;
        }
        let from = prev(&self.text, self.caret);
        if from < self.caret {
            self.text.replace_range(from..self.caret, "");
            self.caret = from;
            self.anchor = from;
        }
    }

    pub(crate) fn delete(&mut self) {
        if self.erase() {
            return;
        }
        let to = next(&self.text, self.caret);
        if to > self.caret {
            self.text.replace_range(self.caret..to, "");
        }
    }

    /// `Ctrl+Backspace`: back over the run of spaces, then over the word before it.
    pub(crate) fn kill_word(&mut self) {
        if self.erase() {
            return;
        }
        let from = word_left(&self.text, self.caret);
        self.text.replace_range(from..self.caret, "");
        self.caret = from;
        self.anchor = from;
    }

    /// Put the caret somewhere. `keep` extends the selection instead of dropping it.
    pub(crate) fn go(&mut self, to: usize, keep: bool) {
        self.caret = to.min(self.text.len());
        if !keep {
            self.anchor = self.caret;
        }
    }

    pub(crate) fn left(&mut self, keep: bool, word: bool) {
        // Without shift, a caret with a selection collapses to its near end rather than stepping
        // one character back from it — which is what every editor does and what makes
        // select-then-arrow feel like an escape rather than an edit.
        let to = match (self.span(), word, keep) {
            (Some(at), false, false) => at.start,
            (_, true, _) => word_left(&self.text, self.caret),
            _ => prev(&self.text, self.caret),
        };
        self.go(to, keep);
    }

    pub(crate) fn right(&mut self, keep: bool, word: bool) {
        let to = match (self.span(), word, keep) {
            (Some(at), false, false) => at.end,
            (_, true, _) => word_right(&self.text, self.caret),
            _ => next(&self.text, self.caret),
        };
        self.go(to, keep);
    }

    pub(crate) fn home(&mut self, keep: bool) {
        self.go(0, keep);
    }

    pub(crate) fn end(&mut self, keep: bool) {
        self.go(self.text.len(), keep);
    }

    pub(crate) fn all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// Which column the caret is in, counting characters.
    pub(crate) fn col(&self) -> usize {
        self.text[..self.caret].chars().count()
    }

    /// Where a column falls in the line, as a byte offset.
    pub(crate) fn at_col(&self, col: usize) -> usize {
        self.text
            .char_indices()
            .nth(col)
            .map_or(self.text.len(), |(at, _)| at)
    }

    /// Put the caret at a column, dragging the selection with it if `keep`.
    pub(crate) fn seek(&mut self, col: usize, keep: bool) {
        let at = self.at_col(col);
        self.go(at, keep);
    }
}

/// Back over whitespace, then over the word before it.
pub(crate) fn word_left(text: &str, from: usize) -> usize {
    let mut at = from;
    while at > 0 {
        let back = prev(text, at);
        if !text[back..at].chars().all(char::is_whitespace) {
            break;
        }
        at = back;
    }
    while at > 0 {
        let back = prev(text, at);
        if text[back..at].chars().all(char::is_whitespace) {
            break;
        }
        at = back;
    }
    at
}

/// Forward over the word, then over the whitespace after it.
pub(crate) fn word_right(text: &str, from: usize) -> usize {
    let mut at = from;
    while at < text.len() {
        let ahead = next(text, at);
        if text[at..ahead].chars().all(char::is_whitespace) {
            break;
        }
        at = ahead;
    }
    while at < text.len() {
        let ahead = next(text, at);
        if !text[at..ahead].chars().all(char::is_whitespace) {
            break;
        }
        at = ahead;
    }
    at
}

/// The one thing the arrows steer and a copy is about.
///
/// Three selections could be on screen at once — a drag in the log, a block reached with
/// `Shift+Up`, a range in the prompt — and `Ctrl+C` has to mean exactly one of them. So only one
/// exists at a time, and this is it. Typing always goes to the prompt whatever this says; what it
/// decides is what `Ctrl+C`, `Del` and `Left`/`Right` are *for*.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) enum Aim {
    /// The command line. The caret and any selection in it live on [`Editor`].
    #[default]
    Prompt,
    /// A range dragged out in the log.
    Text(Span),
    /// A whole block, by [`Block::id`] — reached with `Shift+Up`, folded with `Left`/`Right`,
    /// copied whole, and thrown away with `Del`.
    Block(u64),
}

/// A place in the log: which row, and how many characters into it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Spot {
    pub(crate) row: usize,
    pub(crate) col: usize,
}

/// A range in the log, always the right way round.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Span {
    pub(crate) from: Spot,
    pub(crate) to: Spot,
}

impl Span {
    pub(crate) fn new(a: Spot, b: Spot) -> Self {
        let (from, to) = if a <= b { (a, b) } else { (b, a) };
        Self { from, to }
    }

    pub(crate) fn empty(&self) -> bool {
        self.from == self.to
    }

    /// The columns of `row` this covers, given how long the row is.
    pub(crate) fn cut(&self, row: usize, len: usize) -> Option<std::ops::Range<usize>> {
        if row < self.from.row || row > self.to.row {
            return None;
        }
        let start = if row == self.from.row { self.from.col } else { 0 };
        let end = if row == self.to.row { self.to.col } else { len };
        (start < end.min(len)).then(|| start..end.min(len))
    }
}

/// What a drag in the log is selecting by.
///
/// A double click puts it in [`Grab::Word`] until the button comes back up, which is what "select
/// text like an editor" means everywhere else: the two ends of the selection snap out to whole words
/// as the pointer moves. A single press selects by character, exactly.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) enum Grab {
    #[default]
    Char,
    Word,
}

/// The three kinds of run a double click can land in.
///
/// Punctuation is its own class rather than part of a word, which is what makes `cpp` selectable on
/// its own in `source.cpp`: the dot is a run of one and the name either side of it is a word. Lumping
/// them together would select the whole of `source.cpp` and nothing shorter.
#[derive(PartialEq, Eq, Copy, Clone)]
pub(crate) enum Class {
    Word,
    Space,
    Mark,
}

pub(crate) fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Mark
    }
}

/// The run of like characters `col` falls in.
pub(crate) fn word_at(chars: &[char], col: usize) -> std::ops::Range<usize> {
    if chars.is_empty() {
        return 0..0;
    }
    let at = col.min(chars.len() - 1);
    let kind = class(chars[at]);
    let mut from = at;
    while from > 0 && class(chars[from - 1]) == kind {
        from -= 1;
    }
    let mut to = at + 1;
    while to < chars.len() && class(chars[to]) == kind {
        to += 1;
    }
    from..to
}
