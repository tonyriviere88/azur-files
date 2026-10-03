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
/// [`crate::preview::HITS`], which caps the other layer for the same reason.
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

/// How a string is delimited.
#[derive(Clone, Copy)]
enum Quote {
    /// Runs to the matching delimiter — or, unless `multiline`, to the end of the
    /// line, so one stray quote does not paint the rest of the file.
    Run {
        delim: u8,
        escape: bool,
        multiline: bool,
    },
    /// A `'c'` character literal: one character or one escape, and nothing else.
    ///
    /// **This is why `'` is not simply a delimiter in Rust and C.** A lifetime is an
    /// unclosed quote — `impl<'a, 'b>` has two — and a delimiter that runs to the next
    /// `'` would colour `'a, '` as a string and then invert the rest of the line.
    Letter,
}

/// Everything the generic tokeniser needs to know about one language.
struct Rules {
    /// Comment openers that run to the end of the line.
    line: &'static [&'static str],
    /// Comment pairs. Not nested, even in Rust, where they can be.
    block: &'static [(&'static str, &'static str)],
    quotes: &'static [Quote],
    /// `"""` and `'''` run across lines. Python only.
    triple: bool,
    keywords: &'static [&'static str],
    /// Words that name a type. The heuristic in `capitals` catches the rest.
    types: &'static [&'static str],
    /// Keywords compare without regard to case: SQL, batch, PowerShell, CMake.
    fold: bool,
    /// A word with `(` immediately after it is a call.
    calls: bool,
    /// A word that starts uppercase and comes back down is a type.
    capitals: bool,
    /// A word or string followed by one of these is a key.
    label: &'static [u8],
    /// Characters that start a variable.
    sigils: &'static [u8],
    /// A `#word` at the head of a line is a preprocessor directive.
    hash: bool,
    /// A `[bracketed]` line names a section.
    sections: bool,
}

/// The defaults every entry starts from: nothing.
const BARE: Rules = Rules {
    line: &[],
    block: &[],
    quotes: &[],
    triple: false,
    keywords: &[],
    types: &[],
    fold: false,
    calls: false,
    capitals: false,
    label: &[],
    sigils: &[],
    hash: false,
    sections: false,
};

/// A double-quoted string with backslash escapes, which is nearly every language's.
const DOUBLE: Quote = Quote::Run {
    delim: b'"',
    escape: true,
    multiline: false,
};
/// And a single-quoted one, for the languages where that is a string and not a letter.
const SINGLE: Quote = Quote::Run {
    delim: b'\'',
    escape: true,
    multiline: false,
};

fn rules(lang: Lang) -> &'static Rules {
    match lang {
        Lang::CLike => &C_LIKE,
        Lang::Rust => &RUST,
        Lang::Web => &WEB,
        Lang::Python => &PYTHON,
        Lang::Go => &GO,
        Lang::Sql => &SQL,
        Lang::Shell => &SHELL,
        Lang::PowerShell => &POWERSHELL,
        Lang::Batch => &BATCH,
        Lang::Css => &CSS,
        Lang::Json => &JSON,
        Lang::Yaml => &YAML,
        Lang::Toml => &TOML,
        Lang::Ini => &INI,
        Lang::Cmake => &CMAKE,
        // `spans` routes these away before this is reached.
        Lang::None | Lang::Markup | Lang::Diff | Lang::Markdown => &BARE,
    }
}

