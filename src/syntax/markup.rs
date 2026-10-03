//! The two languages that are not lexed like code: a unified diff, and tag markup.

use super::*;

/// A unified diff, coloured **by the line** rather than by the token.
///
/// Which is the whole reason it is not a `Rules` entry: what matters in a diff is not
/// what the code says but which side of the change it is on, and that is a property of
/// the first character of the line.
pub(crate) fn diff(body: &str, out: &mut Vec<Span>) {
    let mut at = 0;
    for line in body.split_inclusive('\n') {
        // Trailing blanks are outside the span: a line's colour is a fill behind its
        // ink, and there is no ink in the newline.
        let end = at + line.trim_end().len();
        // The file headers first: `+++` and `---` start with the characters that mean
        // added and removed, and are neither.
        let tok = if line.starts_with("+++") || line.starts_with("---") {
            Some(Tok::Kind)
        } else if line.starts_with("@@") {
            Some(Tok::Name)
        } else if line.starts_with('+') {
            Some(Tok::Added)
        } else if line.starts_with('-') {
            Some(Tok::Removed)
        } else if line.starts_with("diff ") || line.starts_with("index ") {
            Some(Tok::Comment)
        } else {
            None
        };
        if let Some(tok) = tok {
            if end > at {
                out.push(Span { at: at..end, tok });
            }
        }
        at += line.len();
    }
}

/// XML and HTML, whose structure is in the angle brackets and not in a keyword list.
///
/// The tag is a [`Tok::Kind`] — it names a thing — the attributes are [`Tok::Name`] for
/// the same reason a JSON key is, and an entity is a [`Tok::Variable`] because that is
/// what `&amp;` is: a name standing for a character. Text between tags is left alone,
/// which is most of an HTML document and all of the part somebody is reading.
pub(crate) fn markup(body: &str, out: &mut Vec<Span>) {
    let b = body.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        if b[i] == b'&' {
            // `&amp;` — a name and a semicolon, close by. Anything else is just an
            // ampersand in the text, which HTML is full of.
            const MOST: usize = 12;
            let stop = (i + 1 + MOST).min(n);
            let mut at = i + 1;
            while at < stop && b[at] != b';' {
                at += 1;
            }
            if at > i + 1
                && at < stop
                && b[at] == b';'
                && b[i + 1..at].iter().all(|&c| is_word(c) || c == b'#')
            {
                out.push(Span {
                    at: i..at + 1,
                    tok: Tok::Variable,
                });
                i = at + 1;
                continue;
            }
        }
        if b[i] != b'<' {
            i = step(body, i);
            continue;
        }
        // The four things that open with a bracket and are not a tag.
        for (open, close, tok) in [
            ("<!--", "-->", Tok::Comment),
            ("<![CDATA[", "]]>", Tok::Str),
            ("<?", "?>", Tok::Kind),
            ("<!", ">", Tok::Kind),
        ] {
            if body[i..].starts_with(open) {
                let from = i + open.len();
                let end = body[from..]
                    .find(close)
                    .map_or(n, |at| from + at + close.len());
                out.push(Span { at: i..end, tok });
                i = end;
                break;
            }
        }
        if i >= n || b[i] != b'<' {
            continue;
        }

        // A tag: the name, then its attributes until the bracket closes.
        let name = i + 1 + usize::from(b.get(i + 1) == Some(&b'/'));
        let end = word_end_tag(b, name);
        if end == name {
            // `<` with nothing that can be a name after it: `a < b` in a script, or
            // text somebody did not escape.
            i = step(body, i);
            continue;
        }
        out.push(Span {
            at: i..end,
            tok: Tok::Kind,
        });
        i = end;
        while i < n && b[i] != b'>' {
            let c = b[i];
            if c == b'"' || c == b'\'' {
                let close = body[i + 1..]
                    .find(c as char)
                    .map_or(n, |at| i + 1 + at + 1)
                    .min(n);
                out.push(Span {
                    at: i..close,
                    tok: Tok::Str,
                });
                i = close;
                continue;
            }
            if starts_word(c) {
                let end = word_end_tag(b, i);
                out.push(Span {
                    at: i..end,
                    tok: Tok::Name,
                });
                i = end;
                continue;
            }
            i = step(body, i);
        }
    }
}

/// A tag or attribute name, which may hold the punctuation XML allows in one: `xsl:if`,
/// `xml-stylesheet`, `data-id`.
pub(crate) fn word_end_tag(b: &[u8], i: usize) -> usize {
    let mut at = i;
    while at < b.len() && (is_word(b[at]) || matches!(b[at], b'-' | b':' | b'.')) {
        at += 1;
    }
    at
}
