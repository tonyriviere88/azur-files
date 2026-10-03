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
