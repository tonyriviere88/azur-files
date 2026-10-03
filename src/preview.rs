//! What is in the file the keyboard is on, read off the UI thread.
//!
//! The panel that shows it is [`crate::ui::preview`]; this is the half that decides **what a
//! file is** and then **goes and gets it**. Three kinds, and they have nothing in common except
//! that answering takes long enough that the window must not wait for it:
//!
//! - **A picture.** Decoded by `image`, or rasterised by `resvg` where it is vector art, and
//!   handed over as pixels for the UI thread to upload. A 40-megapixel photograph takes a
//!   quarter of a second to decode and 160 MB to hold, so [`CAP`] is not optional.
//! - **Text.** Read up to [`TEXT_CAP`], and only if it really is text — a preview panel must
//!   never paint a megabyte of `\0` into a wrapped paragraph.
//! - **A binary.** [`crate::pe`]'s dependency walk, which is the interesting thing a `.dll`
//!   has inside it.
//!
//! # One service, one token, one answer
//!
//! Every kind goes through [`Previews`] and comes back as a [`Payload`] tagged with the token
//! that asked for it, so the panel has one thing to poll and one rule for staleness: an answer
//! to a token nobody holds any more is dropped. That is the same shape as
//! [`crate::shell::links`] and [`crate::loader`], for the same reason — and it is what lets the
//! panel debounce the selection in one place rather than three.
//!
//! Detached threads rather than a pool. A preview is one file at a time and the request rate is
//! bounded by a human moving a selection; there is nothing to queue, and there is nothing useful
//! to do with a decode that is still running when the window closes.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// What a file will be shown as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A raster image, or vector art to be rasterised.
    Picture,
    /// Something to read.
    Text,
    /// A Windows binary: what it imports, and from where.
    Binary,
    /// No extension, or one nothing here has heard of. **Decided on the worker** by looking at
    /// the first few kilobytes, because "is this text?" is a question about contents and the
    /// answer for `README`, `LICENSE`, `Makefile` and `.gitignore` is yes.
    Unknown,
}

/// Every extension shown as a picture.
///
/// `image`'s pure-Rust codecs plus `resvg`'s, and nothing that would need a C library. `svgz` is
/// not here: it is gzip, `resvg`'s decompression is behind the feature this build leaves off, and
/// a compressed SVG is rare enough not to be worth a second decompressor.
const PICTURES: [&str; 21] = [
    "png", "jpg", "jpeg", "jfif", "gif", "bmp", "dib", "ico", "cur", "tif", "tiff", "webp", "svg",
    "tga", "dds", "hdr", "qoi", "ff", "pbm", "pgm", "ppm",
];

/// Every extension read as text **where the columns mean something**: source, configuration,
/// data, logs, diffs. Shown in the monospace role.
///
/// Long on purpose, and it is still not the rule — see [`Kind::Unknown`], which is what catches
/// the ones that matter and cannot be listed.
const CODE: [&str; 59] = [
    "log", "csv", "tsv", "json", "jsonc", "yaml", "yml", "toml", "ini", "cfg", "conf",
    "properties", "env", "xml", "xsd", "xsl", "svgz", "html", "htm", "css", "scss", "less", "js",
    "mjs", "cjs", "ts", "tsx", "jsx", "rs", "c", "h", "cc", "cpp", "cxx", "hpp", "hxx", "cs",
    "java", "kt", "py", "rb", "go", "php", "pl", "lua", "sh", "bash", "zsh", "ps1", "psm1", "bat",
    "cmd", "sql", "diff", "patch", "gitignore", "gitattributes", "editorconfig", "cmake",
];

/// And every extension read as text where it does **not**: prose, shown in the body role.
///
/// The line between the two lists is a question with an answer, not a matter of taste: does moving
/// a character sideways change what the file means? In a log, a table, a diff or any source file it
/// does — a column that no longer lines up is information lost — so those get the monospace role
/// even though it is the less comfortable one to read a paragraph in. Markdown and a `.txt` are
/// paragraphs, and paragraphs are what the proportional face is for.
const PROSE: [&str; 4] = ["txt", "md", "markdown", "rst"];

/// Names with no extension that are prose all the same.
///
/// A file with nothing to go on is monospaced, because most of what has no extension in a source
/// folder is a build or configuration file where the columns matter — `Makefile`, `Dockerfile`,
/// `.npmrc`. These are the exceptions, and they are exceptions worth listing: a `README` set in a
/// monospace face is the one file in the folder somebody is going to sit and read.
const PROSE_NAMES: [&str; 8] = [
    "readme",
    "license",
    "licence",
    "copying",
    "notice",
    "changelog",
    "authors",
    "contributing",
];

/// Names that are code whatever their extension claims. **Without the extension**, like
/// [`PROSE_NAMES`] — see [`is_code`].
///
/// `CMakeLists.txt` is the case, and it earns a list of its own rather than an entry in either of the
/// two above. Its extension is `txt`, which is prose and right about nearly every other file carrying
/// it; this one is a build script full of indented blocks and aligned arguments. So the name has to
/// win over the extension, which makes this the mirror image of [`PROSE_NAMES`], where the name loses
/// to an extension and only speaks when there is none.
///
/// `Makefile` and `Dockerfile` need nothing here: a file with no extension is monospaced already.
///
/// Neighbours deliberately left out, because each is its own question and not this one:
/// `requirements.txt`, `CMakeCache.txt`, `robots.txt`.
const CODE_NAMES: [&str; 1] = ["cmakelists"];

/// Whether a file read as text has columns that mean something, and so wants the monospace role.
///
/// Asked of the *name* rather than carried on [`Kind`], because a sniffed file has no extension to
/// classify and the answer for it comes from the same place: what it is called.
///
/// **`stem` is the name with the extension taken off**, which is what [`code_of`] has always passed
/// and what both name lists are therefore written without. Worth spelling out in the signature: the
/// other function here that takes a `name` — [`kind_of`] — takes the whole one, and the first version
/// of `CODE_NAMES` held `cmakelists.txt`, which no caller could ever have matched. Its unit test
/// passed, because the test handed it the string the program does not.
pub fn is_code(stem: &str, ext: &str) -> bool {
    let is = |list: &[&str], what: &str| list.iter().any(|known| what.eq_ignore_ascii_case(known));
    // **The name beats the extension**, so it is asked first. The other way round, `CMakeLists.txt`
    // would already have been answered by `txt`.
    if is(&CODE_NAMES, stem) {
        return true;
    }
    if is(&PROSE, ext) {
        return false;
    }
    if ext.is_empty() && is(&PROSE_NAMES, stem) {
        return false;
    }
    true
}

