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

// ---------------------------------------------------------------------------
// The symbols
// ---------------------------------------------------------------------------

/// The one DLL that is on every Windows machine, exports thousands of symbols, and that this
/// process itself imports from — which makes it the fixture for both halves of the question.
fn kernel32() -> PathBuf {
    let graph = walk(&me(), BUDGET, PATIENCE);
    graph
        .modules
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case("kernel32.dll"))
        .and_then(|m| m.path.clone())
        .expect("every Windows binary reaches kernel32")
}

/// **A real export table, read whole.** The three shapes an export comes in are all in
/// `kernel32.dll` and all asserted here, because each one is a way of getting it wrong:
///
/// - the ordinary named export with an address;
/// - the **forwarder**, which is most of this DLL — `NTDLL.RtlAllocateHeap` and its like — and whose
///   address field is a string rather than an entry point;
/// - the **ordinal-only** export, which has no name at all and so cannot be found by one.
#[test]
fn a_real_export_table_is_read_whole() {
    let dll = kernel32();
    let exports = exports(&dll).expect("kernel32 is a PE file");
    assert!(
        exports.len() > 500,
        "kernel32 exports {} symbols, which is not a real export table",
        exports.len()
    );

    // Named, and something famous is among them.
    let named: Vec<&str> = exports.iter().filter_map(|e| e.name.as_deref()).collect();
    for known in ["CreateFileW", "GetProcAddress", "LoadLibraryW"] {
        assert!(named.contains(&known), "kernel32 does not export {known}");
    }
    // The ordinals rise with the table, which is what says the address array was read by slot
    // rather than by name-table position.
    let ordinals: Vec<u16> = exports.iter().map(|e| e.ordinal).collect();
    assert!(
        ordinals.windows(2).all(|pair| pair[0] < pair[1]),
        "the ordinals are not in order, so a slot was mispaired"
    );
    // An export is code or a redirection — which `Bound` makes structural, so what is left to check
    // is that the redirections were read as strings and not as addresses.
    let mut forwarders = 0;
    for export in &exports {
        if let Bound::Forward(to) = &export.bound {
            forwarders += 1;
            assert!(
                to.contains('.') && !to.contains(' '),
                "{to:?} does not look like a forwarder target"
            );
        }
    }
    assert!(
        forwarders > 0,
        "no forwarder was found, and most of kernel32's surface is forwarders"
    );
    // **Every name is claimed once**, which is the check that the two parallel arrays were read
    // against each other rather than by position: the name table is sorted and the ordinal table is
    // what maps it back onto a function, so a mispairing shows up as one name on two slots.
    let mut named: Vec<&str> = exports.iter().filter_map(|e| e.name.as_deref()).collect();
    let before = named.len();
    named.sort_unstable();
    named.dedup();
    assert_eq!(before, named.len(), "one name is attached to two exports");
}

/// **What one binary uses out of another**, which is the other half of the question and a much
/// shorter list than the export table it is drawn from.
///
/// Asserted against the export table rather than against a list of names: every symbol this
/// process imports from `kernel32.dll` has to *be* one of the things `kernel32.dll` exports, and
/// that is a property no change to this crate can invalidate.
#[test]
fn what_a_binary_uses_out_of_one_dll_is_a_subset_of_what_that_dll_exports() {
    let dll = kernel32();
    let used = imported_from(&me(), "kernel32.dll").expect("the test binary is a PE file");
    if used.is_empty() {
        // A Rust binary can reach kernel32 entirely through the API sets, in which case there is
        // nothing to check and saying so is better than a green test that asserted nothing.
        println!("this binary imports nothing from kernel32 by name");
        return;
    }
    let offered: HashSet<String> = exports(&dll)
        .expect("kernel32 is a PE file")
        .into_iter()
        .filter_map(|e| e.name)
        .collect();
    for symbol in &used {
        // By name or by ordinal, and exactly one of the two — that is what the top bit of a
        // thunk means.
        assert_ne!(
            symbol.name.is_some(),
            symbol.ordinal.is_some(),
            "a symbol imported both ways, or neither"
        );
        if let Some(name) = &symbol.name {
            assert!(
                offered.contains(name),
                "{name} is imported from kernel32, which does not export it"
            );
        }
    }
    // The case matters: this is looked up by the name off the graph's edge, which is however the
    // import table happened to spell it.
    assert_eq!(
        imported_from(&me(), "KERNEL32.DLL").unwrap().len(),
        used.len(),
        "the DLL name is being matched case-sensitively"
    );
    // And a DLL this binary does not import from is an empty list rather than a wrong one.
    assert!(imported_from(&me(), "no-such-module.dll")
        .unwrap()
        .is_empty());
}

