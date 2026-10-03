use super::*;

/// Read a directory this program's own source lives in, which is guaranteed to
/// exist and to have both files and subdirectories in it.
fn here() -> Dir {
    scan(Path::new(env!("CARGO_MANIFEST_DIR")))
}

/// This crate's own `src`, which has files at the top, three subdirectories under it and
/// nothing enormous anywhere — unlike the crate root, which has `target` in it.
fn sources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// What a flatten costs, and what reading in parallel buys.
///
/// ```text
/// cargo test --release -- --ignored --nocapture flatten_speed
/// ```
///
/// Over this crate's own `target` directory by default, which is the largest warm tree on
/// hand; `YAFE_FLATTEN_ROOT` points it somewhere else. One worker is the serial walk this
/// replaced, so the comparison is against the real alternative rather than against nothing.
/// Two passes, and the second is the one to read: the first warms the file system's cache,
/// and a flatten of a folder somebody is looking at is a warm read.
#[test]
#[ignore = "walks a large tree; run explicitly"]
fn flatten_speed() {
    let root = std::env::var("YAFE_FLATTEN_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    if !root.is_dir() {
        println!("no {}; skipping", root.display());
        return;
    }
    println!("flattening {}", root.display());
    let patience = std::time::Duration::from_secs(600);
    for pass in 1..=2 {
        for hands in [1usize, 2, 4, 8, 16] {
            let started = Instant::now();
            let dir = walk(&root, FLATTEN_BUDGET, patience, hands);
            let elapsed = started.elapsed();
            let per = elapsed.as_nanos() as f64 / dir.len().max(1) as f64;
            println!(
                "pass {pass}  {hands:>2} thread(s): {:>7} entries in {:>8.1} ms  ({per:>6.0} ns/entry){}",
                dir.len(),
                elapsed.as_secs_f64() * 1000.0,
                if dir.truncated { "  [truncated]" } else { "" }
            );
        }
    }
}

/// However many threads read it, the answer is the same listing.
///
/// The order is part of that: it is what makes a truncated walk a fair picture of the tree,
/// and it is the thing a parallel read is most likely to lose.
#[test]
fn reading_in_parallel_does_not_change_the_answer() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let names = |hands: usize| -> Vec<String> {
        let dir = walk(&root, FLATTEN_BUDGET, FLATTEN_PATIENCE, hands);
        (0..dir.len()).map(|i| dir.name(i).to_owned()).collect()
    };
    let serial = names(1);
    assert!(serial.len() > 20, "the fixture tree is too small to mean much");
    for hands in [2, 4, 8] {
        assert_eq!(
            names(hands),
            serial,
            "{hands} threads produced a different listing"
        );
    }
    // And the budget still cuts in the same place, which is the property the order is for.
    let cut = |hands: usize| -> Vec<String> {
        let dir = walk(&root, 12, FLATTEN_PATIENCE, hands);
        assert!(dir.truncated);
        (0..dir.len()).map(|i| dir.name(i).to_owned()).collect()
    };
    assert_eq!(cut(8), cut(1), "a truncated walk kept different rows");
}

#[test]
fn a_flattened_listing_holds_paths_relative_to_the_folder_it_walked() {
    let dir = scan_deep(&sources(), FLATTEN_BUDGET, FLATTEN_PATIENCE);
    assert!(dir.error.is_none(), "{:?}", dir.error);
    assert!(!dir.truncated, "this crate's own sources fit in any budget");
    let named = |want: &str| (0..dir.len()).find(|&i| dir.name(i) == want);

    // A child of the root is a bare name, exactly as a shallow scan has it.
    assert!(named("main.rs").is_some(), "the root's own files are in it");
    // A folder is content too: listed, as well as walked into.
    let ui = named("ui").expect("`src\\ui` is a row of its own");
    assert!(dir.entries[ui].is_dir());

    // And a grandchild carries the folders in front of it, which is what the row shows and
    // what the sort and the filter see.
    // Found by shape rather than named: this walks the crate's own `src`, and a test that
    // names a file in it fails the next time one of them moves.
    let deep = (0..dir.len())
        .find(|&i| dir.name(i).starts_with("ui\\") && dir.ext(i) == "rs")
        .expect("the walk reached into `ui`");
    let name = dir.name(deep).to_owned();
    let leaf = name.rsplit('\\').next().expect("a last component").to_owned();
    assert_eq!(dir.leaf(deep), leaf, "the file's own name");
    assert_eq!(dir.ext(deep), "rs", "the extension is the last component's");
    assert_eq!(
        dir.target(deep),
        sources().join(name.replace('\\', "/")),
        "a row has to lead to the file it names"
    );
    // Every count is the tree's, not the folder's.
    assert!(
        dir.file_count > 20,
        "only {} files: the walk did not go down",
        dir.file_count
    );
}

