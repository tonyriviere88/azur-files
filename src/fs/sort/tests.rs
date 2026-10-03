use super::*;
use crate::fs::dir::{DirBuilder, FLAG_DIR};

/// What one keystroke in the filter box costs on a listing big enough to notice.
///
/// ```text
/// cargo test --release -- --ignored --nocapture filter_speed
/// ```
///
/// This is the measurement behind [`crate::pane::FILTER_DELAY`]: a filter is re-applied from
/// scratch on every change, so if one pass is slow then *typing* is slow, and no amount of
/// making the pass faster fixes a listing where every keystroke costs a pass. The tree is
/// whatever `YAFE_FLATTEN_ROOT` names, flattened — which is the biggest listing this program
/// can produce and the case the delay exists for.
///
/// **Both flatten modes**, because the delay covers both and the tree's pass does strictly more
/// work: it groups every row by the folder it is in before it sorts anything. If that were the
/// difference between a pause and a stall it would be the tree mode's own problem and not the
/// delay's, which is the thing this is here to say either way.
#[test]
#[ignore = "walks a large tree; run explicitly"]
fn filter_speed() {
    let root = std::env::var("YAFE_FLATTEN_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target")
        });
    if !root.is_dir() {
        println!("no {}; skipping", root.display());
        return;
    }
    let dir = crate::fs::scan::scan_deep(
        &root,
        crate::fs::scan::FLATTEN_BUDGET,
        std::time::Duration::from_secs(600),
    );
    println!("{} entries from {}", dir.len(), root.display());

    let mut order = Vec::new();
    // Regrouped, which is how a tree is shown by default — so the figures below are the ones a
    // keystroke actually costs.
    let mut shape = Vec::new();
    // The single words are what typing `exe` costs, one keystroke at a time. The last
    // three are the syntax: a word that keeps everything, two words that each cost a
    // pass over every path, and an exclusion — which is the worst case, because a term
    // that fails is a term that has searched the whole path first.
    for filter in [
        "", "e", "ex", "exe", "microsoft", "^c:", "micro exe", "!exe",
    ] {
        // Twice, and the second is the one to read: the first pass warms the caches the
        // listing's own arena needs, which a real second keystroke would find warm too.
        let mut took = std::time::Duration::ZERO;
        let mut as_tree = std::time::Duration::ZERO;
        let mut tree_rows = 0;
        for _ in 0..2 {
            let started = std::time::Instant::now();
            build_order(&dir, &mut order, Column::Type, true, false, filter, None);
            took = started.elapsed();

            let started = std::time::Instant::now();
            build_tree_order(
                &dir,
                &mut order,
                &mut shape,
                Column::Type,
                true,
                false,
                filter,
                None,
                &|_| false,
                true,
            );
            as_tree = started.elapsed();
            tree_rows = order.len();
        }
        println!(
            "  filter {:>12}: list {:>7.2} ms · tree {:>7.2} ms ({tree_rows} rows)",
            format!("{filter:?}"),
            took.as_secs_f64() * 1000.0,
            as_tree.as_secs_f64() * 1000.0,
        );
    }
}

fn sorted(mut names: Vec<&str>) -> Vec<&str> {
    names.sort_by(|a, b| natural_cmp(a, b));
    names
}

/// A listing of the given names, folders marked by a trailing `/`.
fn listing(names: &[&str]) -> Dir {
    let mut builder = DirBuilder::new(r"C:\x");
    for name in names {
        match name.strip_suffix('/') {
            Some(folder) => builder.push(folder, 0, 0, FLAG_DIR),
            None => builder.push(name, 1, 0, 0),
        }
    }
    builder.finish(0)
}

fn by(dir: &Dir, column: Column, ascending: bool) -> Vec<String> {
    let mut order = Vec::new();
    build_order(dir, &mut order, column, ascending, true, "", None);
    order
        .iter()
        .map(|&i| dir.name(i as usize).to_owned())
        .collect()
}

