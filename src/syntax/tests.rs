use super::*;

/// What each token covers, as `(text, kind)` — which is what a test about colour actually
/// wants to say.
fn toks(body: &str, lang: Lang) -> Vec<(&str, Tok)> {
    spans(body, lang)
        .into_iter()
        .map(|span| (&body[span.at], span.tok))
        .collect()
}

/// Everything with a colour in one line of C++, in order.
#[test]
fn a_line_of_code_comes_apart_into_its_pieces() {
    let body = "int n = parse(\"12\", 0x1f); // and why\n";
    assert_eq!(
        toks(body, Lang::CLike),
        [
            ("int", Tok::Kind),
            ("parse", Tok::Name),
            ("\"12\"", Tok::Str),
            ("0x1f", Tok::Number),
            ("// and why", Tok::Comment),
        ]
    );
    // Punctuation and plain identifiers get nothing at all, which is the module's own rule and
    // the reason a source file is not one span per character.
    assert!(
        !toks(body, Lang::CLike)
            .iter()
            .any(|(text, _)| *text == "n" || *text == "="),
        "punctuation and identifiers are being given spans"
    );
}

/// A Rust lifetime is not the start of a string.
///
/// The bug this is here for is worth stating: a `'` treated as a plain delimiter runs to the
/// next one, so `impl<'a, 'b>` colours `'a, '` as a character literal and then the rest of the
/// file is inside-out. The rule is that a letter literal is *one* character or one escape and
/// nothing else — see [`Quote::Letter`].
#[test]
fn a_lifetime_is_not_a_character_literal() {
    // Two lifetimes four bytes apart, which is exactly the distance a plain delimiter would
    // pair them across — and one real literal on the same line, so the test cannot pass by the
    // rule refusing everything.
    let body = "impl<'a, 'b> Trait for &'a str { const NL: char = '\\n'; }";
    let found = toks(body, Lang::Rust);
    assert_eq!(
        found
            .iter()
            .filter(|(_, tok)| *tok == Tok::Str)
            .map(|(text, _)| *text)
            .collect::<Vec<_>>(),
        ["'\\n'"],
        "a lifetime was read as a literal: {found:?}"
    );
    // And the real literals on the same line still are ones.
    assert_eq!(
        toks(
            "let a = 'x'; let b = '\\n'; let c = '\\u{1F600}';",
            Lang::Rust
        )
        .into_iter()
        .filter(|(_, tok)| *tok == Tok::Str)
        .map(|(text, _)| text)
        .collect::<Vec<_>>(),
        ["'x'", "'\\n'", "'\\u{1F600}'"]
    );
}

/// An unclosed string stops at the end of its line, and an unclosed comment does not.
///
/// Both matter for the same reason and it is not a hypothetical: this is a *preview*, and the
/// body it is handed is the first megabyte of a file. Half of it is routinely mid-expression.
#[test]
fn an_unterminated_string_does_not_swallow_the_file() {
    let body = "say(\"oh no\nint after = 1;\n";
    let found = toks(body, Lang::CLike);
    assert_eq!(found[1], ("\"oh no", Tok::Str), "{found:?}");
    // The line after it is code again, which is the whole point.
    assert!(found.contains(&("int", Tok::Kind)), "{found:?}");

    // A block comment is the other way round: it really does run on, and a file cut in the
    // middle of one has no closing mark to find.
    assert_eq!(
        toks("/* still going\nand going", Lang::CLike),
        [("/* still going\nand going", Tok::Comment)]
    );
}

/// A type is guessed from its capital, and a constant is not a type.
#[test]
fn a_capital_that_comes_back_down_is_a_type() {
    let found = toks("let r: Rect = MAX_SIZE.into();", Lang::Rust);
    assert!(found.contains(&("Rect", Tok::Kind)), "{found:?}");
    assert!(
        !found.iter().any(|(text, _)| *text == "MAX_SIZE"),
        "an all-capitals constant was coloured as a type: {found:?}"
    );
}