#[test]
fn a_reparse_point_is_listed_but_never_descended_into() {
    // The rule that keeps a flatten of `C:\` finite. `C:\Users\All Users` is a junction to
    // `C:\ProgramData` and `C:\Documents and Settings` is one to `C:\Users`, so a walk that
    // follows either never finishes — it does not even loop visibly, it just keeps finding
    // more tree. Every backup tool refuses the same thing for the same reason.
    assert!(descends(FLAG_DIR), "a plain folder is walked into");
    assert!(
        !descends(FLAG_DIR | FLAG_LINK),
        "a junction must not be followed"
    );
    assert!(!descends(0), "a file is not a folder");
    assert!(!descends(FLAG_HIDDEN), "nor is a hidden one");
    // A hidden *folder* is still a folder: `.git` is content of the tree, and hiding a row
    // is the display's business — `show_hidden` — not the walk's.
    assert!(descends(FLAG_DIR | FLAG_HIDDEN));
}

#[test]
fn a_flatten_stops_at_its_budget_and_admits_it() {
    let dir = scan_deep(&sources(), 3, FLATTEN_PATIENCE);
    assert_eq!(dir.len(), 3, "the budget is a hard stop");
    assert!(dir.truncated, "a listing missing rows has to say so");
}

#[test]
fn a_flatten_out_of_patience_hands_back_what_it_has() {
    // No time at all: the folder itself is still read — a flatten that came back with
    // nothing would be a worse answer than the listing it replaced — and nothing under it
    // is opened.
    let dir = scan_deep(&sources(), FLATTEN_BUDGET, std::time::Duration::ZERO);
    assert!(dir.truncated);
    assert!(!dir.is_empty(), "the root's own children are the floor");
    assert!(
        (0..dir.len()).all(|i| !dir.name(i).contains('\\')),
        "it descended anyway, with no time to do it in"
    );
}

#[test]
fn a_listing_has_no_dot_entries() {
    let dir = here();
    assert!(dir.error.is_none(), "{:?}", dir.error);
    for i in 0..dir.len() {
        let name = dir.name(i);
        assert!(name != "." && name != "..", "`{name}` should be filtered out");
        assert!(!name.is_empty());
    }
}

#[test]
fn sizes_and_kinds_come_from_the_enumeration() {
    let dir = here();
    let named = |want: &str| (0..dir.len()).find(|&i| dir.name(i) == want);

    let cargo = named("Cargo.toml").expect("this crate has a manifest");
    assert!(!dir.entries[cargo].is_dir());
    assert!(dir.entries[cargo].size > 0, "the size came from the find data");
    assert!(
        dir.entries[cargo].modified > super::super::time::UNIX_EPOCH_FILETIME,
        "and so did the timestamp"
    );
    assert_eq!(dir.ext(cargo), "toml");

    let src = named("src").expect("this crate has sources");
    assert!(dir.entries[src].is_dir());
    assert_eq!(dir.ext(src), "", "a directory's dots are part of its name");
}

#[test]
fn a_missing_directory_reports_why() {
    let dir = scan(Path::new(r"Q:\no\such\place\at\all"));
    assert!(dir.is_empty());
    assert!(dir.error.is_some(), "a failed read has to say so");
}

#[test]
fn an_empty_directory_is_not_an_error() {
    let path = crate::sandbox::dir("empty");
    std::fs::create_dir_all(&path).expect("temp dir");

    let dir = scan(&path);
    assert!(dir.is_empty());
    assert!(
        dir.error.is_none(),
        "an empty folder reports `no more files` from the *first* call, which is \
         not a failure: {:?}",
        dir.error
    );

    crate::sandbox::remove_dir(&path);
}

