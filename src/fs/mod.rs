//! The filesystem layer: everything that touches the disk, and nothing that
//! touches the screen.
//!
//! The whole layer is built around one rule — **enumerate once, and never go back
//! to the disk for something the enumeration already told you.** A directory read
//! hands over a name, a size, a timestamp and an attribute word per entry; the
//! moment any code asks `Path::is_dir` or `metadata()` on top of that, it has turned
//! a single sequential read into one round trip per file. That is not a small
//! difference: [`scan`]'s benchmark measures it at 202× on a folder of 60,000.
//!
//! | module | responsibility |
//! | --- | --- |
//! | [`dir`] | the listing: one string of names, one vector of 32-byte records |
//! | [`scan`] | reading a directory as fast as the platform allows |
//! | [`sort`] | ordering and filtering, over indices rather than entries |
//! | [`fmt`] | sizes, dates and type names, written into a reused buffer |
//! | [`keywords`] | keywords on any file, kept beside the settings and keyed by file ID |
//! | [`time`] | `FILETIME` to local civil time without a syscall per row |
//! | [`drives`] | mounted volumes, and the synthetic "This PC" |
//! | [`places`] | the shell's known folders |
//! | [`recycle`] | the Recycle Bin, read as a listing |
//! | [`shell`] | handing a path back to the operating system |

pub mod dir;
pub mod drives;
pub mod fmt;
pub mod keywords;
pub mod places;
pub mod recycle;
pub mod scan;
pub mod shell;
pub mod sort;
pub mod time;

pub use dir::{display_name, Dir};
pub use sort::Column;

use std::path::{Path, PathBuf};

/// The parent of a path, in navigation terms.
///
/// Differs from [`Path::parent`] at the two ends of the tree: a drive root's
/// parent is "This PC" (the empty path) rather than nothing, and "This PC" has no
/// parent at all. Without that, Up stops working one level too early.
///
/// **And it differs in the middle, for a UNC path**, because `Path` does not model a machine as a
/// level. Measured, on Windows:
///
/// | path | `Path::parent` | here |
/// | --- | --- | --- |
/// | `\\fileserver\web` | `None` — the whole thing is one *prefix* | `\\fileserver` |
/// | `\\fileserver` | `\` — a bare root, and nowhere at all | This PC |
///
/// The second row was a real fault rather than a tidying: Up from a machine navigated to `\`.
/// The first makes the machine the level it is now — see [`drives::server_dir`], which lists one —
/// so Up from a share goes to the server that offers it, and Up again to This PC.
pub fn parent_of(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    // Explorer puts the bin on the desktop, beside This PC rather than inside it. This program has
    // no desktop level, and This PC is where every other walk up ends.
    if recycle::is_bin(path) {
        return Some(PathBuf::new());
    }
    // A machine. Above it is only the list of machines, which is This PC.
    if drives::unc_server(path).is_some() {
        return Some(PathBuf::new());
    }
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Some(parent.to_path_buf()),
        // A root. `C:\` goes to This PC; `\\server\share` goes to the machine, which is a place
        // of its own — and is why this asks for the share's *own* server rather than reusing the
        // `None` above, which `Path` gives for both.
        _ => Some(match drives::split_unc(path) {
            Some((host, _)) if !host.contains(['\\', '/']) => PathBuf::from(format!("\\\\{host}")),
            _ => PathBuf::new(),
        }),
    }
}

/// A path that came from outside this program, in the form the *shell* also understands.
///
/// Every file API on Windows accepts a forward slash, so `D:/Sources` scans, navigates,
/// draws a correct breadcrumb and looks entirely fine — and then `SHParseDisplayName`
/// refuses it, `IContextMenu` is never obtained, and the right-click menu comes up with
/// this program's own entries and none of the shell's. Nothing reports an error; the menu
/// is just half a menu.
///
/// So a path is rewritten once, at each door it can come in by: the command line and the
/// path bar. Inside, every path is built by joining onto one of those.
pub fn normalize(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if text.contains('/') {
            // A forward slash cannot be part of a Windows file name, so this can only be
            // a separator — including inside a `\\?\` or UNC prefix.
            return PathBuf::from(text.replace('/', "\\"));
        }
    }
    path.to_path_buf()
}

