//! The one place on disk a test is allowed to touch.
//!
//! `target/sandbox`, and nothing else. Not the repository, not `%TEMP%`, not `C:\Windows`, not a
//! path that arrived in an environment variable — the sandbox or a panic.
//!
//! # Why this exists
//!
//! `cargo test probe_invokable -- --ignored` reads a folder from `YAFE_PROBE`, asks the shell for
//! that folder's background menu, and **invokes** the commands it finds. Run once with `YAFE_PROBE`
//! pointing at this repository, it enumerated eighteen verbs, ran them, and permanently deleted the
//! entire working tree — sources, `Cargo.toml`, `.git`. Nothing reached the Recycle Bin.
//!
//! [`crate::shell::ops::FOR_REAL`] did not help, and could not have: it gates `IFileOperation`,
//! which is one of the several ways this program reaches the shell. A guard per choke point is not
//! a policy, so this module is the policy — one containment rule, applied at every point where a
//! test can reach the real filesystem.
//!
//! # Using it
//!
//! A test that needs somewhere to work asks for [`dir`], which is empty and inside the sandbox:
//!
//! ```ignore
//! let work = crate::sandbox::dir("copy-into-its-own-folder");
//! std::fs::write(work.join("one.txt"), b"x").unwrap();
//! ```
//!
//! If a test genuinely cannot be written against the sandbox, that is a conversation with the user
//! and not a call to [`allow_outside`] — see the note on that function.

use std::path::{Component, Path, PathBuf};

/// `target/sandbox`, created if it is not already there.
///
/// Derived from `CARGO_MANIFEST_DIR` rather than the working directory, because a test's working
/// directory is the crate root only by convention and nothing enforces it.
pub fn root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox");
    let _ = std::fs::create_dir_all(&root);
    root
}

/// A directory inside the sandbox, named for the test that wants it. Created if it is not there,
/// and **left alone if it is**.
///
/// Idempotent on purpose. Several tests reach their fixture through a one-line helper —
/// `fn scratch(name) -> PathBuf { dir("preview").join(name) }` — called once per file they want.
/// A version of this that emptied the directory deleted each fixture as the next was asked for,
/// which is a confusing way for a test to fail and was worth one round of it to learn.
///
/// When a test does want to know it is starting from nothing, that is [`fresh`].
pub fn dir(name: &str) -> PathBuf {
    let path = named(name);
    std::fs::create_dir_all(&path)
        .unwrap_or_else(|e| panic!("could not make the sandbox directory {}: {e}", path.display()));
    path
}

/// As [`dir`], but empty: whatever the last run left is removed first.
///
/// For a test that counts what is in the directory, which is otherwise a test of how many times
/// the suite has been run. The removal is [`std::fs`], never the shell — the shell is the thing
/// under test, and preparing a fixture with it is how a broken delete passes its own test.
pub fn fresh(name: &str) -> PathBuf {
    let path = named(name);
    let _ = std::fs::remove_dir_all(&path);
    dir(name)
}

/// `root()/name`, having checked that `name` is a single plain name and cannot climb anywhere.
fn named(name: &str) -> PathBuf {
    assert!(
        !name.is_empty() && !name.contains("..") && !name.contains(['/', '\\', ':']),
        "a sandbox directory is one plain name, not a path: {name:?}"
    );
    root().join(name)
}

/// Delete a tree, but only if it is inside the sandbox.
///
/// **This is the call that cost the repository its working tree**, and it is worth being exact
/// about how, because nothing about it looked dangerous. `probe_invokable` ended with
///
/// ```ignore
/// let _ = std::fs::remove_dir_all(&dir);
/// ```
///
/// to clear up the scratch folder it had made. Three lines earlier, `dir` had been reassigned from
/// `YAFE_PROBE` if that was set. Run as
/// `YAFE_PROBE='D:\Sources\MyTools\yet-another-file-explorer' cargo test probe_invokable`, the
/// cleanup deleted the repository: sources, `Cargo.toml`, `.git`. `std::fs` and not the shell, so
/// there was no Recycle Bin copy and no confirmation; `let _ =` so the failure it hit part way
/// through — `target`, whose files were open — was discarded in silence. That is why `target`
/// is all that survived.
///
/// So: no test removes anything through `std::fs` directly. It comes through here, the path is
/// checked, and a mistake is a panic instead of a loss.
pub fn remove(path: &Path) {
    guard("a test cleanup (remove_dir_all)", &[path.to_path_buf()]);
    let _ = std::fs::remove_dir_all(path);
}

/// Delete one file, but only if it is inside the sandbox. See [`remove`].
pub fn remove_file(path: &Path) {
    guard("a test cleanup (remove_file)", &[path.to_path_buf()]);
    let _ = std::fs::remove_file(path);
}

/// Delete one empty directory, but only if it is inside the sandbox. See [`remove`].
pub fn remove_dir(path: &Path) {
    guard("a test cleanup (remove_dir)", &[path.to_path_buf()]);
    let _ = std::fs::remove_dir(path);
}

/// Whether `path` is the sandbox or something inside it.
///
/// Compared component by component, so `target/sandboxes` is not inside `target/sandbox`, and after
/// [`real`] has resolved `..`, junctions and 8.3 names — the three ways a path that looks contained
/// turns out not to be.
pub fn inside(path: &Path) -> bool {
    let root = real(&root());
    let path = real(path);
    fold(&path).starts_with(fold(&root))
}