/// What a file is, from its name alone. `None` for something with no preview at all.
///
/// A folder is not previewed: what a folder contains is what the listing beside the panel is
/// already showing, and a second copy of it would be the same answer twice.
pub fn kind_of(name: &str, ext: &str, is_dir: bool) -> Option<Kind> {
    if is_dir {
        return None;
    }
    let is = |list: &[&str]| list.iter().any(|known| ext.eq_ignore_ascii_case(known));
    if is(&PICTURES) {
        Some(Kind::Picture)
    } else if is(&CODE) || is(&PROSE) {
        Some(Kind::Text)
    } else if crate::pe::is_image(ext) {
        Some(Kind::Binary)
    } else if ext.is_empty() || name.starts_with('.') {
        // `README`, `LICENSE`, `Makefile`, `.gitignore`, `.npmrc`. A leading dot makes a dotfile
        // rather than an extension, so `Dir::ext` is empty for those anyway — the second test is
        // for `.gitignore`-style names whose *extension* is a word this list does happen to know.
        Some(Kind::Unknown)
    } else {
        None
    }
}

/// The most pixels a decoded picture may hold.
///
/// Four megapixels, which is 16 MB as RGBA. Not a limit on what can be *opened* — anything
/// larger is scaled down to fit inside it and says so on the canvas — but on what a preview is
/// allowed to cost. A phone photograph is 12 Mpx and a scanned drawing can be 200; holding one of
/// those at full size to show it in a 400-point panel would be most of the memory this program
/// uses on a thing nobody asked to keep.
///
/// The panel is at most a few hundred points wide, so 2048 on the long edge still leaves room to
/// zoom several times past fit before the softness shows.
pub const CAP: u32 = 2048;

/// How much of a text file is read.
///
/// A megabyte, which is about fifteen thousand lines. Past that a preview is not what you want —
/// and egui lays out the whole galley whether or not it is on screen, so a 200 MB log would be a
/// frozen window rather than a slow one. Truncation is reported.
pub const TEXT_CAP: usize = 1 << 20;

/// How much is looked at before deciding an unknown file is text.
const SNIFF: usize = 4096;

/// The most pixels this will *decode*, as opposed to hold.
///
/// A different and larger bound than [`CAP`], and it has to exist separately: `image` has no
/// streaming resize, so scaling something down to the cap means decoding all of it first. Forty
/// megapixels is 160 MB while that is happening, which is a camera's full output and a great deal
/// more than any preview needs. Past it the panel says how large the thing is instead, which is
/// more useful than a window that stops for two seconds and then shows a thumbnail.
const DECODE_MAX: u64 = 40_000_000;

/// A decoded picture, waiting to be uploaded.
pub struct Picture {
    pub pixels: egui::ColorImage,
    /// What it is on disk, which is what the panel reports — `pixels` may be smaller.
    pub natural: [u32; 2],
    /// It was larger than [`CAP`] and has been scaled down.
    pub scaled: bool,
    /// Vector art, rasterised at [`CAP`] rather than decoded at a natural size.
    pub vector: bool,
}

/// A file read as text.
pub struct Text {
    pub body: String,
    /// It is longer than [`TEXT_CAP`] and this is the front of it.
    pub truncated: bool,
    /// Its columns mean something, so it wants the monospace role. See [`is_code`].
    pub code: bool,
    /// What language it is in, for colour — and [`crate::syntax::Lang::Markdown`], which
    /// is how the panel knows to render the document rather than the markup.
    ///
    /// Decided from the name here on the worker rather than in the panel, because it is
    /// the same question `code` is and comes from the same two halves of it.
    pub lang: crate::syntax::Lang,
    /// What git says has changed in it, when it is in a repository and something has.
    ///
    /// Read here, on the same worker and in the same answer as the file itself: the diff and the text
    /// it describes are one fact about one moment, and fetching them separately would let a file
    /// arrive with a diff of the version before it. `None` for a file outside a repository, an
    /// untracked one — git has no diff for a file it does not know — and anything git could not be
    /// asked about.
    pub changes: Option<crate::git::Changes>,
}

// ---------------------------------------------------------------------------
// Searching the text on show
// ---------------------------------------------------------------------------

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

/// Two pictures, and where they differ.
pub struct Diff {
    pub a: Picture,
    pub b: Picture,
    /// **Where they differ, as a mask**: white, with the alpha carrying how much. Transparent
    /// wherever the two agree.
    ///
    /// A mask and not a coloured image, deliberately. What colour "different" is painted in is a
    /// decision for [`crate::ui::preview`], where every other colour in this program is decided —
    /// so what comes off the worker is a measurement and the panel tints it.
    pub mask: Picture,
    /// The share of pixels that differ at all, 0..1. The number you actually want: "they are the
    /// same file" and "0.02% of it moved" are different answers and a picture shows neither.
    pub differing: f32,
}

/// What came back.
pub enum Payload {
    Picture(Box<Picture>),
    /// Two pictures compared. Boxed: three decoded images is up to 48 MB, and an enum is as large
    /// as its largest variant everywhere it is passed.
    Diff(Box<Diff>),
    Text(Text),
    Binary(Arc<crate::pe::Graph>),
    /// Nothing to show, and why — short enough to put in the middle of the panel.
    Failed(String),
}

/// What a panel has asked for: one file, two to be compared, or one against the version in the last
/// commit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ask {
    One(std::path::PathBuf, Kind),
    /// Two pictures, in the order they appear in the listing.
    Pair(std::path::PathBuf, std::path::PathBuf),
    /// One picture, and the same picture as `HEAD` has it.
    ///
    /// **The same comparison [`Ask::Pair`] asks for, against a version that is not a file.** A diff
    /// of a `.png` has no lines to put a band behind, so what "show me what changed" means for a
    /// picture is the two of them and the difference — which is a view this panel already had.
    ///
    /// A separate variant rather than a flag on `One`, because [`Ask`] is what the panel compares to
    /// decide whether the answer it is holding is still the answer: turning the diff off has to be a
    /// different question, or the panel would go on showing the comparison.
    AgainstHead(std::path::PathBuf),
}

impl Ask {
    /// The file the panel is about, for anything that needs one path — the title's folder, a
    /// staleness test.
    pub fn first(&self) -> &Path {
        match self {
            Self::One(path, _) => path,
            Self::Pair(a, _) => a,
            Self::AgainstHead(path) => path,
        }
    }

    /// What the panel's bar calls it.
    pub fn title(&self) -> String {
        let name = |path: &Path| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        match self {
            Self::One(path, _) => name(path),
            // The two names, and a mark that says they are being compared rather than listed.
            Self::Pair(a, b) => format!("{} \u{2194} {}", name(a), name(b)),
            // Its own name and no more: there is one file here, and what the two views of it are is
            // written above each of them. The latched diff button is what says the panel is
            // comparing.
            Self::AgainstHead(path) => name(path),
        }
    }
}

