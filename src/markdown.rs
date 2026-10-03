//! Markdown, as blocks to draw rather than markup to read.
//!
//! What comes out of [`parse`] is a [`Doc`]: a flat string of **everything that will be
//! on screen**, and a list of blocks saying where each piece of it goes and what it is.
//! `#`, `**` and `](https://…)` are not in that string — they are the instructions, and
//! a rendered document is what happens when you follow them.
//!
//! # Why one string and not one per block
//!
//! Because of the find bar. [`crate::ui::preview`]'s search is a set of byte ranges into
//! one body, and everything built on it — the counter, next and previous, the highlight,
//! keeping your place as you type — works on offsets into that one body. Handing it a
//! string per block would mean a hit becoming a pair of numbers and every one of those
//! parts growing a case. So the document is one string in reading order, each block owns
//! a slice of it, and a hit is still one number.
//!
//! It also settles a question that has to be settled somewhere: **the search searches
//! what you can see**. Looking for `bold` in `**bold**` finds it; looking for `**` finds
//! nothing, because there are no asterisks on the screen.
//!
//! # The subset
//!
//! Headings both ways round, paragraphs, `>` quotes, bullet and numbered lists with
//! nesting, task boxes, fenced and indented code, rules, tables, and inline emphasis,
//! code, strikethrough, links and images. Deliberately left out, each because it is a
//! layout engine rather than a parser: footnotes, reference-style link definitions,
//! nested block quotes containing lists, and HTML — which is passed through as the text
//! it is, on the grounds that a `<br>` shown as `<br>` is a smaller lie than a `<br>`
//! shown as nothing.
//!
//! Emphasis inside a link's text is not parsed either; a link is one run. That one is
//! about the recursion rather than the layout, and it is noted at [`inline`].

use std::ops::Range;

use crate::syntax::Lang;

/// A parsed document: what to draw, and the text to draw it with.
#[derive(Default)]
pub struct Doc {
    /// Every visible character, in reading order, one block after another with a newline
    /// between. **This is what the find bar searches** — see the module header.
    pub text: String,
    pub blocks: Vec<Block>,
    /// Every code block's syntax colouring, as offsets into [`Doc::text`].
    ///
    /// Here rather than worked out while drawing, because it is a function of the text
    /// and nothing else: a fenced ```` ```rust ```` block gets the same colouring a `.rs`
    /// file does, from the same [`crate::syntax`] pass, and doing it once when the file
    /// is read is the difference between that and doing it sixty times a second.
    pub spans: Vec<crate::syntax::Span>,
}

impl Doc {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// One thing to draw, and where its text is.
pub struct Block {
    pub kind: Kind,
    /// This block's slice of [`Doc::text`].
    pub at: Range<usize>,
    /// The inline runs covering `at`, contiguous and in order. Empty for a [`Kind::Rule`]
    /// and for code, whose colour comes from [`crate::syntax`] instead.
    pub runs: Vec<Run>,
    /// How many `>` it was behind.
    pub quote: u8,
    /// List nesting, from the leading blanks. Two spaces to a level.
    pub indent: u8,
}

/// What kind of block it is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `# Heading`, 1 to 6.
    Heading(u8),
    Paragraph,
    /// A list item and its marker: `None` for a bullet, `Some(n)` for `n.`.
    Item(Option<u64>),
    /// A fenced or indented code block, and what language its fence claimed.
    Code(Lang),
    /// `---`. No text.
    Rule,
    /// One row of a table, and whether it is the one above the rule.
    ///
    /// Rows and not a grid: the cells are left in the text with their pipes, and the row
    /// is set in the monospace face so that a table whose source is aligned stays
    /// aligned. A real grid means measuring every column before drawing any of it, which
    /// is a different shape of code from everything else here — it is worth doing and it
    /// is not done.
    Row {
        header: bool,
    },
}

/// A run of one style inside a block.
pub struct Run {
    pub at: Range<usize>,
    pub style: Style,
}

/// How a run is set. All five compose.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    /// `` `inline code` `` — the monospace face on a tinted fill.
    pub code: bool,
    pub strike: bool,
    /// The text of a link, or of an image. The destination is not shown: it is a
    /// document, not a source file, and a paragraph interrupted by three URLs is not one
    /// you can read.
    pub link: bool,
}

