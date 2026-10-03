//! What one row of the log is, and how a run of them becomes the lines on screen.

use super::*;

/// What one row of the log is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Row {
    /// The blank row a block starts with — air between one command's output and the next one's
    /// header, so a run of commands reads as a run of blocks and not as one wall of text.
    ///
    /// The block it belongs to is the one *below* it, and the first block has none: a log that opens
    /// with an empty row looks like a log with something missing off the top of it.
    Gap(usize),
    /// A block's header: the command, and how it ended. The second number is which slice of a wrapped
    /// command this row is, and zero whenever wrapping is off.
    Head(usize, usize),
    /// A line the command printed, and which slice of it this row is.
    Text(usize, usize, usize),
    /// The note a capped block wears, saying what it threw away.
    Cut(usize),
}

impl Row {
    pub(crate) fn block(self) -> usize {
        match self {
            Row::Gap(at) | Row::Head(at, _) | Row::Text(at, _, _) | Row::Cut(at) => at,
        }
    }

    /// Which slice of a wrapped line this is.
    pub(crate) fn slice(self) -> usize {
        match self {
            Row::Head(_, at) | Row::Text(_, _, at) => at,
            Row::Gap(_) | Row::Cut(_) => 0,
        }
    }

    /// Whether two rows are slices of the same logical line — which is what stops a copy putting a
    /// newline into the middle of a path that only *looks* like two lines because it wrapped.
    pub(crate) fn same_line(self, other: Self) -> bool {
        match (self, other) {
            (Row::Head(a, _), Row::Head(b, _)) => a == b,
            (Row::Text(a, x, _), Row::Text(b, y, _)) => a == b && x == y,
            _ => false,
        }
    }
}

/// A block as it would be pasted: the command, then everything it printed.
///
/// Through [`columns`] like everything else, so that a whole-block copy and a dragged one give the
/// same characters for the same line. What you see is what you get, and a panel where those two
/// disagree about a tab is a panel where one of them is wrong.
pub(crate) fn transcript(block: &Block) -> String {
    let mut text = format!("{MARK}{}", columns(&block.command));
    for line in &block.lines {
        text.push_str("\r\n");
        text.push_str(&columns(&line.text));
    }
    text
}

/// The text on a row, exactly as the log draws it — so what is copied is what was selected.
///
/// `cols` is the wrap width, and slicing here rather than at the paint is what keeps that true: the
/// row the pointer is over, the characters drawn on it and the characters a copy takes are one string.
pub(crate) fn shown(blocks: &[Block], row: Row, cols: Option<usize>) -> Cow<'_, str> {
    let whole = match row {
        // Nothing on it, and nothing is what a copy across it should take: a blank row between two
        // blocks pastes as the blank line it looks like.
        Row::Gap(_) => return Cow::Borrowed(""),
        Row::Head(at, _) => columns(&blocks[at].command),
        Row::Text(at, line, _) => columns(&blocks[at].lines[line].text),
        Row::Cut(at) => {
            return Cow::Owned(format!("… {} earlier lines dropped", blocks[at].dropped))
        }
    };
    match cols {
        None => whole,
        Some(cols) => Cow::Owned(
            whole
                .chars()
                .skip(row.slice() * cols.max(1))
                .take(cols.max(1))
                .collect(),
        ),
    }
}

/// How many rows a line takes at `cols` columns. At least one, so a blank line is still a line.
pub(crate) fn slices(text: &str, cols: usize) -> usize {
    let width = columns(text).chars().count();
    width.div_ceil(cols.max(1)).max(1)
}

/// What the wrapped index depends on: the blocks, their lengths, their folds, and the width.
///
/// Hashed rather than compared, because comparing means keeping a copy of all of it. One pass over the
/// *blocks* — never over their lines — so asking "has anything moved" stays cheap even when answering
/// "what is on every row" is not.
pub(crate) fn shape_of(blocks: &[Block], cols: usize) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cols.hash(&mut hasher);
    blocks.len().hash(&mut hasher);
    for block in blocks {
        block.id.hash(&mut hasher);
        block.lines.len().hash(&mut hasher);
        block.dropped.hash(&mut hasher);
        block.collapsed.hash(&mut hasher);
        // The last line is the one that grows in place while a progress bar redraws itself, and its
        // length is what decides how many rows it takes.
        block.lines.last().map(|line| line.text.len()).hash(&mut hasher);
    }
    hasher.finish()
}

/// One character, one column — tabs expanded and anything else invisible turned into a space.
///
/// **The whole panel is columns**: the character under the pointer is `(x - left) / advance`, the x of
/// a character is the multiplication back, and a copy slices the row by those same numbers. All of
/// which is only true while every character is exactly one column wide, and a tab is not: epaint
/// advances it to the next stop, so a single leading tab put every character after it four columns
/// along while this counted one.
///
/// Nothing looked wrong, which is what made it worth a bug report rather than a glance. The highlight
/// is drawn at the column the pointer was over, so it lands on the glyphs you dragged across; the copy
/// takes the characters at that column *index*, three of which were the tab's. Drag across
/// `LgsxDetailsWidget.h` on a line starting with a tab and the clipboard says `xDetailsWidget.h` — the
/// same number of characters, three along.
///
/// So the tab is expanded once, on the way to being shown, and the panel never sees one. Control
/// characters go the same way for the same reason: they have no glyph, so whatever a font does with
/// them is not one column either.
///
/// What is left is scripts wider than one column, which is a trade a terminal makes too — see the
/// module header.
pub(crate) fn columns(text: &str) -> Cow<'_, str> {
    if !text.contains(|c: char| c.is_control()) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + TAB);
    let mut col = 0;
    for c in text.chars() {
        match c {
            '\t' => {
                let stop = (col / TAB + 1) * TAB;
                for _ in col..stop {
                    out.push(' ');
                }
                col = stop;
            }
            c if c.is_control() => {
                out.push(' ');
                col += 1;
            }
            c => {
                out.push(c);
                col += 1;
            }
        }
    }
    Cow::Owned(out)
}