/// The languages whose structure is a key rather than a keyword.
#[test]
fn a_key_is_coloured_wherever_the_language_has_keys() {
    // JSON's keys are strings, so the same characters are a key in one position and a value in
    // the other — which is the case a word-only rule would miss.
    assert_eq!(
        toks("{\"name\": \"name\", \"on\": true}", Lang::Json),
        [
            ("\"name\"", Tok::Name),
            ("\"name\"", Tok::Str),
            ("\"on\"", Tok::Name),
            ("true", Tok::Keyword),
        ]
    );
    assert_eq!(
        toks("[build]\ntarget = \"x\"  # why\n", Lang::Toml),
        [
            ("[build]", Tok::Kind),
            ("target", Tok::Name),
            ("\"x\"", Tok::Str),
            ("# why", Tok::Comment),
        ]
    );
    assert_eq!(
        toks("a:\n  b: 2\n", Lang::Yaml),
        [("a", Tok::Name), ("b", Tok::Name), ("2", Tok::Number)]
    );
    assert_eq!(
        toks(".card { color: red; }", Lang::Css),
        [("color", Tok::Name)]
    );
}

/// A sigil variable, in the three shapes the shells write one.
#[test]
fn a_variable_keeps_its_sigil() {
    assert_eq!(
        toks("echo \"$HOME/${sub}/$1\" $?", Lang::Shell),
        [
            ("echo", Tok::Keyword),
            ("\"$HOME/${sub}/$1\"", Tok::Str),
            ("$?", Tok::Variable),
        ],
        "a variable inside a string belongs to the string"
    );
    assert_eq!(
        toks("$path = $env:TEMP", Lang::PowerShell),
        [("$path", Tok::Variable), ("$env", Tok::Variable)]
    );
    assert_eq!(
        toks("copy %TEMP%\\%~n1 x\nrem gone\n", Lang::Batch),
        [
            ("copy", Tok::Keyword),
            ("%TEMP%", Tok::Variable),
            ("%~n1", Tok::Variable),
            ("rem gone", Tok::Comment),
        ]
    );
    // `rem` away from the head of a line is an argument and not a comment.
    assert_eq!(toks("echo rem gone", Lang::Batch), [("echo", Tok::Keyword)]);
}

/// Keywords that do not care about case, and the ones that do.
#[test]
fn case_matters_where_the_language_says_it_does() {
    assert!(toks("SELECT * FROM t", Lang::Sql).contains(&("SELECT", Tok::Keyword)));
    assert!(toks("select * from t", Lang::Sql).contains(&("select", Tok::Keyword)));
    // Rust is not SQL: `If` is a name somebody chose.
    assert!(!toks("If x", Lang::Rust).contains(&("If", Tok::Keyword)));
}

/// A diff is coloured by the line, and its headers are not additions.
#[test]
fn a_diff_is_coloured_a_line_at_a_time() {
    let body =
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n context\n-gone\n+here\n";
    assert_eq!(
        toks(body, Lang::Diff),
        [
            ("diff --git a/x b/x", Tok::Comment),
            ("--- a/x", Tok::Kind),
            ("+++ b/x", Tok::Kind),
            ("@@ -1,2 +1,2 @@", Tok::Name),
            ("-gone", Tok::Removed),
            ("+here", Tok::Added),
        ],
        "the `---` and `+++` headers are being read as a removal and an addition"
    );
}

/// Markup: the tag names a thing, the attributes are keys, and the text is left alone.
#[test]
fn a_tag_is_a_name_and_its_text_is_not() {
    assert_eq!(
        toks(
            "<!-- hi --><a href=\"x\" data-id='2'>Text &amp; more</a>",
            Lang::Markup
        ),
        [
            ("<!-- hi -->", Tok::Comment),
            ("<a", Tok::Kind),
            ("href", Tok::Name),
            ("\"x\"", Tok::Str),
            ("data-id", Tok::Name),
            ("'2'", Tok::Str),
            ("&amp;", Tok::Variable),
            ("</a", Tok::Kind),
        ]
    );
    // A `<` that is not a tag is arithmetic, and must not eat the line.
    assert!(toks("if (a < b) {}", Lang::Markup).is_empty());
}

/// Which language a name lands in, including the one the extension gets wrong.
#[test]
fn the_language_comes_from_the_name() {
    for (stem, ext, want) in [
        ("main", "rs", Lang::Rust),
        ("main", "RS", Lang::Rust),
        ("app", "tsx", Lang::Web),
        ("CMakeLists", "txt", Lang::Cmake),
        ("cmakelists", "TXT", Lang::Cmake),
        ("toolchain", "cmake", Lang::Cmake),
        ("readme", "md", Lang::Markdown),
        ("notes", "txt", Lang::None),
        // The four in `preview::CODE` that are deliberately left plain.
        ("app", "rb", Lang::None),
        ("app", "pl", Lang::None),
        ("index", "php", Lang::None),
        ("init", "lua", Lang::None),
    ] {
        assert_eq!(lang_of(stem, ext), want, "{stem}.{ext}");
    }
}

