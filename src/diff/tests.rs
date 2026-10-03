use super::*;
use crate::fs::dir::{DirBuilder, FLAG_DIR, FLAG_HIDDEN};

/// A flattened listing out of `(relative path, modified, flags)`, the shape `scan_deep` gives.
fn tree(rows: &[(&str, u64, u16)]) -> Arc<Dir> {
    sized(&rows.iter().map(|&(name, modified, flags)| (name, 1, modified, flags)).collect::<Vec<_>>())
}

/// The same, with a size for each row.
fn sized(rows: &[(&str, u64, u64, u16)]) -> Arc<Dir> {
    let mut builder = DirBuilder::new("X:\\root");
    for &(name, size, modified, flags) in rows {
        builder.push(name, size, modified, flags);
    }
    Arc::new(builder.finish(0))
}

fn changed(kind: bool, size: bool, modified: bool) -> Option<Mark> {
    Some(Mark::Differs(Cells { kind, size, modified }))
}

fn marks(dir: &Arc<Dir>, side: &Side) -> Vec<(String, Option<Mark>)> {
    (0..dir.len())
        .map(|i| (dir.name(i).to_owned(), side.mark(dir, i)))
        .collect()
}

fn mark_of(dir: &Arc<Dir>, side: &Side, name: &str) -> Option<Mark> {
    marks(dir, side)
        .into_iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, m)| m)
}

fn pair(left: &Arc<Dir>, right: &Arc<Dir>, show_hidden: bool) -> Pair {
    Pair {
        left: left.clone(),
        right: right.clone(),
        show_hidden,
    }
}

/// What the status line says for the left side.
fn summary_of(side: &Side) -> String {
    side.comparison.as_ref().unwrap().left.summary.clone()
}

fn sides(left: &Arc<Dir>, right: &Arc<Dir>, hidden: bool) -> (Side, Side) {
    let comparison = Arc::new(compare(pair(left, right, hidden)));
    let l = Side {
        twin: Some(7),
        comparison: Some(comparison.clone()),
        ..Side::default()
    };
    let r = Side {
        comparison: Some(comparison),
        ..Side::default()
    };
    (l, r)
}

#[test]
fn identical_trees_have_nothing_to_highlight() {
    let rows = [("src", 0, FLAG_DIR), ("src\\main.rs", 100, 0), ("README", 50, 0)];
    let (a, b) = (tree(&rows), tree(&rows));
    let (l, r) = sides(&a, &b, false);
    assert!(marks(&a, &l).iter().all(|(_, m)| *m == Some(Mark::Same)));
    assert!(marks(&b, &r).iter().all(|(_, m)| *m == Some(Mark::Same)));
    assert_eq!(summary_of(&l), "No differences  ·  3 identical");
}

#[test]
fn a_file_on_one_side_only_and_the_folders_above_it() {
    let a = tree(&[
        ("src", 0, FLAG_DIR),
        ("src\\ui", 0, FLAG_DIR),
        ("src\\ui\\new.rs", 100, 0),
        ("src\\ui\\old.rs", 100, 0),
        ("docs", 0, FLAG_DIR),
    ]);
    let b = tree(&[
        ("src", 0, FLAG_DIR),
        ("src\\ui", 0, FLAG_DIR),
        ("src\\ui\\old.rs", 100, 0),
        ("docs", 0, FLAG_DIR),
    ]);
    let (l, r) = sides(&a, &b, false);
    assert_eq!(mark_of(&a, &l, "src\\ui\\new.rs"), Some(Mark::Only));
    assert_eq!(mark_of(&a, &l, "src\\ui"), Some(Mark::Holds));
    assert_eq!(mark_of(&a, &l, "src"), Some(Mark::Holds));
    assert_eq!(mark_of(&a, &l, "docs"), Some(Mark::Same));
    assert_eq!(mark_of(&a, &l, "src\\ui\\old.rs"), Some(Mark::Same));
    // The other side is missing it, so its folders are the way down to where it is missing.
    assert_eq!(mark_of(&b, &r, "src"), Some(Mark::Holds));
    assert_eq!(mark_of(&b, &r, "src\\ui"), Some(Mark::Holds));
    assert_eq!(mark_of(&b, &r, "docs"), Some(Mark::Same));
    assert_eq!(mark_of(&b, &r, "src\\ui\\old.rs"), Some(Mark::Same));
}

