//! Colouring source: one pass over the text, one span per thing worth a colour.
//!
//! What comes out is a list of byte ranges tagged with a [`Tok`], which
//! [`crate::ui::preview`] turns into layout sections over the same galley the plain
//! view uses. So the find bar, the line-number gutter, wrapping and selection are all
//! untouched by this — colour is a *format* over the body, not a different way of
//! drawing it.
//!
//! # Not a parser, and the three places that shows
//!
//! There is no grammar here and there will not be one: a preview pane has to answer in
//! a millisecond over a file it has never seen, in whatever language, half of it
//! possibly truncated mid-expression. What it needs is the reading a person gets from
//! squinting — *that is a comment, that is a string, that is a word the language owns*
//! — and that reading is available from a lexer with a table per language.
//!
//! The three things that costs, all of them deliberate:
//!
//! - **A type is guessed from its capital.** `Rect` is a type because it starts
//!   uppercase and goes on in lowercase; `MAX_SIZE` is not, because it never comes
//!   back down. That is right nearly all the time in the languages it is switched on
//!   for and it costs nothing, where knowing for certain costs a symbol table.
//! - **A call is a word with a `(` welded to it.** `parse(` is a call, `parse (` is
//!   not. Every editor that has no language server does this.
//! - **The C family shares one keyword list.** C, C++, C#, Java and Kotlin are one
//!   [`Lang`], so `class` is a keyword in a `.c` file. The alternative is five tables
//!   that are 80% the same and a sixth bug when somebody edits one of them.
//!
//! # Punctuation is not a token
//!
//! `(`, `;`, `,` and the operators keep [`text.primary`](crate::theme::Syntax) and get
//! no span at all. Partly taste — dimming them is a scheme decision and this one does
//! not make it — but mostly arithmetic: punctuation is about a quarter of the tokens in
//! a source file, and every span becomes a layout section that egui hashes on every
//! frame it draws the galley. The same reasoning caps the whole thing at [`CAP`].
//!
//! # Speed
//!
//! `colour_speed` in the tests below is the measurement, over the largest file in this
//! repository. It is a single pass with no allocation but the output vector, and the
//! keyword lookup is a linear scan of an array of `&'static str` — 90 length checks per
//! word, which is faster than hashing the word would be.

use std::ops::Range;

/// What a run of characters is, for colour. See [`crate::theme::Theme::tok`].
///
/// Only things that *get* a colour are here. A plain identifier and a semicolon are
/// the absence of a token rather than a variant of one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    Comment,
    Str,
    Number,
    /// A word the language owns, `true` and `null` among them.
    Keyword,
    /// A name that names a thing: a type, an XML tag, an `[ini]` section.
    Kind,
    /// A name in the key position: a call, a JSON key, a CSS property, an attribute.
    Name,
    /// `$PATH`, `%TEMP%`, `${prefix}`, `&amp;`.
    Variable,
    /// A line a diff adds, and one it takes away.
    Added,
    Removed,
}

/// One coloured run of the body.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub at: Range<usize>,
    pub tok: Tok,
}

/// How much of a file is coloured.
///
/// 128 KB, which is around three thousand lines — past any source file somebody is
/// reading in a preview pane, and comfortably inside what the layout can carry. Over it
/// the body is drawn plain rather than coloured to the cap and abruptly stopping, which
/// is the important half of the decision: a file that goes monochrome two thirds of the
/// way down reads as a bug, where a large file that is simply not coloured reads as a
/// large file.
///
/// The number is about **layout sections, not this pass**. `colour_speed` measures both:
/// colouring 128 KB takes 6 ms over this repository's own source and 12 ms over dense
/// code, in a debug build, which would be survivable once. What is not survivable is that
/// every span becomes a section on a job egui hashes each time it draws the galley — and
/// dense code runs to 139 spans per kilobyte, so [`crate::preview::TEXT_CAP`]'s megabyte
/// would be around 140,000 of them. The same argument and the same shape as
/// [`crate::preview::search::HITS`], which caps the other layer for the same reason.
pub const CAP: usize = 128 << 10;

/// A language, as far as colour is concerned — a family and not a dialect.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    /// Coloured by nothing: an unknown extension, or one of the ones listed in
    /// [`lang_of`] as deliberately left plain.
    #[default]
    None,
    /// C, C++, C#, Java, Kotlin — one keyword list, see the module header.
    CLike,
    Rust,
    /// JavaScript and TypeScript.
    Web,
    Python,
    Go,
    Sql,
    Shell,
    PowerShell,
    Batch,
    /// XML, HTML, and the rest of the angle brackets. Its own tokeniser.
    Markup,
    Css,
    Json,
    Yaml,
    Toml,
    /// `.ini`, `.cfg`, `.conf`, `.properties`, `.env`.
    Ini,
    Cmake,
    /// A unified diff. Its own tokeniser, and the only one that colours whole lines.
    Diff,
    /// Recognised so that [`crate::ui::preview`] can render the document rather than
    /// the markup. [`spans`] returns nothing for it — see there.
    Markdown,
}

