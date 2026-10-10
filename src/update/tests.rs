//! What can be checked without the network: the version order, reading a release, the day's cache
//! and the swap. See the module header for why nothing here reaches GitHub or the executable.

use super::*;

#[test]
fn versions_are_ordered_by_number_and_not_by_text() {
    assert!(newer("1.2.0", "1.1.0"));
    assert!(newer("v1.10.0", "1.9.3"), "1.10 is after 1.9, which text order gets wrong");
    assert!(newer("2", "1.99.99"));
    assert!(!newer("1.1.0", "1.1.0"));
    assert!(!newer("1.1", "1.1.0"), "a trailing zero is the same version");
    assert!(!newer("1.0.9", "1.1.0"));
    // A suffix is not ordered, so it can never make a build look newer than itself.
    assert!(!newer("1.1.0-beta", "1.1.0"));
    // And anything unreadable is never newer — a tag somebody typed by hand cannot raise the badge.
    assert!(!newer("latest", "1.1.0"));
    assert!(!newer("1.x", "1.0"));
    assert!(!newer("", "1.0"));
}

/// The shape `releases/latest` answers with, trimmed to the parts that matter and the kind of
/// thing around them: nested objects, escapes, a surrogate pair in the notes.
const RELEASE: &str = r#"{
  "url": "https://api.github.com/repos/tonyriviere88/azur-files/releases/1",
  "html_url": "https://github.com/tonyriviere88/azur-files/releases/tag/v1.2.0",
  "tag_name": "v1.2.0",
  "name": "Azur Files 1.2.0",
  "draft": false,
  "prerelease": false,
  "id": 123456789,
  "author": { "login": "tonyriviere88", "site_admin": false },
  "assets": [
    {
      "name": "azur-files.exe.sha256",
      "size": 81,
      "browser_download_url": "https://github.com/tonyriviere88/azur-files/releases/download/v1.2.0/azur-files.exe.sha256"
    },
    {
      "name": "azur-files.exe",
      "size": 1.2e7,
      "uploader": null,
      "browser_download_url": "https://github.com/tonyriviere88/azur-files/releases/download/v1.2.0/azur-files.exe"
    }
  ],
  "body": "* Sorted column is saved \u2014 \"really\" \ud83c\udf89\n* C:\\path\/x"
}"#;

#[test]
fn a_release_is_read_for_its_version_page_and_two_files() {
    let release = release(RELEASE).expect("a release");
    assert_eq!(release.version, "1.2.0", "the tag, without its v");
    assert_eq!(
        release.page,
        "https://github.com/tonyriviere88/azur-files/releases/tag/v1.2.0"
    );
    assert!(release.exe.ends_with("/v1.2.0/azur-files.exe"), "{}", release.exe);
    assert!(
        release.sha.as_deref().is_some_and(|sha| sha.ends_with("azur-files.exe.sha256")),
        "{:?}",
        release.sha
    );
}

#[test]
fn a_release_without_a_checksum_is_still_read_and_one_without_an_exe_is_not() {
    // Offered, and refused at install — see the module header.
    let old = RELEASE.replace("azur-files.exe.sha256", "notes.txt");
    assert_eq!(release(&old).expect("a release").sha, None);

    let none = RELEASE.replace(".exe", ".zip");
    assert_eq!(release(&none), None, "nothing to install");

    // An executable by another name is still the program, and its checksum follows its name.
    let renamed = RELEASE.replace("azur-files.exe", "AzurFiles-1.2.0.exe");
    let renamed = release(&renamed).expect("a release");
    assert!(renamed.exe.ends_with("AzurFiles-1.2.0.exe"));
    assert!(renamed.sha.is_some());

    // A tag that is not a version is not a release this program can compare against.
    assert_eq!(release(&RELEASE.replace("v1.2.0\"", "nightly\"")), None);
}