/// Whether a path is a UNC path — `\\server`, `\\server\share`, and everything under it.
///
/// Named rather than spelled out where it is used, because it is load-bearing in two places that
/// must not drift: [`resolve_input`], which hands a network path to the loader instead of asking the
/// disk about it, and [`scan::wants_credentials`], which will only offer a sign-in for one.
///
/// **Backslashes only, and the callers see to that.** Every path in this program has been through
/// [`normalize`] at the door, so `//server/share` is already `\\server\share` by the time anything
/// asks — the same guarantee [`drives::unc_server`] documents for itself.
pub fn is_unc(path: &Path) -> bool {
    path.to_string_lossy().starts_with(r"\\")
}

/// Whether some text is one of the shell's own names rather than a path: `shell:Downloads`,
/// `shell:RecycleBinFolder`, `::{20D04FE0-…}`.
///
/// Case-insensitive, because the shell reads them that way — `Shell:AppData` is `shell:AppData` to
/// the Run box, and a test that only knew the lower-case spelling handed the other one to Explorer
/// as a file to select. One function for every place that has to tell the two apart: resolving what
/// was typed, revealing it, and asking for its icon.
pub fn is_shell_name(text: &str) -> bool {
    text.get(..6)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("shell:"))
        || text.starts_with("::{")
}

/// Whether a pane at this path is showing a listing this program **assembles** rather than a
/// directory it reads: This PC, or the Recycle Bin.
///
/// The question behind every "there is no folder here" refusal — nothing can be created, pasted or
/// measured in either, nothing flattened, watched, asked of git or opened in a terminal — and the
/// reason it is one function: those checks were written against This PC alone, as an empty path,
/// and a second synthetic listing spelled at each of them is a second list to keep in step.
pub fn is_synthetic(path: &Path) -> bool {
    path.as_os_str().is_empty() || recycle::is_bin(path)
}

/// What a typed path turned out to name.
///
/// Two things happen to a path out of the bar — a folder is opened in the pane and a file is opened
/// with its program — and **which one is decided here rather than by the caller asking again.** It
/// used to ask: `resolve_input` called `is_dir` and then `is_file`, handed back a bare path, and the
/// breadcrumb called `is_file` on it a third time. Three stats of one path, each of them a syscall
/// that can block for twenty-two seconds on a share that has gone away, all inside one frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Typed {
    /// Somewhere to navigate to. Also what a UNC path that would not answer comes back as — see
    /// [`resolve_input`].
    Folder(PathBuf),
    /// A file, to be opened with whatever opens it.
    File(PathBuf),
    /// Somewhere only the shell can show — `shell:ControlPanelFolder`, `::{…}` for Network. Handed
    /// to Explorer, as the text that named it. See [`places::ShellPlace::Elsewhere`].
    Elsewhere(PathBuf),
}

