use super::*;

/// The test binary is itself a PE file, which makes it the one fixture that is real, is
/// always there, and is built for whatever this machine is.
fn me() -> PathBuf {
    std::env::current_exe().expect("a test process has an executable")
}

#[test]
fn a_real_binary_names_what_it_needs() {
    let image = read(&me()).expect("the test binary is a PE file");
    assert_ne!(image.machine, 0, "a real image says what it was built for");
    assert!(
        !image.imports.is_empty(),
        "every Windows binary imports something"
    );
    // Whatever else a Rust program on Windows imports, it imports the C runtime's host or
    // the API sets that stand in for it. What is asserted here is the shape of the answer
    // rather than one name: every entry is a plausible module name.
    for (name, _) in &image.imports {
        assert!(
            name.len() > 3 && name.contains('.'),
            "{name:?} does not look like a DLL name"
        );
    }
    // The two tables are merged with no duplicates, which is what the panel counts on.
    let mut keys: Vec<String> = image
        .imports
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    let before = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(before, keys.len(), "a DLL is named twice: {keys:?}");
}

#[test]
fn something_that_is_not_a_binary_says_so_rather_than_guessing() {
    let text = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    assert_eq!(read(&text).err(), Some("not a binary"));
    assert_eq!(
        read(&PathBuf::from("no-such-file-anywhere.dll")).err(),
        Some("cannot be opened")
    );
}

/// The point of the whole module: a real graph, more than one level deep, with every
/// module either accounted for or honestly marked.
#[test]
fn a_walk_finds_where_each_dependency_lives_and_keeps_going() {
    let graph = walk(&me(), BUDGET, PATIENCE);
    assert_eq!(graph.root().state, State::Found);
    assert!(
        graph.modules.len() > 2,
        "a walk of a real binary reaches more than its own imports: {}",
        graph.modules.len()
    );

    // Recursion: something in the graph is a dependency of a dependency. Without this the
    // test would pass on a walk that read the root's import table and stopped.
    let second_level = graph.modules[0]
        .imports
        .iter()
        .any(|edge| !graph.modules[edge.to].imports.is_empty());
    assert!(second_level, "nothing was followed past the first level");

    // Every module is one of the four things it can be, and a found one has a path that
    // exists and came off the search list.
    for (i, module) in graph.modules.iter().enumerate() {
        match module.state {
            State::Found => {
                let path = module.path.as_ref().expect("a found module has a path");
                assert!(path.is_file(), "{path:?} was reported and is not there");
                assert!(
                    graph
                        .search
                        .iter()
                        .any(|dir| path.parent() == Some(dir.as_path())),
                    "{path:?} is not in any directory that was searched"
                );
                assert_ne!(module.machine, 0, "{:?} has no machine", module.name);
            }
            State::ApiSet => {
                assert!(is_api_set(&module.name));
                assert!(module.path.is_none(), "an API set is not a file");
            }
            State::Missing | State::Unreadable(_) => {}
            State::Unvisited => assert!(
                graph.truncated,
                "{:?} was never visited and the walk does not admit stopping",
                module.name
            ),
        }
        // Nothing points outside the graph, and the root is nobody's index by accident.
        for edge in &module.imports {
            assert!(edge.to < graph.modules.len(), "module {i} has a wild edge");
        }
    }

    // The system DLLs are found through `PATH`, which is what the search order claims.
    assert!(
        graph
            .modules
            .iter()
            .any(|m| matches!(m.state, State::Found) && m.path.as_ref() != Some(&me())),
        "nothing but the root was resolved"
    );
}

/// A cycle does not become an endless walk, and it does not become a duplicated module
/// either. Real graphs have them — `kernel32` and `kernelbase` refer to each other through
/// forwarders — so this is checked against whatever this machine's really are.
#[test]
fn a_graph_holds_each_module_once_however_many_things_want_it() {
    let graph = walk(&me(), BUDGET, PATIENCE);
    let mut names: Vec<String> = graph
        .modules
        .iter()
        .map(|m| m.name.to_ascii_lowercase())
        .collect();
    let before = names.len();
    names.sort();
    names.dedup();
    assert_eq!(before, names.len(), "a module appears twice in the graph");
}