/// Parse `source` into blocks.
pub fn parse(source: &str) -> Doc {
    let mut doc = Doc::default();
    for raw in split(source) {
        let start = if doc.text.is_empty() {
            0
        } else {
            doc.text.push('\n');
            doc.text.len()
        };
        let mut runs = Vec::new();
        match raw.kind {
            // Code keeps every character it had, including its indentation, and gets no
            // inline parsing at all: an asterisk in a shell command is an asterisk.
            Kind::Code(_) => doc.text.push_str(&raw.text),
            Kind::Rule => {}
            _ => inline(&raw.text, &mut doc.text, &mut runs, Style::default(), 0),
        }
        doc.blocks.push(Block {
            kind: raw.kind,
            at: start..doc.text.len(),
            runs,
            quote: raw.quote,
            indent: raw.indent,
        });
    }
    // The code blocks, coloured once. Offsets are shifted into the document's own, which
    // is what lets the renderer slice this the same way it slices the find's hits.
    for block in &doc.blocks {
        let Kind::Code(lang) = block.kind else {
            continue;
        };
        for mut span in crate::syntax::spans(&doc.text[block.at.clone()], lang) {
            span.at.start += block.at.start;
            span.at.end += block.at.start;
            doc.spans.push(span);
        }
    }
    doc
}

// ---------------------------------------------------------------------------
// Lines into blocks
// ---------------------------------------------------------------------------

/// A block before its inline markup has been read.
struct Raw {
    kind: Kind,
    text: String,
    quote: u8,
    indent: u8,
}

/// How deep a list may nest before the indent stops meaning anything.
const DEEP: u8 = 6;

fn split(source: &str) -> Vec<Raw> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<Raw> = Vec::new();
    // The block a plain line continues, if any. Cleared by a blank line and by anything
    // that is a block in its own right.
    let mut open: Option<usize> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // ---- A fence, which suspends every other rule until it closes ------
        if let Some((mark, info)) = fence(line.trim_start()) {
            let mut text = String::new();
            i += 1;
            while i < lines.len() && fence(lines[i].trim_start()).map(|(m, _)| m) != Some(mark) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(lines[i]);
                i += 1;
            }
            // Past the closing fence, or past the end if it never closed — which a
            // truncated preview will hand this often.
            i += 1;
            out.push(Raw {
                kind: Kind::Code(crate::syntax::lang_of_tag(info)),
                text,
                quote: 0,
                indent: 0,
            });
            open = None;
            continue;
        }

        let (quote, rest) = quotes(line);
        let trimmed = rest.trim_start();
        let lead = rest.len() - trimmed.len();
        if trimmed.is_empty() {
            open = None;
            i += 1;
            continue;
        }

        // ---- The blocks that are one line and nothing else -----------------
        //
        // **Setext before the rule**, and that order is the whole of the `---`
        // ambiguity: under a paragraph it promotes it to a heading, and anywhere else
        // the same three characters are a horizontal rule.
        if let (Some(level), Some(at)) = (setext(trimmed), open) {
            if out[at].kind == Kind::Paragraph && !out[at].text.contains('\n') {
                out[at].kind = Kind::Heading(level);
                open = None;
                i += 1;
                continue;
            }
        }
        if is_rule(trimmed) {
            out.push(Raw {
                kind: Kind::Rule,
                text: String::new(),
                quote,
                indent: 0,
            });
            open = None;
            i += 1;
            continue;
        }
        if let Some((level, text)) = atx(trimmed) {
            out.push(Raw {
                kind: Kind::Heading(level),
                text: text.to_owned(),
                quote,
                indent: 0,
            });
            open = None;
            i += 1;
            continue;
        }

        // ---- A table, recognised by the rule under its first row -----------
        if trimmed.contains('|') && lines.get(i + 1).is_some_and(|next| is_table_rule(next)) {
            out.push(Raw {
                kind: Kind::Row { header: true },
                text: trimmed.to_owned(),
                quote,
                indent: 0,
            });
            i += 2;
            while let Some(row) = lines.get(i) {
                let (_, rest) = quotes(row);
                let row = rest.trim();
                if row.is_empty() || !row.contains('|') {
                    break;
                }
                out.push(Raw {
                    kind: Kind::Row { header: false },
                    text: row.to_owned(),
                    quote,
                    indent: 0,
                });
                i += 1;
            }
            open = None;
            continue;
        }

        // ---- A list item, before indented code can claim the same spaces ---
        if let Some((marker, text)) = item(trimmed) {
            out.push(Raw {
                kind: Kind::Item(marker),
                text,
                quote,
                indent: ((lead / 2) as u8).min(DEEP),
            });
            open = Some(out.len() - 1);
            i += 1;
            continue;
        }

        // ---- Indented code, which is only code outside a list --------------
        //
        // Inside one, four spaces is how a second paragraph of an item is written, and
        // `open` is what tells the two apart. Tabs have already become four spaces by
        // the time a body reaches here — see `preview::text`.
        if lead >= 4 && open.is_none() {
            let mut text = String::new();
            while let Some(line) = lines.get(i) {
                let (_, rest) = quotes(line);
                if !rest.trim().is_empty() && rest.len() - rest.trim_start().len() < 4 {
                    break;
                }
                if rest.trim().is_empty()
                    && !lines.get(i + 1).is_some_and(|n| n.starts_with("    "))
                {
                    break;
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(rest.get(4..).unwrap_or(""));
                i += 1;
            }
            out.push(Raw {
                kind: Kind::Code(Lang::None),
                text,
                quote,
                indent: 0,
            });
            continue;
        }

        // ---- Otherwise it is a paragraph, or more of one -------------------
        match open {
            Some(at) => {
                // A newline inside a paragraph is a space, unless the line before it
                // asked for a break with two trailing blanks.
                let hard = lines[i - 1].ends_with("  ");
                out[at].text.push(if hard { '\n' } else { ' ' });
                out[at].text.push_str(trimmed.trim_end());
            }
            None => {
                out.push(Raw {
                    kind: Kind::Paragraph,
                    // Trimmed at the end, because the two blanks that ask for a hard break are
                    // markup and the newline above is what they turned into. Left on, they would
                    // be two spaces at the end of the line — invisible until somebody selects the
                    // paragraph and finds them.
                    text: trimmed.trim_end().to_owned(),
                    quote,
                    indent: 0,
                });
                open = Some(out.len() - 1);
            }
        }
        i += 1;
    }
    out
}