/// Resolve what the user typed in the breadcrumb into somewhere to go.
///
/// Expands `%VARS%` and `~`, accepts either slash, and tolerates a trailing one — and takes the
/// names Explorer's own bar takes, which are not paths at all:
///
/// | typed | goes to |
/// | --- | --- |
/// | `%AppData%`, `%localappdata%\Temp`, `~\src` | the folder, expanded — any case, since Windows' variables have none |
/// | `Downloads`, `recycle bin`, `This PC` | the place the sidebar shows under that name. See [`places::named`] |
/// | `shell:Downloads`, `shell:startup\sub`, `::{…}` | what the shell says the name is. See [`places::shell_place`] |
/// | `shell:RecycleBinFolder`, `D:\$Recycle.Bin` | the Recycle Bin, listed here. See [`recycle::canonical`] |
/// | `shell:ControlPanelFolder` | Explorer, since only it can show that — [`Typed::Elsewhere`] |
/// | `file:///C:/My%20Files` | `C:\My Files`, as pasted out of a browser |
///
/// `None` means there is nothing there and nothing to do, and the caller leaves the text in the
/// field to be corrected.
///
/// # A UNC path is never answered `None`
///
/// `Path::is_dir` hands back a **bool**, so *there is no such share* and *the server will not say
/// who is asking* arrive as the same `false` — and only one of them is a reason to stay put. This
/// used to answer `None` for both, and the caller's `None` arm reopens the field: **typing a network
/// path did nothing whatever, and the sign-in the failure should have raised was never reached,
/// because nothing had yet asked a question that could fail.** See [`scan::wants_credentials`] for
/// the error code that made that the *normal* case on a domain-joined machine rather than a corner.
///
/// So a UNC path is handed on regardless — [`is_unc`] is the test. Whether it is really there is
/// [`crate::loader`]'s question, asked on a worker, where a refusal becomes a listing that says why
/// and a dead server costs that worker rather than the window: the same argument [`typed_folder`] is
/// built on, applied to the one press that was still asking. It fixes a bare `\\machine` too, which
/// had never worked from the bar, since a server is not a file and `is_dir` is false for every one
/// of them. A mistyped share now navigates and the pane says *The network path was not found*, which
/// is what Explorer does and better than a field that sits there.
pub fn resolve_input(text: &str) -> Option<Typed> {
    let text = text.trim().trim_matches('"');
    if text.is_empty() {
        return None;
    }
    // A place by name. This PC is one of them, and so is the Recycle Bin.
    if let Some(place) = places::named(text) {
        return Some(Typed::Folder(place));
    }

    let expanded = match resolve_names(text) {
        Expanded::Path(expanded) => expanded,
        Expanded::ThisPc => return Some(Typed::Folder(PathBuf::new())),
        Expanded::Elsewhere => return Some(Typed::Elsewhere(PathBuf::from(text))),
    };

    // A drive letter on its own means its root: `D:` is where `D:\` is.
    if expanded.len() == 2 && expanded.as_bytes()[1] == b':' {
        return Some(Typed::Folder(PathBuf::from(format!("{expanded}\\"))));
    }

    // Normalised *before* anything is asked about it, so the UNC test below sees `\\server`
    // whichever slash was typed — `//server/share` names the same place.
    let path = recycle::canonical(normalize(&PathBuf::from(&expanded)));
    // Not a directory, and not a question for the disk.
    if recycle::is_bin(&path) {
        return Some(Typed::Folder(path));
    }

    // **One `metadata`, where this used to be `is_dir` and then `is_file`.** Those are two stats of
    // the same path, on the UI thread, inside a frame — and each of them throws the error away,
    // which is exactly what has to survive here.
    match std::fs::metadata(&path) {
        Ok(found) if found.is_dir() => Some(Typed::Folder(path)),
        // A file: the caller opens it.
        Ok(_) => Some(Typed::File(path)),
        // Could not be asked, or was refused. For a UNC path that is not an answer — see above.
        Err(_) if is_unc(&path) => Some(Typed::Folder(path)),
        Err(_) => None,
    }
}

/// The folder a half-typed path names, worked out **without asking the disk anything**.
///
/// [`resolve_input`] stops one step further on than this, and that step is the whole difference:
/// it answers "is this somewhere to go", which it can only do by calling [`Path::is_dir`] — and
/// `is_dir` against a mapped drive whose share has gone away blocks for as long as SMB takes to
/// give up, which [`drives`] measures at **22 seconds**. Once per `Enter`, that is a cost somebody
/// asked for. Once per *keystroke*, which is what the path bar's completion would make of it, it is
/// a window that stops redrawing while you type.
///
/// So this expands `%VARS%` and `~`, rewrites the slashes, and stops. Whether the answer is a
/// folder at all is [`crate::loader`]'s question, asked on a worker where a dead share costs
/// nothing but that worker.
///
/// `None` when nothing has been typed yet — there is no folder in `Doc`, only a name that might
/// still become one.
pub fn typed_folder(prefix: &str) -> Option<PathBuf> {
    let prefix = prefix.trim_start().trim_start_matches('"');
    if prefix.is_empty() {
        return None;
    }
    Some(match resolve_names(prefix) {
        Expanded::Path(expanded) => recycle::canonical(normalize(Path::new(&expanded))),
        Expanded::ThisPc => PathBuf::new(),
        // Nothing this program can list; handed on as typed, where the loader will say so.
        Expanded::Elsewhere => PathBuf::from(prefix),
    })
}

/// A path from outside this program — `--open=` — resolved the way the path bar resolves one, and
/// **without asking the disk**, since it is read before there is a window to wait in.
///
/// `%VARS%`, `~`, `shell:` names, `file:` URLs and either slash, as [`typed_folder`] takes them. A
/// name only the shell can show has nowhere to go in a pane, and comes back as typed.
pub fn from_outside(text: &str) -> PathBuf {
    if let Some(place) = places::named(text.trim()) {
        return place;
    }
    typed_folder(text).unwrap_or_default()
}

/// What [`resolve_names`] made of some text.
enum Expanded {
    /// A path, with its variables and names written out.
    Path(String),
    /// A shell name for This PC, which is the empty path — and so not something a string of path
    /// text can carry, since an empty one is also "nothing typed yet".
    ThisPc,
    /// A shell name for somewhere that is not a folder. See [`Typed::Elsewhere`].
    Elsewhere,
}