#[test]
fn a_folder_only_here_takes_everything_under_it() {
    let a = tree(&[("gone", 0, FLAG_DIR), ("gone\\a.txt", 1, 0), ("gone\\b", 0, FLAG_DIR)]);
    let b = tree(&[]);
    let (l, _) = sides(&a, &b, false);
    assert!(marks(&a, &l).iter().all(|(_, m)| *m == Some(Mark::Only)));
    assert_eq!(summary_of(&l), "3 only here  ·  0 identical");
}

#[test]
fn a_changed_file_differs_in_exactly_the_cells_that_changed() {
    let a = sized(&[
        ("lib", 0, 0, FLAG_DIR),
        ("lib\\x.dll", 10, 500_000_000, 0),
        ("lib\\y.dll", 10, 100, 0),
        ("lib\\z.dll", 10, 100, 0),
    ]);
    let b = sized(&[
        ("lib", 0, 0, FLAG_DIR),
        ("lib\\x.dll", 10, 100_000_000, 0),
        ("lib\\y.dll", 20, 100, 0),
        ("lib\\z.dll", 20, 900_000_000, 0),
    ]);
    let (l, r) = sides(&a, &b, false);
    assert_eq!(mark_of(&a, &l, "lib\\x.dll"), changed(false, false, true));
    assert_eq!(mark_of(&b, &r, "lib\\x.dll"), changed(false, false, true));
    assert_eq!(mark_of(&a, &l, "lib\\y.dll"), changed(false, true, false));
    assert_eq!(mark_of(&a, &l, "lib\\z.dll"), changed(false, true, true));
    assert_eq!(mark_of(&a, &l, "lib"), Some(Mark::Holds));
    assert_eq!(mark_of(&b, &r, "lib"), Some(Mark::Holds));
    assert!(summary_of(&l).starts_with("3 different"));
}

#[test]
fn two_seconds_apart_is_the_same_time() {
    let a = tree(&[("f", 1_000_000_000, 0)]);
    let b = tree(&[("f", 1_000_000_000 + TOLERANCE, 0)]);
    let (l, _) = sides(&a, &b, false);
    assert_eq!(mark_of(&a, &l, "f"), Some(Mark::Same));
    let c = tree(&[("f", 1_000_000_000 + TOLERANCE + 1, 0)]);
    let (l, _) = sides(&a, &c, false);
    assert_eq!(mark_of(&a, &l, "f"), changed(false, false, true));
}

#[test]
fn names_match_whatever_their_case_or_slash() {
    let a = tree(&[("Src", 0, FLAG_DIR), ("Src\\Main.RS", 5, 0)]);
    let b = tree(&[("src", 0, FLAG_DIR), ("src/main.rs", 5, 0)]);
    let (l, r) = sides(&a, &b, false);
    assert!(marks(&a, &l).iter().all(|(_, m)| *m == Some(Mark::Same)));
    assert!(marks(&b, &r).iter().all(|(_, m)| *m == Some(Mark::Same)));
}

#[test]
fn a_sibling_sharing_a_prefix_does_not_confuse_the_walk_up() {
    // `a-b` sorts between `a` and `a\x` under a plain byte order; the separator folds below it.
    let a = tree(&[("a", 0, FLAG_DIR), ("a-b", 0, FLAG_DIR), ("a\\x", 1, 0)]);
    let b = tree(&[("a", 0, FLAG_DIR), ("a-b", 0, FLAG_DIR)]);
    let (l, _) = sides(&a, &b, false);
    assert_eq!(mark_of(&a, &l, "a\\x"), Some(Mark::Only));
    assert_eq!(mark_of(&a, &l, "a"), Some(Mark::Holds));
    assert_eq!(mark_of(&a, &l, "a-b"), Some(Mark::Same));
}

#[test]
fn a_folder_here_and_a_file_there_differ_in_type() {
    let a = tree(&[("thing", 0, FLAG_DIR), ("thing\\inner", 1, 0)]);
    let b = tree(&[("thing", 1, 0)]);
    let (l, r) = sides(&a, &b, false);
    // The name is on both sides, so it is the Type cell that says so and not the name.
    assert_eq!(mark_of(&a, &l, "thing"), changed(true, false, false));
    assert_eq!(mark_of(&b, &r, "thing"), changed(true, false, false));
    assert_eq!(mark_of(&a, &l, "thing\\inner"), Some(Mark::Only));
}

#[test]
fn hidden_files_count_only_while_they_are_shown() {
    let a = tree(&[("d", 0, FLAG_DIR), ("d\\.secret", 1, FLAG_HIDDEN)]);
    let b = tree(&[("d", 0, FLAG_DIR)]);
    let (l, _) = sides(&a, &b, false);
    assert_eq!(mark_of(&a, &l, "d"), Some(Mark::Same));
    let (l, _) = sides(&a, &b, true);
    assert_eq!(mark_of(&a, &l, "d"), Some(Mark::Holds));
    assert_eq!(mark_of(&a, &l, "d\\.secret"), Some(Mark::Only));
}

