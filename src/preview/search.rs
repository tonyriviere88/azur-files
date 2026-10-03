//! Finding a string in the text preview: the query, the hits, and the walk that collects them.
//!
//! On the UI thread, on every keystroke — see [`hits`] for what that rules out.

/// What to look for in the text on show, and how.
///
/// The three flags are the three buttons inside the field, in the order every editor's find bar
/// puts them. They **compose**: whole-word and regex together is `\b(?:pattern)\b`, which is what
/// the literal path applies by hand so that the flag means one thing rather than two.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Search {
    pub text: String,
    /// `Aa`: `Foo` no longer finds `foo`.
    pub case: bool,
    /// `ab`: `word` no longer finds `wording`.
    pub word: bool,
    /// `.*`: the text is a pattern rather than the characters typed.
    pub regex: bool,
}

/// How many hits are collected before the search stops looking.
///
/// A budget, like [`crate::fs::scan::FLATTEN_BUDGET`] and [`crate::pe::BUDGET`], and for the same
/// reason: the number is unbounded and the work per hit is not free. Every hit becomes two sections
/// of the layout job that draws the body — see `ui::preview::text_canvas` — and one letter typed
/// into the field over a megabyte of source is on the order of 80,000 of them, which is 160,000
/// sections egui would lay out to draw one screenful.
///
/// It is reached often, because the first keystroke of any search reaches it. So the counter says
/// `4096+` rather than pretending, exactly as VS Code says `more than 19999 results`.
pub const HITS: usize = 4096;

/// How large a pattern is allowed to compile to.
///
/// Legal patterns can be enormous — `a{1000}{1000}` is nine characters — and the engine builds the
/// program before it knows whether anybody wanted it. `regex` cannot be made to *hang* (there is no
/// backtracking in it, which is why a pattern somebody is still typing can be run on the UI thread
/// at all), but it can be made to allocate, and the answer to that is a ceiling rather than a
/// timeout. A megabyte is far above any pattern typed into a find bar and far below trouble.
const PATTERN_LIMIT: usize = 1 << 20;

/// Where a search matched.
#[derive(Default)]
pub struct Hits {
    /// Byte ranges into the body, in the order they occur. Never overlapping, never empty.
    pub at: Vec<std::ops::Range<usize>>,
    /// Stopped at [`HITS`], so `at.len()` is a floor and not a count.
    pub capped: bool,
    /// The pattern is not a pattern. Only reachable with [`Search::regex`] on.
    pub bad: bool,
}

/// Find every occurrence of `search` in `body`.
pub fn hits(body: &str, search: &Search) -> Hits {
    let mut found = Hits::default();
    if search.text.is_empty() {
        return found;
    }
    if !search.regex {
        literal(body, search, &mut found);
        return found;
    }

    let pattern = if search.word {
        format!(r"\b(?:{})\b", search.text)
    } else {
        search.text.clone()
    };
    // **Unicode mode stays on, and it is the `unicode-case` and `unicode-perl` features in
    // `Cargo.toml` that pay for it.** Turning it off would drop a quarter of a megabyte of Unicode
    // tables from the executable, and it was measured and rejected: `.unicode(false)` makes `.`
    // match a *byte* rather than a character, which can split a UTF-8 sequence, so `regex` refuses
    // to build the pattern at all rather than hand back a `Regex` that could slice a string in
    // half. A find bar where `.` is an error is not a find bar. Leaving the mode on and dropping
    // the features instead only moves the failure: `\d`, `\w`, `\s` and `\b` stop compiling, and
    // the whole-word toggle above wraps every pattern in `\b`.
    let Ok(re) = regex::RegexBuilder::new(&pattern)
        .case_insensitive(!search.case)
        .size_limit(PATTERN_LIMIT)
        .build()
    else {
        found.bad = true;
        return found;
    };
    for m in re.find_iter(body) {
        // **An empty match is not a hit.** `a*` matches at every position in the file and none of
        // the characters of it: a megabyte of them, not one of which can be highlighted, counted
        // usefully or stepped to. Skipped rather than refused, so a pattern that matches emptily
        // *and* substantially — `\d*` — still finds the digits.
        if m.is_empty() {
            continue;
        }
        if found.at.len() == HITS {
            found.capped = true;
            break;
        }
        found.at.push(m.start()..m.end());
    }
    found
}

/// Every occurrence of the text as itself.
///
/// Not `regex::escape` and back through the engine, which is what it would take to have one path:
/// this is the search that runs on every keystroke of every find, and without the `perf` feature
/// group the engine walks a megabyte an order of magnitude slower than a folded character compare
/// does. It is also the path where the answer has to be exactly the characters typed.
///
/// Folded per character, and **not by lowercasing the body**: `char::to_lowercase` can change how
/// many bytes a character takes, so an offset into a lowered copy is not an offset into the file —
/// and every offset here ends up as a highlight range over the real one.
fn literal(body: &str, search: &Search, found: &mut Hits) {
    let needle: Vec<char> = search.text.chars().map(|c| fold(c, search.case)).collect();
    // Where the next match may start: the end of the last one, so `aa` finds one hit in `aaa` and
    // not two overlapping ones. A find bar steps through matches, and two that share a character
    // are not two places to go.
    let mut next = 0;
    for (at, _) in body.char_indices() {
        if at < next {
            continue;
        }
        let Some(end) = ends_at(body, at, &needle, search.case) else {
            continue;
        };
        if search.word && !whole_word(body, at, end) {
            continue;
        }
        if found.at.len() == HITS {
            found.capped = true;
            return;
        }
        found.at.push(at..end);
        next = end;
    }
}

/// Where the needle ends if it starts at `at`, or `None` if it does not.
fn ends_at(body: &str, at: usize, needle: &[char], case: bool) -> Option<usize> {
    let mut chars = body[at..].char_indices();
    let mut end = at;
    for &want in needle {
        let (offset, c) = chars.next()?;
        if fold(c, case) != want {
            return None;
        }
        end = at + offset + c.len_utf8();
    }
    Some(end)
}

/// One character as it is compared.
///
/// The first character of the Unicode lowercase, which is all of it for every letter that lowercases
/// to one character — the same rule and the same two exceptions as
/// [`azur_egui_theme::filter`](azur_egui_theme::filter). Keeping the count the same is what lets
/// [`ends_at`] walk the two in step and hand back a byte offset into the body.
fn fold(c: char, case: bool) -> char {
    if case {
        c
    } else if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// Whether `at..end` has a word boundary at each end.
///
/// `\b`'s own rule — a boundary is where *word-ness changes* — rather than the tempting "the
/// characters either side are not word characters". The two differ whenever the needle itself
/// begins or ends with punctuation: `\b-x\b` matches the `-x` in `a-x`, and the tempting rule
/// refuses it because `a` is a word character. Since the regex path gets `\b` from the engine, the
/// literal path has to mean the same thing by it or the toggle would do two different jobs.
fn whole_word(body: &str, at: usize, end: usize) -> bool {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let before = body[..at].chars().next_back();
    let first = body[at..].chars().next();
    let last = body[..end].chars().next_back();
    let after = body[end..].chars().next();
    word(before) != word(first) && word(last) != word(after)
}
