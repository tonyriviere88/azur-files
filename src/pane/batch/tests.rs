use super::*;

fn item(leaf: &str) -> Item {
    Item {
        name: leaf.to_owned(),
        leaf: leaf.to_owned(),
        folder: String::new(),
        folder_name: "Photos".to_owned(),
        is_dir: false,
        modified: 0,
        guid: format!("guid-{leaf}"),
        target: PathBuf::from(leaf),
    }
}

/// The new names, with the listing holding exactly the items and `others`.
fn plan(batch: &BatchRename, others: &[&str]) -> Plan {
    let listing: Vec<(&str, &str)> = batch
        .items
        .iter()
        .map(|i| (i.folder.as_str(), i.leaf.as_str()))
        .chain(others.iter().map(|o| ("", *o)))
        .collect();
    batch.plan(listing, |_| true, &LocalZone::utc())
}

fn names(plan: &Plan) -> Vec<&str> {
    plan.rows.iter().map(|r| r.new.as_str()).collect()
}

#[test]
fn replace_works_on_the_stem_and_ignores_case_unless_asked() {
    let mut b = BatchRename::new(vec![item("IMG_0412.JPG"), item("img_0413.jpg"), item("notes.txt")]);
    b.rule.find = "img_".to_owned();
    b.rule.replace = "Holiday_".to_owned();
    let p = plan(&b, &[]);
    assert_eq!(names(&p), ["Holiday_0412.JPG", "Holiday_0413.jpg", "notes.txt"]);
    assert_eq!(p.changing(), 2);
    assert!(p.ready());

    b.rule.match_case = true;
    assert_eq!(names(&plan(&b, &[])), ["IMG_0412.JPG", "Holiday_0413.jpg", "notes.txt"]);

    // The extension is left alone unless it is included.
    b.rule.match_case = false;
    b.rule.find = "jpg".to_owned();
    b.rule.replace = "jpeg".to_owned();
    assert_eq!(names(&plan(&b, &[])), ["IMG_0412.JPG", "img_0413.jpg", "notes.txt"]);
    b.rule.extension = true;
    assert_eq!(names(&plan(&b, &[])), ["IMG_0412.jpeg", "img_0413.jpeg", "notes.txt"]);
}

#[test]
fn a_regex_replaces_with_groups_and_a_bad_one_is_the_rules_error() {
    let mut b = BatchRename::new(vec![item("2024-03-01 report.pdf")]);
    b.rule.regex = true;
    b.rule.find = r"(\d+)-(\d+)-(\d+)".to_owned();
    b.rule.replace = "$3.$2.$1".to_owned();
    assert_eq!(names(&plan(&b, &[])), ["01.03.2024 report.pdf"]);

    b.rule.find = "(unclosed".to_owned();
    let p = plan(&b, &[]);
    assert!(p.error.as_deref().is_some_and(|e| e.starts_with("Not a valid regex")), "{p:?}");
    assert_eq!(names(&p), ["2024-03-01 report.pdf"], "nothing changes under a broken rule");
    assert!(!p.ready());
}

#[test]
fn a_pattern_fills_its_tokens_and_numbers_in_order() {
    let mut b = BatchRename::new(vec![item("a.jpg"), item("b.jpg"), item("README")]);
    b.rule.mode = Mode::Pattern;
    b.rule.pattern = "{folder}_{n:03}.{ext}".to_owned();
    // No extension leaves no dot: `README` becomes `Photos_003`, not `Photos_003.`.
    assert_eq!(names(&plan(&b, &[])), ["Photos_001.jpg", "Photos_002.jpg", "Photos_003"]);

    b.rule.start = 10.0;
    b.rule.step = 5.0;
    b.rule.pattern = "{name}-{n}".to_owned();
    assert_eq!(names(&plan(&b, &[])), ["a-10.jpg", "b-15.jpg", "README-20"]);

    b.rule.pattern = "{guid}.{ext}".to_owned();
    assert_eq!(names(&plan(&b, &[])), ["guid-a.jpg.jpg", "guid-b.jpg.jpg", "guid-README"]);

    b.rule.pattern = "{nope}".to_owned();
    let p = plan(&b, &[]);
    assert!(p.error.as_deref().is_some_and(|e| e.contains("{nope}")), "{p:?}");
    b.rule.pattern = "{name".to_owned();
    assert!(plan(&b, &[]).error.is_some());
    b.rule.pattern = "{n:x}".to_owned();
    assert!(plan(&b, &[]).error.is_some());
}

