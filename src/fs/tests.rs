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
    let temp = crate::sandbox::dir("typed-path");
    let slashed = temp.to_string_lossy().replace('\\', "/");
    let resolved = resolve_input(&slashed).expect("the sandbox directory exists");
    let Typed::Folder(resolved) = resolved else {
        panic!("the sandbox directory is a folder, got {resolved:?}");
    };
    assert!(
        !resolved.to_string_lossy().contains('/'),
        "`{}` still has a forward slash in it",
        resolved.display()
    );
}

/// **A network path is handed on even when it cannot be asked about**, which is the bug this fixes.
///
/// `is_dir` answers `false` both for a share that is not there and for one the server will not
/// describe without credentials, and only the first is a reason not to go — see [`resolve_input`] for
/// what answering `None` to both cost.
///
/// Checked against `\\localhost\...`, which fails immediately and without a name to resolve or a
/// packet to send: the *shape* of the path is what the rule turns on, not which server it names.
#[test]
#[cfg(windows)]
fn a_network_path_is_handed_on_even_when_it_will_not_answer() {
    let share = r"\\localhost\yafe-no-such-share";
    assert_eq!(
        resolve_input(share),
        Some(Typed::Folder(PathBuf::from(share))),
        "a share that would not answer has to reach the loader, or nothing can raise a prompt"
    );

    // Typed with the other slash, which is the reason the path is normalised before the rule looks
    // at it: `//localhost/...` names the same place and used to fail the `\\` test.
    assert_eq!(
        resolve_input("//localhost/yafe-no-such-share"),
        Some(Typed::Folder(PathBuf::from(share)))
    );

    // A bare machine, which had never worked from the bar for the same reason — a server is not a
    // file, so `is_dir` is false for every one of them.
    assert_eq!(
        resolve_input(r"\\localhost"),
        Some(Typed::Folder(PathBuf::from(r"\\localhost")))
    );

    // And a local path that is not there is still `None`: the field stays open to be corrected,
    // because there is no server to ask and nothing a listing could say that the field cannot.
    assert_eq!(resolve_input(r"Z:\definitely-not-here\9d3f"), None);
}

/// What a real typed path does, all the way to the flag that raises the credential dialog.
///
/// `YAFE_PROBE_TYPED='\\machine\share' cargo test probe_typed_path -- --ignored --nocapture`.
/// Ignored because it reaches the network and its answer is the running machine's.
///
/// The three steps a press of `Enter` is, in order, so a break can be seen where it is rather than
/// as "nothing happened": what [`resolve_input`] made of the text, what the scanner got back, and
/// whether that came out marked as wanting credentials — which is the one bit
/// [`crate::app::App::ask_credentials`] acts on. Everything past it is Windows' own dialog.
#[test]
#[ignore = "reaches the network; run it deliberately"]
#[cfg(windows)]
fn probe_typed_path() {
    let typed = std::env::var("YAFE_PROBE_TYPED")
        .expect("set YAFE_PROBE_TYPED to the path to type, e.g. '\\\\machine\\share'");
    println!("typed:    {typed}");

    let started = std::time::Instant::now();
    let resolved = resolve_input(&typed);
    println!("resolved: {resolved:?}   in {:?}", started.elapsed());
    let Some(Typed::Folder(path)) = resolved else {
        panic!("nothing to navigate to -- this is where the bug was: the field would just reopen");
    };

    let started = std::time::Instant::now();
    let dir = crate::fs::scan::scan(&path);
    println!(
        "scanned:  {} entries, error {:?}, credentials {}   in {:?}",
        dir.len(),
        dir.error,
        dir.credentials,
        started.elapsed()
    );
    if dir.error.is_some() {
        assert!(
            dir.credentials,
            "the read failed and nothing will ask about it -- a code is missing from \
             `wants_credentials`"
        );
        println!("=> a credential dialog would be raised for this path");
    }
}