/// The root binary's own folder is searched before `PATH`.
///
/// Checked by putting a copy of the test binary in a folder of its own under a name
/// nothing else could resolve to, and walking *that*: its first search directory has to be
/// the folder the copy is in.
#[test]
fn the_binarys_own_folder_is_looked_in_first() {
    let root = crate::sandbox::dir("pe");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("a directory in the temp folder");
    let copy = root.join("yafe-fixture.dll");
    std::fs::copy(me(), &copy).expect("a copy of the test binary");

    let search = search_paths(&copy);
    assert_eq!(
        search.first().map(|p| p.as_path()),
        Some(root.as_path()),
        "the binary's own folder is not the first place looked"
    );
    // And a name that only exists there resolves to it, which is the behaviour that
    // search order is *for*.
    assert_eq!(
        resolve("yafe-fixture.dll", &search).as_deref(),
        Some(copy.as_path())
    );
    // Nothing reaches out of the search path, however the name is written.
    assert_eq!(resolve(r"..\yafe-fixture.dll", &search), None);
    assert_eq!(resolve(&copy.to_string_lossy(), &search), None);

    let graph = walk(&copy, BUDGET, PATIENCE);
    assert_eq!(graph.root().state, State::Found);
    assert!(graph.modules.len() > 2);
    crate::sandbox::remove(&root);
}

/// What a walk of a real binary actually costs, and what it comes back with.
///
/// Ignored, like `scan::flatten_speed`: it is a measurement rather than an assertion, it
/// prints, and what it measures depends on the machine. `cargo test -- --ignored
/// --nocapture walk_speed` — with `YAFE_WALK` set to walk something larger than the test
/// binary, which is what a number worth having is measured on.
#[test]
#[ignore]
fn walk_speed() {
    let target = std::env::var_os("YAFE_WALK")
        .map(PathBuf::from)
        .unwrap_or_else(me);
    let graph = walk(&target, BUDGET, PATIENCE);
    let (files, api_sets, missing) = graph.tally();
    println!(
        "{} on {} threads: {} modules ({files} files, {api_sets} api sets), \
         {missing} missing, {:.1} ms{}",
        target.display(),
        hands(),
        graph.modules.len(),
        graph.micros as f64 / 1000.0,
        if graph.truncated { " (truncated)" } else { "" }
    );
    for (i, module) in graph.modules.iter().enumerate() {
        let where_ = match (&module.path, module.state) {
            (Some(path), State::Found) => path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            (_, State::ApiSet) => "(api set)".to_owned(),
            (_, State::Missing) => "NOT FOUND".to_owned(),
            (_, State::Unreadable(why)) => why.to_owned(),
            (_, State::Unvisited) => "(not visited)".to_owned(),
            (None, State::Found) => String::new(),
        };
        println!(
            "  {:>3} {:<34} {:<8} {:<3} {where_}",
            i,
            module.name,
            machine_name(module.machine),
            module.imports.len(),
        );
    }
}

/// **The system directories come before `PATH`, and the binary's own folder before them.**
///
/// The order is the whole answer this module gives, so it is the thing worth pinning. What it
/// catches is the mistake it was written to fix: `System32` is in `PATH` on every machine, so a
/// search that only knew about `PATH` still *found* `kernel32.dll` and every test passed — it
/// simply reported it as coming from wherever `System32` sat in that user's `PATH`, behind
/// anything installed in front of it.
#[cfg(windows)]
#[test]
fn the_system_directories_are_looked_in_before_path() {
    let me = me();
    let search = search_paths(&me);
    let at = |what: &str| {
        search
            .iter()
            .position(|dir| dir.to_string_lossy().eq_ignore_ascii_case(what))
    };

    let system = system_dirs();
    assert!(
        !system.is_empty(),
        "the platform would not say where Windows is"
    );
    let system32 = system[0].to_string_lossy().to_string();
    assert!(
        system32.to_lowercase().ends_with("system32"),
        "{system32} is not the system directory"
    );

    // The binary's own folder is first, and the system directory is right behind it.
    assert_eq!(
        search.first().map(|p| p.as_path()),
        me.parent(),
        "the binary's own folder is not the first place looked"
    );
    let system32_at = at(&system32).expect("the system directory is in the list");
    assert_eq!(system32_at, 1, "the system directory is not second: {search:?}");

    // And every one of the three comes before every `PATH` entry that is not itself one of
    // them. That is the assertion with teeth: `System32` is nearly always in `PATH` too, so
    // this is checking a *position* and not a presence.
    let from_path: Vec<String> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.to_string_lossy().to_lowercase())
        .collect();
    // Anything that has a *reason* to be near the front, whatever `PATH` also says about it.
    // The binary's own folder is one of them and has to be excluded by name: cargo puts the
    // build directory on `PATH` when it runs a test, so under `cargo test` the first entry is
    // in both lists and the assertion below would fire on it.
    let mut excused: Vec<String> = system
        .iter()
        .map(|dir| dir.to_string_lossy().to_lowercase())
        .collect();
    excused.extend(me.parent().map(|dir| dir.to_string_lossy().to_lowercase()));
    let first_from_path = search
        .iter()
        .position(|dir| {
            let key = dir.to_string_lossy().to_lowercase();
            from_path.contains(&key) && !excused.contains(&key)
        })
        .expect("this machine's PATH has something in it that is not a system directory");
    for (which, dir) in system.iter().enumerate() {
        let Some(found) = at(&dir.to_string_lossy()) else {
            // `System` does not exist on every install, and a directory that is not there is
            // dropped up front — see `search_paths`.
            continue;
        };
        assert!(
            found < first_from_path,
            "system directory {which} ({}) is at {found}, behind a PATH entry at \
             {first_from_path}",
            dir.display()
        );
    }

    // Nothing appears twice, however many of these lists it is in.
    let mut keys: Vec<String> = search
        .iter()
        .map(|dir| dir.to_string_lossy().to_lowercase())
        .collect();
    let before = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(before, keys.len(), "a directory is searched twice");
}