static C_LIKE: Rules = Rules {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: &[DOUBLE, Quote::Letter],
    keywords: &[
        "alignas",
        "alignof",
        "and",
        "asm",
        "auto",
        "base",
        "bool",
        "break",
        "case",
        "catch",
        "class",
        "concept",
        "const",
        "const_cast",
        "consteval",
        "constexpr",
        "constinit",
        "continue",
        "co_await",
        "co_return",
        "co_yield",
        "decltype",
        "default",
        "delegate",
        "delete",
        "do",
        "dynamic_cast",
        "else",
        "enum",
        "event",
        "explicit",
        "export",
        "extends",
        "extern",
        "false",
        "final",
        "finally",
        "fixed",
        "for",
        "foreach",
        "friend",
        "get",
        "goto",
        "if",
        "implements",
        "implicit",
        "import",
        "in",
        "inline",
        "instanceof",
        "interface",
        "internal",
        "is",
        "lock",
        "mutable",
        "namespace",
        "native",
        "new",
        "noexcept",
        "not",
        "null",
        "nullptr",
        "object",
        "operator",
        "or",
        "out",
        "override",
        "package",
        "params",
        "private",
        "protected",
        "public",
        "readonly",
        "record",
        "register",
        "reinterpret_cast",
        "requires",
        "return",
        "sealed",
        "set",
        "sizeof",
        "stackalloc",
        "static",
        "static_assert",
        "static_cast",
        "struct",
        "super",
        "switch",
        "synchronized",
        "template",
        "this",
        "throw",
        "throws",
        "transient",
        "true",
        "try",
        "typedef",
        "typeid",
        "typename",
        "union",
        "unsafe",
        "using",
        "val",
        "var",
        "virtual",
        "volatile",
        "when",
        "where",
        "while",
        "xor",
        "yield",
    ],
    types: &[
        "char",
        "char16_t",
        "char32_t",
        "char8_t",
        "double",
        "float",
        "int",
        "int16_t",
        "int32_t",
        "int64_t",
        "int8_t",
        "long",
        "ptrdiff_t",
        "short",
        "signed",
        "size_t",
        "ssize_t",
        "string",
        "uint",
        "uint16_t",
        "uint32_t",
        "uint64_t",
        "uint8_t",
        "unsigned",
        "void",
        "wchar_t",
    ],
    calls: true,
    capitals: true,
    hash: true,
    ..BARE
};

static RUST: Rules = Rules {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: &[DOUBLE, Quote::Letter],
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true",
        "type", "union", "unsafe", "use", "where", "while", "yield",
    ],
    types: &[
        "bool", "char", "f32", "f64", "i128", "i16", "i32", "i64", "i8", "isize", "str", "u128",
        "u16", "u32", "u64", "u8", "usize",
    ],
    calls: true,
    capitals: true,
    ..BARE
};

static WEB: Rules = Rules {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: &[
        DOUBLE,
        SINGLE,
        // A template literal, which is the one string here that runs across lines.
        Quote::Run {
            delim: b'`',
            escape: true,
            multiline: true,
        },
    ],
    keywords: &[
        "abstract",
        "any",
        "as",
        "async",
        "await",
        "boolean",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "constructor",
        "continue",
        "debugger",
        "declare",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "get",
        "if",
        "implements",
        "import",
        "in",
        "infer",
        "instanceof",
        "interface",
        "is",
        "keyof",
        "let",
        "namespace",
        "never",
        "new",
        "null",
        "of",
        "private",
        "protected",
        "public",
        "readonly",
        "return",
        "satisfies",
        "set",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "unknown",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ],
    types: &["bigint", "number", "object", "string", "symbol"],
    calls: true,
    capitals: true,
    ..BARE
};

static PYTHON: Rules = Rules {
    line: &["#"],
    quotes: &[DOUBLE, SINGLE],
    triple: true,
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
        "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True",
        "try", "while", "with", "yield",
    ],
    types: &[
        "bool",
        "bytes",
        "dict",
        "float",
        "frozenset",
        "int",
        "list",
        "set",
        "str",
        "tuple",
    ],
    calls: true,
    capitals: true,
    ..BARE
};

static GO: Rules = Rules {
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: &[
        DOUBLE,
        Quote::Letter,
        // A raw string, and it does run across lines.
        Quote::Run {
            delim: b'`',
            escape: false,
            multiline: true,
        },
    ],
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "false",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "iota",
        "map",
        "nil",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "true",
        "type",
        "var",
    ],
    types: &[
        "bool",
        "byte",
        "complex128",
        "complex64",
        "error",
        "float32",
        "float64",
        "int",
        "int16",
        "int32",
        "int64",
        "int8",
        "rune",
        "string",
        "uint",
        "uint16",
        "uint32",
        "uint64",
        "uint8",
        "uintptr",
    ],
    calls: true,
    capitals: true,
    ..BARE
};