/// Panic unless every one of `paths` is inside the sandbox.
///
/// `what` names the operation, because the panic is read by somebody who has just been told a test
/// tried to touch the wrong place and needs to know which test and which call.
pub fn guard(what: &str, paths: &[PathBuf]) {
    if allowed() {
        return;
    }
    let strays: Vec<&PathBuf> = paths.iter().filter(|p| !inside(p)).collect();
    if strays.is_empty() {
        return;
    }
    panic!(
        "{what} was asked to work outside the sandbox, which is not allowed.\n\
         \n\
         outside:\n{}\n\
         the sandbox is: {}\n\
         \n\
         Point the test at `crate::sandbox::dir(\"…\")`. If it genuinely needs a real directory,\n\
         that needs the user's consent first — the reasons, and every file that could be\n\
         disrupted — not a wider guard.",
        strays
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n"),
        root().display(),
    );
}

/// Whether the containment rule is currently suspended. See [`allow_outside`].
fn allowed() -> bool {
    ALLOW_OUTSIDE.load(std::sync::atomic::Ordering::SeqCst)
}

static ALLOW_OUTSIDE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Suspend the containment rule for as long as the guard is alive.
///
/// **This is not the escape hatch for a test that is awkward to sandbox.** The user asked to be
/// asked: a test that needs a real directory needs their consent, with the reasons and the list of
/// files that could be disrupted, *before* it is written. This exists so that the guard can be
/// tested — see `the_guard_refuses_a_path_outside_the_sandbox` — and so a reviewer grepping for it
/// finds one call site and not a habit.
#[cfg(test)]
pub fn allow_outside() -> impl Drop {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            ALLOW_OUTSIDE.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    ALLOW_OUTSIDE.store(true, std::sync::atomic::Ordering::SeqCst);
    Guard
}

/// An absolute path with `.` and `..` gone and as much of it resolved as exists.
///
/// `canonicalize` alone is not enough, because it fails outright on a path that is not there yet —
/// which every path a test is about to *create* is. So the lexical cleanup happens first (a `..`
/// must not survive to be resolved against a junction), and then the deepest ancestor that does
/// exist is canonicalised and the rest put back on.
fn real(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };

    let mut cleaned = PathBuf::new();
    for part in abs.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                cleaned.pop();
            }
            Component::Prefix(p) => cleaned.push(p.as_os_str()),
            Component::RootDir => cleaned.push(std::path::MAIN_SEPARATOR_STR),
            Component::Normal(n) => cleaned.push(n),
        }
    }

    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut probe = cleaned.clone();
    loop {
        if let Ok(found) = probe.canonicalize() {
            let mut out = found;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        let Some(name) = probe.file_name().map(std::ffi::OsStr::to_os_string) else {
            return cleaned;
        };
        tail.push(name);
        if !probe.pop() {
            return cleaned;
        }
    }
}

/// The form two paths are compared in: lower-cased on Windows, where `C:\SRC` and `c:\src` are one
/// directory, and with the `\\?\` that `canonicalize` returns taken off both sides by construction.
fn fold(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    if cfg!(windows) {
        PathBuf::from(text.to_lowercase())
    } else {
        PathBuf::from(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sandbox_contains_itself_and_what_is_under_it() {
        assert!(inside(&root()), "the sandbox is not inside itself");
        assert!(inside(&root().join("a").join("b.txt")));
        assert!(inside(&dir("containment")));
    }

    #[test]
    fn the_repository_and_the_world_are_outside() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        assert!(!inside(&repo), "the repository counted as sandboxed");
        assert!(!inside(&repo.join("src").join("main.rs")));
        assert!(!inside(&repo.join("target")), "all of target/ counted as sandboxed");
        assert!(!inside(&std::env::temp_dir()), "%TEMP% counted as sandboxed");
        assert!(!inside(Path::new(r"C:\Windows")));
    }

    /// The three ways a path that reads as contained is not.
    #[test]
    fn a_path_cannot_climb_out_of_the_sandbox() {
        assert!(
            !inside(&root().join("..").join("release")),
            "`..` walked out of the sandbox and the guard did not notice"
        );
        assert!(
            !inside(&root().join("..").join("..").join("src")),
            "two `..` reached the source tree"
        );
        // A sibling whose name merely starts the same way. Component-wise, not textual.
        let sibling = root().with_file_name("sandboxes");
        assert!(
            !inside(&sibling),
            "{} counted as inside {}",
            sibling.display(),
            root().display()
        );
    }

    #[test]
    fn the_guard_passes_a_sandboxed_path() {
        guard("a test", &[dir("guard-ok").join("file.txt")]);
    }

    #[test]
    #[should_panic(expected = "was asked to work outside the sandbox")]
    fn the_guard_refuses_a_path_outside_the_sandbox() {
        guard(
            "a test",
            &[PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")],
        );
    }

    /// The suspension is honoured, and it ends with the guard.
    #[test]
    fn the_rule_can_be_suspended_only_for_a_scope() {
        let outside = [PathBuf::from(env!("CARGO_MANIFEST_DIR"))];
        {
            let _allowed = allow_outside();
            guard("a test that asked", &outside);
        }
        assert!(!allowed(), "the suspension outlived its scope");
    }

    /// [`fresh`] empties; [`dir`] does not. The second half is the one a helper called once per
    /// fixture file depends on.
    #[test]
    fn fresh_empties_and_dir_leaves_alone() {
        let first = fresh("reused");
        std::fs::write(first.join("left-over.txt"), b"x").unwrap();
        assert_eq!(
            std::fs::read_dir(dir("reused")).unwrap().count(),
            1,
            "`dir` threw away what was already there"
        );
        assert_eq!(
            std::fs::read_dir(fresh("reused")).unwrap().count(),
            0,
            "`fresh` kept the last run's files"
        );
    }
}
