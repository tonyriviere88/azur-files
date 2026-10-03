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
//! | [`time`] | `FILETIME` to local civil time without a syscall per row |
//! | [`drives`] | mounted volumes, and the synthetic "This PC" |
//! | [`places`] | the shell's known folders |
//! | [`shell`] | handing a path back to the operating system |

pub mod dir;
pub mod drives;
pub mod fmt;
pub mod places;
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
pub fn parent_of(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Some(parent.to_path_buf()),
        // A root: `C:\` or `\\server\share`.
        _ => Some(PathBuf::new()),
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

/// Resolve what the user typed in the breadcrumb into somewhere to go.
///
/// Expands `%VARS%` and `~`, accepts either slash, and tolerates a trailing one.
/// Returns `None` if there is nothing there — the caller shows that as a message
/// rather than navigating into a void.
pub fn resolve_input(text: &str) -> Option<PathBuf> {
    let text = text.trim().trim_matches('"');
    if text.is_empty() {
        return None;
    }
    if text.eq_ignore_ascii_case("this pc") {
        return Some(PathBuf::new());
    }

    let expanded = expand(text);
    let path = PathBuf::from(&expanded);

    // A drive letter on its own means its root: `D:` is where `D:\` is.
    if expanded.len() == 2 && expanded.as_bytes()[1] == b':' {
        return Some(PathBuf::from(format!("{expanded}\\")));
    }

    if path.is_dir() {
        return Some(normalize(&path));
    }
    // A file: go to its folder and let the caller select it.
    if path.is_file() {
        return Some(normalize(&path));
    }
    None
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
    Some(normalize(Path::new(&expand(prefix))))
}

/// `%APPDATA%`-style variables and a leading `~`.
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

#[cfg(test)]
mod tests;