static SQL: Rules = Rules {
    line: &["--", "#"],
    block: &[("/*", "*/")],
    // A `'literal'`, and `"an identifier"`. Neither takes a backslash escape in
    // standard SQL — a quote is doubled — so a `\` at the end of one does not swallow
    // the delimiter.
    quotes: &[
        Quote::Run {
            delim: b'\'',
            escape: false,
            multiline: false,
        },
        Quote::Run {
            delim: b'"',
            escape: false,
            multiline: false,
        },
    ],
    keywords: &[
        "add",
        "all",
        "alter",
        "and",
        "any",
        "as",
        "asc",
        "begin",
        "between",
        "by",
        "case",
        "cast",
        "check",
        "coalesce",
        "collate",
        "column",
        "commit",
        "constraint",
        "create",
        "cross",
        "database",
        "default",
        "delete",
        "desc",
        "distinct",
        "drop",
        "else",
        "end",
        "escape",
        "except",
        "exists",
        "foreign",
        "from",
        "full",
        "grant",
        "group",
        "having",
        "if",
        "in",
        "index",
        "inner",
        "insert",
        "intersect",
        "into",
        "is",
        "join",
        "key",
        "left",
        "like",
        "limit",
        "not",
        "null",
        "offset",
        "on",
        "or",
        "order",
        "outer",
        "primary",
        "references",
        "returning",
        "right",
        "rollback",
        "select",
        "set",
        "table",
        "then",
        "transaction",
        "trigger",
        "true",
        "false",
        "union",
        "unique",
        "update",
        "using",
        "values",
        "view",
        "when",
        "where",
        "with",
    ],
    types: &[
        "bigint",
        "blob",
        "boolean",
        "char",
        "date",
        "datetime",
        "decimal",
        "double",
        "float",
        "int",
        "integer",
        "numeric",
        "real",
        "smallint",
        "text",
        "time",
        "timestamp",
        "varchar",
    ],
    fold: true,
    calls: true,
    ..BARE
};

static SHELL: Rules = Rules {
    line: &["#"],
    quotes: &[
        DOUBLE,
        // `'` is literal in a shell: no escapes inside it at all.
        Quote::Run {
            delim: b'\'',
            escape: false,
            multiline: false,
        },
    ],
    keywords: &[
        "case", "cd", "do", "done", "echo", "elif", "else", "esac", "exit", "export", "fi", "for",
        "function", "if", "in", "local", "return", "set", "shift", "source", "then", "unset",
        "until", "while",
    ],
    sigils: b"$",
    ..BARE
};

static POWERSHELL: Rules = Rules {
    line: &["#"],
    block: &[("<#", "#>")],
    quotes: &[DOUBLE, SINGLE],
    keywords: &[
        "begin",
        "break",
        "catch",
        "class",
        "continue",
        "data",
        "do",
        "dynamicparam",
        "else",
        "elseif",
        "end",
        "enum",
        "exit",
        "filter",
        "finally",
        "for",
        "foreach",
        "function",
        "hidden",
        "if",
        "in",
        "param",
        "process",
        "return",
        "switch",
        "throw",
        "trap",
        "try",
        "until",
        "using",
        "while",
    ],
    fold: true,
    sigils: b"$",
    ..BARE
};

static BATCH: Rules = Rules {
    // `rem` is a command rather than a marker, so it only counts as a comment at the
    // head of a line — which is where the tokeniser tests it.
    line: &["::", "rem "],
    quotes: &[DOUBLE],
    keywords: &[
        "call", "cd", "copy", "del", "do", "echo", "else", "endlocal", "exist", "exit", "for",
        "goto", "if", "in", "md", "move", "not", "pause", "popd", "pushd", "set", "setlocal",
        "shift", "start",
    ],
    fold: true,
    sigils: b"%!",
    ..BARE
};

static CSS: Rules = Rules {
    block: &[("/*", "*/")],
    quotes: &[DOUBLE, SINGLE],
    keywords: &[
        "and",
        "auto",
        "important",
        "inherit",
        "initial",
        "none",
        "not",
        "only",
        "revert",
        "unset",
    ],
    calls: true,
    // A property is a word before its colon, which is the whole of CSS's structure that
    // a lexer can see.
    label: b":",
    ..BARE
};

