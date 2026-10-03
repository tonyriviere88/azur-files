use super::*;

/// Every sample here is real output, captured from a repository built to have one of each — a
/// staged add, a staged-and-modified file, a deletion, a rename, an untracked file, an untracked
/// directory, a change in a subfolder and a change further down. The NULs are written as `\0`.
const REAL: &str = concat!(
    "# branch.oid ab41008277b4bce1dd164b4c7e49073939048bf2\0",
    "# branch.head main\0",
    "# branch.upstream origin/main\0",
    "# branch.ab +2 -1\0",
    "1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 2ce75e2a24f7d6841a504cf3616ef5a59edb3a2d added.txt\0",
    "1 .M N... 100644 100644 100644 fe7900bcbd294970da3296db5cf2020b4391a639 fe7900bcbd294970da3296db5cf2020b4391a639 deep/inner/buried.txt\0",
    "1 .D N... 100644 100644 000000 2bdf67abb163a4ffb2d7f3f0880c9fe5068ce782 2bdf67abb163a4ffb2d7f3f0880c9fe5068ce782 deleted.txt\0",
    "1 MM N... 100644 100644 100644 f719efd430d52bcfc8566a43b2eb655688d38871 f642f860d1de0bbe29b4f23400c1df3625454d08 modified.txt\0",
    "2 R. N... 100644 100644 100644 5626abf0f72e58d7a153368ba57db4c673c0e171 5626abf0f72e58d7a153368ba57db4c673c0e171 R100 moved.txt\0clean.txt\0",
    "1 .M N... 100644 100644 100644 ffe2fce498955b628014618b28c6bcf152466a4a ffe2fce498955b628014618b28c6bcf152466a4a sub/modified.txt\0",
    "? brandnew/\0",
    "? untracked.txt\0",
);

fn at(prefix: &str) -> Repo {
    let mut repo = Repo::default();
    parse_status(REAL.as_bytes(), prefix, &mut repo);
    repo
}

#[test]
fn the_branch_line_is_the_status_bar() {
    let repo = at("");
    assert_eq!(repo.head, "main");
    assert!(!repo.detached);
    assert_eq!(repo.upstream.as_deref(), Some("origin/main"));
    assert_eq!((repo.ahead, repo.behind), (2, 1));
}

/// One count per path, whatever git had to say about it — and `MM` is one file, not two.
#[test]
fn every_path_is_counted_once() {
    let repo = at("");
    assert_eq!(repo.changed, 8, "six tracked paths and two untracked");
    assert_eq!(repo.staged, 3, "added, MM, and the rename");
    assert_eq!(repo.unstaged, 4, "buried, deleted, MM, sub/modified");
    assert_eq!(repo.untracked, 2);
    assert_eq!(repo.conflicted, 0);
}

/// The row's own name, from the folder that asked.
#[test]
fn a_row_wears_its_own_state() {
    let repo = at("");
    assert_eq!(repo.state("added.txt"), Some(State::Staged));
    assert_eq!(repo.state("modified.txt"), Some(State::Modified));
    assert_eq!(repo.state("deleted.txt"), Some(State::Deleted));
    assert_eq!(repo.state("moved.txt"), Some(State::Renamed));
    assert_eq!(repo.state("untracked.txt"), Some(State::Untracked));
    assert_eq!(repo.state("nothing-to-say.txt"), None);
}

/// A folder wears the strongest thing under it, however deep that is — which is the whole of what
/// a row for a folder can usefully say.
#[test]
fn a_folder_wears_the_strongest_state_beneath_it() {
    let repo = at("");
    assert_eq!(repo.state("sub"), Some(State::Modified));
    assert_eq!(repo.state("deep"), Some(State::Modified), "two levels down");
    assert_eq!(
        repo.state("brandnew"),
        Some(State::Untracked),
        "an untracked directory is one record with a slash on it"
    );
}

/// The same output read from a subfolder: the paths are still the repository's, so the folder's
/// own prefix is what turns them into rows.
#[test]
fn a_subfolder_sees_its_own_names() {
    let repo = at("sub/");
    assert_eq!(repo.state("modified.txt"), Some(State::Modified));
    assert_eq!(repo.state("sub/modified.txt"), None, "not from in here");
    assert_eq!(repo.state("added.txt"), None, "that one is upstairs");
    // Counted all the same: the branch line is the repository's, not the folder's.
    assert_eq!(repo.changed, 8);
}