/// The claim this whole module exists to make, checked rather than asserted.
///
/// Ignored by default because it writes 60,000 files. Run it deliberately:
///
/// ```text
/// cargo test --release -- --ignored --nocapture scan_speed
/// ```
#[test]
#[ignore = "creates 60k files; run explicitly"]
fn scan_speed() {
    const COUNT: usize = 60_000;

    let root = crate::sandbox::dir("bench");
    std::fs::create_dir_all(&root).expect("temp dir");

    // Names of mixed length and extension, so the transcode and the extension
    // split are both exercised rather than measured on one shape.
    let exts = ["rs", "txt", "png", "e57", "", "tar.gz"];
    for i in 0..COUNT {
        let ext = exts[i % exts.len()];
        let name = if ext.is_empty() {
            format!("entry_{i:06}")
        } else {
            format!("some_moderately_long_name_{i:06}.{ext}")
        };
        let _ = std::fs::write(root.join(name), b"x");
    }

    // Warm: the first read pays for the directory's metadata coming into cache,
    // and what is being measured is the steady state a user actually sees.
    let _ = scan(&root);

    let mut best = u64::MAX;
    for _ in 0..5 {
        let dir = scan(&root);
        assert_eq!(dir.len(), COUNT, "{:?}", dir.error);
        best = best.min(dir.scan_micros);
    }
    let per_entry_ns = best as f64 * 1000.0 / COUNT as f64;
    println!(
        "scan of {COUNT} entries: {:.1} ms  ({per_entry_ns:.0} ns/entry)",
        best as f64 / 1000.0
    );

    // The same folder through the standard library, for the comparison the module
    // documentation makes. Same shape of work: every name, every size, every
    // timestamp, every attribute — into the same arena.
    let mut std_best = u128::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        let mut names = String::new();
        let mut count = 0usize;
        let mut bytes = 0u64;
        for entry in std::fs::read_dir(&root).expect("read_dir").flatten() {
            let name = entry.file_name();
            names.push_str(&name.to_string_lossy());
            let meta = entry.metadata().expect("metadata");
            bytes += meta.len();
            count += 1;
        }
        std::hint::black_box((&names, bytes));
        assert_eq!(count, COUNT);
        std_best = std_best.min(started.elapsed().as_micros());
    }
    println!(
        "std::fs::read_dir, same work: {:.1} ms  ({:.0} ns/entry, {:.2}x)",
        std_best as f64 / 1000.0,
        std_best as f64 * 1000.0 / COUNT as f64,
        std_best as f64 / best as f64
    );

    // The mistake that actually costs: asking the filesystem about each entry
    // *again*, by path, after the enumeration has already answered. This is what
    // `Path::is_dir` in a loop compiles down to, and it is the single easiest way
    // to turn a fast listing into a slow one.
    {
        let started = Instant::now();
        let mut dirs = 0usize;
        for entry in std::fs::read_dir(&root).expect("read_dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs += 1;
            }
            let _ = std::fs::metadata(&path).map(|m| m.len());
        }
        std::hint::black_box(dirs);
        let restat = started.elapsed().as_micros();
        println!(
            "read_dir + a stat per entry:  {:.1} ms  ({:.0} ns/entry, {:.1}x slower)",
            restat as f64 / 1000.0,
            restat as f64 * 1000.0 / COUNT as f64,
            restat as f64 / best as f64
        );
    }

    // And the other per-entry cost this program refuses to pay: asking the shell
    // what a file *is*, which is what fills Explorer's Type column.
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_TYPENAME};

        // A thousand is plenty to get a per-file cost, and sixty thousand of these
        // would make the test unbearable — which is rather the point.
        const SAMPLE: usize = 1_000;
        let names: Vec<Vec<u16>> = (0..SAMPLE)
            .map(|i| {
                let ext = exts[i % exts.len()];
                let name = if ext.is_empty() {
                    format!("entry_{i:06}")
                } else {
                    format!("some_moderately_long_name_{i:06}.{ext}")
                };
                root.join(name)
                    .as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect()
            })
            .collect();

        let started = Instant::now();
        for wide in &names {
            let mut info = SHFILEINFOW::default();
            unsafe {
                SHGetFileInfoW(
                    wide.as_ptr(),
                    0,
                    &mut info,
                    std::mem::size_of::<SHFILEINFOW>() as u32,
                    SHGFI_TYPENAME,
                )
            };
            std::hint::black_box(info.szTypeName[0]);
        }
        let shell_ns = started.elapsed().as_nanos() as f64 / SAMPLE as f64;
        println!(
            "SHGetFileInfo type name:      {:.0} ns/entry  \
             (= {:.0} ms for {COUNT} entries, {:.0}x the whole scan)",
            shell_ns,
            shell_ns * COUNT as f64 / 1_000_000.0,
            shell_ns * COUNT as f64 / 1000.0 / best as f64
        );

        // The table this program uses instead, over the same sample.
        let started = Instant::now();
        let mut label = String::new();
        for i in 0..SAMPLE {
            label.clear();
            super::super::fmt::type_label(exts[i % exts.len()], false, &mut label);
            std::hint::black_box(label.len());
        }
        println!(
            "the static table instead:     {:.0} ns/entry",
            started.elapsed().as_nanos() as f64 / SAMPLE as f64
        );
    }

    // Sorting is the other half of what happens before a listing appears.
    let mut order = Vec::new();
    let dir = scan(&root);
    let started = Instant::now();
    super::super::sort::build_order(
        &dir,
        &mut order,
        super::super::Column::Name,
        true,
        false,
        "",
        None,
        None,
    );
    let sort_us = started.elapsed().as_micros();
    println!("natural sort of {COUNT} entries: {:.1} ms", sort_us as f64 / 1000.0);
    assert_eq!(order.len(), COUNT);

    // Not a tight bound — a loaded machine or a slow volume can be several times
    // this. It is here to catch a regression of the kind that matters: a `stat`
    // per entry, or an allocation per name, which would be ten times over.
    assert!(
        per_entry_ns < 3_000.0,
        "{per_entry_ns:.0} ns per entry — something has started doing per-file work"
    );

    crate::sandbox::remove(&root);
}