static JSON: Rules = Rules {
    // `.jsonc` allows comments and plain `.json` does not. Colouring them in both is
    // the forgiving direction: a `//` in strict JSON is a syntax error, and showing it
    // as a comment is a better description of what somebody meant by it than showing it
    // as nothing.
    line: &["//"],
    block: &[("/*", "*/")],
    quotes: &[DOUBLE],
    keywords: &["false", "null", "true"],
    label: b":",
    ..BARE
};

static YAML: Rules = Rules {
    line: &["#"],
    quotes: &[DOUBLE, SINGLE],
    keywords: &["false", "no", "null", "off", "on", "true", "yes", "~"],
    label: b":",
    ..BARE
};

static TOML: Rules = Rules {
    line: &["#"],
    quotes: &[DOUBLE, SINGLE],
    keywords: &["false", "true"],
    label: b"=",
    sections: true,
    ..BARE
};

static INI: Rules = Rules {
    line: &["#", ";"],
    quotes: &[DOUBLE, SINGLE],
    keywords: &["false", "no", "true", "yes"],
    label: b"=:",
    sections: true,
    ..BARE
};

static CMAKE: Rules = Rules {
    line: &["#"],
    quotes: &[DOUBLE],
    keywords: &[
        "and",
        "break",
        "continue",
        "elseif",
        "else",
        "endforeach",
        "endfunction",
        "endif",
        "endmacro",
        "endwhile",
        "foreach",
        "function",
        "if",
        "macro",
        "not",
        "or",
        "return",
        "while",
    ],
    types: &[
        "BOOL",
        "CACHE",
        "FALSE",
        "FILEPATH",
        "INTERNAL",
        "OFF",
        "ON",
        "PARENT_SCOPE",
        "PATH",
        "PRIVATE",
        "PUBLIC",
        "REQUIRED",
        "STRING",
        "TRUE",
    ],
    fold: true,
    calls: true,
    sigils: b"$",
    ..BARE
};

// ---------------------------------------------------------------------------
// The generic tokeniser
// ---------------------------------------------------------------------------

fn code(body: &str, r: &Rules, out: &mut Vec<Span>) {
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
fn classify(word: &str, b: &[u8], end: usize, r: &Rules) -> Option<Tok> {
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
fn labelled(b: &[u8], end: usize, r: &Rules) -> bool {
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
fn line_comment(body: &str, i: usize, r: &Rules, head: bool) -> Option<usize> {
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
fn block_comment(body: &str, i: usize, r: &Rules) -> Option<usize> {
    let rest = &body[i..];
    let (open, close) = r.block.iter().find(|(open, _)| rest.starts_with(open))?;
    let from = i + open.len();
    Some(match body[from..].find(close) {
        Some(at) => from + at + close.len(),
        None => body.len(),
    })
}

/// Where a string starting at `i` ends, if one starts there.
fn string(body: &str, i: usize, r: &Rules) -> Option<usize> {
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
fn letter(body: &str, i: usize) -> Option<usize> {
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
fn number_end(b: &[u8], i: usize) -> usize {
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
fn variable_end(b: &[u8], i: usize) -> Option<usize> {
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
fn starts_word(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

fn word_end(b: &[u8], i: usize) -> usize {
    let mut at = i;
    while at < b.len() && is_word(b[at]) {
        at += 1;
    }
    at
}

/// One character on from `i`.
fn step(body: &str, i: usize) -> usize {
    let mut at = i + 1;
    while at < body.len() && !body.is_char_boundary(at) {
        at += 1;
    }
    at
}

// ---------------------------------------------------------------------------
// The two tokenisers that are not the generic one
// ---------------------------------------------------------------------------

/// A unified diff, coloured **by the line** rather than by the token.
///
/// Which is the whole reason it is not a `Rules` entry: what matters in a diff is not
/// what the code says but which side of the change it is on, and that is a property of
/// the first character of the line.
fn diff(body: &str, out: &mut Vec<Span>) {
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
fn markup(body: &str, out: &mut Vec<Span>) {
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
fn word_end_tag(b: &[u8], i: usize) -> usize {
    let mut at = i;
    while at < b.len() && (is_word(b[at]) || matches!(b[at], b'-' | b':' | b'.')) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
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
        for name in ["app.rs", "ui/preview.rs", "syntax.rs", "pe.rs"] {
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
}