/// A flattened listing's rows are paths with the platform's separators in them.
#[test]
fn a_flattened_row_finds_itself_too() {
    let repo = at("");
    assert_eq!(repo.state("deep\\inner\\buried.txt"), Some(State::Modified));
    assert_eq!(repo.state("deep\\inner"), Some(State::Modified));
}

/// A detached head has no branch name, so it wears the commit instead — and nothing about a
/// remote, because there is no branch to be ahead of one.
#[test]
fn a_detached_head_shows_its_commit() {
    let mut repo = Repo::default();
    parse_status(
        concat!(
            "# branch.oid ed036ba1dab0b7a04bcfff16806b18543fadec4c\0",
            "# branch.head (detached)\0",
        )
        .as_bytes(),
        "",
        &mut repo,
    );
    assert!(repo.detached);
    assert_eq!(repo.head, "ed036ba");
    assert_eq!(repo.upstream, None);
}

/// A conflict outranks everything, both on the file and on the folders above it.
#[test]
fn a_conflict_is_the_loudest_thing_in_a_folder() {
    let mut repo = Repo::default();
    parse_status(
        concat!(
            "1 .M N... 100644 100644 100644 aaa bbb sub/quiet.txt\0",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc sub/both.txt\0",
        )
        .as_bytes(),
        "",
        &mut repo,
    );
    assert_eq!(repo.conflicted, 1);
    assert_eq!(repo.state("sub"), Some(State::Conflicted));
    assert_eq!(repo.state("sub/quiet.txt"), Some(State::Modified));
}

/// A path with a space in it is why `-z` and a counted field split: the path is whatever is
/// left, spaces and all.
#[test]
fn a_name_with_spaces_survives() {
    let mut repo = Repo::default();
    parse_status(
        "1 .M N... 100644 100644 100644 aaa bbb my notes v2.txt\0".as_bytes(),
        "",
        &mut repo,
    );
    assert_eq!(repo.state("my notes v2.txt"), Some(State::Modified));
}

/// **The real thing, against a repository this test builds.**
///
/// Everything above reads captured output; this reads git. It is what catches an argument list
/// that has gone stale, a `git` that is not on the `PATH`, and the two commands whose output is
/// *not* checked in above — `rev-parse --show-prefix`, which decides what every path means, and
/// `ls-tree`, which is where a committed file's tick comes from.
///
/// Read-only, on a repository in the temp directory, with no remote and no network. The `git`
/// invoked is whatever the developer has, which is the point.
#[test]
fn a_real_repository_answers_for_its_own_files() {
    let root = crate::sandbox::dir("git");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(root.join("sub")).expect("a temp folder");

    let run = |args: &[&str]| {
        let mut command = Command::new("git");
        command.args(args).current_dir(&root);
        crate::shell::no_window(&mut command);
        command
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    };
    // No git on this machine is not a test failure; there is nothing to test.
    if !run(&["init", "--quiet"]) {
        eprintln!("no git: skipping");
        return;
    }
    // A repository of this program's own making, so nothing here depends on the developer's
    // name, editor or signing key.
    assert!(run(&["config", "user.email", "test@example.invalid"]));
    assert!(run(&["config", "user.name", "Test"]));
    assert!(run(&["config", "commit.gpgsign", "false"]));

    std::fs::write(root.join("clean.txt"), b"one\n").unwrap();
    std::fs::write(root.join("changed.txt"), b"two\n").unwrap();
    std::fs::write(root.join("sub/deep.txt"), b"three\n").unwrap();
    assert!(run(&["add", "-A"]));
    assert!(run(&["commit", "--quiet", "-m", "base"]));

    // One of each, from here on: a change on disk, a staged addition, an untracked file, and a
    // change one folder down.
    std::fs::write(root.join("changed.txt"), b"two and more\n").unwrap();
    std::fs::write(root.join("staged.txt"), b"four\n").unwrap();
    assert!(run(&["add", "staged.txt"]));
    std::fs::write(root.join("untracked.txt"), b"five\n").unwrap();
    std::fs::write(root.join("sub/deep.txt"), b"three and more\n").unwrap();

    let repo = read(&root).expect("the folder is a repository");
    assert!(!repo.head.is_empty(), "on some branch");
    assert_eq!(repo.upstream, None, "nowhere to push to");
    assert_eq!(repo.changed, 4, "changed, staged, untracked, and one below");
    assert_eq!(repo.state("clean.txt"), Some(State::Clean), "committed");
    assert_eq!(repo.state("changed.txt"), Some(State::Modified));
    assert_eq!(repo.state("staged.txt"), Some(State::Staged));
    assert_eq!(repo.state("untracked.txt"), Some(State::Untracked));
    assert_eq!(
        repo.state("sub"),
        Some(State::Modified),
        "a folder wears what is under it"
    );
    assert_eq!(repo.dot_git, root.join(".git"), "what to watch");
    assert!(repo.micros > 0, "and it was timed");

    // From inside the subfolder, the same repository answers about *its* names.
    let below = read(&root.join("sub")).expect("still a repository");
    assert_eq!(below.state("deep.txt"), Some(State::Modified));
    assert_eq!(below.dot_git, root.join(".git"), "the same repository");
    assert_eq!(below.changed, 4, "and the same counts");

    // The gate in front of all of it: this folder has a `.git`, so it is worth spawning git for.
    // The negative is deliberately not asserted here — whether the temp directory happens to sit
    // inside somebody's repository is not this test's to know.
    assert!(under_a_git_dir(&root.join("sub")));

    crate::sandbox::remove(&root);
}