#[test]
fn type_sorts_by_the_label_the_column_shows() {
    // Not by extension: `cpp` < `exe`, but "Application" < "C++ source". Sorting on
    // the extension put the column visibly out of order, which is what this is for.
    let dir = listing(&["b.cpp", "a.exe"]);
    assert_eq!(by(&dir, Column::Type, true), ["a.exe", "b.cpp"]);
    assert_eq!(by(&dir, Column::Type, false), ["b.cpp", "a.exe"]);
}

#[test]
fn two_extensions_with_one_label_form_one_group() {
    // Both are "JPEG image", so they rank equal and the name tie-break interleaves
    // them — one group in the column rather than two that happen to be adjacent.
    let dir = listing(&["z.jpg", "a.jpeg", "m.jpg"]);
    assert_eq!(by(&dir, Column::Type, true), ["a.jpeg", "m.jpg", "z.jpg"]);
}

#[test]
fn a_type_sort_still_orders_by_name_inside_a_type() {
    let dir = listing(&["b.txt", "a.txt", "c.exe"]);
    assert_eq!(by(&dir, Column::Type, true), ["c.exe", "a.txt", "b.txt"]);
}

#[test]
fn folders_lead_every_column_and_ignore_its_key() {
    // A folder has no type of its own, and none of the labels may reorder it out of
    // the block at the top.
    let dir = listing(&["zebra/", "a.exe", "alpha/"]);
    assert_eq!(
        by(&dir, Column::Type, true),
        ["alpha", "zebra", "a.exe"],
        "folders first, in name order"
    );
    assert_eq!(
        by(&dir, Column::Type, false)[0],
        "zebra",
        "still folders first when reversed"
    );
}

#[test]
fn digits_compare_as_numbers() {
    assert_eq!(
        sorted(vec!["file10.txt", "file2.txt", "file1.txt"]),
        vec!["file1.txt", "file2.txt", "file10.txt"]
    );
    assert_eq!(
        sorted(vec!["scan_100", "scan_9", "scan_20"]),
        vec!["scan_9", "scan_20", "scan_100"]
    );
}

#[test]
fn case_is_ignored_but_still_breaks_ties() {
    assert_eq!(natural_cmp("Alpha", "alpha"), natural_cmp("Alpha", "alpha"));
    assert_eq!(natural_cmp("beta", "Alpha"), Ordering::Greater);
    // A pure case difference has to be *some* consistent order, not Equal, or
    // an unstable sort could swap two rows between frames.
    assert_ne!(natural_cmp("Alpha", "alpha"), Ordering::Equal);
}

#[test]
fn leading_zeros_do_not_change_the_value() {
    assert_eq!(
        sorted(vec!["v007", "v10", "v7", "v0008"]),
        vec!["v7", "v007", "v0008", "v10"]
    );
}

#[test]
fn prefixes_come_first() {
    assert_eq!(
        sorted(vec!["report.txt", "report", "reports"]),
        vec!["report", "report.txt", "reports"]
    );
}

#[test]
fn ordering_is_a_total_order() {
    // An unstable sort with an inconsistent comparator can loop or panic, so
    // check antisymmetry across a set built to collide.
    let names = [
        "a", "A", "a1", "a01", "a2", "a10", "b", "", "1", "01", "2", "z9z", "z10z",
    ];
    for x in names {
        for y in names {
            assert_eq!(
                natural_cmp(x, y).reverse(),
                natural_cmp(y, x),
                "`{x}` vs `{y}`"
            );
        }
    }
}

/// What survives `filter` in a listing of `C:\x`.
fn kept(names: &[&str], filter: &str) -> Vec<String> {
    let dir = listing(names);
    let mut order = Vec::new();
    build_order(&dir, &mut order, Column::Name, true, true, filter, None);
    order
        .iter()
        .map(|&i| dir.name(i as usize).to_owned())
        .collect()
}