/// Neither table is asked of something that is not a binary, and neither invents an answer for a
/// binary that has nothing to say — an `.exe` normally exports nothing at all.
#[test]
fn a_binary_with_nothing_to_say_says_nothing() {
    let text = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    assert_eq!(exports(&text).err(), Some("not a binary"));
    assert_eq!(imported_from(&text, "kernel32.dll").err(), Some("not a binary"));

    // The test binary is a real image, and a Rust test harness exports nothing.
    let mine = exports(&me()).expect("the test binary is a PE file");
    assert!(
        mine.len() < 8,
        "the test executable exports {} symbols, which is unexpected enough to look at",
        mine.len()
    );
}

/// **A decorated C++ name comes back readable**, and a name that is not one comes back `None`
/// rather than mangled further.
///
/// The fixtures are `RshApp3D.dll`'s, out of the panel this was written against — a real MSVC
/// C++ DLL's export table, which is the case the whole thing exists for. Not read off that file:
/// it is on one machine and these are the strings, which is what the demangler is being asked
/// about.
#[test]
#[cfg(windows)]
fn a_decorated_name_comes_back_readable() {
    // A member function with a namespace, a class and parameters.
    let got = demangle("?Exec@App3D@rsh@@SAHAEAVIDocument@2@@Z").expect("a decorated name");
    assert!(
        got.starts_with("rsh::App3D::Exec("),
        "the qualified name is not what it leads with: {got}"
    );
    // **None of the decoration around it**, which is what the flags are chosen for: a row 300 points
    // wide cannot spend nineteen characters on `public: int __cdecl`, and a name that begins with its
    // access specifier sorts every `public:` in the DLL together.
    for noise in ["public:", "static", "__cdecl", "__ptr64", "int "] {
        assert!(
            !got.contains(noise),
            "{noise:?} survived the flags: {got}"
        );
    }

    // A plain C export is already what it is called, and is refused before a syscall is spent.
    assert_eq!(demangle("CreateFileW"), None);
    assert_eq!(demangle(""), None);
    // Itanium mangling is another scheme and this one does not claim to read it.
    assert_eq!(demangle("_ZN4core3fmt5write17h0123456789abcdefE"), None);
    // And something that starts like a decorated name but is not one is refused rather than
    // handed back as itself — `dbghelp` returns the input unchanged, which would read as a
    // successful demangling of nonsense.
    assert_eq!(demangle("?"), None);
    assert_eq!(demangle("?not a symbol at all"), None);
}

/// The two together: over a **real** export table, every name either demangles or is left alone,
/// and nothing comes back with its decoration still on it.
///
/// The DLL is looked for rather than named: which module on a given machine exports C++ is not
/// something a test can assume, so this walks its own graph and takes the first export table with
/// decorated names in it. Saying which one it found — and saying so when there was none — is the
/// difference between a test that checked something and one that passed.
#[test]
#[cfg(windows)]
fn a_real_export_table_demangles_or_is_left_alone() {
    let graph = walk(&me(), BUDGET, PATIENCE);
    let mut checked = None;
    for module in graph.modules.iter() {
        let Some(path) = &module.path else { continue };
        let Ok(exports) = exports(path) else { continue };
        let mut decorated = 0;
        for name in exports.iter().filter_map(|e| e.name.as_deref()) {
            match demangle(name) {
                Some(readable) => {
                    decorated += 1;
                    assert!(
                        !readable.starts_with('?'),
                        "{}: {name} demangled to something still decorated: {readable}",
                        module.name
                    );
                    assert!(!readable.is_empty());
                }
                // Left alone, which is the right answer for a C export and the only answer for a
                // scheme this does not read.
                None => assert!(!name.is_empty()),
            }
        }
        if decorated > 0 {
            checked = Some((module.name.clone(), decorated, exports.len()));
            break;
        }
    }
    match checked {
        Some((name, decorated, all)) => println!("{name}: {decorated} of {all} are decorated"),
        None => println!("nothing in this graph exports a decorated name; only the C path was run"),
    }
}