/// What one folder's worth of git actually costs, on whatever repository this is run in.
///
/// ```text
/// cargo test --release -- --ignored --nocapture what_a_git_query_costs
/// ```
///
/// Three processes, and on Windows the startup is most of it. The number worth watching is not
/// this one but the one beside it: a folder with no `.git` above it, which is what browsing a disk
/// costs and which must be indistinguishable from zero.
#[test]
#[ignore = "a measurement, not a check"]
fn what_a_git_query_costs() {
    let here = std::env::current_dir().expect("a working directory");
    for round in 0..5 {
        let start = std::time::Instant::now();
        let repo = read(&here);
        let took = start.elapsed();
        match repo {
            Some(repo) => println!(
                "round {round}: {:.1} ms for {} paths, branch {}",
                took.as_secs_f64() * 1000.0,
                repo.changed,
                repo.head
            ),
            None => println!("round {round}: not a repository"),
        }
    }
    let nowhere = std::env::temp_dir();
    let start = std::time::Instant::now();
    for _ in 0..100 {
        let _ = under_a_git_dir(&nowhere);
    }
    println!(
        "the gate: {:.1} µs per folder with no repository above it",
        start.elapsed().as_secs_f64() * 1e6 / 100.0
    );
}

/// Which states there is a `HEAD` version to compare against.
///
/// The two `false`s worth stating are the ones that look like changes: an **untracked** file and a
/// **renamed** one both differ from what is committed, and neither has a path `HEAD` can be asked
/// about — so a comparison against one would be a comparison against nothing.
#[test]
fn only_some_kinds_of_change_have_something_to_compare_with() {
    for state in [State::Staged, State::Modified, State::Conflicted] {
        assert!(state.differs_from_head(), "{state:?} has a HEAD version");
    }
    for state in [
        State::Clean,
        State::Untracked,
        State::Renamed,
        State::Deleted,
    ] {
        assert!(
            !state.differs_from_head(),
            "{state:?} was offered a comparison it cannot have"
        );
    }
}