#[test]
fn the_other_side_scrolls_to_the_same_row_or_the_nearest_one_before_it() {
    let dir = tree(&[
        ("a", 0, FLAG_DIR),
        ("a\\one", 1, 0),
        ("a\\three", 1, 0),
        ("b", 0, FLAG_DIR),
        ("b\\x", 1, 0),
    ]);
    let c = compare(pair(&dir, &dir, false));
    // A display order that is not name order, the way a tree sorted by type is not.
    let order: Vec<u32> = vec![3, 4, 0, 2, 1];
    let mut lookup = Lookup::default();
    let row = |lookup: &mut Lookup, name: &str| lookup.position_of(&c.left, &dir, &order, 1, name);
    assert_eq!(row(&mut lookup, "B\\X"), Some(1));
    assert_eq!(row(&mut lookup, "a"), Some(2));
    // Not here: the row before it by name, which is its sibling `a\three`.
    assert_eq!(row(&mut lookup, "a\\two"), Some(3));
    // Before everything: the first row by name.
    assert_eq!(row(&mut lookup, "0"), Some(2));
    // Shut folders leave the order, and the lookup is rebuilt with it.
    let shut: Vec<u32> = vec![3, 0];
    assert_eq!(lookup.position_of(&c.left, &dir, &shut, 2, "b\\x"), Some(0));
    // And a name inside a shut folder lands on the folder.
    assert_eq!(lookup.position_of(&c.left, &dir, &shut, 2, "a\\one"), Some(1));
}

#[test]
fn the_narrowed_views_keep_what_differs_and_the_way_down_to_it() {
    let a = sized(&[
        ("same", 0, 0, FLAG_DIR),
        ("same\\f", 1, 1, 0),
        ("changed", 0, 0, FLAG_DIR),
        ("changed\\f", 1, 1, 0),
        ("added", 0, 0, FLAG_DIR),
        ("added\\new", 1, 1, 0),
        ("added\\f", 1, 1, 0),
    ]);
    let b = sized(&[
        ("same", 0, 0, FLAG_DIR),
        ("same\\f", 1, 1, 0),
        ("changed", 0, 0, FLAG_DIR),
        ("changed\\f", 2, 1, 0),
        ("added", 0, 0, FLAG_DIR),
        ("added\\f", 1, 1, 0),
    ]);
    let c = compare(pair(&a, &b, false));
    let kept = |dir: &Arc<Dir>, left: bool, show: Show| -> Vec<String> {
        (0..dir.len())
            .filter(|&i| c.half(left).1.keeps(show, i))
            .map(|i| dir.name(i).to_owned())
            .collect()
    };
    assert_eq!(kept(&a, true, Show::All).len(), 7);
    assert_eq!(
        kept(&a, true, Show::Changes),
        ["changed", "changed\\f", "added", "added\\new"]
    );
    assert_eq!(kept(&a, true, Show::Names), ["added", "added\\new"]);
    // The right has the folder a name is missing from, and nothing else of that folder.
    assert_eq!(kept(&b, false, Show::Names), ["added"]);
    assert_eq!(kept(&b, false, Show::Changes), ["changed", "changed\\f", "added"]);
}

#[test]
fn a_listing_the_comparison_was_not_made_of_gets_no_marks() {
    let rows = [("f", 1, 0)];
    let (a, b) = (tree(&rows), tree(&rows));
    let (l, _) = sides(&a, &b, false);
    // The same rows, read again: a different listing, so nothing is claimed about it.
    let again = tree(&rows);
    assert_eq!(l.mark(&again, 0), None);
    assert_eq!(l.mark(&a, 0), Some(Mark::Same));
}

#[test]
fn the_path_bar_is_picked_out_from_when_it_is_first_drawn_and_then_fades() {
    let mut flash = Flash::Wanted;
    // The clock starts on the first frame it is drawn, whenever that is.
    assert_eq!(flash.strength(100.0), Some(1.0));
    assert_eq!(flash.strength(100.0 + Flash::HOLD), Some(1.0));
    let halfway = flash.strength(100.0 + Flash::HOLD + Flash::FADE / 2.0).unwrap();
    assert!((halfway - 0.5).abs() < 1e-6);
    assert_eq!(flash.strength(100.0 + Flash::HOLD + Flash::FADE), None);
    assert_eq!(flash, Flash::Off);
    assert_eq!(Flash::Off.strength(0.0), None);
}
