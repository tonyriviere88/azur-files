//! The lexer: one pass over a body, emitting a span per run that is not plain text.
//!
//! Table-driven, so a new language is an entry in [`super::langs`] and no code here.

use super::*;

pub(crate) fn code(body: &str, r: &Rules, out: &mut Vec<Span>) {
    let b = body.as_bytes();
    let n = b.len();
    let mut i = 0;
    // Whether nothing but blanks has been seen on this line yet, which is what the
    // directive, section and `rem` tests are about.
    let mut fresh = true;
    while i < n {
        let c = b[i];
        if c == b'\n' {
            fresh = true;
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        let head = fresh;
        fresh = false;

        // ---- Comments, before anything else can claim the characters -------
        if let Some(end) = line_comment(body, i, r, head) {
            out.push(Span {
                at: i..end,
                tok: Tok::Comment,
            });
            i = end;
            continue;
        }
        if let Some(end) = block_comment(body, i, r) {
            out.push(Span {
                at: i..end,
                tok: Tok::Comment,
            });
            // A block comment can span lines, and what follows the close is not at the
            // head of one.
            i = end;
            continue;
        }

        // ---- The two things that are only true at the head of a line -------
        if head {
            if r.hash && c == b'#' {
                let end = word_end(b, i + 1);
                if end > i + 1 {
                    out.push(Span {
                        at: i..end,
                        tok: Tok::Keyword,
                    });
                    i = end;
                    continue;
                }
            }
            if r.sections && c == b'[' {
                if let Some(close) = b[i..].iter().position(|&x| x == b']' || x == b'\n') {
                    if b[i + close] == b']' {
                        out.push(Span {
                            at: i..i + close + 1,
                            tok: Tok::Kind,
                        });
                        i += close + 1;
                        continue;
                    }
                }
            }
        }

        // ---- Strings ------------------------------------------------------
        if let Some(end) = string(body, i, r) {
            // A string in the key position is a key: `"name": value`, which is most of
            // what there is to colour in JSON.
            let tok = if labelled(b, end, r) {
                Tok::Name
            } else {
                Tok::Str
            };
            out.push(Span { at: i..end, tok });
            i = end;
            continue;
        }

        // ---- Numbers ------------------------------------------------------
        //
        // No test for what came before it: every branch above consumes a word to its
        // end, so a digit reached here is the start of one and not the `2` of `utf8`.
        if c.is_ascii_digit() {
            let end = number_end(b, i);
            out.push(Span {
                at: i..end,
                tok: Tok::Number,
            });
            i = end;
            continue;
        }

        // ---- Variables ----------------------------------------------------
        if r.sigils.contains(&c) {
            if let Some(end) = variable_end(b, i) {
                out.push(Span {
                    at: i..end,
                    tok: Tok::Variable,
                });
                i = end;
                continue;
            }
        }

        // ---- Words --------------------------------------------------------
        if starts_word(c) {
            let end = word_end(b, i);
            let word = &body[i..end];
            if let Some(tok) = classify(word, b, end, r) {
                out.push(Span { at: i..end, tok });
            }
            i = end;
            continue;
        }

        // Punctuation, an operator, or a character no rule claims. One character on,
        // and by character rather than byte so a `é` in the middle of a line does not
        // land inside itself.
        i = step(body, i);
    }
}

/// Which colour a word gets, or `None` for a plain identifier.
pub(crate) fn classify(word: &str, b: &[u8], end: usize, r: &Rules) -> Option<Tok> {
    let listed = |list: &[&str]| {
        list.iter().any(|&k| {
            if r.fold {
                k.eq_ignore_ascii_case(word)
            } else {
                k == word
            }
        })
    };
    if listed(r.keywords) {
        return Some(Tok::Keyword);
    }
    if listed(r.types) {
        return Some(Tok::Kind);
    }
    if labelled(b, end, r) {
        return Some(Tok::Name);
    }
    // A call: the `(` has to be welded on. See the module header.
    if r.calls && b.get(end) == Some(&b'(') {
        return Some(Tok::Name);
    }
    // A capital that comes back down. `Rect` yes, `MAX_SIZE` no — the second word is a
    // constant, and colouring it as a type would be a claim about the code that is
    // wrong more often than right.
    if r.capitals {
        let mut chars = word.chars();
        let first = chars.next()?;
        if first.is_uppercase() && chars.any(char::is_lowercase) {
            return Some(Tok::Kind);
        }
    }
    None
}

/// Whether what ends at `end` is followed by one of the label characters, blanks
/// allowed in between.
pub(crate) fn labelled(b: &[u8], end: usize, r: &Rules) -> bool {
    if r.label.is_empty() {
        return false;
    }
    let mut at = end;
    while matches!(b.get(at), Some(b' ') | Some(b'\t')) {
        at += 1;
    }
    b.get(at).is_some_and(|c| r.label.contains(c))
}

/// Where a line comment starting at `i` ends, if one does.
pub(crate) fn line_comment(body: &str, i: usize, r: &Rules, head: bool) -> Option<usize> {
    let rest = &body[i..];
    let opens = r.line.iter().any(|&open| {
        // `rem ` is a command, not a marker: it is a comment at the head of a line and
        // an argument anywhere else. Recognised by the space in the entry, which is the
        // only way a marker in this table can contain one.
        if open.ends_with(' ') && !head {
            return false;
        }
        if open.starts_with(|c: char| c.is_ascii_alphabetic()) {
            rest.len() >= open.len() && rest[..open.len()].eq_ignore_ascii_case(open)
        } else {
            rest.starts_with(open)
        }
    });
    if !opens {
        return None;
    }
    Some(match rest.find('\n') {
        Some(nl) => i + nl,
        None => body.len(),
    })
}

/// Where a block comment starting at `i` ends, if one does. Unclosed runs to the end of
/// the file, which is what a truncated preview will often hand this.
pub(crate) fn block_comment(body: &str, i: usize, r: &Rules) -> Option<usize> {
    let rest = &body[i..];
    let (open, close) = r.block.iter().find(|(open, _)| rest.starts_with(open))?;
    let from = i + open.len();
    Some(match body[from..].find(close) {
        Some(at) => from + at + close.len(),
        None => body.len(),
    })
}

/// Where a string starting at `i` ends, if one starts there.
pub(crate) fn string(body: &str, i: usize, r: &Rules) -> Option<usize> {
    let b = body.as_bytes();
    if r.triple {
        for delim in ["\"\"\"", "'''"] {
            if body[i..].starts_with(delim) {
                let from = i + delim.len();
                let close = body[from..].find(delim).map(|at| from + at + delim.len());
                return Some(close.unwrap_or(body.len()));
            }
        }
    }
    for quote in r.quotes {
        match *quote {
            Quote::Run {
                delim,
                escape,
                multiline,
            } if b[i] == delim => {
                let mut at = i + 1;
                while at < b.len() {
                    let c = b[at];
                    if escape && c == b'\\' {
                        // Two on, and never past the end: a `\` as the last byte of a
                        // truncated file must not step outside the slice.
                        at = (at + 2).min(b.len());
                        continue;
                    }
                    if c == delim {
                        return Some(at + 1);
                    }
                    if c == b'\n' && !multiline {
                        return Some(at);
                    }
                    at += 1;
                }
                return Some(b.len());
            }
            Quote::Letter if b[i] == b'\'' => {
                return letter(body, i);
            }
            _ => {}
        }
    }
    None
}

/// Where a `'c'` literal ends, or `None` if what is at `i` is a lifetime, a
/// contraction, or anything else that merely starts with a quote.
///
/// The strictness is the point. See [`Quote::Letter`].
pub(crate) fn letter(body: &str, i: usize) -> Option<usize> {
    let b = body.as_bytes();
    if b.get(i + 1) == Some(&b'\\') {
        // An escape, which can be `\n` or `\u{1F600}`: scan for the close, but only
        // within one literal's worth and never past the line. By byte rather than by
        // slicing, because a cap counted in bytes can land inside a character.
        const MOST: usize = 12;
        let stop = (i + 2 + MOST).min(b.len());
        let mut at = i + 2;
        while at < stop {
            match b[at] {
                b'\'' => return Some(at + 1),
                b'\n' => return None,
                _ => at += 1,
            }
        }
        return None;
    }
    // Otherwise exactly one character, then the closing quote.
    let one = body[i + 1..].chars().next()?;
    if one == '\n' {
        return None;
    }
    let close = i + 1 + one.len_utf8();
    (b.get(close) == Some(&b'\'')).then_some(close + 1)
}

/// Where the number starting at `i` ends.
pub(crate) fn number_end(b: &[u8], i: usize) -> usize {
    let mut at = i;
    while at < b.len() {
        let c = b[at];
        if c.is_ascii_alphanumeric() || c == b'_' {
            at += 1;
        } else if c == b'.' && b.get(at + 1).is_some_and(u8::is_ascii_digit) {
            // A dot only continues a number when a digit follows it, which is what
            // keeps Rust's `0..10` three tokens rather than one.
            at += 1;
        } else if (c == b'+' || c == b'-')
            && at > i
            && matches!(b[at - 1], b'e' | b'E')
            && b.get(at + 1).is_some_and(u8::is_ascii_digit)
        {
            // `1e-5`.
            at += 1;
        } else {
            break;
        }
    }
    at
}

/// Where the variable starting at the sigil at `i` ends, or `None` if the sigil is on
/// its own — `$` at the end of a line, or a bare `%` in arithmetic.
pub(crate) fn variable_end(b: &[u8], i: usize) -> Option<usize> {
    let sigil = b[i];
    let mut at = i + 1;
    // `${prefix}` and `$(command)`, whose braces belong to the name.
    if let Some(&open @ (b'{' | b'(')) = b.get(at) {
        let close = if open == b'{' { b'}' } else { b')' };
        while at < b.len() && b[at] != close && b[at] != b'\n' {
            at += 1;
        }
        return (at < b.len() && b[at] == close).then_some(at + 1);
    }
    // `%~dp0` in a batch file, and `$?`, `$@`, `$1` in a shell.
    while at < b.len() && (is_word(b[at]) || b[at] == b'~') {
        at += 1;
    }
    if at == i + 1 {
        // One punctuation character can still be a name: `$?`, `$*`, `$#`.
        return match b.get(at) {
            Some(&c) if sigil == b'$' && matches!(c, b'?' | b'*' | b'#' | b'@' | b'!') => {
                Some(at + 1)
            }
            _ => None,
        };
    }
    // `%VAR%` closes; `%1` does not.
    if sigil == b'%' && b.get(at) == Some(&b'%') {
        at += 1;
    }
    Some(at)
}

/// Whether a byte can begin a word. Anything non-ASCII can: an identifier is allowed to
/// be in somebody's own language, and a token that stopped at the first accent would
/// split it in half.
pub(crate) fn starts_word(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

pub(crate) fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

pub(crate) fn word_end(b: &[u8], i: usize) -> usize {
    let mut at = i;
    while at < b.len() && is_word(b[at]) {
        at += 1;
    }
    at
}

/// One character on from `i`.
pub(crate) fn step(body: &str, i: usize) -> usize {
    let mut at = i + 1;
    while at < body.len() && !body.is_char_boundary(at) {
        at += 1;
    }
    at
}