pub struct Loaded {
    pub token: u64,
    pub payload: Payload,
}

/// The preview-reading service. One per application.
pub struct Previews {
    answers: Receiver<Loaded>,
    /// Kept so each request can be given a live channel to answer on.
    replies: Sender<Loaded>,
    next: u64,
    ctx: egui::Context,
}

impl Previews {
    pub fn new(ctx: &egui::Context) -> Self {
        let (replies, answers) = channel();
        Self {
            answers,
            replies,
            next: 1,
            ctx: ctx.clone(),
        }
    }

    /// Go and get it. The returned token identifies the answer.
    pub fn request(&mut self, ask: &Ask) -> u64 {
        let token = self.next;
        self.next += 1;
        let ask = ask.clone();
        let replies = self.replies.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("preview".to_owned())
            .spawn(move || {
                crate::fs::scan::silence_device_dialogs();
                let payload = read(&ask);
                if replies.send(Loaded { token, payload }).is_ok() {
                    ctx.request_repaint();
                }
            });
        // A machine that will not give us a thread leaves the panel waiting for a token that
        // never arrives, which the next request clears. Nothing else is affected.
        let _ = spawned;
        token
    }

    /// Everything that has come back since the last call.
    pub fn drain(&self) -> impl Iterator<Item = Loaded> + '_ {
        self.answers.try_iter()
    }
}

/// Do the reading. On a worker, always.
fn read(ask: &Ask) -> Payload {
    let (path, kind) = match ask {
        Ask::One(path, kind) => (path.as_path(), *kind),
        Ask::Pair(a, b) => return compare(a, b),
        Ask::AgainstHead(path) => return against_head(path),
    };
    match kind {
        Kind::Picture => picture(path),
        Kind::Text => text(path),
        Kind::Binary => {
            let graph = crate::pe::walk(path, crate::pe::BUDGET, crate::pe::PATIENCE);
            // A root that is not a binary has no tree to draw, and one lonely row marked
            // unreadable is a poor way to answer a question. The reason goes in the middle of
            // the panel instead — which is what happens to a 16-bit `.exe`, a `.sys` that is
            // really something else, and a file on a share that cannot be read.
            match graph.root().state {
                crate::pe::State::Found => Payload::Binary(Arc::new(graph)),
                crate::pe::State::Unreadable(why) => Payload::Failed(capitalise(why)),
                _ => Payload::Failed("Cannot be read".to_owned()),
            }
        }
        // Text if it looks like text, and nothing if it does not.
        Kind::Unknown => match sniff(path) {
            Some(true) => text(path),
            Some(false) => Payload::Failed("Not something this can show".to_owned()),
            None => Payload::Failed("Cannot be read".to_owned()),
        },
    }
}

/// Whether the front of `path` reads as text. `None` if it cannot be opened.
///
/// Two tests, and both matter. **A NUL byte** is the oldest and still the best binary tell: no
/// text encoding this would show puts one in the middle of a document, and every executable,
/// archive and database is full of them. **Valid UTF-8** over the same window catches the rest —
/// with the last few bytes forgiven, because a 4 KB window will usually cut a multi-byte
/// character in half and that is not a reason to refuse the file.
fn sniff(path: &Path) -> Option<bool> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; SNIFF];
    let read = file.read(&mut head).ok()?;
    head.truncate(read);
    if head.contains(&0) {
        return Some(false);
    }
    Some(match std::str::from_utf8(&head) {
        Ok(_) => true,
        // `valid_up_to` past all but the last few bytes means the only invalid sequence is the
        // character the window cut in half.
        Err(why) => why.valid_up_to() + 4 >= head.len(),
    })
}

/// Whether the file at `path` wants the monospace role, from its name.
fn code_of(path: &Path) -> bool {
    let (stem, ext) = halves(path);
    is_code(&stem, &ext)
}

/// And what language it is in, from the same two halves. See [`crate::syntax::lang_of`].
fn lang_of(path: &Path) -> crate::syntax::Lang {
    let (stem, ext) = halves(path);
    crate::syntax::lang_of(&stem, &ext)
}

/// The name with the extension off, and the extension — the two things every question
/// about a file's *kind of text* is asked of.
fn halves(path: &Path) -> (String, String) {
    let stem = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (stem, extension_of(path))
}

fn text(path: &Path) -> Payload {
    use std::io::Read as _;

    let Ok(mut file) = std::fs::File::open(path) else {
        return Payload::Failed("Cannot be opened".to_owned());
    };
    // One byte past the cap, so a file exactly at it is not reported as truncated.
    let mut buffer = Vec::new();
    if file
        .by_ref()
        .take(TEXT_CAP as u64 + 1)
        .read_to_end(&mut buffer)
        .is_err()
    {
        return Payload::Failed("Cannot be read".to_owned());
    }
    let truncated = buffer.len() > TEXT_CAP;
    buffer.truncate(TEXT_CAP);
    // Lossy rather than a refusal: a file that is text apart from one bad byte is still worth
    // reading, and `from_utf8_lossy` puts a replacement character where the byte was.
    let mut body = String::from_utf8_lossy(&buffer).into_owned();
    // A `\r` that survives into a galley is laid out as a glyph — a hollow box, at the end of
    // every line of every file written on this platform.
    if body.contains('\r') {
        body = body.replace("\r\n", "\n").replace('\r', "\n");
    }
    // And a tab, which egui lays out as a single space. Four, because the alternative is that
    // every indented file in the preview is flat.
    if body.contains('\t') {
        body = body.replace('\t', "    ");
    }
    Payload::Text(Text {
        body,
        truncated,
        code: code_of(path),
        lang: lang_of(path),
        // Asked for after the file has been read, so a folder with no repository above it costs the
        // `stat` walk and nothing else — see [`crate::git::changes`]. An empty answer is kept as
        // `None`: "nothing changed" and "no diff to show" are the same thing to the panel.
        changes: crate::git::changes(path).filter(|changes| !changes.is_empty()),
    })
}

/// Two pictures, and a mask of where they differ.
///
/// **Compared at the larger of the two sizes**, with a pixel that exists in only one of them
/// counted as differing. Two files of different dimensions are not the same picture, and saying so
/// by lighting up the region one of them does not reach is more useful than either refusing to
/// compare them or quietly cropping to the overlap and reporting a small difference.
fn compare(a: &Path, b: &Path) -> Payload {
    let (Payload::Picture(a), Payload::Picture(b)) = (picture(a), picture(b)) else {
        // Whichever failed, its own complaint is the useful one — so it is read again rather than
        // guessed at. Both are page-cache warm by now.
        return match picture(a) {
            Payload::Failed(why) => Payload::Failed(why),
            _ => picture(b),
        };
    };
    difference(*a, *b)
}