/// Which language a file is in, from its name.
///
/// `stem` is the name with the extension off, exactly as
/// [`crate::preview::is_code`] takes it and for the same reason: `CMakeLists.txt` is a
/// build script and its extension says otherwise.
///
/// # What is deliberately not here
///
/// `.rb`, `.pl`, `.php`, `.lua` — all of them in [`crate::preview::CODE`], all of them
/// shown plain. Each needs its own comment marker and its own keyword list (Lua's
/// comment is `--`, Perl's variables are sigils three ways, PHP is two languages in one
/// file), and a language coloured with a nearly-right table is worse than one coloured
/// with none: a wrong keyword is a claim about the code. Adding one is a `Rules` entry
/// and a row in the table below.
pub fn lang_of(stem: &str, ext: &str) -> Lang {
    // The name beats the extension, exactly as in `preview::is_code`.
    if stem.eq_ignore_ascii_case("cmakelists") {
        return Lang::Cmake;
    }
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "cs" | "java" | "kt" => Lang::CLike,
        "rs" => Lang::Rust,
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => Lang::Web,
        "py" => Lang::Python,
        "go" => Lang::Go,
        "sql" => Lang::Sql,
        "sh" | "bash" | "zsh" => Lang::Shell,
        "ps1" | "psm1" => Lang::PowerShell,
        "bat" | "cmd" => Lang::Batch,
        "xml" | "xsd" | "xsl" | "html" | "htm" | "svg" => Lang::Markup,
        "css" | "scss" | "less" => Lang::Css,
        "json" | "jsonc" => Lang::Json,
        "yaml" | "yml" => Lang::Yaml,
        "toml" => Lang::Toml,
        "ini" | "cfg" | "conf" | "properties" | "env" | "editorconfig" | "gitconfig" => Lang::Ini,
        "cmake" => Lang::Cmake,
        "diff" | "patch" => Lang::Diff,
        "md" | "markdown" => Lang::Markdown,
        _ => Lang::None,
    }
}

/// Which language a fenced code block claims, from the word after its ```` ``` ````.
///
/// **Not the same table as [`lang_of`]**, and the difference is not cosmetic: a fence is labelled
/// with a language's *name* where a file is labelled with its extension, and the two agree almost
/// nowhere — ```` ```rust ```` against `.rs`, ```` ```bash ```` against `.sh`,
/// ```` ```javascript ```` against `.js`. Reading a fence with the extension table gets `json`,
/// `toml` and `go` right and everything else wrong, which is worse than it sounds: it is exactly the
/// languages people label their examples with.
///
/// The extension table is still tried second, so ```` ```rs ```` and ```` ```py ```` also answer.
pub fn lang_of_tag(tag: &str) -> Lang {
    let tag = tag.to_ascii_lowercase();
    let named = match tag.as_str() {
        "rust" => Lang::Rust,
        "c++" | "cpp" | "csharp" | "objc" | "kotlin" => Lang::CLike,
        "javascript" | "typescript" | "node" | "json5" => Lang::Web,
        "python" | "python3" => Lang::Python,
        "golang" => Lang::Go,
        "mysql" | "postgres" | "postgresql" | "psql" | "sqlite" => Lang::Sql,
        "shell" | "console" | "terminal" | "shell-session" => Lang::Shell,
        "powershell" | "pwsh" | "ps" => Lang::PowerShell,
        "batch" | "dos" => Lang::Batch,
        "xhtml" | "vue" | "svelte" => Lang::Markup,
        "sass" => Lang::Css,
        "dosini" => Lang::Ini,
        "udiff" => Lang::Diff,
        // `text` and `plain` are what a fence says when it means "not a language", and saying so
        // is different from saying nothing: neither should fall through to the extension table,
        // where `conf` and `env` would take them.
        "text" | "plain" | "plaintext" | "none" | "output" | "log" => Lang::None,
        _ => Lang::None,
    };
    if named != Lang::None {
        return named;
    }
    // `rs`, `py`, `js`, `json`, `toml`, `yaml`, `sh`, `go`, `sql`, `css`, `html`, `cmake`, `diff` —
    // every tag that happens to be spelled like an extension.
    lang_of("", &tag)
}

/// Every coloured run in `body`, in the order they occur and never overlapping.
///
/// Empty for [`Lang::None`], for [`Lang::Markdown`] — whose colouring is the document
/// renderer's job, not a format over the markup — and for a body over [`CAP`].
pub fn spans(body: &str, lang: Lang) -> Vec<Span> {
    let mut out = Vec::new();
    if body.len() > CAP {
        return out;
    }
    match lang {
        Lang::None | Lang::Markdown => {}
        Lang::Diff => diff(body, &mut out),
        Lang::Markup => markup(body, &mut out),
        other => code(body, rules(other), &mut out),
    }
    out
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The generic tokeniser
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The two tokenisers that are not the generic one
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

mod langs;
mod lexer;
mod markup;

// One lexer, one table per language, and two languages that are not code. The glob is what says
// these were one module: a rule table and the pass that reads it are not independent.
pub(crate) use langs::*;
pub(crate) use lexer::*;
pub(crate) use markup::*;