/// What `HEAD` has of a file, which is what a picture is compared against.
///
/// The point of the test is the *version*: `blob` has to come back with what was committed and
/// not with what is on disk, or the comparison would be a picture against itself.
#[test]
fn what_head_has_of_a_file() {
    // Its own folder, not [`a_real_repository_answers_for_its_own_files`]'s: two tests in one
    // process share a directory name at their peril.
    let root = crate::sandbox::dir("blob");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("a temp folder");

    let run = |args: &[&str]| {
        let mut command = Command::new("git");
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

    // Bytes rather than text, because that is what this is for: a picture is not lines.
    std::fs::write(root.join("kept.bin"), [0u8, 1, 2, 3]).unwrap();
    std::fs::write(root.join("moved.bin"), [9u8, 9]).unwrap();
    assert!(run(&["add", "-A"]));
    assert!(run(&["commit", "--quiet", "-m", "base"]));
    std::fs::write(root.join("moved.bin"), [7u8, 7, 7]).unwrap();
    std::fs::write(root.join("new.bin"), [5u8]).unwrap();

    assert_eq!(
        blob(&root.join("kept.bin")).as_deref(),
        Some(&[0u8, 1, 2, 3][..]),
        "an unchanged file is itself"
    );
    assert_eq!(
        blob(&root.join("moved.bin")).as_deref(),
        Some(&[9u8, 9][..]),
        "the committed version, not the one on disk"
    );
    assert_eq!(
        blob(&root.join("new.bin")),
        None,
        "HEAD has no such path, so there is nothing to compare with"
    );
    assert_eq!(blob(&root.join("gone.bin")), None, "nor of a file at all");

    // **And it is the file rather than the object**, which is the whole reason this runs
    // `cat-file --filters` and not `show`. Eol conversion stands in for Git LFS here: both are
    // filters configured per path, both make the stored object something other than the bytes a
    // checkout would write, and this one needs no external program to set up. `show` would answer
    // with the `\n` version — which for a picture is every pixel row after the first shifted by
    // however many `\r`s were in front of it.
    std::fs::write(root.join(".gitattributes"), b"*.crlf text eol=crlf\n").unwrap();
    std::fs::write(root.join("wrapped.crlf"), b"one\r\ntwo\r\n").unwrap();
    assert!(run(&["add", "-A"]));
    assert!(run(&["commit", "--quiet", "-m", "with an attribute"]));
    assert_eq!(
        blob(&root.join("wrapped.crlf")).as_deref(),
        Some(&b"one\r\ntwo\r\n"[..]),
        "the bytes a checkout would write, not the ones the object holds"
    );

    crate::sandbox::remove(&root);
}

/// Real `git diff -U0` output, from a file whose eight lines were changed in all three ways: one
/// line replaced, two inserted, two taken away.
///
/// `a b c d e f g h` became `a B c NEW1 NEW2 d e h`.
const DIFFED: &str = concat!(
    "diff --git a/lines.txt b/lines.txt\n",
    "index 71ac1b5..5c16a65 100644\n",
    "--- a/lines.txt\n",
    "+++ b/lines.txt\n",
    "@@ -2 +2 @@ a\n",
    "-b\n",
    "+B\n",
    "@@ -3,0 +4,2 @@ c\n",
    "+NEW1\n",
    "+NEW2\n",
    "@@ -6,2 +7,0 @@ e\n",
    "-f\n",
    "-g\n",
);

#[test]
fn a_diff_says_which_lines_are_new_and_what_the_old_ones_were() {
    let changes = parse_diff(DIFFED.as_bytes());
    assert_eq!(changes.hunks.len(), 3);

    // One line replaced by one: line 2 is new, and `b` was there before it.
    let one = &changes.hunks[0];
    assert_eq!((one.added, one.added_count), (2, 1));
    assert_eq!(one.removed, vec!["b".to_owned()]);
    assert_eq!(one.after, 1, "shown in front of the line that replaced it");
    assert_eq!(one.removed_at, 2);

    // Two lines inserted and nothing removed.
    let two = &changes.hunks[1];
    assert_eq!((two.added, two.added_count), (4, 2));
    assert!(two.removed.is_empty());

    // Two lines removed and nothing added — `+7,0` means they sat after line 7.
    let three = &changes.hunks[2];
    assert_eq!(three.added_count, 0);
    assert_eq!(three.after, 7);
    assert_eq!(three.removed, vec!["f".to_owned(), "g".to_owned()]);
    assert_eq!(three.removed_at, 6);
}

/// The file's own name is not a removed line, however much it looks like one.
#[test]
fn the_diff_header_is_not_read_as_content() {
    let changes = parse_diff(DIFFED.as_bytes());
    let removed: Vec<&String> = changes.hunks.iter().flat_map(|h| &h.removed).collect();
    assert!(
        !removed.iter().any(|line| line.starts_with("- a/")),
        "the `--- a/lines.txt` header came through as content: {removed:?}"
    );
}

/// A file git has nothing to say about, and one it cannot diff.
#[test]
fn nothing_to_report_is_no_hunks_rather_than_no_answer() {
    assert!(parse_diff(b"").is_empty());
    assert!(parse_diff(b"Binary files a/x.png and b/x.png differ\n").is_empty());
}

/// A removed line is drawn among the file's own, so it is spelled the way the file's lines are.
#[test]
fn a_removed_line_is_laid_out_like_the_rest() {
    let changes = parse_diff(b"@@ -1 +1 @@\n-\tindented\r\n+    indented\n");
    assert_eq!(changes.hunks[0].removed, vec!["    indented".to_owned()]);
}

/// **A diff has no size the file's own cap implies**, so it is read with a ceiling — see [`DIFF_CAP`].
///
/// Two halves, both against a real git. The ordinary one is that [`changes`] still answers: it is the
/// only path a preview of a changed file takes, and it is now the one call here that does not simply
/// wait for the process to finish. The other is the ceiling itself, which means killing a git that has
/// not finished writing — and a child whose pipe is full and will never be read again is a deadlock
/// waiting to be written. So the cap is an argument rather than the constant, and sixty-four bytes of a
/// diff longer than that asks the question without a four-megabyte fixture.
#[test]
fn a_diff_is_read_with_a_ceiling_and_still_answers() {
    // Its own folder, for the reason [`what_head_has_of_a_file`] gives.
    let root = crate::sandbox::dir("diffcap");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("a temp folder");

    let run = |args: &[&str]| {
        let mut command = Command::new("git");
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

    // The fixture [`DIFFED`] was captured from: `a b c d e f g h` becoming `a B c NEW1 NEW2 d e h`.
    let file = root.join("lines.txt");
    std::fs::write(&file, b"a\nb\nc\nd\ne\nf\ng\nh\n").unwrap();
    assert!(run(&["add", "-A"]));
    assert!(run(&["commit", "--quiet", "-m", "base"]));
    std::fs::write(&file, b"a\nB\nc\nNEW1\nNEW2\nd\ne\nh\n").unwrap();

    let changed = changes(&file).expect("a diff of a changed file");
    assert_eq!(changed.hunks.len(), 3, "{changed:?}");
    assert_eq!(changed.hunks[0].removed, vec!["b".to_owned()]);
    assert_eq!(
        changed.hunks[2].removed,
        vec!["f".to_owned(), "g".to_owned()]
    );

    // And with a ceiling low enough to reach: what fitted, cut back to a whole line, and back at all.
    let out = finish_capped(
        start_git(
            &root,
            &[
                "--no-optional-locks",
                "diff",
                "HEAD",
                "-U0",
                "--no-color",
                "--",
                "lines.txt",
            ],
        ),
        64,
    )
    .expect("the front of a diff");
    assert!(out.len() <= 64, "{} bytes came back", out.len());
    assert!(
        out.ends_with(b"\n"),
        "cut mid-line, so half a hunk header could parse as a whole one: {:?}",
        String::from_utf8_lossy(&out)
    );
    assert!(
        out.starts_with(b"diff --git"),
        "not the front of the diff: {:?}",
        String::from_utf8_lossy(&out)
    );

    crate::sandbox::remove(&root);
}

/// **A number in a hunk header is not a promise about memory.** `-1,4000000000` is fourteen
/// characters, and reserving the removed lines from it — which is what `with_capacity(old_count)` did —
/// is 96 GB of `String` asked for before a single line of the hunk has been read. The lines themselves
/// are what say how many there were.
#[test]
fn a_hunk_header_is_not_a_capacity() {
    let changes = parse_diff(b"@@ -1,4000000000 +1 @@\n-a\n+b\n");
    assert_eq!(changes.hunks.len(), 1);
    assert_eq!(changes.hunks[0].removed, vec!["a".to_owned()]);
}

/// Two places in the status parser took the first character of a string by *byte*, which panics on a
/// multi-byte character and, for the empty string, on nothing at all.
///
/// Neither shape comes out of a real git — every record it writes begins with an ASCII letter it chose
/// itself — so this pins the guard rather than a bug that was seen. What it must not do is stop reading
/// the rest of the records.
#[test]
fn a_record_that_starts_with_anything_at_all_is_survived() {
    let mut repo = Repo::default();
    parse_status(
        concat!(
            "°not a record git would write\0",
            "# branch.head main\0",
            "? untracked.txt\0",
        )
        .as_bytes(),
        "",
        &mut repo,
    );
    assert_eq!(repo.head, "main", "the records after it were not read");
    assert_eq!(repo.state("untracked.txt"), Some(State::Untracked));

    // And the ahead/behind line, where an empty field is one space too many rather than anything exotic.
    let mut spaced = Repo::default();
    parse_status(b"# branch.ab +2  -1\0", "", &mut spaced);
    assert_eq!((spaced.ahead, spaced.behind), (2, 1));
}

/// The tracked names from `ls-tree` only fill in what `status` said nothing about: a file that is
/// both tracked and changed keeps the change.
#[test]
fn a_tick_never_overwrites_a_change() {
    let mut repo = at("");
    for name in ["added.txt", "modified.txt", "quiet.txt"] {
        repo.marks.entry(name.to_owned()).or_insert(State::Clean);
    }
    assert_eq!(repo.state("modified.txt"), Some(State::Modified));
    assert_eq!(repo.state("added.txt"), Some(State::Staged));
    assert_eq!(repo.state("quiet.txt"), Some(State::Clean));
}