/// One picture, and the same picture as the last commit has it.
///
/// **The picture alone when there is nothing to compare it with**, which is the answer for an
/// untracked file, a file outside a repository, and a `HEAD` version this build cannot decode. The
/// panel asks for this only where git has already said the file has changed, so those are the odd
/// cases rather than the common one — but every one of them has to end in a picture, because the file
/// is there and somebody asked to see it.
///
/// The blob is decoded from memory rather than written out and read back. It is already in memory —
/// git wrote it down a pipe — and a preview that left temporary files behind would be a preview that
/// left temporary files behind.
fn against_head(path: &Path) -> Payload {
    let now = match picture(path) {
        Payload::Picture(now) => now,
        // The file itself will not decode: its own complaint, not a comparison's.
        other => return other,
    };
    let Some(bytes) = crate::git::blob(path) else {
        return Payload::Picture(now);
    };
    // The same name, so the same question about vector art: a blob has no extension of its own.
    let vector = is_vector(path);
    let Ok(before) = decode(&bytes, vector) else {
        return Payload::Picture(now);
    };
    difference(before, *now)
}

/// Two decoded pictures, and a mask of where they differ. See [`compare`] for the size rule.
fn difference(a: Picture, b: Picture) -> Payload {
    let size = [
        a.pixels.size[0].max(b.pixels.size[0]),
        a.pixels.size[1].max(b.pixels.size[1]),
    ];
    let at = |picture: &Picture, x: usize, y: usize| -> Option<egui::Color32> {
        let [w, h] = picture.pixels.size;
        (x < w && y < h).then(|| picture.pixels.pixels[y * w + x])
    };

    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    let mut differing = 0usize;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let delta = match (at(&a, x, y), at(&b, x, y)) {
                (Some(one), Some(other)) => {
                    // The largest single-channel difference, alpha included. A per-channel maximum
                    // rather than a sum, so a picture that differs in one channel by a lot is not
                    // averaged down towards one that differs in three by a little.
                    let ([p, q], [r, s]) = ([one.r(), one.g()], [other.r(), other.g()]);
                    (p.abs_diff(r))
                        .max(q.abs_diff(s))
                        .max(one.b().abs_diff(other.b()))
                        .max(one.a().abs_diff(other.a()))
                }
                // Present in one and not the other: as different as it gets.
                _ => 255,
            };
            if delta > 0 {
                differing += 1;
            }
            // **Amplified eightfold.** A one-level difference at alpha 1 is invisible, and the
            // whole job of this image is to be *findable*: past a delta of 32 it is fully opaque,
            // and below that it fades rather than vanishing. The count above is the honest
            // measurement; this is the visible one.
            pixels.push(egui::Color32::from_white_alpha(
                ((delta as u32) * 8).min(255) as u8,
            ));
        }
    }

    let total = (size[0] * size[1]).max(1);
    Payload::Diff(Box::new(Diff {
        differing: differing as f32 / total as f32,
        mask: Picture {
            pixels: egui::ColorImage {
                size,
                pixels,
                source_size: egui::vec2(size[0] as f32, size[1] as f32),
            },
            natural: [size[0] as u32, size[1] as u32],
            scaled: false,
            vector: false,
        },
        a,
        b,
    }))
}

fn picture(path: &Path) -> Payload {
    match if is_vector(path) {
        vector_art(path)
    } else {
        raster(path)
    } {
        Ok(picture) => Payload::Picture(Box::new(picture)),
        Err(why) => Payload::Failed(why),
    }
}

/// Whether this name is vector art, which is the one thing about a picture that is decided by its
/// name rather than by its bytes.
///
/// Asked of the *name* because that is all a blob out of git has — and because `usvg` and `image` are
/// two different decoders rather than two formats one decoder sniffs between.
fn is_vector(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
}

/// A picture already in memory: the bytes git had for it.
///
/// The same two decoders [`picture`] chooses between, and the same bounds — [`raster_from`] is where
/// the megapixel cap and the [`CAP`] scale-down live, so a blob cannot get past a limit a file cannot.
fn decode(bytes: &[u8], vector: bool) -> Result<Picture, String> {
    if vector {
        vector_from(bytes)
    } else {
        raster_from(|| {
            image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .map_err(|_| "Cannot be read".to_owned())
        })
    }
}

/// Anything `image` can decode, out of a file.
fn raster(path: &Path) -> Result<Picture, String> {
    // The format from the *contents* rather than from the extension, which is how a `.jpg` that
    // is really a PNG — and there are a great many of those — still opens.
    raster_from(|| {
        image::ImageReader::open(path)
            .map_err(|_| "Cannot be opened".to_owned())?
            .with_guessed_format()
            .map_err(|_| "Cannot be read".to_owned())
    })
}