// ---------------------------------------------------------------------------
// File IDs
// ---------------------------------------------------------------------------

/// **The whole reason the directory is read with IDs**: keywords are kept under one, and a rename
/// is exactly the thing they have to survive. Two files are two IDs, too.
#[test]
#[cfg(windows)]
fn a_file_keeps_its_id_through_a_rename() {
    let root = crate::sandbox::fresh("file-ids");
    std::fs::write(root.join("before.txt"), b"x").expect("fixture");
    std::fs::write(root.join("other.txt"), b"y").expect("fixture");
    let key_of = |dir: &Dir, name: &str| {
        let i = (0..dir.len()).find(|&i| dir.name(i) == name).expect("listed");
        dir.key(i)
    };

    let dir = scan(&root);
    assert!(dir.keyed(), "the sandbox is on NTFS, so its listing has IDs");
    let before = key_of(&dir, "before.txt").expect("an ID");
    assert_ne!(Some(before), key_of(&dir, "other.txt"), "two files are two IDs");

    std::fs::rename(root.join("before.txt"), root.join("after.txt")).expect("rename");
    let dir = scan(&root);
    assert_eq!(key_of(&dir, "after.txt"), Some(before), "the renamed file is the same file");

    crate::sandbox::remove(&root);
}

/// The ID read and the find API describe every entry the same way, because the rest of the program
/// was written against the find API's answer: the same names, in the same order, with the same
/// sizes, times and flags.
#[test]
#[cfg(windows)]
fn both_ways_of_reading_a_directory_agree() {
    let path = sources();
    let ids = scan(&path);
    let find = win::scan_find(&path, Instant::now());
    assert!(ids.keyed(), "this repository is on NTFS");
    assert!(!find.keyed(), "the find API has no IDs to give");
    let row = |dir: &Dir, i: usize| {
        let e = dir.entries[i];
        (dir.name(i).to_owned(), e.size, e.modified, e.flags)
    };
    let all = |dir: &Dir| (0..dir.len()).map(|i| row(dir, i)).collect::<Vec<_>>();
    assert_eq!(all(&ids), all(&find));
}

/// A flattened listing carries the IDs of what it walked, so a file's keywords are the same in
/// a flatten as in its own folder.
#[test]
#[cfg(windows)]
fn a_flattened_listing_keeps_the_ids_it_walked() {
    let root = crate::sandbox::fresh("flat-ids");
    std::fs::create_dir_all(root.join("sub")).expect("fixture");
    std::fs::write(root.join("sub").join("deep.txt"), b"x").expect("fixture");

    let own = scan(&root.join("sub"));
    let flat = walk(&root, FLATTEN_BUDGET, FLATTEN_PATIENCE, 1);
    let at = |dir: &Dir, name: &str| (0..dir.len()).find(|&i| dir.name(i) == name).expect("listed");
    assert!(own.key(at(&own, "deep.txt")).is_some());
    assert_eq!(flat.key(at(&flat, r"sub\deep.txt")), own.key(at(&own, "deep.txt")));

    crate::sandbox::remove(&root);
}

/// A drive's root is opened with its backslash — without one, `\?\D:` names the *volume*, and the
/// read would quietly fall back to having no IDs. Read-only: the root of the drive the sandbox is on.
#[test]
#[cfg(windows)]
fn a_drive_root_is_read_with_ids() {
    let sandbox = crate::sandbox::root();
    let root: PathBuf = sandbox.components().take(2).collect();
    assert!(root.to_string_lossy().ends_with(":\\"), "{}", root.display());
    let dir = scan(&root);
    assert!(dir.error.is_none(), "{:?}", dir.error);
    assert!(dir.keyed(), "{} came back without IDs", root.display());
}
