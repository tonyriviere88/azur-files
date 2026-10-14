//! Is a thumbnail provider registered for this file type?
//!
//! The Windows half of [`crate::shell::providers`]. A handful of registry reads and no COM: nothing
//! here loads or runs anybody's DLL, which is the whole reason it is safe to ask on the UI thread.
//!
//! Nothing is taken from the parent module, unlike every other file in here: a key path is a string
//! and a registry handle is its own thing, so this needs neither `wide` nor a `Path` — only
//! [`crate::shell::registry`], which opens and closes the keys.

#[cfg(windows)]
use crate::shell::registry::Key;

/// `IThumbnailProvider` — the interface a shell extension registers under when it can draw a
/// picture of a file.
///
/// Written out rather than taken from the `windows` crate's constants, because it is used here as a
/// **registry key name** and not as an interface id: this string is what appears under `ShellEx`, and
/// formatting a `GUID` back into it would be a conversion in order to compare against the text it
/// came from.
const THUMBNAIL_PROVIDER: &str = "{E357FCCD-A995-4576-B01F-234630154E96}";

/// `IExtractImage` — the *older* way to register the same thing, and Windows still honours it.
///
/// Checked beside the one above rather than instead of it, because a shell extension written before
/// Vista uses this one and Explorer draws its thumbnails perfectly happily. Leaving it out would make
/// the probe answer "no" for a type whose pictures are visible in Explorer's own window — a false
/// negative in exactly the place somebody turns the probe on to investigate.
const EXTRACT_IMAGE: &str = "{BB2E617C-0920-11D1-9A0B-00C04FC2D6C1}";

/// Whether either interface is registered under `base` — the one thing asked at each of the four
/// places a registration can live.
#[cfg(windows)]
fn under(base: &str) -> bool {
    exists(&format!("{base}\\ShellEx\\{THUMBNAIL_PROVIDER}"))
        || exists(&format!("{base}\\ShellEx\\{EXTRACT_IMAGE}"))
}

/// Whether a key exists under `HKEY_CLASSES_ROOT`. Opened for reading and closed at once — the
/// question is only whether it is there.
#[cfg(windows)]
fn exists(path: &str) -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CLASSES_ROOT, KEY_READ};

    Key::open(HKEY_CLASSES_ROOT, path, KEY_READ).is_ok()
}

/// A string value under `HKEY_CLASSES_ROOT\<path>`: the key's own default when `name` is `None`, and
/// a named one otherwise.
///
/// `None` for a key that is not there, a value that is not a string, and an empty one — all three
/// mean the same thing to every caller here.
#[cfg(windows)]
fn value(path: &str, name: Option<&str>) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CLASSES_ROOT, KEY_READ, REG_SZ};

    let key = Key::open(HKEY_CLASSES_ROOT, path, KEY_READ).ok()?;
    // Everything read here is a ProgID or a perceived type — short identifiers. Anything that does
    // not fit is not one, and the fixed buffer is what keeps a probe allocation-light.
    let (kind, text) = key.string::<260>(name)?;
    if kind != REG_SZ {
        return None;
    }
    (!text.is_empty()).then_some(text)
}

/// Whether `ext` — without the dot, as [`crate::fs::Dir::ext`] stores it — has a thumbnail provider.
///
/// # The four places it can be registered, and why all four are read
///
/// Windows resolves a provider through the association chain, and an extension can join that chain
/// at any point in it. Reading only the first place would answer `false` for most of the types
/// anybody actually wants:
///
/// | key | who registers there |
/// | --- | --- |
/// | `.ext\ShellEx\{…}` | an installer claiming the extension outright |
/// | `<ProgID>\ShellEx\{…}` | the usual one — `.pdf` → `AcroExch.Document.DC` → the provider |
/// | `SystemFileAssociations\.ext\ShellEx\{…}` | a provider that does *not* want to own the type, which is where Windows puts its own |
/// | `SystemFileAssociations\<PerceivedType>\ShellEx\{…}` | one registration covering a whole family — `image`, `video` |
///
/// And at each of the four, either of [`THUMBNAIL_PROVIDER`] and [`EXTRACT_IMAGE`] — see the second of
/// those for why the older interface is not merely history.
///
/// `HKEY_CLASSES_ROOT` rather than `HKLM` and `HKCU` separately: it is the merged view of the two
/// with the user's own registrations winning, which is the order the shell itself resolves in. The
/// `.svg` case in [`crate::shell::thumbs`]' header is a live example of a per-user one.
///
/// # What a `true` here does and does not promise
///
/// It says **a provider is registered for this type** — which is what decides whether Explorer's own
/// views show a picture of the file rather than its icon, and so is the question
/// [`crate::pane::AutoTiles`] wants. It does not promise the provider will succeed on a *particular*
/// file: a truncated `.pdf` still has Acrobat's provider registered against it. The only call that
/// can answer that is `GetImage` on the file itself — 12–22 ms per file when the answer is no, and it
/// runs a stranger's code in this process. See [`crate::shell::providers`], where that trade is made.
#[cfg(windows)]
pub fn registered(ext: &str) -> bool {
    if ext.is_empty() {
        return false;
    }
    // An extension arriving from a file name can hold anything a file name can. A separator or a NUL
    // in it would make the paths below name something else entirely, so it is refused rather than
    // escaped: there is no such extension to be right about.
    let sane = |text: &str| !text.is_empty() && !text.contains(['\\', '/', '\0']);
    if !sane(ext) {
        return false;
    }

    if under(&format!(".{ext}")) {
        return true;
    }
    if let Some(prog_id) = value(&format!(".{ext}"), None) {
        if sane(&prog_id) && under(&prog_id) {
            return true;
        }
    }
    if under(&format!("SystemFileAssociations\\.{ext}")) {
        return true;
    }
    // And the family the extension says it belongs to, which is one registration covering every type
    // in it. `PerceivedType` is a word — `image`, `video`, `audio`, `text` — and it is why a machine
    // can draw a format nobody wrote a provider for by name.
    if let Some(perceived) = value(&format!(".{ext}"), Some("PerceivedType")) {
        if sane(&perceived) && under(&format!("SystemFileAssociations\\{perceived}")) {
            return true;
        }
    }
    false
}
