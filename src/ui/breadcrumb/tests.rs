use super::*;

/// **The box's tooltip is the design system's syntax and nothing this window invented, and the
/// listings it does add are the funnel's.**
///
/// This used to assert the opposite: the list was the library's three with `@git` appended, and
/// what needed testing was that the appended pair reached the tooltip at all. The word is a menu
/// now, so the claim is inverted — a marker of this program's own here would send a reader to the
/// box to type something it no longer understands, which is the one wrong answer a syntax tooltip
/// can give.
///
/// Pairs and not lines, because what makes the two columns possible is that the key and the
/// meaning never become one string. A marker smuggled in as `"!word — …"` would draw as one long
/// key in `text-secondary` with an empty column beside it, which is the failure this catches — the
/// design system tests that its own three are *present*
/// (`filter::tests::the_markers_are_all_documented_as_pairs`), not that they are still two halves
/// by the time this window draws them.
#[test]
fn the_filter_tooltip_is_the_design_systems_and_the_lenses_are_the_funnels() {
    let markers = azur_egui_theme::filter::MARKERS;
    for want in ["!word", "^word", "word$"] {
        let (key, meaning) = markers
            .iter()
            .find(|(key, _)| *key == want)
            .unwrap_or_else(|| panic!("`{want}` is not in the tooltip: {markers:?}"));
        assert!(
            !meaning.trim().is_empty(),
            "`{key}` is in the tooltip with nothing said about it"
        );
        // Neither half may carry the other: that is what makes them two columns.
        assert!(
            !key.contains('—') && !meaning.contains('—') && !meaning.contains(key),
            "`{key}`/`{meaning}` has been folded into one string"
        );
    }

    // Nothing in the box's syntax is about git or about a kind of file: both are questions about
    // this program's data, and the funnel is where they are asked.
    for (key, meaning) in markers {
        assert!(
            !key.starts_with('@') && !meaning.contains("git"),
            "`{key}`/`{meaning}` is a lens dressed up as syntax"
        );
    }

    // And each lens says what it does, because the menu is the only place either is named.
    let labels: Vec<&str> = Lens::ALL.iter().map(|lens| lens.label()).collect();
    assert_eq!(labels.len(), 2, "{labels:?}");
    assert!(
        labels.iter().all(|label| label.starts_with("Show ")),
        "an entry that does something is a sentence, not a value: {labels:?}"
    );
    assert_ne!(labels[0], labels[1]);
}

/// **The chevron's folder is asked of the loader and never read here**, and the menu holds nothing
/// until the answer lands.
///
/// The freeze this is about: [`fs::scan::scan`] says of itself that it only ever runs on a
/// [`crate::loader`] worker, because a bare `\\server` is answered by `NetShareEnum` — 22.1 seconds
/// for a name that does not resolve, and a share that has just gone away costs the same wait. This
/// menu called it from inside the popup body, which is the UI thread on the frame the dropdown
/// opens, and the trail `\\server\share\a` hands it exactly that path from the chevron between
/// `server` and `share`. The window stopped painting for the whole of it, once per chevron the
/// pointer crossed.
///
/// So the first ask **must** come back empty-handed, whatever the folder is: the answer cannot be
/// there yet, because nothing on this thread went to look. A cache probe is a hash lookup and a
/// worker has a directory to read, so there is no race to lose here.
///
/// Driven against a real [`crate::loader::Loader`] rather than through the window, because what is
/// being checked is the one call — `click_tests::breadcrumb` is where the popup itself is opened and
/// waited on.
#[test]
fn the_chevron_menu_asks_the_loader_and_reads_nothing_itself() {
    let ctx = egui::Context::default();
    let mut loader = crate::loader::Loader::new(&ctx);
    let mut menu = CrumbMenu::default();

    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loader.invalidate(&here);
    assert!(
        !menu.ensure(&here, &mut loader),
        "the menu answered on the frame it was asked, so it read the folder itself"
    );
    assert_eq!(menu.listing(), None, "it has rows nothing has read yet");

    // The worker answers, and the next probe — an ordinary frame, drawn because the loader woke
    // the window — is what fills the menu.
    for _ in 0..200 {
        if loader.cached(&here).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        menu.ensure(&here, &mut loader),
        "the listing landed in the loader and the menu did not take it"
    );
    let (path, count) = menu.listing().expect("the menu has the folder it asked for");
    assert_eq!(path, here);
    assert!(count > 0, "this crate's own folder has subfolders in it");

    // And the rows go the moment the pointer moves to another chevron, rather than when that
    // folder's answer arrives: they are the last folder's, and offering them under this one's name
    // is the one wrong answer this menu can give. The folder is the whole of the staleness check —
    // nothing is ever handed to this menu, it looks up the one key it wants.
    let other = here.join("src");
    loader.invalidate(&other);
    assert!(!menu.ensure(&other, &mut loader), "{other:?} cannot be read yet");
    assert_eq!(
        menu.listing(),
        None,
        "the rows for the chevron before it survived the move"
    );
}