#[test]
fn json_decodes_strings_properly_and_refuses_what_is_not_json() {
    let doc = json::parse(RELEASE).expect("json");
    let body = doc.get("body").and_then(json::Value::as_str).expect("a body");
    assert_eq!(body, "* Sorted column is saved — \"really\" 🎉\n* C:\\path/x");

    for bad in [
        "",
        "{",
        "{\"a\":}",
        "{\"a\":1,}",
        "[1 2]",
        "\"unterminated",
        "\"\\x\"",
        "\"\\u12\"",
        "\"\\u+123\"",
        "\"\\ud83c\"",
        "{} trailing",
        "\"a\nb\"",
    ] {
        assert_eq!(json::parse(bad), None, "{bad:?} is not JSON");
    }
    // A hostile depth costs a bounded amount of stack, not all of it.
    let deep = "[".repeat(100_000);
    assert_eq!(json::parse(&deep), None);
}

#[test]
fn a_checksum_file_is_read_as_sha256sum_writes_it() {
    let hex = "A3".repeat(32);
    assert_eq!(digest(&format!("{hex} *azur-files.exe\r\n")), Some(hex.to_lowercase()));
    assert_eq!(digest(&format!("\u{feff}{hex}")), Some(hex.to_lowercase()));
    assert_eq!(digest("abc azur-files.exe"), None, "too short");
    assert_eq!(digest(&"zz".repeat(32)), None, "not hex");
    assert_eq!(digest(""), None);
}

#[test]
fn only_https_addresses_are_fetched() {
    assert_eq!(
        split_url("https://api.github.com/repos/a/b/releases/latest"),
        Some(("api.github.com", 443, "/repos/a/b/releases/latest"))
    );
    assert_eq!(split_url("https://example.com:8443"), Some(("example.com", 8443, "/")));
    assert_eq!(split_url("https://example.com/x?y=1#z"), Some(("example.com", 443, "/x?y=1")));
    assert_eq!(split_url("http://example.com/x"), None);
    assert_eq!(split_url("https://user@example.com/x"), None);
    assert_eq!(split_url("https://example.com:port/x"), None);
    assert_eq!(split_url("https:///x"), None);
}

#[test]
fn the_days_answer_round_trips_and_goes_stale() {
    let cache = Cache {
        checked: 1_760_000_000,
        release: release(RELEASE),
    };
    assert_eq!(Cache::parse(&cache.to_text()), Some(cache.clone()));

    // "There is no release" is an answer too, and is kept like one.
    let empty = Cache {
        checked: 5,
        release: None,
    };
    assert_eq!(Cache::parse(&empty.to_text()), Some(empty));
    assert_eq!(Cache::parse("version=1.2.0\n"), None, "no time, no answer");

    let day = Every::Day.seconds().unwrap();
    assert!(cache.fresh(cache.checked, day));
    assert!(cache.fresh(cache.checked + day - 1, day));
    assert!(!cache.fresh(cache.checked + day, day));
    assert!(cache.fresh(cache.checked + day, Every::Week.seconds().unwrap()));
    assert!(!cache.fresh(cache.checked - 1, day), "a clock gone backwards asks again");
}

#[test]
fn the_swap_sets_the_old_build_aside_and_the_next_start_clears_it() {
    let dir = crate::sandbox::fresh("update-swap");
    let exe = dir.join("azur-files.exe");
    let staged = sibling(&exe, "new");
    std::fs::write(&exe, "old build").unwrap();
    std::fs::write(&staged, "new build").unwrap();

    swap(&exe, &staged).expect("the swap");
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new build");
    assert!(!staged.exists(), "the staged file became the program");
    assert_eq!(
        std::fs::read_to_string(sibling(&exe, "old")).unwrap(),
        "old build"
    );

    // A second update finds the first one's `.old` and replaces it, rather than numbering.
    std::fs::write(&staged, "newer build").unwrap();
    swap(&exe, &staged).expect("the second swap");
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "newer build");
    assert_eq!(
        std::fs::read_to_string(sibling(&exe, "old")).unwrap(),
        "new build"
    );

    // The next start tidies what was set aside, numbered ones included, and nothing else.
    std::fs::write(sibling(&exe, "3.old"), "older").unwrap();
    std::fs::write(dir.join("other.exe.old"), "not ours").unwrap();
    tidy(&exe);
    assert!(exe.exists());
    assert!(!sibling(&exe, "old").exists());
    assert!(!sibling(&exe, "3.old").exists());
    assert!(dir.join("other.exe.old").exists(), "another program's leftovers are its own");
}

