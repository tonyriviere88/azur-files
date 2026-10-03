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
        dark: false,
        preview: crate::ui::preview::Layout {
            at: crate::ui::preview::Where::Bottom,
            share: 0.615,
            numbers: true,
            markup: true,
            // Both away from their defaults, which is the only way a round trip proves anything:
            // a value that is never written still comes back right if the default happens to
            // match what was asked for.
            diff: false,
            collapse: true,
        },
        console_share: 0.28,
        console_shell: crate::console::Kind::PowerShell,
        // Away from their defaults for the same reason the two above are.
        flat_mode: crate::pane::FlatMode::Tree,
        regroup: false,
        forward_slashes: true,
        ..Config::default()
    };

    let back = Config::parse(&saved.to_text());
    assert_eq!(back.window, saved.window);
    assert_eq!(back.position, saved.position);
    assert_eq!(back.layout, saved.layout);
    assert_eq!(back.focus, 1);
    assert_eq!(back.sidebar_width, 260.0);
    assert!(!back.dark);
    // The preview panel's four preferences. Worth pinning together with the window's shape,
    // because they are the same kind of thing — how the window comes back — and because a
    // value that is written and not read is the failure this round trip is for: `preview=` was
    // parsed for a while before anything wrote it, and the panel silently forgot its position
    // every session.
    assert_eq!(back.preview.at, crate::ui::preview::Where::Bottom);
    assert!((back.preview.share - 0.615).abs() < 1e-3);
    assert!(back.preview.numbers);
    assert!(back.preview.markup);
    assert!(!back.preview.diff, "the one flag whose default is on");
    assert!(back.preview.collapse);
    // A settings file from the build before this feature has no line for either, and the diff
    // has to come back *on* — its default — rather than off because the key was missing.
    let older = Config::parse("theme=dark\nline_numbers=1\n");
    assert!(older.preview.diff, "on by default");
    assert!(!older.preview.collapse);
    // Which flatten mode the button produces, written as a word for the same reason the shell
    // is — and a file without the line comes back as the list, which is the view the button
    // produced before there was a choice.
    assert_eq!(back.flat_mode, crate::pane::FlatMode::Tree);
    assert!(saved.to_text().contains("flatten=tree"));
    assert_eq!(older.flat_mode, crate::pane::FlatMode::List);
    // And whether that tree merges its chains of single folders, which is the other flag whose
    // default is *on* — so the older file has to come back with it set, and the saved one, which
    // turned it off, has to come back off.
    assert!(!back.regroup);
    assert!(older.regroup, "on by default");
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
    // Which slash the path field writes. The other way round from the two above — its default is
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
    assert!(!back.dark, "and the line after the mess still applies");
}
