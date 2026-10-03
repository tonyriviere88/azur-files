use super::*;

/// A folder of this test's own, under the scratch directory rather than a profile.
///
/// [`dir`] refuses to answer in a test at all, so nothing here can reach a real one — this is
/// only somewhere to put files that [`prune`] is allowed to delete.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("azur-cwd-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch folder");
    dir
}

fn publish(dir: &Path, pid: u32, path: &str) {
    // The mtime is what picks the newest, and two writes in the same tick can land on the
    // same one — so each publication is separated enough to be ordered.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(dir.join(format!("{pid}.at")), path).expect("published");
}

/// The whole of the follow direction: a `cd` moves the window, and nothing else does.
///
/// The second half is the one worth holding down. Every prompt used to publish, so pressing
/// Enter in the terminal dragged the pane back to the shell's folder from wherever the user
/// had browsed to — which makes the feature actively hostile. Both halves defend against it:
/// the hook writes nothing when the folder has not changed, and this compares contents rather
/// than timestamps in case something else does.
#[test]
fn only_a_change_of_folder_is_followed() {
    let dir = scratch("followed");
    let here = std::env::current_dir().expect("a working directory");
    let parent = here.parent().expect("and a parent").to_path_buf();

    // Something published before the window opened is where the shell is, not a move.
    publish(&dir, 4242, &here.to_string_lossy());
    let mut link = Link::for_tests(dir.clone());
    assert_eq!(link.arrived(), None, "a window opening must not navigate itself");

    // A `cd`.
    publish(&dir, 4242, &parent.to_string_lossy());
    assert_eq!(link.arrived(), Some(parent.clone()));

    // The same folder again — a prompt, not a move.
    publish(&dir, 4242, &parent.to_string_lossy());
    assert_eq!(link.arrived(), None, "pressing Enter is not a `cd`");

    // And back.
    publish(&dir, 4242, &here.to_string_lossy());
    assert_eq!(link.arrived(), Some(here));
}

/// The newest publication wins, and a shell sitting still cannot outvote the one being used.
#[test]
fn the_terminal_last_typed_in_is_the_one_that_is_followed() {
    let dir = scratch("newest");
    let here = std::env::current_dir().expect("a working directory");
    let parent = here.parent().expect("and a parent").to_path_buf();

    publish(&dir, 1, &here.to_string_lossy());
    let mut link = Link::for_tests(dir.clone());

    // A second shell, moving.
    publish(&dir, 2, &parent.to_string_lossy());
    assert_eq!(link.arrived(), Some(parent.clone()));

    // The first shell is still sitting in its own folder and must not pull anything back.
    assert_eq!(link.arrived(), None);

    // And it is the second shell that a send goes to, since it published last.
    assert!(link.send(&here));
    assert!(dir.join("2.to").is_file(), "sent to the shell last typed in");
    assert!(!dir.join("1.to").exists());
}

/// What `send` leaves behind is what the hook expects, and the folder is not followed back.
#[test]
fn a_folder_sent_over_is_written_for_either_shell_and_not_followed_back() {
    let dir = scratch("sent");
    let here = std::env::current_dir().expect("a working directory");
    publish(&dir, 7, &here.to_string_lossy());
    let mut link = Link::for_tests(dir.clone());

    let target = here.join("src");
    assert!(link.send(&target));
    let written = std::fs::read_to_string(dir.join("7.to")).expect("a request");
    assert!(!written.contains('\\'), "a backslash in `cd \"...\"` is an escape: {written}");
    assert_eq!(windows_path(&written), Some(target.clone()));

    // The shell will arrive there and publish it, and that echo must not navigate the pane
    // the folder came from.
    publish(&dir, 7, &target.to_string_lossy());
    assert_eq!(link.arrived(), None, "the window followed its own request back");
}