/// **And from the body, where the name said nothing** — XML and JSON, and the several things that
/// merely start with the same character.
///
/// The negatives are the half worth having. Each one is a real format that opens with `<`, `{` or
/// `[`, and each one would have been claimed by a probe that looked only at the first character:
/// RTF, awk, a desktop entry, an ini section, a PHP file, and C arithmetic inside markup's own
/// tokeniser test. See [`lang_of_body`], which is why every test in it is two characters and not one.
#[test]
fn the_language_comes_from_the_body_when_the_name_says_nothing() {
    for (body, want) in [
        // XML, five ways in.
        ("<?xml version=\"1.0\"?>\n<a/>", Lang::Markup),
        ("<?XML version=\"1.0\"?>", Lang::Markup),
        ("<!DOCTYPE html>\n<html>", Lang::Markup),
        ("<!-- a comment first -->\n<a/>", Lang::Markup),
        ("<configuration>\n  <appSettings/>\n</configuration>", Lang::Markup),
        // A body that begins mid-document, which is what a file cut out of a larger one looks like.
        ("</item>\n</items>", Lang::Markup),
        // Leading blank lines, and a BOM in front of everything — which is not whitespace, and is
        // what any XML written by a Windows tool has.
        ("\n\n  <root/>", Lang::Markup),
        ("\u{feff}<?xml version=\"1.0\"?>", Lang::Markup),
        ("\u{feff}{\"a\": 1}", Lang::Json),
        // JSON: an object keyed by a string, an empty one, and arrays of each kind of value.
        ("{\"a\": 1}", Lang::Json),
        ("{\n  \"name\": \"thing\"\n}", Lang::Json),
        ("{ }", Lang::Json),
        ("[\"a\", \"b\"]", Lang::Json),
        ("[{\"a\": 1}]", Lang::Json),
        ("[[1], [2]]", Lang::Json),
        ("[]", Lang::Json),
        ("[1, 2, 3]", Lang::Json),
        ("[-1.5e3]", Lang::Json),
        ("[true, false, null]", Lang::Json),
        // **And everything that merely starts with one of those three characters.**
        ("{\\rtf1\\ansi\\deff0 A document.}", Lang::None),
        ("{ print $1 }", Lang::None),
        ("[Desktop Entry]\nName=A thing", Lang::None),
        ("[package]\nname = \"thing\"", Lang::None),
        // PHP is plain by decision, so the one processing instruction that counts is `<?xml`.
        ("<?php echo 'hello'; ?>", Lang::None),
        ("<?xm", Lang::None),
        // A `<` with arithmetic after it, which is the case the markup tokeniser is already careful
        // about — and which must not get the whole file coloured as markup to begin with.
        ("if (a < b) { return 0; }", Lang::None),
        // Python's dict repr, which is a `{` and a quote of the wrong kind.
        ("{'a': 1}", Lang::None),
        // Prose, an empty file, and one holding only blanks.
        ("Just some notes about the thing.", Lang::None),
        ("", Lang::None),
        ("   \n\n  ", Lang::None),
        // A multi-byte character right behind the opening one: the answer is no, and the point is
        // that asking does not panic on the character boundary.
        ("<élan", Lang::None),
        ("{é: 1}", Lang::None),
    ] {
        assert_eq!(lang_of_body(body), want, "{body:?}");
    }
}

/// Nothing is coloured past the cap, and nothing is coloured for a language with no table.
#[test]
fn a_large_file_is_shown_plain_rather_than_half_coloured() {
    let one = "let x = 1; // a line of Rust\n";
    let big = one.repeat(CAP / one.len() + 2);
    assert!(big.len() > CAP);
    assert!(
        spans(&big, Lang::Rust).is_empty(),
        "a file over the cap was coloured"
    );
    // And just under it is coloured, or the cap would be the whole behaviour.
    let small = one.repeat(CAP / one.len() / 2);
    assert!(!spans(&small, Lang::Rust).is_empty());
    // Markdown is a document rather than a format over its markup — see `ui::preview`.
    assert!(spans(small.as_str(), Lang::Markdown).is_empty());
    assert!(spans(small.as_str(), Lang::None).is_empty());
}