#[test]
fn a_swap_that_cannot_finish_puts_the_program_back() {
    let dir = crate::sandbox::fresh("update-swap-fails");
    let exe = dir.join("azur-files.exe");
    std::fs::write(&exe, "old build").unwrap();

    // No staged file: the second rename fails, and the first has to be undone.
    let missing = sibling(&exe, "new");
    assert!(swap(&exe, &missing).is_err());
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old build");
}

#[test]
fn nothing_reaches_the_network_from_a_test() {
    // `check` and `install` return before starting a thread under `cfg!(test)`, so nothing ever
    // arrives — a badge here could only have come from GitHub.
    let ctx = egui::Context::default();
    let mut updater = Updater::new(&ctx);
    updater.check(Every::Day);
    updater.check_now();
    updater.install();
    assert_eq!(updater.drain(), Drained::default());
    assert_eq!(updater.badge(""), None);
    assert!(relaunch(Path::new("azur-files.exe")).is_err());
}

#[test]
fn the_badge_follows_what_was_found_and_what_was_skipped() {
    let ctx = egui::Context::default();
    let mut updater = Updater::new(&ctx);
    updater.current = "1.1.0".into();
    let mut offered = release(RELEASE).unwrap();

    updater.found(Some(offered.clone()));
    assert_eq!(
        updater.badge(""),
        Some(Badge::Available {
            version: "1.2.0".into()
        })
    );
    assert_eq!(updater.badge("1.2.0"), None, "skipped");
    assert!(updater.release().is_some(), "still there for the menu");

    // The same build or an older one says nothing.
    offered.version = "1.1.0".into();
    updater.found(Some(offered.clone()));
    assert_eq!(updater.badge(""), None);
    updater.found(None);
    assert_eq!(updater.badge(""), None);

    // A failed install is shown even for a skipped version: it is the answer to a click.
    offered.version = "1.2.0".into();
    updater.state = State::Downloading(offered.clone(), 40);
    assert_eq!(
        updater.badge("1.2.0"),
        Some(Badge::Downloading {
            version: "1.2.0".into(),
            percent: 40
        })
    );
    updater
        .tx
        .send(Event::Installed(Err("No permission".into())))
        .unwrap();
    assert_eq!(updater.drain(), Drained::default());
    assert_eq!(
        updater.badge("1.2.0"),
        Some(Badge::Failed {
            version: "1.2.0".into(),
            why: "No permission".into()
        })
    );

    // A check that fails changes nothing; one that answers does.
    updater.state = State::Available(offered.clone());
    updater.tx.send(Event::Checked(Err("offline".into()))).unwrap();
    updater.drain();
    assert!(updater.badge("").is_some());
    updater.tx.send(Event::Checked(Ok(None))).unwrap();
    assert_eq!(updater.drain().told, None, "nobody asked, so nothing is said");
    assert_eq!(updater.badge(""), None);

    // *Check now* always answers: up to date, or why not — and nothing when the badge is the answer.
    updater.asked = true;
    updater.tx.send(Event::Checked(Ok(None))).unwrap();
    assert_eq!(updater.drain().told.as_deref(), Some("Azur Files 1.1.0 is up to date"));
    updater.asked = true;
    updater.tx.send(Event::Checked(Err("offline".into()))).unwrap();
    assert_eq!(
        updater.drain().told.as_deref(),
        Some("Could not check for updates: offline")
    );
    updater.asked = true;
    updater.tx.send(Event::Checked(Ok(Some(offered)))).unwrap();
    assert_eq!(updater.drain().told, None);
    assert!(updater.badge("").is_some());
}

#[test]
fn how_often_is_written_as_a_word() {
    for every in Every::ALL {
        assert_eq!(Every::parse(every.as_str()), Some(every));
    }
    assert_eq!(Every::default(), Every::Day);
    assert_eq!(Every::parse("hourly"), None);
    assert_eq!(Every::Never.seconds(), None, "never is never asking");
}