#[test]
fn the_filter_is_a_case_insensitive_substring_of_the_name() {
    // The one-word case, which is what a filter box holds nearly all of the time.
    let names = ["README.md", "some-readme-file", "read.me"];
    assert_eq!(
        kept(&names, "  ReadMe "),
        ["README.md", "some-readme-file"],
        "either case, and not `read.me`, which has the letters but not the word"
    );
    assert_eq!(kept(&names, "").len(), 3, "no filter keeps everything");
    assert_eq!(kept(&names, "   ").len(), 3, "nor does whitespace");
}

#[test]
fn the_filter_is_asked_about_the_whole_path() {
    // The folder is `C:\x`, so it is there to be matched, anchored and excluded — and
    // it is shared, so a word only it holds keeps every row.
    assert_eq!(kept(&["a.txt", "b.txt"], r"c:\x").len(), 2);
    assert_eq!(kept(&["a.txt", "b.txt"], "^C:").len(), 2);
    assert_eq!(kept(&["a.txt", "b.txt"], "!x").len(), 0);
    assert_eq!(kept(&["a.txt", "b.txt"], "^y").len(), 0);

    // And the folders in front of a flattened row's name are matched with it.
    assert_eq!(
        kept(&[r"deep\a.txt", "b.txt"], "deep"),
        [r"deep\a.txt"],
        "the row's own folders count"
    );
}

#[test]
fn every_word_of_the_filter_narrows_in_any_order() {
    let names = ["hello world.txt", "world hello.txt", "hello to you.txt"];
    for filter in ["wor he", "he wor"] {
        assert_eq!(
            kept(&names, filter),
            ["hello world.txt", "world hello.txt"],
            "`{filter}`"
        );
    }
    // The two markers that make it a filter rather than a search. `.txt$` would hold
    // for all three, so the exclusion is what is being read here.
    assert_eq!(kept(&names, "!world .txt$"), ["hello to you.txt"]);
}

#[test]
fn a_filter_of_nothing_but_markers_keeps_everything() {
    // Every prefix of a query is typed on the way to it, and a listing that empties on
    // the first keystroke of `^src` and fills again on the second is a flicker.
    for filter in ["^", "!", "$", "^$"] {
        assert_eq!(kept(&["a.txt", "b.txt"], filter).len(), 2, "`{filter}`");
    }
}

/// **The pictures, and in a tree the folders that lead to one — and nothing else either way.**
///
/// The three claims worth holding. The type comes from the extension table, so `.PNG` is a
/// picture and `.rs` is not, whatever case it is written in. A flat list gets **no folders at
/// all**: every row already carries the path down to it, so a folder row there is a row saying
/// nothing. And a tree gets exactly the folders on the way to a picture — `src`, holding only
/// source, is not one of them, which is the whole difference between this and letting every
/// folder through for the sake of the ones that matter.
#[test]
fn the_images_lens_keeps_the_pictures_and_the_folders_that_lead_to_them() {
    let dir = listing(&[
        "photos/",
        "src/",
        "readme.md",
        r"photos\a.PNG",
        r"photos\raw/",
        r"photos\raw\b.tif",
        r"src\main.rs",
    ]);
    // Named for what it answers rather than `kept`, which in this module is the *name* filter's
    // own helper a few tests up.
    let shown = |tree: bool| -> Vec<String> {
        image_rows(&dir, tree)
            .iter()
            .enumerate()
            .filter(|(_, &keep)| keep)
            .map(|(i, _)| dir.name(i).to_owned())
            .collect()
    };

    assert_eq!(shown(false), [r"photos\a.PNG", r"photos\raw\b.tif"]);
    assert_eq!(
        shown(true),
        [
            "photos",
            r"photos\a.PNG",
            r"photos\raw",
            r"photos\raw\b.tif"
        ]
    );
}

/// The lens half: a row has to pass both tests.
#[test]
fn the_lens_half_of_a_filter_is_asked_of_every_row() {
    let mut builder = DirBuilder::new(r"C:\repo");
    for name in ["kept.rs", "gone.rs", "kept.txt"] {
        builder.push(name, 1, 0, 0);
    }
    let dir = builder.finish(0);
    let changed = |entry: usize| dir.name(entry).starts_with("kept");

    let mut order = Vec::new();
    build_order(
        &dir,
        &mut order,
        Column::Name,
        true,
        true,
        "",
        Some(&changed),
    );
    let names: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();
    assert_eq!(names, ["kept.rs", "kept.txt"], "the git test alone");

    build_order(
        &dir,
        &mut order,
        Column::Name,
        true,
        true,
        ".rs$",
        Some(&changed),
    );
    let names: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();
    assert_eq!(names, ["kept.rs"], "and both tests together");
}