#[test]
fn an_api_set_is_told_apart_from_a_missing_dll() {
    assert!(is_api_set("api-ms-win-core-file-l1-2-0.dll"));
    assert!(is_api_set("API-MS-WIN-CRT-RUNTIME-L1-1-0.DLL"));
    assert!(is_api_set("ext-ms-win-ntuser-window-l1-1-0.dll"));
    assert!(!is_api_set("kernel32.dll"));
    // Not merely a prefix: the name has to have something after it.
    assert!(!is_api_set("api-ms-"));
    assert!(!is_api_set("apiset.dll"));
}

#[test]
fn only_a_binary_is_offered_a_walk() {
    for yes in ["exe", "EXE", "dll", "Sys", "ocx", "node", "pyd"] {
        assert!(is_image(yes), "{yes} is a binary");
    }
    for no in ["", "txt", "rs", "zip", "exe2", "dl"] {
        assert!(!is_image(no), "{no} is not a binary");
    }
}

/// A walk with no patience hands back what it has rather than nothing at all.
///
/// The root is read whatever the deadline says — it is read before the clock is consulted —
/// so the honest answer to "no time at all" is one module and `truncated`.
#[test]
fn a_walk_out_of_patience_admits_it() {
    let graph = walk(&me(), BUDGET, Duration::ZERO);
    assert_eq!(graph.root().state, State::Found, "the root is always read");
    assert!(graph.truncated, "a walk that stopped early has to say so");
    assert!(graph
        .modules
        .iter()
        .skip(1)
        .all(|m| m.state == State::Unvisited));
}

/// And a walk with no budget stops at the budget, for the same reason.
#[test]
fn a_walk_stops_at_its_budget_and_admits_it() {
    let graph = walk(&me(), 2, PATIENCE);
    assert!(graph.modules.len() <= 2);
    assert!(graph.truncated);
}

/// Reading in parallel gives the same answer as reading one at a time.
///
/// The thread count is what [`probe_all`] branches on — under it the level goes inline —
/// so this drives both paths over the same real graph and compares them name for name.
#[test]
fn reading_in_parallel_does_not_change_the_answer() {
    let search = search_paths(&me());
    let image = read(&me()).expect("the test binary is a PE file");
    let names: Vec<String> = image.imports.into_iter().map(|(name, _)| name).collect();
    if names.len() < 2 {
        println!("this binary imports {} modules; nothing to compare", names.len());
        return;
    }
    let until = Instant::now() + PATIENCE;
    let (inline, _) = probe_all(&names, &search, until, 1);
    for hands in [2, 4, 8] {
        let (threaded, gave_up) = probe_all(&names, &search, until, hands);
        assert!(!gave_up, "a warm walk on {hands} threads ran out of time");
        assert_eq!(inline.len(), threaded.len());
        for (one, many) in inline.iter().zip(&threaded) {
            assert_eq!(one.path, many.path, "a different file was resolved");
            assert_eq!(one.state, many.state);
            assert_eq!(one.machine, many.machine);
            assert_eq!(one.imports, many.imports, "a different import list");
        }
    }
}