/// `%APPDATA%`-style variables, a leading `~`, a leading `shell:` name and a `file:` URL.
///
/// A `shell:` name is resolved as the first component only — `shell:Downloads\sub\deeper` is the
/// Downloads folder with `sub\deeper` joined on — which is how the Run box reads one too, and what
/// lets the path bar's completion carry on typing below one.
fn resolve_names(text: &str) -> Expanded {
    if let Some(path) = from_file_url(text) {
        return Expanded::Path(path);
    }
    if is_shell_name(text) {
        let cut = text.find(['\\', '/']).unwrap_or(text.len());
        let (name, rest) = text.split_at(cut);
        match places::shell_place(name) {
            Some(places::ShellPlace::Folder(folder)) => {
                let mut out = folder.to_string_lossy().into_owned();
                let rest = rest.trim_start_matches(['\\', '/']);
                if !rest.is_empty() {
                    if !out.ends_with('\\') {
                        out.push('\\');
                    }
                    out.push_str(rest);
                } else if rest.len() < text.len() - cut {
                    // A trailing separator, kept: the completion reads it as "offer what is in here".
                    out.push('\\');
                }
                return Expanded::Path(out);
            }
            Some(places::ShellPlace::RecycleBin) => {
                return Expanded::Path(recycle::LOCATION.to_owned());
            }
            Some(places::ShellPlace::ThisPc) => return Expanded::ThisPc,
            Some(places::ShellPlace::Elsewhere) => return Expanded::Elsewhere,
            // Not a name the shell knows. Left as it is, so it fails as a path would.
            None => {}
        }
    }
    Expanded::Path(expand(text))
}