// -----------------------------------------------------------------------
// The tree the same rows make
// -----------------------------------------------------------------------

/// A flattened tree, in the order the walk produces one: a folder is always pushed before
/// anything inside it, and the levels arrive one after another. The listing under test has to
/// be built this way round or it is not testing what `scan_deep` hands over.
fn tree_listing() -> Dir {
    listing(&[
        "docs/",
        "src/",
        "readme.md",
        r"docs\guide.md",
        r"src\ui/",
        r"src\main.rs",
        r"src\ui\list.rs",
        r"src\ui\theme.rs",
    ])
}

fn as_tree_by(dir: &Dir, column: Column, filter: &str, shut: &[&str]) -> Vec<String> {
    as_shown_by(dir, column, filter, shut, false)
        .into_iter()
        .map(|(name, _, _)| name)
        .collect()
}

/// The tree as it is **shown**: every row's name, the depth it is drawn at, and how many folders
/// are merged into it. Which is the whole of what [`TreeRow`] says, so the merge tests can assert
/// the shape rather than infer it from the names.
fn as_shown_by(
    dir: &Dir,
    column: Column,
    filter: &str,
    shut: &[&str],
    regroup: bool,
) -> Vec<(String, u32, u32)> {
    let (mut order, mut shape) = (Vec::new(), Vec::new());
    build_tree_order(
        dir,
        &mut order,
        &mut shape,
        column,
        true,
        true,
        filter,
        None,
        &|name| shut.contains(&name),
        regroup,
    );
    assert_eq!(order.len(), shape.len(), "the order and its shape came apart");
    order
        .iter()
        .zip(&shape)
        .map(|(&i, row)| (dir.name(i as usize).to_owned(), row.depth, row.merged))
        .collect()
}

/// The same, regrouped, which is how a tree is shown by default.
fn regrouped(dir: &Dir, filter: &str, shut: &[&str]) -> Vec<(String, u32, u32)> {
    as_shown_by(dir, Column::Name, filter, shut, true)
}

fn as_tree(dir: &Dir, filter: &str, shut: &[&str]) -> Vec<String> {
    as_tree_by(dir, Column::Name, filter, shut)
}

/// **A folder, then what is inside it, then the next folder** — and folders before files at
/// every level, which is the one rule the flat order already had.
#[test]
fn a_tree_is_pre_order_with_each_level_sorted_among_itself() {
    assert_eq!(
        as_tree(&tree_listing(), "", &[]),
        [
            "docs",
            r"docs\guide.md",
            "src",
            r"src\ui",
            r"src\ui\list.rs",
            r"src\ui\theme.rs",
            r"src\main.rs",
            "readme.md",
        ]
    );
}

/// The same rows the list mode shows, and only those: the two modes are two orders over one
/// listing, so a row missing from one of them is a bug in that one.
#[test]
fn the_tree_and_the_list_hold_the_same_rows() {
    let dir = tree_listing();
    let mut flat = as_tree(&dir, "", &[]);
    let mut list = by(&dir, Column::Name, true);
    flat.sort();
    list.sort();
    assert_eq!(flat, list);
}

/// A shut folder takes its whole subtree out of the order — not merely off the screen.
///
/// Which is what makes a collapsed tree cheap: the rows below it are not drawn, not hit-tested
/// and not scrolled past, because as far as the listing is concerned they are not there.
#[test]
fn a_shut_folder_keeps_its_subtree_out_of_the_order() {
    let dir = tree_listing();
    assert_eq!(
        as_tree(&dir, "", &["src"]),
        ["docs", r"docs\guide.md", "src", "readme.md"],
        "`src` is still a row; nothing under it is"
    );
    // Shutting a folder in the middle keeps the folder and drops the two leaves under it.
    assert_eq!(
        as_tree(&dir, "", &[r"src\ui"]),
        [
            "docs",
            r"docs\guide.md",
            "src",
            r"src\ui",
            r"src\main.rs",
            "readme.md",
        ]
    );
}