/// The `>` markers at the front of a line, and what is left after them.
fn quotes(line: &str) -> (u8, &str) {
    let mut depth = 0;
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start_matches(' ');
        match trimmed.strip_prefix('>') {
            Some(after) if depth < DEEP => {
                depth += 1;
                rest = after.strip_prefix(' ').unwrap_or(after);
            }
            _ => return (depth, rest),
        }
    }
}

/// A ``` or ~~~ fence: which character it was, and its info string.
fn fence(line: &str) -> Option<(char, &str)> {
    for mark in ['`', '~'] {
        let run = line.chars().take_while(|&c| c == mark).count();
        if run >= 3 {
            // The info string is the first word of what follows, so ```rust,ignore and
            // ```js {highlight} both answer.
            let info = line[run..]
                .trim()
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '+' && c != '#')
                .next()
                .unwrap_or("");
            return Some((mark, info));
        }
    }
    None
}

/// `### Heading` — the level, and the text with its hashes taken off both ends.
fn atx(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    // A space is required, so `#hashtag` is a word and `#` alone is not a heading.
    let text = rest.strip_prefix(' ')?.trim();
    Some((hashes as u8, text.trim_end_matches('#').trim_end()))
}

/// `====` or `----` under a paragraph.
fn setext(line: &str) -> Option<u8> {
    let mark = line.chars().next()?;
    if !matches!(mark, '=' | '-') || !line.chars().all(|c| c == mark) {
        return None;
    }
    Some(if mark == '=' { 1 } else { 2 })
}