/// The decode itself, over whatever the bytes are coming from.
///
/// `open` is called **twice** — once for the dimensions and once for the pixels — which is why it is
/// a closure rather than a reader: the header has to be read before there is any decision to decode,
/// and a reader that has been read is a reader that has moved. Opening a file twice costs a handle
/// and a warm page cache; making a second `Cursor` over a slice costs nothing.
fn raster_from<R: std::io::BufRead + std::io::Seek>(
    open: impl Fn() -> Result<image::ImageReader<R>, String>,
) -> Result<Picture, String> {
    // **How big it is, before deciding to decode it.** Only the header is read for this, and it
    // is the one bound that has to come first: `image` has no streaming resize, so a decode is
    // the full size in memory however small the answer is going to be. A 40-megapixel image is
    // 160 MB while it is being scaled down, and that is as far as a preview gets to go.
    let natural = open()?
        .into_dimensions()
        .map_err(|why| short(&why.to_string()))?;
    let pixels = natural.0 as u64 * natural.1 as u64;
    if pixels > DECODE_MAX {
        return Err(format!(
            "Too large to preview: {:.0} megapixels",
            pixels as f64 / 1e6
        ));
    }

    let decoded = open()?
        .decode()
        .map_err(|why| short(&why.to_string()))?
        .into_rgba8();

    let (width, height) = (decoded.width(), decoded.height());
    let scaled = width > CAP || height > CAP;
    let decoded = if scaled {
        // `Triangle` rather than `Lanczos3`: this is a preview, the difference is invisible at
        // the size it is shown, and Lanczos over a 200-megapixel scan is seconds rather than
        // milliseconds.
        let ratio = (CAP as f32 / width.max(height) as f32).min(1.0);
        image::imageops::resize(
            &decoded,
            ((width as f32 * ratio) as u32).max(1),
            ((height as f32 * ratio) as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        decoded
    };

    Ok(Picture {
        pixels: egui::ColorImage::from_rgba_unmultiplied(
            [decoded.width() as usize, decoded.height() as usize],
            decoded.as_raw(),
        ),
        natural: [natural.0, natural.1],
        scaled,
        vector: false,
    })
}

/// The extension, lowercased, for the panel's "no preview for this" line.
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// SVG, rasterised.
///
/// At [`CAP`] on the long edge of the document's own aspect, rather than at the size the panel
/// happens to be: the panel is resized and zoomed, and re-rasterising on every frame that changes
/// would be a parse and a fill per frame. The cost is that zooming far past fit goes soft, which
/// is what happens to a raster image too.
///
/// **Text inside the SVG is not drawn.** `usvg`'s text support means a font database, shaping and
/// system font enumeration — see the dependency's justification in `Cargo.toml` — and the honest
/// thing is to say so, which [`Picture::vector`] is for.
fn vector_art(path: &Path) -> Result<Picture, String> {
    let bytes = std::fs::read(path).map_err(|_| "Cannot be opened".to_owned())?;
    vector_from(&bytes)
}

/// The same, from a drawing already in memory.
fn vector_from(bytes: &[u8]) -> Result<Picture, String> {
    use resvg::tiny_skia;
    use resvg::usvg;

    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default())
        .map_err(|why| short(&why.to_string()))?;

    let size = tree.size();
    if size.width() < 1.0 || size.height() < 1.0 {
        return Err("The drawing has no size".to_owned());
    }
    // Always to the cap on the long edge, up as well as down. A 16-point icon is the commonest
    // thing in a folder of SVGs and rasterising it at its nominal size would put a 16-pixel
    // square in the middle of a 400-point panel — the whole point of vector art is that there is
    // no natural size to respect. The cap is what bounds the cost, and it is the same cap every
    // other picture here is held at.
    let ratio = CAP as f32 / size.width().max(size.height());
    let width = ((size.width() * ratio) as u32).max(1);
    let height = ((size.height() * ratio) as u32).max(1);

    let mut canvas =
        tiny_skia::Pixmap::new(width, height).ok_or_else(|| "Too large to draw".to_owned())?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(ratio, ratio),
        &mut canvas.as_mut(),
    );

    // `tiny_skia` hands back premultiplied RGBA, which is what egui wants — so the bytes go
    // across as they are rather than through the unmultiplied constructor, which would divide
    // the alpha back out and then multiply it in again.
    let pixels = egui::ColorImage {
        size: [width as usize, height as usize],
        pixels: canvas
            .pixels()
            .iter()
            .map(|p| {
                egui::Color32::from_rgba_premultiplied(p.red(), p.green(), p.blue(), p.alpha())
            })
            .collect(),
        source_size: egui::vec2(width as f32, height as f32),
    };
    Ok(Picture {
        pixels,
        natural: [size.width() as u32, size.height() as u32],
        scaled: false,
        vector: true,
    })
}

/// A word or two, as a sentence for the middle of a panel.
fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Cannot be read".to_owned(),
    }
}