/// `file:///C:/My%20Files/a.txt` as `C:/My Files/a.txt`, and `file://server/share` as
/// `//server/share` — the slashes are [`normalize`]'s to turn round.
///
/// What a browser's address bar, a Markdown link or an editor's "copy as URL" hands over, and a
/// path in every sense but its spelling. Percent-escapes are decoded as UTF-8, which is what a URL
/// is; one that is not valid UTF-8 is left as it was rather than guessed at.
fn from_file_url(text: &str) -> Option<String> {
    let rest = text
        .get(..5)
        .filter(|scheme| scheme.eq_ignore_ascii_case("file:"))
        .map(|_| &text[5..])?;
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    // `file:///C:/x` has an empty host and a path beginning `/C:`; `file://server/share` has one.
    let path = match rest.strip_prefix('/') {
        Some(local) => local.to_owned(),
        None => format!("//{rest}"),
    };
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[at], bytes.get(at + 1).copied().and_then(hex), bytes.get(at + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => {
                decoded.push((high * 16 + low) as u8);
                at += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    Some(String::from_utf8(decoded).unwrap_or(path))
}

/// `%APPDATA%`-style variables and a leading `~`.
///
/// The names are Windows', so they have no case: `%appdata%`, `%AppData%` and `%APPDATA%` are
/// one variable, which is what `std::env::var_os` asks the environment block for.
fn expand(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    let text = if let Some(rest) = text.strip_prefix('~') {
        if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
            out.push_str(&home.to_string_lossy());
        }
        rest
    } else {
        text
    };

    let mut rest = text;
    while let Some(open) = rest.find('%') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('%') {
            Some(close) => {
                let name = &after[..close];
                match std::env::var_os(name) {
                    Some(value) => out.push_str(&value.to_string_lossy()),
                    // Leave an unknown variable as it was written, so the user can
                    // see what did not resolve instead of watching it vanish.
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push('%');
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Split a path into the segments a breadcrumb shows, each with the path that
/// reaching it navigates to.
///
/// The first segment is always "This PC", so this is a route from the machine rather than from
/// an arbitrary root — which is what makes it the walk [`crate::pane::Tab::go_to`] asks for the
/// child to reveal on arrival, where a raw component walk has nothing above `C:`.
///
/// The bar itself does not *draw* that first segment unless it is the folder on show; see
/// [`crate::ui::breadcrumb::segments`].
pub fn breadcrumb_segments(path: &Path) -> Vec<(String, PathBuf)> {
    let mut out = vec![("This PC".to_owned(), PathBuf::new())];
    if path.as_os_str().is_empty() {
        return out;
    }
    // One level below This PC, as [`parent_of`] has it — and not a path to walk, since its one
    // component is a name the disk has never heard of.
    if recycle::is_bin(path) {
        out.push((display_name(path), path.to_path_buf()));
        return out;
    }

    // **A UNC path is walked by hand, because `Path::components` does not model a machine.**
    // Measured on Windows: `\\fileserver\web` arrives as a single `Prefix(UNC)` component — the
    // server and the share welded into one — and `\\fileserver` as a bare `RootDir` plus a
    // `Normal`, which put a segment labelled `\` on the bar leading nowhere.
    //
    // Split on the separator instead, which gives the levels [`parent_of`] walks: the machine,
    // the share on it, then the folders. Every segment is somewhere to go — the machine lists its
    // shares (see [`drives::server_dir`]) and the share root lists its contents.
    if let Some(inner) = path.to_string_lossy().strip_prefix("\\\\") {
        let mut walked = String::from("\\\\");
        for (index, part) in inner.split('\\').filter(|p| !p.is_empty()).enumerate() {
            if index > 0 {
                walked.push('\\');
            }
            walked.push_str(part);
            out.push((part.to_owned(), PathBuf::from(&walked)));
        }
        return out;
    }

    let mut walked = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::Prefix(prefix) => {
                walked.push(prefix.as_os_str());
                // A prefix on its own is not a directory: `C:` means "the current
                // directory on C:". The root component that follows completes it,
                // so the label is deferred until then.
            }
            Component::RootDir => {
                walked.push(std::path::MAIN_SEPARATOR_STR);
                out.push((display_name(&walked), walked.clone()));
            }
            Component::Normal(name) => {
                walked.push(name);
                out.push((name.to_string_lossy().into_owned(), walked.clone()));
            }
            // `.` and `..` cannot appear: every path here has been resolved.
            Component::CurDir | Component::ParentDir => {}
        }
    }
    out
}

/// Why Windows will not have this as the name of a file, or `None` if it will.
///
/// **The rename box had no validation at all**, and the one that matters is the separator: a name
/// with a `\` or a `/` in it is not a name, it is a path, and `F2` then `..\report.txt` asked the
/// shell to rename a file to somewhere else entirely. `IFileOperation::RenameItem` is documented to
/// refuse that — "you cannot use this method to move an item to a different location" — so what was
/// actually shipping was very likely a refusal with a raw `HRESULT` in the status line rather than a
/// moved file. That is not a thing to leave resting on somebody else's contract when the check is
/// this cheap, and the codebase already names the hazard: the flattened listing seeds the field with
/// `Dir::leaf` rather than the entry's name precisely so that accepting it unchanged cannot ask for
/// this.
///
/// The rest are the rules Explorer's own rename box enforces, and each is refused here for a reason
/// beyond tidiness:
///
/// | | |
/// | --- | --- |
/// | `< > : " \| ? *` | the shell answers with an `HRESULT` and no explanation of which character |
/// | a control character | the same, and invisible in the field |
/// | ending in a dot or a space | **Win32 strips it**, so `a.txt.` is a rename to `a.txt` — a silent no-op, or a rename *onto* the `a.txt` already there |
/// | `.` or `..` | not names at all |
/// | `CON`, `NUL`, `COM1`, and the same with any extension | reserved for devices; the shell refuses, having first looked like it might not |
/// | over 255 characters | past what NTFS holds in a directory entry |
///
/// Returned as the sentence to show rather than as a bool, because a rename that is refused with no
/// reason reads as a rename that is broken.
pub fn why_not_a_name(name: &str) -> Option<String> {
    // The separator first, so the message names the actual mistake rather than listing characters.
    if name.contains('\\') || name.contains('/') {
        return Some("A name cannot contain \\ or /".to_owned());
    }
    if name == "." || name == ".." {
        return Some(format!("`{name}` is not a name"));
    }
    if let Some(bad) = name.chars().find(|c| "<>:\"|?*".contains(*c)) {
        return Some(format!("A name cannot contain {bad}"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Some("A name cannot contain control characters".to_owned());
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("A name cannot end in a dot or a space".to_owned());
    }
    // Counted in UTF-16 units, which is what the filesystem's 255 is measured in — an emoji is two
    // of them and a `char` is one.
    if name.encode_utf16().count() > 255 {
        return Some("That name is too long".to_owned());
    }
    // The device names, which are reserved with *and* without an extension: `CON.txt` is as
    // refused as `CON`. Compared against the stem, case-insensitively, which is how Windows
    // compares them.
    const DEVICES: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or(name);
    if let Some(device) = DEVICES
        .iter()
        .find(|device| stem.eq_ignore_ascii_case(device))
    {
        return Some(format!("`{device}` is a name Windows reserves"));
    }
    None
}

#[cfg(test)]
mod tests;