/// `***`, `---`, `___` — three or more, blanks allowed between them.
///
/// Asked **before** [`setext`], so a `---` on its own line is a rule; the one under a
/// paragraph never reaches here, because that case is answered first.
fn is_rule(line: &str) -> bool {
    let Some(mark) = line.chars().find(|c| !c.is_whitespace()) else {
        return false;
    };
    if !matches!(mark, '*' | '-' | '_') {
        return false;
    }
    let marks = line.chars().filter(|&c| c == mark).count();
    marks >= 3 && line.chars().all(|c| c == mark || c == ' ')
}

/// The `|---|:--:|` row that makes the line above it a table header.
fn is_table_rule(line: &str) -> bool {
    let line = line.trim();
    line.contains('-')
        && line.contains('|')
        && line
            .chars()
            .all(|c| matches!(c, '-' | ':' | '|' | ' ' | '\t'))
}

/// A list item: its marker, and the text after it.
fn item(line: &str) -> Option<(Option<u64>, String)> {
    // A bullet. `*` needs the space, or every `**bold**` paragraph would be a list.
    if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        return Some((None, task(rest.trim())));
    }
    // Or a number, then `.` or `)`.
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits <= 9 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            let n = line[..digits].parse().ok()?;
            return Some((Some(n), task(rest.trim())));
        }
    }
    None
}

/// A task box, which is markup rather than text: `[ ]` and `[x]` become the two
/// characters they mean, so an item reads as done or not at a glance.
///
/// The box goes into the **text** rather than being drawn by the renderer, which is what
/// keeps it out of the list of things a renderer has to know: it is a character somebody
/// can search for and select, like every other character in the document.
fn task(text: &str) -> String {
    for (open, mark) in [
        ("[ ]", '\u{2610}'),
        ("[x]", '\u{2611}'),
        ("[X]", '\u{2611}'),
    ] {
        if let Some(rest) = text.strip_prefix(open) {
            return format!("{mark} {}", rest.trim_start());
        }
    }
    text.to_owned()
}

// ---------------------------------------------------------------------------
// Inline markup
// ---------------------------------------------------------------------------

/// How deep emphasis may nest before the parser stops looking inside it.
const NESTED: u8 = 4;