/// **A filter keeps the folders that lead to a match**, or the match is a row with nothing
/// above it to say where it came from.
#[test]
fn a_filtered_tree_keeps_the_folders_that_lead_to_a_match() {
    let dir = tree_listing();
    assert_eq!(
        as_tree(&dir, "theme", &[]),
        ["src", r"src\ui", r"src\ui\theme.rs"],
        "the two folders are not matches; they are the way to the one that is"
    );
    // And a match that is itself a folder brings what is inside it, because the filter matches
    // every row under it too — every one of their paths contains the folder's name.
    assert_eq!(
        as_tree(&dir, "docs", &[]),
        ["docs", r"docs\guide.md"]
    );
}

/// And it ignores what is shut, because a filter that answered and then hid the answer inside a
/// folder you closed an hour ago would be worse than one that found nothing.
#[test]
fn a_filter_reaches_into_a_shut_folder() {
    let dir = tree_listing();
    assert_eq!(
        as_tree(&dir, "list.rs", &["src", r"src\ui"]),
        ["src", r"src\ui", r"src\ui\list.rs"],
    );
    // The set is not modified by that — clearing the filter puts the tree back as it was, which
    // is `Tab::collapsed` living on the tab and this taking a closure over it.
    assert_eq!(as_tree(&dir, "", &["src", r"src\ui"]).len(), 4);
}

/// A row that is out takes its subtree with it: you cannot see inside a folder you cannot see.
///
/// The same statement for both tests that can exclude a row — `show_hidden` and the git
/// question — because a tree has nothing to hang an orphan off. Browsing does the same: a
/// hidden folder hides what is in it, and it is the folder that is marked hidden, not each file.
#[test]
fn a_hidden_folder_hides_what_is_under_it() {
    use crate::fs::dir::FLAG_HIDDEN;
    let mut builder = DirBuilder::new(r"C:\x");
    builder.push("shown", 0, 0, FLAG_DIR);
    builder.push("hidden", 0, 0, FLAG_DIR | FLAG_HIDDEN);
    builder.push(r"shown\a.txt", 1, 0, 0);
    // Not hidden itself — only the folder it is in is.
    builder.push(r"hidden\b.txt", 1, 0, 0);
    let dir = builder.finish(0);

    let (mut order, mut shape) = (Vec::new(), Vec::new());
    build_tree_order(
        &dir,
        &mut order,
        &mut shape,
        Column::Name,
        true,
        false,
        "",
        None,
        &|_| false,
        false,
    );
    let names: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();
    assert_eq!(names, ["shown", r"shown\a.txt"]);

    // With hidden files shown, both branches are there — which is what says the rule above is
    // about the folder being hidden and not about `b.txt`.
    build_tree_order(
        &dir,
        &mut order,
        &mut shape,
        Column::Name,
        true,
        true,
        "",
        None,
        &|_| false,
        false,
    );
    assert_eq!(order.len(), 4);
}

/// A sort by a column orders each folder's own children, and never moves a row out of the
/// folder it is in — which is the difference between sorting a tree and flattening it.
#[test]
fn a_column_sort_orders_siblings_and_not_the_tree() {
    let mut builder = DirBuilder::new(r"C:\x");
    builder.push("a", 0, 0, FLAG_DIR);
    builder.push("z.txt", 9, 0, 0);
    builder.push(r"a\big.txt", 100, 0, 0);
    builder.push(r"a\small.txt", 1, 0, 0);
    let dir = builder.finish(0);

    let (mut order, mut shape) = (Vec::new(), Vec::new());
    build_tree_order(
        &dir,
        &mut order,
        &mut shape,
        Column::Size,
        false,
        true,
        "",
        None,
        &|_| false,
        false,
    );
    let names: Vec<&str> = order.iter().map(|&i| dir.name(i as usize)).collect();
    assert_eq!(
        names,
        ["a", r"a\big.txt", r"a\small.txt", "z.txt"],
        "biggest first inside `a`, and `z.txt` is still not inside it"
    );
}

