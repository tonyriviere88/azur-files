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
//!
//! # One file per kind of file
//!
//! What a preview *is* differs by content type and nothing else, so the reading is split the same
//! way. This module keeps only what every kind shares — the classifier, the caps, the request
//! service — and each kind's decoder is its own file:
//!
//! | module | what it reads |
//! | --- | --- |
//! | [`text`] | anything with lines in it, up to [`TEXT_CAP`] |
//! | [`picture`] | raster art, through `image`, bounded by [`CAP`] |
//! | [`vector`] | `.svg`, rasterised by `resvg` at the size asked for |
//! | [`diff`] | two pictures, or one against `HEAD` |
//! | [`search`] | the find bar's walk over a text body |

use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub mod diff;
pub mod picture;
pub mod search;
pub mod text;
pub mod vector;

// The names the rest of the program knows this module by. Splitting the reading up by content
// type is an arrangement of *this* module's insides; nothing outside it should have to learn
// which file a `Picture` now lives in.
pub use diff::Diff;
pub use picture::Picture;
pub use search::{hits, Search};
pub use text::Text;
// And the two [`crate::shell::thumbs`] needs, for the one file type it draws itself instead of
// handing to the shell — see that module's header. Named here for the reason just above: which file
// the SVG decoder lives in is this module's business, and these were the first two names in the
// program to reach past this line for it.
pub(crate) use picture::is_vector;
pub(crate) use vector::art as vector_art;

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

// ---------------------------------------------------------------------------
// Searching the text on show
// ---------------------------------------------------------------------------

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
        Ask::Pair(a, b) => return diff::compare(a, b),
        Ask::AgainstHead(path) => return diff::against_head(path),
    };
    match kind {
        Kind::Picture => picture::load(path),
        Kind::Text => text::load(path),
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
        Kind::Unknown => match text::sniff(path) {
            Some(true) => text::load(path),
            Some(false) => Payload::Failed("Not something this can show".to_owned()),
            None => Payload::Failed("Cannot be read".to_owned()),
        },
    }
}

/// The extension, lowercased, for the panel's "no preview for this" line.
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
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
mod tests;
