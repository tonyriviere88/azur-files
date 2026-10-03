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
/// The first segment is always "This PC", so a breadcrumb is a route from the
/// machine rather than from an arbitrary root.
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
mod tests {
    use super::*;

    #[test]
    fn this_pc_is_the_top() {
        assert_eq!(parent_of(&PathBuf::new()), None);
    }

    #[test]
    #[cfg(windows)]
    fn a_path_from_outside_comes_back_in_the_form_the_shell_parses() {
        // The failure this prevents is silent: a forward-slash path lists and navigates
        // perfectly and then has no shell context menu at all, because
        // `SHParseDisplayName` will not parse it.
        assert_eq!(
            normalize(Path::new("D:/Sources/MyTools")),
            PathBuf::from("D:\\Sources\\MyTools")
        );
        assert_eq!(
            normalize(Path::new("//server/share/x")),
            PathBuf::from("\\\\server\\share\\x")
        );
        // Already right: returned unchanged, allocation and all.
        let plain = Path::new("C:\\Users\\tony");
        assert_eq!(normalize(plain), plain);
    }

    #[test]
    #[cfg(windows)]
    fn a_typed_path_is_normalised_on_the_way_in() {
        let temp = std::env::temp_dir();
        let slashed = temp.to_string_lossy().replace('\\', "/");
        let resolved = resolve_input(&slashed).expect("the temp directory exists");
        assert!(
            !resolved.to_string_lossy().contains('/'),
            "`{}` still has a forward slash in it",
            resolved.display()
        );
    }

    /// The completion's half of the door: the same expansion and the same slashes, and **no
    /// question asked of the disk** — which is the whole reason it exists beside `resolve_input`
    /// rather than being it.
    #[test]
    #[cfg(windows)]
    fn a_half_typed_path_expands_without_touching_the_disk() {
        // Nothing names a folder yet.
        assert_eq!(typed_folder(""), None);
        assert_eq!(typed_folder("   "), None);

        // A folder that is certainly not there still comes back, because whether it is there is
        // not this function's question. `resolve_input` gives `None` for the same text.
        let missing = r"Z:\definitely-not-here\9d3f\";
        assert_eq!(typed_folder(missing), Some(PathBuf::from(missing)));
        assert_eq!(resolve_input(missing), None);

        // Variables and `~` are expanded, and the slashes are the shell's.
        let home = std::env::var_os("USERPROFILE").expect("Windows always sets USERPROFILE");
        assert_eq!(typed_folder("~/"), Some(PathBuf::from(format!("{}\\", home.to_string_lossy()))));
        let expanded = typed_folder(r"%USERPROFILE%\Doc").expect("the variable is set");
        assert!(
            !expanded.to_string_lossy().contains('%'),
            "`{}` still has the variable in it",
            expanded.display()
        );
        assert_eq!(
            typed_folder("D:/Sources/"),
            Some(PathBuf::from("D:\\Sources\\")),
            "a forward slash has to be rewritten here too, or the shell would not parse it"
        );

        // A pasted path arrives in quotes.
        assert_eq!(typed_folder("\"C:\\Program Files\\"), Some(PathBuf::from("C:\\Program Files\\")));
    }

    #[test]
    #[cfg(windows)]
    fn a_drive_root_goes_up_to_this_pc() {
        assert_eq!(
            parent_of(Path::new("C:\\")),
            Some(PathBuf::new()),
            "Up from a drive root has to reach This PC, not stop"
        );
        assert_eq!(
            parent_of(Path::new("C:\\Users\\tony")),
            Some(PathBuf::from("C:\\Users"))
        );
    }

    #[test]
    #[cfg(windows)]
    fn breadcrumbs_start_at_this_pc_and_keep_the_drive() {
        let crumbs = breadcrumb_segments(Path::new("C:\\Users\\tony\\Documents"));
        let labels: Vec<&str> = crumbs.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["This PC", "C:", "Users", "tony", "Documents"]);
        assert_eq!(crumbs[1].1, PathBuf::from("C:\\"));
        assert_eq!(crumbs[4].1, PathBuf::from("C:\\Users\\tony\\Documents"));
    }

    #[test]
    fn breadcrumbs_of_this_pc_are_just_this_pc() {
        assert_eq!(breadcrumb_segments(&PathBuf::new()).len(), 1);
    }

    #[test]
    fn unknown_variables_survive_expansion() {
        assert_eq!(expand("%NOT_A_REAL_VAR_XYZ%\\x"), "%NOT_A_REAL_VAR_XYZ%\\x");
        assert_eq!(expand("plain"), "plain");
        assert_eq!(expand("50% done"), "50% done");
    }

    #[test]
    fn resolve_understands_this_pc_and_bare_drives() {
        assert_eq!(resolve_input("  This PC "), Some(PathBuf::new()));
        assert_eq!(resolve_input(""), None);
        #[cfg(windows)]
        assert_eq!(resolve_input("C:"), Some(PathBuf::from("C:\\")));
    }
}