#[test]
fn the_bold_segment_is_the_folder_being_shown() {
    let trail = fs::breadcrumb_segments(Path::new(r"C:\a\b\c"));
    // This PC, C:\, a, b, c.
    assert_eq!(trail.len(), 5, "{trail:?}");

    // At the end of the trail it is the last segment, as it always used to be.
    assert_eq!(active_index(&trail, Path::new(r"C:\a\b\c")), 4);
    // Walked up two: the trail still shows five, and `a` is the one in bold.
    assert_eq!(active_index(&trail, Path::new(r"C:\a")), 2);
    // All the way up. This PC is a segment like any other.
    assert_eq!(active_index(&trail, Path::new("")), 0);
}

#[test]
fn a_path_that_is_not_on_the_trail_falls_back_to_its_end() {
    // Should not happen -- `Tab::go_to` keeps the two in step -- but a bar with nothing
    // bold on it would be worse than this.
    let trail = fs::breadcrumb_segments(Path::new(r"C:\a\b"));
    assert_eq!(active_index(&trail, Path::new(r"D:\somewhere")), trail.len() - 1);
}

/// The cut has to be lossless: the two halves put back together are the text that came in,
/// which is what lets a completion leave `%appdata%\` alone instead of resolving it away.
#[test]
fn the_typed_path_splits_at_the_last_separator() {
    assert_eq!(split_typed(r"C:\Users\to"), (r"C:\Users\", "to"));
    assert_eq!(split_typed(r"C:\Users\"), (r"C:\Users\", ""));
    assert_eq!(split_typed(r"C:\"), (r"C:\", ""));
    // Either slash, because the field takes either.
    assert_eq!(split_typed("D:/Sources/My"), ("D:/Sources/", "My"));
    // Nothing names a folder yet, so all of it is the name being typed.
    assert_eq!(split_typed("Doc"), ("", "Doc"));
    assert_eq!(split_typed(""), ("", ""));
    // What was typed is never rewritten on the way through.
    assert_eq!(split_typed(r"%APPDATA%\Mic"), (r"%APPDATA%\", "Mic"));
    for text in [r"C:\Users\to", "Doc", "", r"~\", "D:/x/y"] {
        let (a, b) = split_typed(text);
        assert_eq!(format!("{a}{b}"), text, "the split lost something");
    }
}

#[test]
fn matching_a_name_ignores_its_case() {
    assert!(starts_with_folded("Windows", "win"));
    assert!(starts_with_folded("windows", "WIN"));
    assert!(starts_with_folded("Windows", ""));
    assert!(starts_with_folded("Windows", "Windows"));
    assert!(!starts_with_folded("Windows", "Windowsx"));
    assert!(!starts_with_folded("Windows", "ind"), "this matches prefixes, not parts");
    // Not the ASCII fold: a folder can be named in any language, and one you can see that is
    // not offered reads as a broken completion.
    assert!(starts_with_folded("Étude", "é"));
    assert!(starts_with_folded("étude", "É"));
}

/// Two offers and the text that made them, which is enough to drive the highlight and the
/// append without a window or a disk.
fn sample() -> PathComplete {
    PathComplete {
        pane: None,
        typed: r"C:\Users\to".to_owned(),
        asked: None,
        ready: true,
        offers: vec![
            ("tony".to_owned(), PathBuf::from(r"C:\Users\tony")),
            ("tools".to_owned(), PathBuf::from(r"C:\Users\tools")),
        ],
        truncated: false,
        hot: None,
        hidden: false,
        follow: false,
    }
}

/// Down and Up walk the offers, and both ends come back to what was typed rather than
/// wrapping straight round — `Escape` throws the field away, so it cannot be the only way
/// back to your own text.
#[test]
fn the_highlight_walks_the_offers_and_back_to_what_was_typed() {
    let mut complete = sample();
    assert_eq!(complete.hot, None, "nothing starts highlighted");

    complete.step(true);
    assert_eq!(complete.hot, Some(0));
    complete.step(true);
    assert_eq!(complete.hot, Some(1));
    complete.step(true);
    assert_eq!(complete.hot, None, "past the last offer is what was typed");
    complete.step(true);
    assert_eq!(complete.hot, Some(0), "and then round again");

    complete.hot = None;
    complete.step(false);
    assert_eq!(complete.hot, Some(1), "Up from nothing starts at the end");
    complete.step(false);
    assert_eq!(complete.hot, Some(0));
    complete.step(false);
    assert_eq!(complete.hot, None);
}

/// An arrow puts the dropdown back up, which is the only way back to one that was put away
/// without deleting what you typed.
#[test]
fn an_arrow_opens_the_dropdown_and_lands_on_an_offer_at_once() {
    let mut complete = sample();
    complete.hidden = true;
    complete.step(true);
    assert!(!complete.hidden, "Down did not bring the dropdown back");
    assert_eq!(
        complete.hot,
        Some(0),
        "the same press has to land on an offer, or every one of them costs two"
    );
    assert!(complete.follow, "the highlight has to be scrolled into view");
}

/// Appending keeps the prefix exactly as it was typed and adds the separator that starts the
/// next name, so the offers after it are what is inside the folder just chosen.
#[test]
fn accepting_an_offer_appends_a_name_and_a_separator() {
    let mut complete = sample();
    let mut text = complete.typed.clone();

    assert_eq!(
        complete.accept(&mut text, false),
        None,
        "with nothing highlighted there is nothing to append"
    );
    assert_eq!(text, r"C:\Users\to", "and the text is left alone");

    complete.hot = Some(1);
    assert_eq!(
        complete.accept(&mut text, false),
        Some(PathBuf::from(r"C:\Users\tools"))
    );
    assert_eq!(text, "C:\\Users\\tools\\");
}

/// A path typed with forward slashes stays typed with forward slashes: it lists and navigates
/// perfectly — `fs::normalize` sees to that at the door — so mixing the two would be this
/// program's own doing.
///
/// **The prefix wins over the setting**, both ways round, which is the point: `Use / in path`
/// says what the field is *filled* with, and once there is a path in it the separators in that
/// path are the better evidence of what somebody wants.
#[test]
fn appending_keeps_the_separator_that_was_being_used() {
    let mut complete = sample();
    complete.offers = vec![("src".to_owned(), PathBuf::from(r"D:\Sources\src"))];
    complete.hot = Some(0);

    for slashes in [false, true] {
        let mut text = "D:/Sources/s".to_owned();
        complete.accept(&mut text, slashes);
        assert_eq!(text, "D:/Sources/src/");

        let mut text = r"D:\Sources\s".to_owned();
        complete.accept(&mut text, slashes);
        assert_eq!(text, "D:\\Sources\\src\\");
    }
}

/// With no separator typed yet the setting decides, which is a path typed from nothing: the
/// first offer is a *drive*, and the separator after it is the first one the field will hold.
///
/// The only place `Use / in path` reaches the completion, and it has to — a field filled with
/// `/` and a `Tab` that answers `D:\` would be the program disagreeing with its own setting on
/// the first keystroke.
#[test]
fn appending_to_a_bare_name_uses_the_slash_the_setting_asks_for() {
    let mut complete = sample();
    complete.offers = vec![("D:".to_owned(), PathBuf::from("D:\\"))];
    complete.hot = Some(0);

    let mut text = "d".to_owned();
    complete.accept(&mut text, true);
    assert_eq!(text, "D:/");

    let mut text = "d".to_owned();
    complete.accept(&mut text, false);
    assert_eq!(text, "D:\\");
}

/// The swap the field's separator setting is: both ways, exact, and the same length either way.
#[test]
fn the_separator_swap_is_lossless_in_both_directions() {
    assert_eq!(with_separator(r"D:\Sources\ui", true), "D:/Sources/ui");
    assert_eq!(with_separator("D:/Sources/ui", false), r"D:\Sources\ui");
    // Already the way it was asked for: nothing to do, and nothing done.
    assert_eq!(with_separator("D:/Sources", true), "D:/Sources");
    assert_eq!(with_separator(r"D:\Sources", false), r"D:\Sources");
    // A mixture is what half-typing a path leaves, and it comes out consistent.
    assert_eq!(with_separator(r"D:\Sources/ui\x", true), "D:/Sources/ui/x");
    // A share. Both leading separators are separators, so both turn.
    assert_eq!(with_separator(r"\\nas\music", true), "//nas/music");
    assert_eq!(with_separator("//nas/music", false), r"\\nas\music");
    // `This PC` has nothing in it to swap, and neither does a name being typed.
    assert_eq!(with_separator("This PC", true), "This PC");
    // Length preserved, which is what lets the caret stay where it was.
    for text in [r"D:\a\b", "D:/a/b", r"\\nas\music", "This PC", ""] {
        for slashes in [false, true] {
            assert_eq!(with_separator(text, slashes).len(), text.len(), "{text}");
        }
    }
    // And a round trip is the identity for a path written either way.
    for text in [r"D:\a\b", r"\\nas\music"] {
        assert_eq!(with_separator(&with_separator(text, true), false), text);
    }
}

/// A field rewritten from outside is not somebody typing, so the dropdown stays as it was.
///
/// Which is the whole of [`PathComplete::rewritten`]: `refresh` reads any change to the text as
/// a keystroke and puts the offers up, and ticking `Use / in path` on a field holding where you
/// already are would raise a list of the folder you are standing in — the one list `Ctrl+L` is
/// careful not to show.
#[test]
fn rewriting_the_field_from_outside_leaves_the_dropdown_down() {
    let mut complete = sample();
    complete.pane = Some(1);
    complete.hidden = true;
    complete.hot = Some(1);

    // The wrong pane's field: not this one's business. The other pane draws its own bar on
    // every frame of a split window.
    complete.rewritten(2, "C:/Users/to");
    assert_eq!(complete.typed, r"C:\Users\to");

    complete.rewritten(1, "C:/Users/to");
    assert_eq!(
        complete.typed, "C:/Users/to",
        "the new text was not claimed, so `refresh` will read it as a keystroke"
    );
    assert!(complete.hidden, "the dropdown came up on its own");
    assert!(!complete.ready, "the offers carry the old prefix and were kept");
    assert_eq!(complete.hot, None, "the highlight stayed on offers that are about to move");
}

/// The one pane that has a field open keeps it while the other pane's bar is drawn, which
/// happens on every frame of a split window.
#[test]
fn the_other_panes_bar_does_not_clear_this_ones_offers() {
    let mut complete = sample();
    complete.pane = Some(1);
    complete.hot = Some(0);

    complete.close(2);
    assert_eq!(complete.hot, Some(0), "the wrong pane cleared the field's state");

    complete.close(1);
    assert!(complete.offers.is_empty(), "its own pane did not clear it");
    assert_eq!(complete.pane, None);
}
