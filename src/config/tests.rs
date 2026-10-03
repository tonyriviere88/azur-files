use super::*;

fn pane(active: usize, paths: &[&str]) -> PaneTabs {
    PaneTabs {
        paths: paths.iter().map(PathBuf::from).collect(),
        active,
    }
}

/// Everything about the window's shape survives a write and a read.
///
/// Round-tripped rather than compared against a fixture: the file is something a user may
/// edit, but what it has to *do* is bring the window back, and that is the claim worth
/// pinning. The position is the awkward one — it is the only value here in physical
/// pixels, and it can be negative, which is what a monitor to the left of the primary one
/// means.
#[test]
fn the_window_comes_back_the_way_it_was_left() {
    let saved = Config {
        panes: vec![pane(1, &[r"C:\src", r"C:\src\ui"]), pane(0, &[r"D:\"])],
        layout: Some("h0.400(0,1)".to_owned()),
        focus: 1,
        window: Some([1380.0, 840.0]),
        position: Some([-1920.0, -8.0]),
        sidebar_width: 260.0,
        // Away from its default, which is the panel showing.
        sidebar_shown: false,
        palette: crate::theme::Palette::Light,
        preview: crate::ui::preview::Layout {
            at: crate::ui::preview::Where::Bottom,
            share: 0.615,
            // Away from its default too, which is the gutter showing — so it is the `0` that has to
            // survive here, the same way round as the two below.
            numbers: false,
            markup: true,
            // Both away from their defaults, which is the only way a round trip proves anything:
            // a value that is never written still comes back right if the default happens to
            // match what was asked for.
            diff: false,
            collapse: true,
            muted: true,
            // All three away from their defaults, for the reason the flags above are: a value that
            // happened to match its default would round-trip whether it was ever written or not.
            // This is the one that caught the shares being written and never read back.
            deps: crate::ui::deps::Sizes {
                list: true,
                share: 0.325,
                split: 0.675,
            },
        },
        console_share: 0.28,
        console_shell: crate::console::Kind::PowerShell,
        // Away from their defaults for the same reason the two above are.
        flat_mode: crate::pane::FlatMode::Tree,
        regroup: false,
        show_hidden: true,
        forward_slashes: true,
        // Both away from their defaults again: the rule is *on* by default and the threshold is
        // `TILES_THRESHOLD`, so a round trip that used either would prove nothing. There is no third
        // key — whether the count asks this machine about `.pdf` and `.3dr` is not a setting, it is
        // what the count *is*. See [`crate::shell::providers`].
        auto_tiles: crate::pane::AutoTiles {
            on: false,
            threshold: 35.0,
        },
        ..Config::default()
    };

    let back = Config::parse(&saved.to_text());
    assert_eq!(back.window, saved.window);
    assert_eq!(back.position, saved.position);
    assert_eq!(back.layout, saved.layout);
    assert_eq!(back.focus, 1);
    assert_eq!(back.sidebar_width, 260.0);
    // Whether the panel is on screen at all, which is remembered beside how wide it is — and its own
    // key rather than a width of zero, so bringing it back finds the width it was dragged to.
    assert!(!back.sidebar_shown);
    assert_eq!(back.sidebar_width, saved.sidebar_width);
    // Not `dark: false` any more but a palette by name. `Palette::parse`'s own test covers the
    // words — including the retired ones — and what this adds is that the name survives the *file*:
    // written by `Config::save`, read back by `Config::load`, through everything else in it.
    assert_eq!(back.palette, crate::theme::Palette::Light);
    // The preview panel's four preferences. Worth pinning together with the window's shape,
    // because they are the same kind of thing — how the window comes back — and because a
    // value that is written and not read is the failure this round trip is for: `preview=` was
    // parsed for a while before anything wrote it, and the panel silently forgot its position
    // every session.
    assert_eq!(back.preview.at, crate::ui::preview::Where::Bottom);
    assert!((back.preview.share - 0.615).abs() < 1e-3);
    assert!(!back.preview.numbers, "the `0` did not survive");
    assert!(back.preview.markup);
    assert!(!back.preview.diff, "the `0` did not survive");
    assert!(back.preview.collapse);
    // Whether video plays with its sound, which is a habit in exactly the way the four above are.
    assert!(back.preview.muted);
    // And how the dependency view is set up. **All three**, because they were the failure this
    // round trip exists to catch: the two shares were written on every save and had no reader
    // at all, so a dragged panel came back the size it started. One key and one codec now,
    // which is what makes that not expressible.
    assert_eq!(back.preview.deps, saved.preview.deps);
    assert!(saved.to_text().contains("dependency=1,0.325,0.675"));
    // A settings file from the build before these features has no line for any of them, and the two
    // whose default is *on* — the gutter and the diff — have to come back on rather than off because
    // the key was missing.
    let older = Config::parse("theme=dark\n");
    assert!(older.preview.numbers, "on by default");
    assert!(older.preview.diff, "on by default");
    assert!(!older.preview.collapse);
    // And a file from before there was a player in this program comes back with the sound on, which
    // is what a video did the first time anybody previewed one.
    assert!(!older.preview.muted);
    // Which flatten mode the button produces, written as a word for the same reason the shell
    // is — and a file without the line comes back as the **tree**, which is the default the button
    // produces now. It was the list, and a settings file from before the key existed used to come
    // back that way on the grounds that it was what the button had always done; that reasoning was
    // dropped when the default moved, and this assertion is the last place still saying otherwise.
    assert_eq!(back.flat_mode, crate::pane::FlatMode::Tree);
    assert!(saved.to_text().contains("flatten=tree"));
    assert_eq!(older.flat_mode, crate::pane::FlatMode::Tree);
    // And whether that tree merges its chains of single folders, which is the other flag whose
    // default is *on* — so the older file has to come back with it set, and the saved one, which
    // turned it off, has to come back off.
    assert!(!back.regroup);
    assert!(older.regroup, "on by default");
    // The panel down the left is a third of those, and it is the one where getting the default wrong
    // would be worst: a file written by a build that predates the key has no line for it, and a
    // window that opened with no way to reach a drive or a bookmark would read as broken.
    assert!(older.sidebar_shown, "on by default");
    // And **nothing at all about rows or tiles**, which is the one listing setting this file
    // deliberately does not carry: a tab always opens in the details view. A `view=` key written
    // by hand is ignored, and one appearing here again would mean the argument on
    // `crate::pane::ViewMode` had been undone without anybody noticing.
    // Matched with the newline in front of it, or `preview=` would satisfy it and the assertion
    // would pass whatever this file wrote.
    assert!(
        !saved.to_text().contains("\nview="),
        "a `view=` key is being written: {}",
        saved.to_text()
    );
    // What *is* here about rows and tiles: the rule for choosing between them as a folder opens,
    // which is a habit and not an answer — see the module header. Its default is **on**, so it is the
    // `0` that has to survive and the *missing* line that has to come back on — one of the four flags
    // here read as "anything but 0", beside `regroup`, `diff` and `line_numbers`. And the threshold an
    // older file comes
    // back with has to be the default rather than a zero read off a line that is not there: a rule
    // that is on by default against a threshold of 0 would make a grid of every folder with one
    // picture in it.
    assert!(!back.auto_tiles.on, "the `0` did not survive");
    assert_eq!(back.auto_tiles.threshold, 35.0);
    assert!(older.auto_tiles.on, "on by default");
    assert_eq!(older.auto_tiles.threshold, crate::pane::TILES_THRESHOLD);
    // Whole percent in the file, because that is what the slider produces and what somebody
    // editing the file by hand would write.
    assert!(
        saved.to_text().contains("tiles_threshold=35"),
        "the threshold is not written as a percentage: {}",
        saved.to_text()
    );
    // And a hand-edited nonsense value cannot make the rule mean something it has no name for.
    let edited = Config::parse("auto_tiles=1\ntiles_threshold=900\n");
    assert_eq!(edited.auto_tiles.threshold, 100.0);
    // Whether hidden files are rows. The other way round from the three above — its default is
    // *off*, so it is the `1` that has to survive and the missing line that has to come back false.
    // The whole point of it being here is that `Ctrl+H` is not a keystroke you press again every
    // launch, so a value that is written and not read would be the feature quietly absent.
    assert!(back.show_hidden);
    assert!(!older.show_hidden, "off by default");
    // Which slash the path field writes. The same way round as the one above — its default is
    // *off*, so it is the missing line that has to come back false and the `1` that has to
    // survive. A preference nobody can keep is worse than no preference: the whole point of it is
    // that the field opens the same way every time.
    assert!(back.forward_slashes);
    assert!(!older.forward_slashes, "off by default");
    // And the console's two, which are the same kind of thing again. The shell is written as the
    // word the dropdown shows, so a settings file stays something you can read and edit.
    assert!((back.console_share - 0.28).abs() < 1e-3);
    assert_eq!(back.console_shell, crate::console::Kind::PowerShell);
    assert!(saved.to_text().contains("console_shell=pwsh"));

    assert_eq!(back.panes.len(), 2, "{:?}", back.panes);
    assert_eq!(back.panes[0].paths, saved.panes[0].paths);
    assert_eq!(back.panes[0].active, 1, "which tab was in front is part of it");
    assert_eq!(back.panes[1].paths, saved.panes[1].paths);
}

/// Which sidebar groups were folded, and what a file written before the fourth one existed means.
///
/// Its own test because this line is a *list of positions* rather than a key per value, which is
/// the one shape where adding a field can silently reinterpret every file already on disk. Two
/// claims: the four flags survive a round trip, and a three-flag line — every settings file this
/// program has ever written until now — still means the three groups it was written about.
#[test]
fn which_sidebar_groups_are_folded_survives_a_new_group_appearing() {
    let saved = Config {
        sections: Sections {
            drives: false,
            network: true,
            bookmarks: false,
            places: true,
        },
        ..Config::default()
    };
    let back = Config::parse(&saved.to_text()).sections;
    assert!(!back.drives);
    assert!(back.network);
    assert!(!back.bookmarks);
    assert!(back.places);

    // **The line a previous build wrote**, which had no fourth position at all. The three
    // decisions in it are kept — a length check here dropped all three and reopened every group
    // — and the group it says nothing about opens, which is what a group nobody has folded does.
    let older = Config::parse("sections=0,1,0\n").sections;
    assert!(!older.drives, "the older file's own decisions are still read");
    assert!(older.bookmarks);
    assert!(!older.places);
    assert!(older.network, "a group with no flag yet opens");

    // Network is written *last*, out of panel order, so that those first three positions go on
    // meaning what they have always meant. A file that put it second would read an old
    // `sections=1,0,1` as "bookmarks folded" turning into "network folded".
    assert!(
        saved.to_text().contains("sections=0,0,1,1"),
        "the flags moved: {}",
        saved.to_text()
    );
}

/// The bookmarks come back arranged the way they were left: the order, the groups, what is in
/// each of them, and which of them were folded shut.
///
/// Round-tripped for the same reason the window's shape is, and with one thing more to prove:
/// the *order* here is the order on screen, and it is spread across three keys that are read as
/// the file goes by — a `bookmark_in=` belongs to whichever `bookmark_group=` came last. A
/// reader that gathered the groups first and the marks afterwards would pass every assertion
/// about membership and still shuffle the list.
#[test]
fn bookmarks_and_their_groups_survive_a_write_and_a_read() {
    let mut marks = crate::ui::sidebar::Bookmarks::default();
    marks.add(PathBuf::from(r"C:\src"));
    let work = marks.add_group("Work, and more", true);
    marks.add_in(work, PathBuf::from(r"C:\work\api"));
    marks.add_in(work, PathBuf::from(r"C:\work\web"));
    // Folded shut, and after a mark that follows a group — which is what pins the order down.
    marks.add(PathBuf::from(r"D:\photos"));
    let shut = marks.add_group("Archive", false);
    marks.add_in(shut, PathBuf::from(r"E:\2019"));

    let saved = Config {
        bookmarks: marks.clone(),
        ..Config::default()
    };
    let back = Config::parse(&saved.to_text()).bookmarks;
    assert_eq!(back, marks, "in {}", saved.to_text());
    // And the name kept its comma, which is why the fold flag is written first and the split is
    // the first one rather than the last.
    assert_eq!(back.group(1).expect("a group").name, "Work, and more");
    // Entry 3, not 5: what is *in* a group is not a line of the list it sits in.
    assert_eq!(shut, 3);
    assert!(!back.group(shut).expect("a group").open);

    // A file from the build before groups is nothing but `bookmark=` lines, and that is a flat
    // list — the keys are new, so nothing has to be migrated, but a reader that needed a group
    // line before it would take a bookmark would come back empty.
    let older = Config::parse("theme=dark\nbookmark=C:\\a\nbookmark=C:\\b\n").bookmarks;
    assert_eq!(older.paths().count(), 2);
    assert_eq!(older.len_of(None), 2, "and none of them in a group");

    // A `bookmark_in=` with no group above it is a hand-edited file. The folder is worth more
    // than the line it was written on, so it lands at the top level rather than nowhere.
    let orphan = Config::parse("bookmark_in=C:\\a\n").bookmarks;
    assert_eq!(orphan.len_of(None), 1);
}

/// A settings file from the version that remembered tabs but not panes.
///
/// Its `path` lines have no `pane` line above them, and what they meant was one pane
/// holding all of them. Somebody upgrading has that file and no other, so reading it as
/// nothing at all would lose every folder they had open.
#[test]
fn a_file_from_before_panes_opens_as_one_pane() {
    let back = Config::parse("path=C:\\a\npath=C:\\b\ntheme=dark\n");
    assert_eq!(back.panes.len(), 1);
    assert_eq!(back.panes[0].paths.len(), 2);
    assert_eq!(back.layout, None, "and no layout to try to build");
}

/// The caps hold, and never by dropping a pane.
///
/// A pane with no tabs cannot be drawn, and `layout=` is a tree over exactly the panes
/// that follow it — so a cap that emptied one would describe a window this program then
/// refuses to build, and the whole layout would be thrown away over a tab limit.
#[test]
fn the_tab_cap_never_costs_a_pane() {
    let many: Vec<String> = (0..40).map(|i| format!("C:\\{i}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let saved = Config {
        // The first pane alone wants more than the cap allows, and there are two more
        // behind it.
        panes: vec![pane(30, &refs), pane(0, &[r"D:\one"]), pane(0, &[r"D:\two"])],
        layout: Some("h0.500(0,v0.500(1,2))".to_owned()),
        ..Config::default()
    };

    let back = Config::parse(&saved.to_text());
    assert_eq!(back.panes.len(), 3, "every pane has to be written");
    assert!(back.panes.iter().all(|p| !p.paths.is_empty()));
    let total: usize = back.panes.iter().map(|p| p.paths.len()).sum();
    assert!(total <= TABS + 2, "{total} tabs got through the cap");
    assert!(
        back.panes[0].active < back.panes[0].paths.len(),
        "the tab in front has to be one of the ones that survived"
    );
}

/// Nonsense is skipped, not fatal — and the rest of the file still lands.
#[test]
fn a_broken_line_costs_only_itself() {
    let back = Config::parse(
        "# a comment\n\
         \n\
         sidebar_width=lots\n\
         window=wide,tall\n\
         position=\n\
         focus=first\n\
         pane=x\n\
         path=C:\\a\n\
         theme=light\n",
    );
    assert_eq!(back.sidebar_width, SIDEBAR_WIDTH);
    assert_eq!(back.window, None);
    assert_eq!(back.position, None);
    assert_eq!(back.focus, 0);
    assert_eq!(back.panes.len(), 1);
    assert_eq!(back.panes[0].active, 0);
    assert_eq!(
        back.palette,
        crate::theme::Palette::Light,
        "and the line after the mess still applies"
    );
}

/// The context-menu entries the user has moved survive a write and a read, and are written in an
/// order that does not change on its own.
///
/// The sorting is the half worth a test. Both halves of [`crate::shell::menu::Moves`] are
/// `HashSet`s, so written in iteration order these lines shuffle between saves — and a settings file
/// that changes when nothing changed is one nobody can diff, on top of defeating
/// [`crate::app::App::save_settings`]'s "has this window actually changed anything" comparison,
/// which is a text compare.
#[test]
fn the_moved_context_menu_entries_survive_a_write_and_a_read() {
    let mut config = Config::default();
    // Verbs, a CLSID, and a label — the three shapes a key comes in. See `Moves::key`.
    for key in ["sendto", "{9F156763-7844-4DC4-B2B1-901F640F5155}", "Open with Code"] {
        config.menu_moves.record(key.to_owned(), false);
    }
    config.menu_moves.record("PreviousVersions".to_owned(), true);

    let text = config.to_text();
    let back = Config::parse(&text);
    assert_eq!(back.menu_moves, config.menu_moves);

    // Written sorted, and the same twice.
    let promoted: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("menu_promote="))
        .collect();
    assert_eq!(
        promoted,
        vec![
            "Open with Code",
            "sendto",
            "{9F156763-7844-4DC4-B2B1-901F640F5155}"
        ]
    );
    assert_eq!(text, back.to_text(), "a round trip changed the file");

    // A default config writes no lines at all, so an entry nobody has touched costs nothing.
    assert!(!Config::default().to_text().contains("menu_promote"));
    assert!(!Config::default().to_text().contains("menu_demote"));
}