/// The spans come back in order, never overlapping, and always on character boundaries.
///
/// Three invariants the drawing side *depends* on and cannot check: `overlay` walks the spans
/// and the find's hits together assuming both are sorted, and a byte range that splits a
/// character is a panic inside epaint rather than a wrong colour.
#[test]
fn every_span_is_ordered_and_lands_on_a_character() {
    // One body per tokeniser, each holding the things most likely to go wrong: accents inside
    // identifiers and strings, an unclosed construct, and a marker at the very end.
    let cases = [
        (Lang::Rust, "fn é() { let s = \"café ☕\"; } // ünicode"),
        (Lang::CLike, "#include <x>\nchar* café = \"ünï\";\n/*"),
        (
            Lang::Python,
            "def f():\n    \"\"\"dôc\n    still\"\"\"\n    return 'é'",
        ),
        (Lang::Markup, "<p class=\"é\">tëxt &amp; ☕</p><!--"),
        (Lang::Diff, "+añadido\n-quitado\n context é\n"),
        (Lang::Shell, "echo \"$HOME/café\" # ünicode"),
        (Lang::Yaml, "clé: valeur\n# ünicode\n"),
    ];
    for (lang, body) in cases {
        let found = spans(body, lang);
        let mut end = 0;
        for span in &found {
            assert!(
                span.at.start >= end,
                "{lang:?}: {span:?} overlaps or precedes what came before"
            );
            assert!(span.at.start < span.at.end, "{lang:?}: {span:?} is empty");
            assert!(
                body.is_char_boundary(span.at.start) && body.is_char_boundary(span.at.end),
                "{lang:?}: {span:?} cuts a character in half"
            );
            end = span.at.end;
        }
        assert!(end <= body.len(), "{lang:?}: a span past the end");
        assert!(!found.is_empty(), "{lang:?}: nothing was coloured at all");
    }
}

/// How long colouring takes at the cap, and — the number that actually matters — how many
/// spans it produces.
///
/// **Two fixtures, because they answer different questions.** This repository's own source is
/// what a real file looks like and it is unusually comment-heavy, so it produces few spans; the
/// synthetic body is code with a token every few characters, which is the upper bound and the
/// one [`CAP`] is sized from. The time is a debug build's, which is the only build this suite
/// runs in.
#[test]
fn colour_speed() {
    let mut real = String::new();
    for name in [
        "app/mod.rs",
        "app/frame.rs",
        "app/perform.rs",
        "app/menu.rs",
        "ui/preview/mod.rs",
        "ui/preview/text.rs",
        "ui/preview/header.rs",
        "ui/filelist/rows.rs",
        "ui/console/mod.rs",
        "syntax/mod.rs",
        "syntax/langs.rs",
        "pe/mod.rs",
    ] {
        if let Ok(text) = std::fs::read_to_string(format!("src/{name}")) {
            real.push_str(&text);
        }
    }
    assert!(real.len() > 200_000, "the fixture is {} bytes", real.len());
    let dense = "    let name: Vec<u32> = parse(\"text\", 0x1f).unwrap_or(3); // why\n"
        .repeat(CAP / 64 + 2);

    for (what, mut body) in [("this repository", real), ("dense code", dense)] {
        // Trimmed to the cap, because that is the most this will ever be asked to do.
        body.truncate(CAP);
        while !body.is_char_boundary(body.len()) {
            body.pop();
        }
        let rounds = 20;
        let start = std::time::Instant::now();
        let mut total = 0;
        for _ in 0..rounds {
            total += spans(&body, Lang::Rust).len();
        }
        let each = start.elapsed().as_secs_f64() / rounds as f64;
        let spans = total / rounds;
        println!(
            "{what}: {} KB in {:.2} ms, {spans} spans ({} per KB)",
            body.len() / 1024,
            each * 1000.0,
            spans / (body.len() / 1024),
        );
        assert!(
            each < 0.060,
            "{what}: colouring the cap took {:.1} ms",
            each * 1000.0
        );
        // The ceiling the cap was chosen for. A megabyte of this would be eight times as many,
        // which is the number in `CAP`'s own note.
        assert!(
            spans < 40_000,
            "{what}: {spans} spans at the cap, which is more layout sections than the note in \
             `CAP` allows for"
        );
    }
}