/// A file out of the bar is opened, not navigated into — and **the classification comes back with
/// the path** rather than being asked for a second time. See [`Typed`].
#[test]
#[cfg(windows)]
fn a_typed_file_comes_back_as_a_file() {
    let dir = crate::sandbox::dir("typed-file");
    let file = dir.join("readme.txt");
    std::fs::write(&file, b"x").expect("the sandbox is writable");
    assert_eq!(
        resolve_input(&file.to_string_lossy()),
        Some(Typed::File(file.clone()))
    );
    assert_eq!(resolve_input(&dir.to_string_lossy()), Some(Typed::Folder(dir)));
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

/// A machine is a level of the tree, and `Path` does not think so.
///
/// The whole reason both of these are walked by hand. Measured on Windows:
/// `Path::new(r"\\fileserver\web")` is a *single* `Prefix(UNC)` component with the server and
/// the share welded together and a `parent()` of `None`, while `Path::new(r"\\fileserver")` is a
/// bare `RootDir` plus a `Normal` whose `parent()` is `\` — a path that leads nowhere and which Up
/// used to navigate to.
#[test]
#[cfg(windows)]
fn a_machine_sits_between_this_pc_and_its_shares() {
    // Up: a folder, its share, the machine, This PC. Every step is somewhere with a listing.
    assert_eq!(
        parent_of(Path::new("\\\\fileserver\\web\\owncloud")),
        Some(PathBuf::from("\\\\fileserver\\web\\")),
        "inside a share, `Path::parent` is right and is used as it is"
    );
    assert_eq!(
        parent_of(Path::new("\\\\fileserver\\web")),
        Some(PathBuf::from("\\\\fileserver")),
        "Up from a share reaches the machine that offers it"
    );
    assert_eq!(
        parent_of(Path::new("\\\\fileserver")),
        Some(PathBuf::new()),
        "and Up from the machine reaches This PC, not the bare `\\` that Path::parent gives"
    );

    // A DFS path, where the first component is a domain rather than a server: there is no machine
    // to stop at, so a share root goes straight to This PC. `drives::list_servers` excludes these
    // for the same reason — asking one for its shares takes 22 seconds to fail.
    assert_eq!(
        parent_of(Path::new("\\\\lgs-net.com\\alyo")),
        Some(PathBuf::from("\\\\lgs-net.com")),
        "a two-part UNC is a share on a machine as far as this can tell"
    );

    // And the bar shows those levels rather than one welded segment or a stray `\`.
    let labels = |path: &str| -> Vec<String> {
        breadcrumb_segments(Path::new(path))
            .into_iter()
            .map(|(label, _)| label)
            .collect()
    };
    assert_eq!(labels("\\\\fileserver"), ["This PC", "fileserver"]);
    assert_eq!(
        labels("\\\\fileserver\\web\\owncloud"),
        ["This PC", "fileserver", "web", "owncloud"]
    );

    // Every segment has to lead somewhere: the machine to its share list, the share to its root.
    let crumbs = breadcrumb_segments(Path::new("\\\\fileserver\\web\\owncloud"));
    assert_eq!(crumbs[1].1, PathBuf::from("\\\\fileserver"));
    assert_eq!(crumbs[2].1, PathBuf::from("\\\\fileserver\\web"));
    assert_eq!(crumbs[3].1, PathBuf::from("\\\\fileserver\\web\\owncloud"));
}

#[test]
fn unknown_variables_survive_expansion() {
    assert_eq!(expand("%NOT_A_REAL_VAR_XYZ%\\x"), "%NOT_A_REAL_VAR_XYZ%\\x");
    assert_eq!(expand("plain"), "plain");
    assert_eq!(expand("50% done"), "50% done");
}

#[test]
fn resolve_understands_this_pc_and_bare_drives() {
    assert_eq!(
        resolve_input("  This PC "),
        Some(Typed::Folder(PathBuf::new()))
    );
    assert_eq!(resolve_input(""), None);
    #[cfg(windows)]
    assert_eq!(
        resolve_input("C:"),
        Some(Typed::Folder(PathBuf::from("C:\\")))
    );
}

/// **The names a rename box has to refuse, and the one it must not.**
///
/// The separator is the reason this exists: the field had no validation whatsoever, so `F2` and
/// `..\report.txt` handed the shell a path where it wanted a name. Everything else here is a name
/// Explorer refuses in its own box with a message, and refusing it silently — or handing it over to
/// come back as a bare `HRESULT` — is the same bug in a smaller size.
#[test]
fn a_rename_refuses_what_windows_refuses() {
    for bad in [
        r"..\evil.txt",
        r"sub\one.txt",
        "sub/one.txt",
        "..",
        ".",
        "a<b",
        "a>b",
        "a:b",
        "a\"b",
        "a|b",
        "a?b",
        "a*b",
        "one.txt.",
        "one.txt ",
        "CON",
        "con",
        "CON.txt",
        "NUL",
        "COM1",
        "lpt9.log",
        "\u{7}bell.txt",
    ] {
        assert!(
            why_not_a_name(bad).is_some(),
            "`{bad}` was accepted as a file name"
        );
    }

    // And the names that are fine, which is the half that makes the check worth having rather than
    // merely strict. A trailing dot is refused; a dot anywhere else is most file names there are.
    for good in [
        "one.txt",
        "README",
        "a.tar.gz",
        ".gitignore",
        "CONSOLE.txt",
        "COM10",
        "not a device.CON",
        "café — résumé.pdf",
        "one (2).txt",
        "#hash & ampersand!.txt",
        &"a".repeat(255),
    ] {
        assert_eq!(
            why_not_a_name(good),
            None,
            "`{good}` was refused as a file name"
        );
    }

    // Measured in UTF-16 units, because that is what the filesystem's limit counts.
    assert!(why_not_a_name(&"a".repeat(256)).is_some());
}