#[test]
fn case_changes_the_stem_and_the_extension_only_when_asked() {
    let mut b = BatchRename::new(vec![item("my_HOLIDAY-photo.JPG")]);
    b.rule.mode = Mode::Case;
    b.rule.case_to = CaseTo::Title;
    assert_eq!(names(&plan(&b, &[])), ["My_Holiday-Photo.JPG"]);
    b.rule.case_to = CaseTo::Upper;
    b.rule.case_extension = true;
    assert_eq!(names(&plan(&b, &[])), ["MY_HOLIDAY-PHOTO.jpg"]);
    // A change of case alone is not a collision with itself.
    b.rule.case_to = CaseTo::Lower;
    let p = plan(&b, &[]);
    assert_eq!(names(&p), ["my_holiday-photo.jpg"]);
    assert!(p.ready(), "{p:?}");
}

#[test]
fn every_problem_is_caught_before_anything_is_renamed() {
    // Two rows to one name.
    let mut b = BatchRename::new(vec![item("a1.txt"), item("a2.txt")]);
    b.rule.regex = true;
    b.rule.find = r"\d".to_owned();
    b.rule.replace = String::new();
    let p = plan(&b, &[]);
    assert_eq!(p.rows[0].problem, Some(Problem::Duplicate));
    assert_eq!(p.rows[1].problem, Some(Problem::Duplicate));
    assert!(!p.ready());

    // Onto something already there, in either case.
    let mut b = BatchRename::new(vec![item("draft.txt")]);
    b.rule.find = "draft".to_owned();
    b.rule.replace = "final".to_owned();
    assert_eq!(plan(&b, &["FINAL.txt"]).rows[0].problem, Some(Problem::Exists));
    // In another folder it does not collide.
    let mut there = item("draft.txt");
    there.folder = "sub".to_owned();
    let mut b2 = BatchRename::new(vec![there]);
    b2.rule.find = "draft".to_owned();
    b2.rule.replace = "final".to_owned();
    assert_eq!(plan(&b2, &["final.txt"]).rows[0].problem, None);

    // A name Windows refuses, and an empty one.
    b.rule.replace = "a:b".to_owned();
    assert!(matches!(plan(&b, &[]).rows[0].problem, Some(Problem::Invalid(_))));
    b.rule.extension = true;
    b.rule.find = "draft.txt".to_owned();
    b.rule.replace = String::new();
    assert_eq!(plan(&b, &[]).rows[0].problem, Some(Problem::Empty));

    // Onto another row's name, where that row keeps it: `one` → `two` while `two` stays `two`.
    // Two rows would then share a name — the row that changes is the one marked.
    let mut s = BatchRename::new(vec![item("one.txt"), item("two.txt")]);
    s.rule.regex = true;
    s.rule.find = "one|two".to_owned();
    s.rule.replace = "two".to_owned();
    let p = plan(&s, &[]);
    assert_eq!(p.rows[0].problem, Some(Problem::Duplicate));
    assert_eq!(p.rows[1].problem, None);

    // A shift: `x1` → `x2` and `x2` → `x3`. The first lands on the second's name while the second
    // still has it, so one pass would collide on the way — a swap, owned up to.
    let mut shift = BatchRename::new(vec![item("x1.txt"), item("x2.txt")]);
    shift.rule.mode = Mode::Pattern;
    shift.rule.pattern = "x{n}".to_owned();
    shift.rule.start = 2.0;
    let p = plan(&shift, &[]);
    assert_eq!(names(&p), ["x2.txt", "x3.txt"]);
    assert_eq!(p.rows[0].problem, Some(Problem::Swap));
    assert_eq!(p.rows[1].problem, None);
}

#[test]
fn an_item_that_has_gone_says_so() {
    let mut b = BatchRename::new(vec![item("a.txt")]);
    b.rule.find = "a".to_owned();
    b.rule.replace = "b".to_owned();
    let p = b.plan(Vec::<(&str, &str)>::new(), |_| false, &LocalZone::utc());
    assert_eq!(p.rows[0].problem, Some(Problem::Gone));
}

#[test]
fn a_name_splits_at_its_last_dot_and_a_folder_has_no_extension() {
    assert_eq!(split("a.tar.gz", false), ("a.tar", "gz"));
    assert_eq!(split(".gitignore", false), (".gitignore", ""));
    assert_eq!(split("README", false), ("README", ""));
    assert_eq!(split("my.photos", true), ("my.photos", ""));
}