/// Read `src`'s inline markup, appending what will be shown to `out` and describing it
/// in `runs`.
///
/// Recursive, so `**bold with *both***` works, and capped at [`NESTED`] because the
/// input is a file somebody else wrote. **A link's text is not recursed into**: it
/// becomes one run in the link style, so `[**bold** link]` shows its asterisks. That is
/// the one place this is knowingly wrong, and the reason is that a link's text is the
/// only inline context where the delimiters that end it are not its own.
fn inline(src: &str, out: &mut String, runs: &mut Vec<Run>, base: Style, depth: u8) {
    let b = src.as_bytes();
    let mut i = 0;
    // Where the run being accumulated started, in `out`.
    let mut from = out.len();
    let push = |out: &mut String, runs: &mut Vec<Run>, from: &mut usize| {
        if out.len() > *from {
            let at = *from..out.len();
            // Extend rather than push, where the last run is the same style and ends
            // where this one starts: adjacent runs of one style are one run, and a
            // paragraph of plain text should be a single section in the galley.
            match runs.last_mut() {
                Some(last) if last.style == base && last.at.end == at.start => last.at.end = at.end,
                _ => runs.push(Run { at, style: base }),
            }
            *from = out.len();
        }
    };
    while i < b.len() {
        let c = b[i];
        // An escape: the next character, whatever it is, is text.
        if c == b'\\' {
            if let Some(next) = src[i + 1..].chars().next() {
                if next.is_ascii_punctuation() {
                    push(out, runs, &mut from);
                    out.push(next);
                    push(out, runs, &mut from);
                    i += 1 + next.len_utf8();
                    continue;
                }
            }
        }
        // Inline code, which wins over everything: a `*` inside backticks is a `*`.
        if c == b'`' {
            let ticks = b[i..].iter().take_while(|&&x| x == b'`').count();
            let open = &src[i..i + ticks];
            if let Some(at) = src[i + ticks..].find(open) {
                push(out, runs, &mut from);
                let mut style = base;
                style.code = true;
                let text = src[i + ticks..i + ticks + at].trim();
                let start = out.len();
                out.push_str(text);
                if out.len() > start {
                    runs.push(Run {
                        at: start..out.len(),
                        style,
                    });
                }
                from = out.len();
                i += ticks + at + ticks;
                continue;
            }
        }
        // A link or an image. The `!` is dropped with the rest of the markup: what an
        // image contributes to a document that cannot show it is its alt text.
        let image = c == b'!' && b.get(i + 1) == Some(&b'[');
        if c == b'[' || image {
            let open = i + usize::from(image);
            if let Some((text, skip)) = link(&src[open..]) {
                push(out, runs, &mut from);
                let mut style = base;
                style.link = true;
                let start = out.len();
                out.push_str(text);
                if out.len() > start {
                    runs.push(Run {
                        at: start..out.len(),
                        style,
                    });
                }
                from = out.len();
                i = open + skip;
                continue;
            }
        }
        // `<https://…>`, which is a link with no text of its own.
        if c == b'<' {
            if let Some(at) = src[i..].find('>') {
                let inner = &src[i + 1..i + at];
                if inner.starts_with("http://")
                    || inner.starts_with("https://")
                    || inner.starts_with("mailto:")
                {
                    push(out, runs, &mut from);
                    let mut style = base;
                    style.link = true;
                    let start = out.len();
                    out.push_str(inner);
                    runs.push(Run {
                        at: start..out.len(),
                        style,
                    });
                    from = out.len();
                    i += at + 1;
                    continue;
                }
            }
        }
        // Emphasis. **The longest marker first**, so `***` is never read as `**` and a
        // stray `*`, and `**` never as two italics.
        let mut emphasised = false;
        if depth < NESTED && matches!(c, b'*' | b'_' | b'~') {
            for (mark, bold, italic, strike) in [
                ("~~", false, false, true),
                ("***", true, true, false),
                ("**", true, false, false),
                ("__", true, false, false),
                ("*", false, true, false),
                ("_", false, true, false),
            ] {
                if !src[i..].starts_with(mark) {
                    continue;
                }
                // **An underscore does not open emphasis inside a word.** `snake_case`
                // and `__init__` are one word each, and the asterisk rule — which
                // CommonMark does allow inside a word — would cut them in half.
                if mark.starts_with('_') && src[..i].ends_with(|c: char| c.is_alphanumeric()) {
                    continue;
                }
                let after = i + mark.len();
                // A closing marker with something between: `**` on its own is two
                // asterisks, and emphasis may not begin or end on a blank.
                let Some(at) = src[after..].find(mark).filter(|&at| at > 0) else {
                    continue;
                };
                let inner = &src[after..after + at];
                if inner.starts_with(char::is_whitespace) || inner.ends_with(char::is_whitespace) {
                    continue;
                }
                push(out, runs, &mut from);
                let mut style = base;
                style.bold |= bold;
                style.italic |= italic;
                style.strike |= strike;
                inline(inner, out, runs, style, depth + 1);
                from = out.len();
                i = after + at + mark.len();
                emphasised = true;
                break;
            }
        }
        if emphasised {
            continue;
        }
        // Anything else is a character of text, and it is stepped over by character
        // rather than by byte so an accent does not land inside itself.
        let step = src[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&src[i..i + step]);
        i += step;
    }
    push(out, runs, &mut from);
}

/// `[text](dest)` or `[text][ref]`: the text, and how many bytes the whole thing took.
fn link(src: &str) -> Option<(&str, usize)> {
    let close = balanced(src)?;
    let text = &src[1..close];
    let rest = &src[close + 1..];
    if let Some(after) = rest.strip_prefix('(') {
        let end = after.find(')')?;
        return Some((text, close + 1 + 1 + end + 1));
    }
    // A reference link. There is no definition table here, so the label is dropped and
    // the text stands on its own — which is what it does on the page as well.
    if let Some(after) = rest.strip_prefix('[') {
        let end = after.find(']')?;
        return Some((text, close + 1 + 1 + end + 1));
    }
    None
}