/// Nothing is published, or what is published is not a folder.
#[test]
fn an_empty_folder_and_an_unusable_path_are_both_quiet() {
    let dir = scratch("quiet");
    let mut link = Link::for_tests(dir.clone());
    assert_eq!(link.arrived(), None);
    assert!(!link.send(Path::new(r"C:\")), "nowhere to send it");

    // A shell in a directory that has since been deleted, and one in a directory that is not
    // a Windows path at all.
    publish(&dir, 3, r"D:\gone-a4f1c9");
    assert_eq!(link.arrived(), None);
    publish(&dir, 3, "/usr/local/bin");
    assert_eq!(link.arrived(), None);
}

/// Dead shells are cleared out and live ones are left alone — including one that has not
/// moved for a long time, which is why this is not done by age.
#[test]
fn pruning_keeps_the_shells_that_are_still_running() {
    let dir = scratch("prune");
    // This process is the live "shell"; a pid nothing can be using is the dead one.
    let mine = std::process::id();
    std::fs::write(dir.join(format!("{mine}.at")), r"C:\").expect("live");
    std::fs::write(dir.join("4294967294.at"), r"C:\").expect("dead");
    std::fs::write(dir.join("4294967294.to"), r"C:\").expect("dead request");
    // Not ours, and not a process id either.
    std::fs::write(dir.join("notes.txt"), "leave me").expect("unrelated");

    prune(&dir);
    assert!(dir.join(format!("{mine}.at")).is_file(), "a running shell was cleared out");
    assert!(!dir.join("4294967294.at").exists());
    assert!(!dir.join("4294967294.to").exists());
    assert!(dir.join("notes.txt").is_file(), "somebody else's file was deleted");
}

/// Every spelling of a folder that a shell on Windows can publish.
#[test]
fn a_published_folder_is_understood_in_every_spelling() {
    let cases = [
        (r"D:\Sources\x", Some(r"D:\Sources\x")),
        ("D:/Sources/x", Some(r"D:\Sources\x")),
        ("/d/Sources/x", Some(r"D:\Sources\x")),
        ("/mnt/d/Sources/x", Some(r"D:\Sources\x")),
        ("/d", Some(r"D:\")),
        ("/c/", Some(r"C:\")),
        ("//server/share/x", Some(r"\\server\share\x")),
        (r"\\server\share", Some(r"\\server\share")),
        // Inside the shell's own root: a real directory whose place on this file system
        // cannot be worked out from the path.
        ("/usr/bin", None),
        ("/tmp", None),
        ("/", None),
        ("", None),
        ("   ", None),
    ];
    for (text, want) in cases {
        assert_eq!(
            windows_path(text).as_deref(),
            want.map(Path::new),
            "{text:?}"
        );
    }
    // A trailing newline is what both shells write, and it is not part of the path.
    assert_eq!(windows_path("D:/x\n").as_deref(), Some(Path::new(r"D:\x")));
}

/// Both hooks carry the folder this program actually derived, in the spelling their own shell
/// can read.
///
/// The path is the part that has to be right: it is what pairs the two halves, so an install
/// under `YAFE_PROFILE` publishes somewhere this program is looking rather than into the
/// default profile beside it.
#[test]
fn a_hook_carries_the_folder_in_its_own_shells_spelling() {
    let dir = Path::new(r"C:\Users\x\AppData\Roaming\Azur\cwd");

    let bash = hook_in(dir, Shell::Bash);
    let quoted = bash.lines().find(|line| line.contains("d=")).expect("the folder");
    assert_eq!(quoted.trim(), "local d='C:/Users/x/AppData/Roaming/Azur/cwd'");
    assert!(
        !quoted.contains('\\'),
        "a backslash inside a bash string is an escape: {quoted}"
    );
    assert!(bash.contains("$$.at") && bash.contains("$$.to"));
    assert!(bash.contains("pwd -W"), "the builtin is what makes this free");
    assert!(bash.contains("*__azur_cwd*"), "sourcing twice must add it once");

    let pwsh = hook_in(dir, Shell::PowerShell);
    assert!(pwsh.contains(&format!("$global:AzurCwd = '{}'", dir.display())), "{pwsh}");
    assert!(pwsh.contains("$PID.at") && pwsh.contains("$PID.to"));
    assert!(pwsh.contains("FileSystem"), "`$PWD` can be a registry location");
    assert!(pwsh.contains("if (-not $global:AzurPrompt)"), "sourcing twice must not recurse");
    assert!(pwsh.contains("& $global:AzurPrompt"), "somebody's own prompt has to survive");
}

#[test]
fn the_shells_are_named_the_way_people_would_type_them() {
    for text in ["bash", "Bash", " git-bash ", "zsh", ""] {
        assert_eq!(Shell::parse(text), Some(Shell::Bash), "{text:?}");
    }
    for text in ["pwsh", "PowerShell", "ps1"] {
        assert_eq!(Shell::parse(text), Some(Shell::PowerShell), "{text:?}");
    }
    assert_eq!(Shell::parse("cmd"), None, "cmd has no prompt hook to install");
    assert_eq!(Shell::parse("fish"), None);
}
