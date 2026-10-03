//! Reading a file as text, and deciding which text it is.
//!
//! The cap, the encoding fallback and the two substitutions are all here rather than in the
//! panel: what reaches [`super::Payload::Text`] is already something egui can lay out.

use super::*;

/// A file read as text.
pub struct Text {
    pub body: String,
    /// It is longer than [`TEXT_CAP`] and this is the front of it.
    pub truncated: bool,
    /// Its columns mean something, so it wants the monospace role. From the name, by [`is_code`] —
    /// or from `lang` below having found structure the name did not mention. See [`lang_and_face`].
    pub code: bool,
    /// What language it is in, for colour — and [`crate::syntax::Lang::Markdown`], which
    /// is how the panel knows to render the document rather than the markup.
    ///
    /// Decided here on the worker rather than in the panel, because it is the same question `code`
    /// is and is answered with it: from the name, and from the first two characters of the body
    /// where the name said nothing. See [`lang_and_face`].
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

/// Whether the front of `path` reads as text. `None` if it cannot be opened.
///
/// Two tests, and both matter. **A NUL byte** is the oldest and still the best binary tell: no
/// text encoding this would show puts one in the middle of a document, and every executable,
/// archive and database is full of them. **Valid UTF-8** over the same window catches the rest —
/// with the last few bytes forgiven, because a 4 KB window will usually cut a multi-byte
/// character in half and that is not a reason to refuse the file.
pub(super) fn sniff(path: &Path) -> Option<bool> {
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
pub(super) fn code_of(path: &Path) -> bool {
    let (stem, ext) = halves(path);
    is_code(&stem, &ext)
}

/// And what language it is in: from the same two halves, and **failing that from the body**.
///
/// Two questions in that order, because they are not equally good. A name is what the person who
/// saved the file said it was; the first two characters of the body are a guess, however narrow a
/// one — so the guess is only ever asked where the name said nothing, and can never take a `.md`
/// away from the document renderer or colour a `.rb` with a table this program does not have. See
/// [`crate::syntax::lang_of_body`], which is where the guess and its limits live.
///
/// **The answer feeds the face as well as the colour.** XML and JSON are columns that mean
/// something whatever the file is called, so a `.txt` full of XML gets the monospace role too — the
/// one case where [`is_code`]'s answer is overruled, and the only honest way round: the reason a
/// `.txt` is prose is that most of them are, and this one demonstrably is not.
fn lang_and_face(path: &Path, body: &str) -> (crate::syntax::Lang, bool) {
    let (stem, ext) = halves(path);
    let named = crate::syntax::lang_of(&stem, &ext);
    if named != crate::syntax::Lang::None {
        return (named, code_of(path));
    }
    let sniffed = crate::syntax::lang_of_body(body);
    (sniffed, sniffed != crate::syntax::Lang::None || code_of(path))
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

pub(super) fn load(path: &Path) -> Payload {
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
    // Asked of the name first and of the body only where the name said nothing — and after the two
    // substitutions above, so what is probed is the string the panel will actually lay out.
    let (lang, code) = lang_and_face(path, &body);
    Payload::Text(Text {
        body,
        truncated,
        code,
        lang,
        // Asked for after the file has been read, so a folder with no repository above it costs the
        // `stat` walk and nothing else — see [`crate::git::changes`]. An empty answer is kept as
        // `None`: "nothing changed" and "no diff to show" are the same thing to the panel.
        changes: crate::git::changes(path).filter(|changes| !changes.is_empty()),
    })
}