/// Where the `[` at the front of `src` closes, allowing one level of brackets inside it
/// so `[the [inner] one]` is one label.
fn balanced(src: &str) -> Option<usize> {
    let mut depth = 0;
    for (at, c) in src.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            '\n' => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blocks as `(kind, the text on screen)`, which is what a test about rendering wants.
    fn blocks(source: &str) -> Vec<(Kind, &str)> {
        // Leaked so the slices can borrow it: a test helper, and the process is about to end.
        let doc = Box::leak(Box::new(parse(source)));
        doc.blocks
            .iter()
            .map(|b| (b.kind.clone(), &doc.text[b.at.clone()]))
            .collect()
    }

    /// What one run is set in, as a short word per style, for readable expectations.
    fn styles(source: &str) -> Vec<(String, &'static str)> {
        let doc = parse(source);
        doc.blocks
            .iter()
            .flat_map(|b| b.runs.iter())
            .map(|run| {
                let s = run.style;
                let name = if s.code {
                    "code"
                } else if s.link {
                    "link"
                } else if s.bold && s.italic {
                    "both"
                } else if s.bold {
                    "bold"
                } else if s.italic {
                    "italic"
                } else if s.strike {
                    "strike"
                } else {
                    "plain"
                };
                (doc.text[run.at.clone()].to_owned(), name)
            })
            .collect()
    }

    /// Headings both ways round, and the `---` that is two different things.
    #[test]
    fn a_heading_is_a_heading_written_either_way() {
        assert_eq!(
            blocks("# One\n\n## Two ##\n\n###### Six\n"),
            [
                (Kind::Heading(1), "One"),
                (Kind::Heading(2), "Two"),
                (Kind::Heading(6), "Six"),
            ]
        );
        // Setext, which is what every generated README uses.
        assert_eq!(
            blocks("Title\n=====\n\nSection\n-------\n"),
            [(Kind::Heading(1), "Title"), (Kind::Heading(2), "Section")]
        );
        // And the same three characters with nothing above them is a rule. This is the one
        // ambiguity in the format and the order in `split` is what settles it.
        assert_eq!(
            blocks("Some words.\n\n---\n\nMore.\n"),
            [
                (Kind::Paragraph, "Some words."),
                (Kind::Rule, ""),
                (Kind::Paragraph, "More."),
            ]
        );
        // `#six` and `#!/bin/sh` are not headings: the space is required.
        assert_eq!(
            blocks("#hashtag\n"),
            [(Kind::Paragraph, "#hashtag")],
            "a hash with no space became a heading"
        );
    }

    /// A paragraph's line breaks close up, unless they were asked for.
    #[test]
    fn a_wrapped_paragraph_becomes_one_paragraph() {
        assert_eq!(
            blocks("one line\nand the next\n\na second paragraph\n"),
            [
                (Kind::Paragraph, "one line and the next"),
                (Kind::Paragraph, "a second paragraph"),
            ]
        );
        // Two trailing blanks is markdown's own hard break, and it survives as a newline.
        assert_eq!(
            blocks("first  \nsecond\n"),
            [(Kind::Paragraph, "first\nsecond")]
        );
    }

    /// Lists: both markers, the numbers, the nesting, and the task boxes.
    #[test]
    fn a_list_keeps_its_markers_and_its_depth() {
        let doc = parse("- one\n- two\n  - deeper\n\n1. first\n2. second\n");
        let shape: Vec<_> = doc
            .blocks
            .iter()
            .map(|b| (b.kind.clone(), b.indent, &doc.text[b.at.clone()]))
            .collect();
        assert_eq!(
            shape,
            [
                (Kind::Item(None), 0, "one"),
                (Kind::Item(None), 0, "two"),
                (Kind::Item(None), 1, "deeper"),
                (Kind::Item(Some(1)), 0, "first"),
                (Kind::Item(Some(2)), 0, "second"),
            ]
        );
        // A task box becomes the character it means, in the text — so it can be selected and
        // searched for like anything else on the screen.
        assert_eq!(
            blocks("- [x] done\n- [ ] not\n"),
            [
                (Kind::Item(None), "\u{2611} done"),
                (Kind::Item(None), "\u{2610} not"),
            ]
        );
    }

    /// A fence, its language, and the fact that nothing inside it is markup.
    #[test]
    fn a_fence_is_code_and_knows_its_language() {
        let doc = parse("```rust\nlet x = *p; // a *comment*\n```\n");
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, Kind::Code(crate::syntax::Lang::Rust));
        assert_eq!(doc.text, "let x = *p; // a *comment*");
        assert!(
            doc.blocks[0].runs.is_empty(),
            "code was given inline runs, so an asterisk in it is emphasis"
        );
        // And it is coloured, by the same pass a `.rs` file goes through.
        assert!(
            doc.spans
                .iter()
                .any(|s| s.tok == crate::syntax::Tok::Keyword && doc.text[s.at.clone()] == *"let"),
            "the fence was not coloured: {:?}",
            doc.spans
        );

        // An info string with more in it than a language, and a fence that never closes — which
        // is what a truncated preview hands this.
        assert_eq!(
            parse("```js {1,3}\nlet a\n```\n").blocks[0].kind,
            Kind::Code(crate::syntax::Lang::Web)
        );
        // A fence is labelled with a language's *name*, not with an extension — which is a
        // different table. See `syntax::lang_of_tag`.
        for (tag, want) in [
            ("rust", crate::syntax::Lang::Rust),
            ("bash", crate::syntax::Lang::Shell),
            ("python", crate::syntax::Lang::Python),
            ("powershell", crate::syntax::Lang::PowerShell),
            ("JSON", crate::syntax::Lang::Json),
            ("text", Lang::None),
            ("", Lang::None),
        ] {
            assert_eq!(
                parse(&format!("```{tag}\nx\n```\n")).blocks[0].kind,
                Kind::Code(want),
                "```{tag}"
            );
        }
        assert_eq!(
            blocks("```\nno end\n"),
            [(Kind::Code(Lang::None), "no end")]
        );
        // Four spaces is code as well, but only where a list is not using them for its own.
        assert_eq!(
            blocks("text\n\n    indented();\n"),
            [
                (Kind::Paragraph, "text"),
                (Kind::Code(Lang::None), "indented();")
            ]
        );
        assert_eq!(
            blocks("- item\n    still the item\n"),
            [(Kind::Item(None), "item still the item")],
            "a continuation line was read as an indented code block"
        );
    }

    /// Quotes, and the depth they came from.
    #[test]
    fn a_quote_remembers_how_deep_it_was() {
        let doc = parse("> quoted\n> still\n\n>> deeper\n\nplain\n");
        let shape: Vec<_> = doc
            .blocks
            .iter()
            .map(|b| (b.quote, &doc.text[b.at.clone()]))
            .collect();
        assert_eq!(shape, [(1, "quoted still"), (2, "deeper"), (0, "plain")]);
        // A quoted list is a list, not a paragraph beginning with a dash.
        assert_eq!(blocks("> - one\n"), [(Kind::Item(None), "one")]);
    }

    /// A table's rows, with the rule between them dropped.
    #[test]
    fn a_table_keeps_its_rows_and_loses_its_rule() {
        assert_eq!(
            blocks("| a | b |\n|---|:-:|\n| 1 | 2 |\n\nafter\n"),
            [
                (Kind::Row { header: true }, "| a | b |"),
                (Kind::Row { header: false }, "| 1 | 2 |"),
                (Kind::Paragraph, "after"),
            ]
        );
        // A line with a pipe in it and no rule under it is a paragraph, which is most of them.
        assert_eq!(
            blocks("a | b is a choice\n"),
            [(Kind::Paragraph, "a | b is a choice")]
        );
    }

    /// The inline markup, and what is left on the screen after it is followed.
    #[test]
    fn markup_leaves_the_text_and_takes_the_marks() {
        assert_eq!(
            parse("**bold** and *thin* and `code` and ~~gone~~.").text,
            "bold and thin and code and gone."
        );
        assert_eq!(
            styles("**bold** and *thin* and `code` and ~~gone~~."),
            [
                ("bold".to_owned(), "bold"),
                (" and ".to_owned(), "plain"),
                ("thin".to_owned(), "italic"),
                (" and ".to_owned(), "plain"),
                ("code".to_owned(), "code"),
                (" and ".to_owned(), "plain"),
                ("gone".to_owned(), "strike"),
                (".".to_owned(), "plain"),
            ]
        );
        // Nested, and the three-marker case that must not read as two.
        assert_eq!(styles("***all***"), [("all".to_owned(), "both")]);
        assert_eq!(
            styles("**outer *inner* back**"),
            [
                ("outer ".to_owned(), "bold"),
                ("inner".to_owned(), "both"),
                (" back".to_owned(), "bold"),
            ]
        );
        // A link shows its text and not its destination; an image shows its alt text.
        assert_eq!(
            parse("see [the docs](https://example.com/a) and ![a chart](c.png)").text,
            "see the docs and a chart"
        );
        assert_eq!(parse("<https://example.com>").text, "https://example.com");
        // Backticks win over everything inside them.
        assert_eq!(parse("`a **b** c`").text, "a **b** c");
        // And an escape puts the character back.
        assert_eq!(parse(r"a \*not emphasis\* b").text, "a *not emphasis* b");
    }

    /// An underscore inside a word is part of the word.
    ///
    /// The trap: `snake_case_name` has two underscores an even distance apart, so the emphasis
    /// rule that works for asterisks would set `case` in italics and eat both marks. CommonMark
    /// forbids an `_` that follows an alphanumeric from opening emphasis for exactly this reason,
    /// and a README full of identifiers is the document where it shows.
    #[test]
    fn an_underscore_in_an_identifier_is_not_emphasis() {
        for source in [
            "snake_case_name",
            "a_b_c",
            "call(some_arg_here)",
            "x_1 + y_2",
        ] {
            assert_eq!(parse(source).text, source, "{source}");
        }
        // But a `_word_` standing on its own still is emphasis — and so, deliberately, is a
        // dunder, because the rule is about what *precedes* the marker and there is nothing
        // before this one. It is also what GitHub shows for the same text, which is the
        // rendering somebody comparing the two will expect.
        assert_eq!(styles("a _word_ here")[1], ("word".to_owned(), "italic"));
        assert_eq!(styles("__init__"), [("init".to_owned(), "bold")]);
    }

    /// The three invariants the renderer depends on and cannot check.
    ///
    /// A block's slice has to be inside the text, the blocks have to be in order, and a block's
    /// runs have to cover it end to end — because a run that is missing is not text in the
    /// default style, it is a gap epaint asserts on. Run over a document holding one of
    /// everything.
    #[test]
    fn every_block_is_covered_by_its_own_runs() {
        let source = "\
# Title\n\nSome **bold** words and `code`.\n\n\
> A quote with a [link](x).\n\n\
- one\n- [x] two\n  - deeper\n\n\
1. first\n\n\
```rust\nlet x = 1;\n```\n\n\
    indented();\n\n\
| a | b |\n|---|---|\n| 1 | 2 |\n\n\
---\n\nLast.\n";
        let doc = parse(source);
        assert!(doc.blocks.len() > 10, "{} blocks", doc.blocks.len());
        let mut end = 0;
        for block in &doc.blocks {
            assert!(
                block.at.start >= end && block.at.end <= doc.text.len(),
                "{:?} at {:?} is out of order or out of bounds",
                block.kind,
                block.at
            );
            assert!(
                doc.text.is_char_boundary(block.at.start)
                    && doc.text.is_char_boundary(block.at.end),
                "{:?} cuts a character",
                block.at
            );
            end = block.at.end;

            // Code and rules have no runs by design; everything else is covered.
            if matches!(block.kind, Kind::Code(_) | Kind::Rule) {
                assert!(block.runs.is_empty(), "{:?} has runs", block.kind);
                continue;
            }
            let mut cut = block.at.start;
            for run in &block.runs {
                assert_eq!(
                    run.at.start,
                    cut,
                    "a gap or an overlap before {:?} in {:?}",
                    doc.text[run.at.clone()].to_owned(),
                    block.kind
                );
                cut = run.at.end;
            }
            assert_eq!(
                cut, block.at.end,
                "{:?}'s runs stop before its text does",
                block.kind
            );
        }
    }

    /// A file with nothing in it, and one with nothing a parser can see.
    #[test]
    fn an_empty_document_is_empty_rather_than_one_blank_block() {
        assert!(parse("").is_empty());
        assert!(parse("\n\n   \n").is_empty());
        // And a `.md` that is only prose is one paragraph, which is a document.
        assert!(!parse("just words").is_empty());
    }
}
