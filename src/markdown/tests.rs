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