/// A decoder's complaint, cut to something that fits on one line of a panel.
fn short(why: &str) -> String {
    let first = why.split(['\n', ':']).next().unwrap_or(why).trim();
    let mut out = if first.is_empty() { why } else { first }.to_owned();
    out.truncate(80);
    // Capitalised, because it goes in the middle of a panel as a sentence rather than into a log.
    capitalise(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{PathBuf, MAIN_SEPARATOR};

    fn scratch(name: &str) -> PathBuf {
        let root = crate::sandbox::dir("preview");
        std::fs::create_dir_all(&root).expect("a directory in the temp folder");
        root.join(name)
    }

    /// What a search finds, as the matched text rather than as offsets — a list of byte ranges is
    /// not something you can read an assertion off.
    fn found<'a>(body: &'a str, text: &str, case: bool, word: bool, regex: bool) -> Vec<&'a str> {
        let search = Search {
            text: text.to_owned(),
            case,
            word,
            regex,
        };
        hits(body, &search)
            .at
            .into_iter()
            .map(|at| &body[at])
            .collect()
    }

    #[test]
    fn a_search_is_case_insensitive_until_it_is_told_otherwise() {
        let body = "Foo foo FOO";
        assert_eq!(found(body, "foo", false, false, false), ["Foo", "foo", "FOO"]);
        assert_eq!(found(body, "foo", true, false, false), ["foo"]);
        assert_eq!(found(body, "FOO", true, false, false), ["FOO"]);
        // And past ASCII, which is the whole reason the fold is per character.
        assert_eq!(found("Élan élan", "élan", false, false, false), ["Élan", "élan"]);
        assert_eq!(found("Élan élan", "élan", true, false, false), ["élan"]);
    }

    #[test]
    fn hits_never_overlap() {
        // Two matches sharing a character are not two places to step to.
        assert_eq!(found("aaaa", "aa", false, false, false), ["aa", "aa"]);
        assert_eq!(found("aaa", "aa", false, false, false), ["aa"]);
    }

    #[test]
    fn whole_word_means_what_a_word_boundary_means() {
        let body = "word wording sword _word word_";
        assert_eq!(found(body, "word", false, false, false).len(), 5);
        assert_eq!(
            found(body, "word", false, true, false),
            ["word"],
            "only the one standing on its own"
        );
        // The boundary is where word-ness *changes*, so a needle that starts with punctuation is
        // still findable — the tempting rule refuses this one.
        assert_eq!(found("a-x b", "-x", false, true, false), ["-x"]);
    }

    #[test]
    fn a_regex_is_a_pattern_and_a_literal_is_not() {
        let body = "a1 b22 c333";
        assert_eq!(found(body, r"\d+", false, false, true), ["1", "22", "333"]);
        assert_eq!(
            found(body, r"\d+", false, false, false),
            Vec::<&str>::new(),
            "the literal characters `\\d+` are not in there"
        );
        // The two flags compose the way they read: `\b(?:..)\b`.
        assert_eq!(found("in inn", "in", false, true, true), ["in"]);
        assert_eq!(found("in inn", "in", false, false, true), ["in", "in"]);
        // And case still applies to a pattern.
        assert_eq!(found("Ab ab", "a.", true, false, true), ["ab"]);
    }

    #[test]
    fn a_pattern_that_is_not_a_pattern_says_so() {
        let bad = hits(
            "anything",
            &Search {
                text: "(unclosed".to_owned(),
                regex: true,
                ..Search::default()
            },
        );
        assert!(bad.bad);
        assert!(bad.at.is_empty());
        // The same text as a literal is fine, which is what makes the flag worth having a state for
        // rather than clearing the field.
        assert_eq!(found("a (unclosed b", "(unclosed", false, false, false).len(), 1);
    }

    #[test]
    fn a_pattern_that_matches_nothing_at_all_finds_nothing() {
        // `a*` matches emptily at every one of the 1,048,577 positions in a full body. None of them
        // is a place to go, and collecting them would be the slowest thing in the program.
        assert_eq!(found("bbb", "a*", false, false, true), Vec::<&str>::new());
        // But a pattern that can match emptily *and* substantially still finds the substance.
        assert_eq!(found("b1b22", r"\d*", false, false, true), ["1", "22"]);
    }

    #[test]
    fn the_search_stops_counting_at_its_budget() {
        // The first keystroke of any search over a large file is this case.
        let body = "a".repeat(HITS * 2);
        let all = hits(
            &body,
            &Search {
                text: "a".to_owned(),
                ..Search::default()
            },
        );
        assert_eq!(all.at.len(), HITS);
        assert!(all.capped, "it stopped without saying so");
        // Which the counter has to be able to tell from a search that simply found that many.
        let exact = hits(
            &"a".repeat(HITS),
            &Search {
                text: "a".to_owned(),
                ..Search::default()
            },
        );
        assert_eq!(exact.at.len(), HITS);
        assert!(!exact.capped);
    }

    #[test]
    fn an_empty_search_finds_nothing_rather_than_everything() {
        let nothing = hits("some text", &Search::default());
        assert!(nothing.at.is_empty() && !nothing.capped && !nothing.bad);
    }

    #[test]
    fn every_hit_is_a_range_of_the_body() {
        // The ranges become sections of a layout job over this exact string, where one that is out
        // of bounds or reversed is a panic rather than a wrong colour.
        let body = "héllo wörld héllo";
        let found = hits(
            body,
            &Search {
                text: "héllo".to_owned(),
                ..Search::default()
            },
        );
        assert_eq!(found.at.len(), 2);
        let mut last = 0;
        for at in &found.at {
            assert!(at.start < at.end, "reversed");
            assert!(at.end <= body.len(), "past the end");
            assert!(body.is_char_boundary(at.start) && body.is_char_boundary(at.end));
            assert!(at.start >= last, "out of order");
            last = at.end;
        }
    }

    #[test]
    fn a_name_says_what_it_will_be_shown_as() {
        assert_eq!(kind_of("a.png", "png", false), Some(Kind::Picture));
        assert_eq!(kind_of("a.PNG", "PNG", false), Some(Kind::Picture));
        assert_eq!(kind_of("a.svg", "svg", false), Some(Kind::Picture));
        assert_eq!(kind_of("a.md", "md", false), Some(Kind::Text));
        assert_eq!(kind_of("a.rs", "rs", false), Some(Kind::Text));
        assert_eq!(kind_of("a.dll", "dll", false), Some(Kind::Binary));
        assert_eq!(kind_of("a.exe", "exe", false), Some(Kind::Binary));
        // Nothing to go on: the worker looks instead. This is what makes `README` and
        // `Makefile` previewable, which is most of what is in a source folder without one.
        assert_eq!(kind_of("README", "", false), Some(Kind::Unknown));
        assert_eq!(kind_of("Makefile", "", false), Some(Kind::Unknown));
        assert_eq!(kind_of(".gitignore", "gitignore", false), Some(Kind::Text));
        assert_eq!(kind_of(".npmrc", "npmrc", false), Some(Kind::Unknown));
        // And things that genuinely have no preview.
        assert_eq!(kind_of("a.zip", "zip", false), None);
        assert_eq!(kind_of("a.mp4", "mp4", false), None);
        // A folder is what the listing beside the panel is already showing.
        assert_eq!(kind_of("src", "", true), None);
    }

    /// Text is read, its line endings are made harmless, and a long one is cut and says so.
    #[test]
    fn text_comes_back_readable_and_bounded() {
        let path = scratch("notes.txt");
        std::fs::write(&path, "one\r\ntwo\ttabbed\r\nthree").expect("a file");
        let Payload::Text(text) = read(&Ask::One(path.clone(), Kind::Text)) else {
            panic!("a text file did not come back as text");
        };
        assert!(!text.truncated);
        assert!(
            !text.body.contains('\r'),
            "a CR survived into the galley: {:?}",
            text.body
        );
        assert!(!text.body.contains('\t'), "a tab survived");
        assert_eq!(text.body, "one\ntwo    tabbed\nthree");

        // Past the cap: the front of it, and it says so.
        let long = scratch("long.txt");
        std::fs::write(&long, "x".repeat(TEXT_CAP + 100)).expect("a big file");
        let Payload::Text(text) = read(&Ask::One(long.clone(), Kind::Text)) else {
            panic!("not text");
        };
        assert!(text.truncated, "a file past the cap did not admit it");
        assert_eq!(text.body.len(), TEXT_CAP);

        // Exactly at it is not truncated, which is the off-by-one worth pinning.
        let exact = scratch("exact.txt");
        std::fs::write(&exact, "y".repeat(TEXT_CAP)).expect("a file at the cap");
        let Payload::Text(text) = read(&Ask::One(exact.clone(), Kind::Text)) else {
            panic!("not text");
        };
        assert!(!text.truncated);
        crate::sandbox::remove_file(&path);
        crate::sandbox::remove_file(&long);
        crate::sandbox::remove_file(&exact);
    }

    /// An extensionless file is text if it reads as text, and refused if it does not.
    ///
    /// The refusal is the half that matters: without it, pointing the panel at a file with no
    /// extension would paint whatever bytes it holds into a wrapped paragraph.
    #[test]
    fn an_unknown_file_is_sniffed_rather_than_guessed() {
        let readme = scratch("README");
        std::fs::write(&readme, "# A project\n\nWith some prose in it.\n").expect("a file");
        assert_eq!(sniff(&readme), Some(true));
        assert!(matches!(read(&Ask::One(readme.clone(), Kind::Unknown)), Payload::Text(_)));

        // A NUL is the tell, and it is checked before UTF-8 — the bytes below are valid UTF-8.
        let blob = scratch("blob");
        std::fs::write(&blob, b"MZ\x90\x00\x03\x00\x00\x00").expect("a file");
        assert_eq!(sniff(&blob), Some(false));
        assert!(matches!(read(&Ask::One(blob.clone(), Kind::Unknown)), Payload::Failed(_)));

        // Invalid UTF-8 well inside the window, with no NUL anywhere.
        let latin = scratch("latin");
        let mut bytes = b"caf\xe9 and more prose after it, at length, so the bad byte is not near the end".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 200));
        std::fs::write(&latin, &bytes).expect("a file");
        assert_eq!(sniff(&latin), Some(false));

        // But a multi-byte character cut in half by the window is forgiven, or every UTF-8 file
        // longer than the sniff window would be refused about a quarter of the time.
        let cut = scratch("cut");
        let mut bytes = "é".repeat(SNIFF).into_bytes();
        bytes.truncate(SNIFF - 1);
        std::fs::write(&cut, &bytes).expect("a file");
        assert_eq!(sniff(&cut), Some(true));

        assert_eq!(sniff(Path::new("no-such-file-at-all")), None);
        for path in [readme, blob, latin, cut] {
            crate::sandbox::remove_file(&path);
        }
    }

    /// A picture decodes, keeps its alpha, and is scaled to fit the cap rather than refused.
    ///
    /// The fixture is written here rather than checked in: a binary blob in the repository is
    /// something nobody can read, and `image` is already in the graph to write one with.
    #[test]
    fn a_picture_keeps_its_alpha_and_is_bounded() {
        let path = scratch("swatch.png");
        // Four pixels: opaque red, half-transparent green, transparent, opaque white.
        let mut buffer = image::RgbaImage::new(2, 2);
        buffer.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        buffer.put_pixel(1, 0, image::Rgba([0, 255, 0, 128]));
        buffer.put_pixel(0, 1, image::Rgba([0, 0, 0, 0]));
        buffer.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        buffer.save(&path).expect("a PNG in the temp folder");

        let Payload::Picture(picture) = read(&Ask::One(path.clone(), Kind::Picture)) else {
            panic!("a PNG did not come back as a picture");
        };
        assert_eq!(picture.pixels.size, [2, 2]);
        assert_eq!(picture.natural, [2, 2]);
        assert!(!picture.scaled && !picture.vector);
        // The alpha survived, which is what the checkerboard behind the canvas is for. egui
        // premultiplies on the way in, so the half-transparent green is checked by its alpha.
        assert_eq!(picture.pixels.pixels[0], egui::Color32::from_rgb(255, 0, 0));
        assert_eq!(picture.pixels.pixels[1].a(), 128);
        assert_eq!(picture.pixels.pixels[2].a(), 0);

        // And one past the cap comes back at the cap, saying so.
        let big = scratch("big.png");
        image::RgbaImage::from_pixel(CAP + 40, 10, image::Rgba([1, 2, 3, 255]))
            .save(&big)
            .expect("a wide PNG");
        let Payload::Picture(picture) = read(&Ask::One(big.clone(), Kind::Picture)) else {
            panic!("not a picture");
        };
        assert!(picture.scaled, "an oversized picture did not admit it");
        assert_eq!(picture.pixels.size[0], CAP as usize);
        assert_eq!(
            picture.natural,
            [CAP + 40, 10],
            "the reported size is the scaled one rather than the file's"
        );

        // Something that is not an image at all says so instead of panicking.
        let lie = scratch("lie.png");
        std::fs::write(&lie, b"this is not a PNG").expect("a file");
        assert!(matches!(read(&Ask::One(lie.clone(), Kind::Picture)), Payload::Failed(_)));
        for path in [path, big, lie] {
            crate::sandbox::remove_file(&path);
        }
    }

    /// SVG is rasterised, and its alpha comes through premultiplied the way egui wants it.
    #[test]
    fn vector_art_is_rasterised_to_fit() {
        let path = scratch("mark.svg");
        // A red square on the left half of a 100×50 canvas, and nothing on the right — so the
        // transparent half is there to check.
        std::fs::write(
            &path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
                  <rect x="0" y="0" width="50" height="50" fill="#ff0000"/>
                </svg>"##,
        )
        .expect("an SVG in the temp folder");

        let Payload::Picture(picture) = read(&Ask::One(path.clone(), Kind::Picture)) else {
            panic!("an SVG did not come back as a picture");
        };
        assert!(picture.vector, "it was not reported as vector art");
        assert_eq!(picture.natural, [100, 50]);
        // Rasterised to the cap on its long edge, keeping its aspect.
        assert_eq!(picture.pixels.size, [CAP as usize, CAP as usize / 2]);
        let at = |x: usize, y: usize| picture.pixels.pixels[y * picture.pixels.size[0] + x];
        assert_eq!(at(10, 10), egui::Color32::from_rgb(255, 0, 0), "the fill");
        assert_eq!(at(picture.pixels.size[0] - 10, 10).a(), 0, "the empty half");

        let broken = scratch("broken.svg");
        std::fs::write(&broken, b"<svg").expect("a file");
        assert!(matches!(read(&Ask::One(broken.clone(), Kind::Picture)), Payload::Failed(_)));
        crate::sandbox::remove_file(&path);
        crate::sandbox::remove_file(&broken);
    }

    /// Which files are set in monospace, and which are not.
    ///
    /// The line is a question with an answer rather than a matter of taste: does moving a character
    /// sideways change what the file means?
    #[test]
    fn columns_that_mean_something_get_the_monospace_face() {
        for (name, ext) in [
            ("main", "rs"),
            ("build", "log"),
            ("data", "csv"),
            ("package", "json"),
            ("Makefile", ""),
            (".npmrc", "npmrc"),
            ("setup", "bat"),
            ("toolchain", "cmake"),
            // The one the extension gets wrong on its own: `txt` is prose and this is a build
            // script. Any case, because the name is written every way round in the wild — and the
            // *stem*, not the whole name, which is what the caller passes.
            ("CMakeLists", "txt"),
            ("cmakelists", "txt"),
            ("CMAKELISTS", "TXT"),
        ] {
            assert!(is_code(name, ext), "{name}.{ext} has columns that matter");
        }
        for (name, ext) in [
            ("notes", "txt"),
            // And the exception is the whole stem and not a word in it: a file *about* the build
            // script is still prose.
            ("about-CMakeLists", "txt"),
            ("CMakeLists-notes", "txt"),
            ("README", "md"),
            ("readme", ""),
            ("LICENSE", ""),
            ("CHANGELOG", ""),
        ] {
            assert!(!is_code(name, ext), "{name}.{ext} is prose");
        }
    }

    /// The same question asked the way the program asks it: of a **path**.
    ///
    /// `is_code` takes the name with the extension already off, and its own test can hand it anything
    /// — which is how `CODE_NAMES` first shipped holding `cmakelists.txt`, a string `code_of` cannot
    /// produce and so a rule that could never have fired. This one goes through the function the
    /// worker calls, so the two halves of the name have to be split the way the program splits them.
    #[test]
    fn the_face_is_chosen_from_the_path_the_worker_is_given() {
        // Both separators, because a path arrives here from `Dir::target` and from a command line.
        for path in [
            "C:/src/CMakeLists.txt",
            "CMakeLists.txt",
            "deep/cmake/cmakelists.TXT",
            "C:/src/toolchain.cmake",
        ] {
            assert!(
                code_of(Path::new(path)),
                "`{path}` is a build script and wants the monospace face"
            );
        }
        let native = format!("C:{MAIN_SEPARATOR}src{MAIN_SEPARATOR}CMakeLists.txt");
        assert!(code_of(Path::new(&native)), "`{native}` on this platform");
        for path in [
            "C:/src/notes.txt",
            "C:/src/README.md",
            "C:/src/about-cmakelists.txt",
        ] {
            assert!(!code_of(Path::new(path)), "`{path}` is prose");
        }
    }

    /// Two pictures come back as three images and a number.
    ///
    /// The number is the half that a picture cannot show: "they are the same file" and "0.02% of it
    /// moved" look identical at the size a preview is drawn at.
    #[test]
    fn two_pictures_are_compared_at_the_larger_of_the_two_sizes() {
        let a = scratch("left.png");
        let b = scratch("right.png");
        // 4×2 and 6×2, so the comparison has to reach past the end of the first one.
        let mut one = image::RgbaImage::from_pixel(4, 2, image::Rgba([10, 20, 30, 255]));
        let mut other = image::RgbaImage::from_pixel(6, 2, image::Rgba([10, 20, 30, 255]));
        // One pixel differs inside the overlap, by a lot.
        other.put_pixel(1, 1, image::Rgba([200, 20, 30, 255]));
        one.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
        one.save(&a).expect("a PNG");
        other.save(&b).expect("another PNG");

        let Payload::Diff(diff) = read(&Ask::Pair(a.clone(), b.clone())) else {
            panic!("two pictures did not come back as a comparison");
        };
        assert_eq!(diff.a.pixels.size, [4, 2]);
        assert_eq!(diff.b.pixels.size, [6, 2]);
        // The mask is the larger of the two, which is what the panel places all three against.
        assert_eq!(diff.mask.pixels.size, [6, 2]);

        let at = |x: usize, y: usize| diff.mask.pixels.pixels[y * 6 + x];
        assert_eq!(at(0, 0).a(), 0, "identical pixels are transparent");
        assert_eq!(at(1, 1).a(), 255, "a large difference is opaque");
        // Present in one and not the other: as different as it gets.
        assert_eq!(at(5, 0).a(), 255, "past the end of the narrower one");
        // Five of twelve differ: the one inside the overlap, and the two columns of two that only
        // `b` reaches. Reported honestly, before the amplification the mask uses.
        assert!(
            (diff.differing - 5.0 / 12.0).abs() < 1e-6,
            "{} of the pixels were reported as differing",
            diff.differing
        );

        // A difference of one level is *visible* rather than exact: the mask is amplified so it can
        // be found, and the count above is the honest measurement.
        let faint = scratch("faint.png");
        image::RgbaImage::from_pixel(4, 2, image::Rgba([11, 20, 30, 255]))
            .save(&faint)
            .expect("a PNG");
        let Payload::Diff(diff) = read(&Ask::Pair(a.clone(), faint.clone())) else {
            panic!("not a comparison");
        };
        assert_eq!(diff.mask.pixels.pixels[0].a(), 8, "one level, amplified");
        assert!((diff.differing - 1.0).abs() < 1e-6, "every pixel differs");

        // And if either side is not a picture at all, its own complaint is what comes back.
        let lie = scratch("lie.png");
        std::fs::write(&lie, b"not a PNG").expect("a file");
        assert!(matches!(
            read(&Ask::Pair(a.clone(), lie.clone())),
            Payload::Failed(_)
        ));
        for path in [a, b, faint, lie] {
            crate::sandbox::remove_file(&path);
        }
    }

    /// A picture against the version in the last commit: the same three views, from a blob rather
    /// than from a second file.
    ///
    /// End to end through `read`, because the interesting parts are the ones a unit test of the
    /// pixels would skip: that the *committed* bytes are what `a` holds, and that a file git has
    /// nothing older of comes back as one picture rather than as a failure.
    #[test]
    fn a_picture_is_compared_with_the_one_in_the_last_commit() {
        let root = crate::sandbox::dir("imgdiff");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("a temp folder");

        let run = |args: &[&str]| {
            let mut command = std::process::Command::new("git");
            command.args(args).current_dir(&root);
            crate::shell::no_window(&mut command);
            command
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        };
        if !run(&["init", "--quiet"]) {
            eprintln!("no git: skipping");
            return;
        }
        assert!(run(&["config", "user.email", "test@example.invalid"]));
        assert!(run(&["config", "user.name", "Test"]));
        assert!(run(&["config", "commit.gpgsign", "false"]));

        // 2×1, committed grey, and then one of its two pixels turned red on disk.
        let path = root.join("mark.png");
        let before = image::RgbaImage::from_pixel(2, 1, image::Rgba([10, 20, 30, 255]));
        before.save(&path).expect("a PNG");
        assert!(run(&["add", "-A"]));
        assert!(run(&["commit", "--quiet", "-m", "the picture"]));
        let mut after = before.clone();
        after.put_pixel(1, 0, image::Rgba([200, 20, 30, 255]));
        after.save(&path).expect("a changed PNG");

        let Payload::Diff(diff) = read(&Ask::AgainstHead(path.clone())) else {
            panic!("a changed picture did not come back as a comparison");
        };
        // `a` is the commit's and `b` is the file's, which is the order the captions are written in.
        assert_eq!(diff.a.pixels.pixels[1].r(), 10, "a is what was committed");
        assert_eq!(diff.b.pixels.pixels[1].r(), 200, "b is what is on disk");
        assert_eq!(
            diff.mask.pixels.pixels[0].a(),
            0,
            "the pixel that did not move"
        );
        assert_eq!(diff.mask.pixels.pixels[1].a(), 255, "and the one that did");
        assert!((diff.differing - 0.5).abs() < 1e-6, "one pixel of two");

        // A file git has nothing older of is still a picture. Every path through here has to end in
        // one: the file is there, and somebody asked to see it.
        let fresh = root.join("fresh.png");
        before.save(&fresh).expect("a PNG");
        assert!(
            matches!(read(&Ask::AgainstHead(fresh)), Payload::Picture(_)),
            "an untracked picture should still be shown"
        );
        // And one that will not decode is its own complaint rather than a comparison's.
        let lie = root.join("lie.png");
        std::fs::write(&lie, b"not a PNG").expect("a file");
        assert!(matches!(read(&Ask::AgainstHead(lie)), Payload::Failed(_)));

        crate::sandbox::remove(&root);
    }

    #[test]
    fn a_complaint_is_cut_to_something_that_fits_in_a_panel() {
        assert_eq!(short("Format error: the header is wrong"), "Format error");
        assert_eq!(short("bad magic"), "Bad magic");
        assert_eq!(short(""), "Cannot be read");
        assert!(short(&"very long complaint ".repeat(20)).len() <= 80);
    }
}