/// A folder chain fixture: one of every case the merge has to answer.
///
/// Level by level, the way [`super::scan::scan_deep`] hands a listing over — `conf` with one
/// *file* in it, `deep` with one empty folder, `lib` with two things in it, and `src` with a
/// three-deep chain of only-children ending in a folder that holds two files.
fn chain_listing() -> Dir {
    listing(&[
        "conf/",
        "deep/",
        "lib/",
        "src/",
        r"conf\app.ini",
        r"deep\deeper/",
        r"lib\a.txt",
        r"lib\b/",
        r"src\main/",
        r"lib\b\c.txt",
        r"src\main\java/",
        r"src\main\java\App.java",
        r"src\main\java\Other.java",
    ])
}

/// **A folder whose whole content is one folder is not a row of its own.**
///
/// Four rules in one listing, and the depths are half of what is being asserted: the row that
/// comes out is the *innermost* folder of the chain, drawn where the outermost one stood, with
/// everything inside it one level in from there. A chain merges as many levels as the rule holds
/// for — `src\main\java` is one row of three folders — and it stops at anything else in the
/// folder: a file (`conf`), a second entry (`lib`), and nothing at all (`deep\deeper`, whose
/// chain forms and then ends).
#[test]
fn a_folder_holding_nothing_but_one_folder_is_merged_into_its_row() {
    let shown = regrouped(&chain_listing(), "", &[]);
    assert_eq!(
        shown,
        [
            // One file in it is not one folder in it.
            ("conf".to_owned(), 0, 0),
            (r"conf\app.ini".to_owned(), 1, 0),
            // One folder, and it is empty: the chain is still a chain.
            (r"deep\deeper".to_owned(), 0, 1),
            // Two things in it, so both stay.
            ("lib".to_owned(), 0, 0),
            (r"lib\b".to_owned(), 1, 0),
            (r"lib\b\c.txt".to_owned(), 2, 0),
            (r"lib\a.txt".to_owned(), 1, 0),
            // Three folders, one row, drawn where `src` was — and its files are at depth 1
            // rather than at the 3 their paths would say.
            (r"src\main\java".to_owned(), 0, 2),
            (r"src\main\java\App.java".to_owned(), 1, 0),
            (r"src\main\java\Other.java".to_owned(), 1, 0),
        ]
    );
}

/// Turned off, the same listing is the whole ladder again.
///
/// The other half of the option being an option — and the reason the figures matter: with nothing
/// merged, every row's depth is its path's depth, which is what it was before any of this.
#[test]
fn without_regrouping_every_folder_of_a_chain_keeps_its_row() {
    let shown = as_shown_by(&chain_listing(), Column::Name, "", &[], false);
    let ladder: Vec<(String, u32, u32)> = shown
        .iter()
        .filter(|(name, _, _)| name.starts_with("src"))
        .cloned()
        .collect();
    assert_eq!(
        ladder,
        [
            ("src".to_owned(), 0, 0),
            (r"src\main".to_owned(), 1, 0),
            (r"src\main\java".to_owned(), 2, 0),
            (r"src\main\java\App.java".to_owned(), 3, 0),
            (r"src\main\java\Other.java".to_owned(), 3, 0),
        ]
    );
}

/// A chain cannot reach through a closed door, and the door is the merged row's own.
///
/// Two different shuts, and they are the two halves of the gesture. Shutting the row the user can
/// see — the innermost folder, which is what its twisty is keyed on — leaves the chain merged and
/// takes away what is inside it, which is what shutting a row should do. Shutting a folder in the
/// *middle* of a chain is the state a stale key leaves behind, and it breaks the chain there
/// rather than merging through it: what is behind a closed door is not on show, so it cannot be
/// drawn as part of the row in front of it.
#[test]
fn a_shut_folder_stops_a_chain_where_it_is() {
    let dir = chain_listing();

    // The row as shown, shut: still one row of three folders, and nothing under it.
    assert_eq!(
        regrouped(&dir, "", &[r"src\main\java"]),
        [
            ("conf".to_owned(), 0, 0),
            (r"conf\app.ini".to_owned(), 1, 0),
            (r"deep\deeper".to_owned(), 0, 1),
            ("lib".to_owned(), 0, 0),
            (r"lib\b".to_owned(), 1, 0),
            (r"lib\b\c.txt".to_owned(), 2, 0),
            (r"lib\a.txt".to_owned(), 1, 0),
            (r"src\main\java".to_owned(), 0, 2),
        ]
    );

    // And shut at the top of the chain: `src` is a row of its own again, with the door closed.
    let shown = regrouped(&dir, "", &["src"]);
    assert_eq!(shown.last(), Some(&("src".to_owned(), 0, 0)));
    assert_eq!(shown.len(), 8, "nothing under a shut folder is in the order");
}

/// Under a filter, a chain forms out of whatever the filter left.
///
/// Which is the same rule read honestly — "only one folder in it" is a question about the rows
/// that are *on show* — and it is what makes a filtered tree readable instead of a ladder of
/// single matches. `c.txt` leaves `lib` holding one folder where it held two, so `lib > b`
/// becomes one row that is not there without the filter.
#[test]
fn a_filter_makes_chains_of_the_rows_it_leaves() {
    let dir = chain_listing();
    assert_eq!(
        regrouped(&dir, "c.txt", &[]),
        [
            (r"lib\b".to_owned(), 0, 1),
            (r"lib\b\c.txt".to_owned(), 1, 0),
        ],
        "the filter left `lib` one folder, so it is a chain"
    );

    // And the folders leading to a match still merge as far as the match: a file at the end of
    // one is a file, so the chain stops at the folder holding it. `conf` is in this listing
    // because `app.ini` matches `App` too, which is the case worth having beside the other one —
    // the same filter leaves one folder a chain and the other a row of its own, and the
    // difference is only what is left inside them.
    assert_eq!(
        regrouped(&dir, "App", &[]),
        [
            ("conf".to_owned(), 0, 0),
            (r"conf\app.ini".to_owned(), 1, 0),
            (r"src\main\java".to_owned(), 0, 2),
            (r"src\main\java\App.java".to_owned(), 1, 0),
        ]
    );
}

/// The Type column, which is the one whose ordering costs a table the size of the listing.
///
/// Worth its own test because the tree computes that table **once** and the list computes it
/// per call: one per sibling group would be the listing's length squared. So this is both an
/// ordering assertion and the only thing that would catch the table being indexed wrong.
#[test]
fn a_tree_sorted_by_type_ranks_every_level_from_one_table() {
    let dir = listing(&["a/", r"a\z.exe", r"a\b.cpp", "c.cpp", "b.exe"]);
    assert_eq!(
        as_tree_by(&dir, Column::Type, "", &[]),
        ["a", r"a\z.exe", r"a\b.cpp", "b.exe", "c.cpp"],
        "Application before C++ source, at both levels"
    );
}

#[test]
fn the_filter_survives_a_listing_with_no_folder_of_its_own() {
    // "This PC": no path, rows carrying their own targets. The prefix is empty and the
    // name is the whole haystack, which is why no path is joined onto anything here.
    let mut builder = DirBuilder::new("");
    builder.push_link("Windows (C:)", r"C:\".into(), 0, FLAG_DIR);
    builder.push_link("Data (D:)", r"D:\".into(), 0, FLAG_DIR);
    let dir = builder.finish(0);

    let mut order = Vec::new();
    build_order(&dir, &mut order, Column::Name, true, true, "(c:)", None);
    assert_eq!(order.len(), 1);
    assert_eq!(dir.name(order[0] as usize), "Windows (C:)");
}
