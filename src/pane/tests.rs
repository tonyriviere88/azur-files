use super::*;

/// An empty folder standing beside a merged chain does not wear a twisty.
///
/// **This is the one thing a chain breaks if the tree is read by path depth.** `empty` is at the
/// top level with nothing in it; the row after it is the chain `zip > inner`, whose *path* is one
/// level deeper than the place it is drawn. "The next row is deeper, so this folder has something
/// in it" — which is how the twisty is decided, and rightly — would have given `empty` a chevron
/// that opens nothing. So the comparison is between the depths the rows are drawn at, and this is
/// the listing that tells the two apart.
///
/// It also pins the rest of the shape a chain has to have from the tab's side: the merged row
/// stands where `zip` stood, and the file inside it is one level in from *there* rather than at
/// the two its path would say.
#[test]
fn an_empty_folder_beside_a_merged_chain_has_nothing_below_it() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR};

    // Level by level, the way a deep walk hands a listing over.
    let mut builder = DirBuilder::new(r"C:\x");
    builder.push("empty", 0, 0, FLAG_DIR);
    builder.push("zip", 0, 0, FLAG_DIR);
    builder.push(r"zip\inner", 0, 0, FLAG_DIR);
    builder.push(r"zip\inner\f.txt", 1, 0, 0);

    let mut tab = Tab::new(r"C:\x");
    tab.flat = true;
    tab.flat_mode = FlatMode::Tree;
    tab.regroup = true;
    tab.apply(Arc::new(builder.finish(0)));

    let dir = tab.dir.clone().expect("a listing");
    let names: Vec<&str> = tab.order.iter().map(|&i| dir.name(i as usize)).collect();
    assert_eq!(names, ["empty", r"zip\inner", r"zip\inner\f.txt"]);
    assert_eq!(
        (tab.row_depth(0), tab.row_depth(1), tab.row_depth(2)),
        (0, 0, 1),
        "the chain stands where `zip` stood, and its file is one in from there"
    );
    assert_eq!(tab.row_merged(1), 1, "one folder merged into the chain");

    assert!(
        !tab.has_children_below(0),
        "`empty` was given a twisty by the chain next to it"
    );
    assert!(
        tab.has_children_below(1),
        "the chain has a file in it and no twisty to open it with"
    );
    assert!(!tab.has_children_below(2), "a file is not a folder");
}

/// The size of the selection is kept in step by every gesture that can change it.
///
/// It is maintained rather than measured — see [`Tab::selected_size`] — so the thing worth testing
/// is that no gesture forgets. Every assertion below compares the running figure against a fresh
/// walk over the whole folder, which is the answer the status line would otherwise have to compute
/// on every frame.
#[test]
fn a_selection_keeps_its_own_size() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR};

    // Three files and a folder. The folder carries a size the way the find data does, and it must
    // not be counted: a directory's byte count is noise.
    let mut builder = DirBuilder::new(r"C:\here");
    builder.push("a.txt", 100, 0, 0);
    builder.push("b.txt", 20, 0, 0);
    builder.push("c.txt", 3, 0, 0);
    builder.push("sub", 4096, 0, FLAG_DIR);
    let mut tab = Tab::new(r"C:\here");
    tab.apply(Arc::new(builder.finish(0)));
    assert_eq!(tab.order.len(), 4, "everything is on show");

    // What a walk would say, over the same convention `selected_count` follows: every entry that
    // is selected, whether or not the filter is showing it.
    let walked = |tab: &Tab| -> u64 {
        let dir = tab.dir.as_ref().expect("a listing");
        (0..dir.len())
            .filter(|&entry| tab.selected[entry])
            .map(|entry| Tab::size_at(Some(dir), entry))
            .sum()
    };
    let check = |tab: &Tab, what: &str| {
        assert_eq!(tab.selected_size, walked(tab), "after {what}");
    };

    // One row at a time, and a folder among them.
    let row = |tab: &Tab, name: &str| {
        let dir = tab.dir.as_ref().expect("a listing");
        (0..tab.order.len())
            .find(|&at| tab.entry_at(at).is_some_and(|e| dir.name(e) == name))
            .expect("the row")
    };
    let (a, b, c, sub) = (
        row(&tab, "a.txt"),
        row(&tab, "b.txt"),
        row(&tab, "c.txt"),
        row(&tab, "sub"),
    );

    tab.select_only(a);
    assert_eq!(tab.selected_size, 100);
    check(&tab, "one click");

    tab.toggle(b);
    assert_eq!(tab.selected_size, 120, "and the second one added");
    check(&tab, "ctrl-click");
    tab.toggle(b);
    assert_eq!(tab.selected_size, 100, "and taken away again");
    check(&tab, "ctrl-click off");

    tab.select_only(sub);
    assert_eq!(tab.selected_size, 0, "a folder's own size is not a size");
    check(&tab, "a folder clicked");

    tab.select_only(a);
    tab.anchor = Some(a);
    tab.select_range_to(c);
    assert_eq!(tab.selected_size, 123, "a shift-click covers three files");
    check(&tab, "shift-click");

    tab.select_all();
    assert_eq!(tab.selected_size, 123, "and so does everything");
    check(&tab, "select all");

    tab.clear_selection();
    assert_eq!(tab.selected_size, 0);
    check(&tab, "clearing");

    // A rubber band, which recomputes from its own base rather than accumulating.
    tab.select_only(a);
    tab.band = Some(Band {
        base: tab.selected.clone(),
        anchor: egui::pos2(0.0, 0.0),
        current: egui::pos2(0.0, ROW_HEIGHT * 2.5),
    });
    tab.apply_band();
    check(&tab, "a band");
    assert!(
        tab.selected_size >= 100,
        "the band kept what it started with"
    );

    // And the listing being read again keeps the selection by name — and its size with it.
    let mut again = DirBuilder::new(r"C:\here");
    again.push("a.txt", 100, 0, 0);
    again.push("b.txt", 20, 0, 0);
    tab.band = None;
    tab.select_only(row(&tab, "a.txt"));
    tab.refresh();
    tab.apply(Arc::new(again.finish(0)));
    assert_eq!(tab.selected_count, 1, "a.txt came back selected");
    assert_eq!(tab.selected_size, 100);
    check(&tab, "a re-read");

    // Somewhere else is nothing selected at all.
    tab.go_to(PathBuf::from(r"C:\elsewhere"));
    assert_eq!(tab.selected_size, 0);
}

/// [`Lens::Git`] has three answers, and the one that matters is the middle one.
///
/// Git answers a frame or two after the listing, and in between the lens is a question that
/// cannot be evaluated. Excluding everything until then would empty the listing on every refresh —
/// on every file operation, every `F5`, every time the watcher notices something — and then fill it
/// again, which reads as the folder having been wiped. So an unanswered question excludes nothing,
/// and `App::collect_git` rebuilds the order when the answer lands.
///
/// The third answer is a folder outside a repository: **asked, and there is nothing here**, which
/// keeps no rows. It is the one case that needs `git_answered` rather than `git` to tell it from the
/// second.
#[test]
fn the_git_lens_waits_for_git_rather_than_emptying_the_listing() {
    use crate::fs::dir::DirBuilder;

    let mut builder = DirBuilder::new(r"C:\repo");
    for name in ["touched.rs", "untouched.rs"] {
        builder.push(name, 10, 0, 0);
    }
    let mut tab = Tab::new(r"C:\repo");
    tab.apply(Arc::new(builder.finish(0)));
    tab.lens = Some(Lens::Git);
    assert!(tab.filters_on_git(), "the listing asks about git");

    // Asked and not answered: everything, because nothing here can say otherwise yet.
    tab.rebuild_order();
    assert_eq!(tab.order.len(), 2, "the wait emptied the listing");

    // Answered, and there is no repository: nothing has changed because nothing could have.
    tab.git_answered = true;
    tab.rebuild_order();
    assert_eq!(
        tab.order.len(),
        0,
        "a folder with no repository has no changes"
    );

    // Answered with a repository: the rows git has something to say about, and no others.
    tab.git = Some(Arc::new(crate::git::Repo::of([(
        "touched.rs",
        crate::git::State::Modified,
    )])));
    tab.rebuild_order();
    let names: Vec<String> = tab
        .order
        .iter()
        .map(|&i| {
            tab.dir
                .as_ref()
                .expect("a listing")
                .name(i as usize)
                .to_owned()
        })
        .collect();
    assert_eq!(names, ["touched.rs"]);

    // And the name half still applies on top of it, which is what "the two compose" means: the
    // lens is a different question from the text, not a second answer to it.
    tab.filter = ".txt$".to_owned();
    tab.rebuild_order();
    assert_eq!(tab.order.len(), 0, "changed, but not a .txt");

    // With the lens off the folder comes back whole, whatever git says about it.
    tab.filter.clear();
    tab.lens = None;
    tab.rebuild_order();
    assert_eq!(tab.order.len(), 2);
}

/// Arriving somewhere the breadcrumb still runs past selects the segment below it.
#[test]
fn arriving_at_a_folder_selects_the_child_the_breadcrumb_shows() {
    // One step up: `c` is on the bar and is where you just came from.
    let mut tab = Tab::new(r"C:\a\b\c");
    tab.navigate(r"C:\a\b");
    assert_eq!(tab.reveal.as_deref(), Some("c"));

    // Two at once, which is what clicking a segment does. The old rule only ever managed a
    // single step, because it asked whether the place it landed was the parent.
    let mut tab = Tab::new(r"C:\a\b\c");
    tab.navigate(r"C:\a");
    assert_eq!(tab.reveal.as_deref(), Some("b"));

    // Going *down* leaves nothing behind on the bar to point at.
    let mut tab = Tab::new(r"C:\a");
    tab.navigate(r"C:\a\b");
    assert_eq!(tab.reveal, None);

    // Sideways: the trail is replaced, so again there is nothing.
    let mut tab = Tab::new(r"C:\a\b\c");
    tab.navigate(r"D:\elsewhere");
    assert_eq!(tab.reveal, None);
}

/// The three ways of arriving all get it, because they all go through `go_to`.
#[test]
fn up_back_and_forward_all_highlight_off_the_trail() {
    let mut tab = Tab::new(r"C:\a\b\c");
    tab.go_up();
    assert_eq!(tab.path, PathBuf::from(r"C:\a\b"));
    assert_eq!(tab.reveal.as_deref(), Some("c"), "up left `c` behind");

    tab.go_up();
    assert_eq!(tab.reveal.as_deref(), Some("b"), "and `b` above that");
    // Going up *records* the arrival, so there is nothing forward of here to go to — the
    // history reads `c`, `b`, `a` and the cursor is on its last entry.
    assert!(!tab.can_go_forward());

    // Back down it, which is where `go_back` used to do this for itself.
    tab.go_back();
    assert_eq!(tab.path, PathBuf::from(r"C:\a\b"));
    assert_eq!(tab.reveal.as_deref(), Some("c"));
    tab.go_back();
    assert_eq!(tab.path, PathBuf::from(r"C:\a\b\c"));
    assert_eq!(tab.reveal, None, "arriving at the end of the trail");

    // And forward, which never highlighted anything before and now does.
    tab.go_forward();
    assert_eq!(tab.path, PathBuf::from(r"C:\a\b"));
    assert_eq!(tab.reveal.as_deref(), Some("c"));
}

/// A re-read of the same folder keeps what was selected; going somewhere else does not.
///
/// Every file operation ends in a re-read, so a selection that does not survive one means
/// copying a file and then having nothing selected to copy again. That is what made Ctrl+C
/// then Ctrl+V work exactly once: the second Ctrl+C had an empty selection and copied nothing.
#[test]
fn a_re_read_keeps_the_selection_and_a_new_folder_does_not() {
    use crate::fs::dir::DirBuilder;

    let folder = PathBuf::from(r"C:\somewhere");
    let listing = |at: &std::path::Path, names: &[&str]| {
        let mut build = DirBuilder::new(at.to_path_buf());
        for name in names {
            build.push(name, 1, 0, 0);
        }
        Arc::new(build.finish(0))
    };
    /// The display position of a name, which is what the selection is indexed by.
    fn position_of(tab: &Tab, name: &str) -> usize {
        (0..tab.order.len())
            .find(|at| {
                tab.entry_at(*at)
                    .and_then(|e| tab.dir.as_ref().map(|d| d.name(e) == name))
                    .unwrap_or(false)
            })
            .unwrap_or_else(|| panic!("`{name}` is not in the listing"))
    }

    let mut tab = Tab::new(folder.clone());
    tab.apply(listing(&folder, &["one.txt", "two.txt", "three.txt"]));
    tab.select_only(position_of(&tab, "two.txt"));
    assert_eq!(tab.selection_paths().len(), 1);

    // The same folder read again -- a refresh, or the tail of a file operation.
    tab.apply(listing(&folder, &["one.txt", "two.txt", "three.txt", "four.txt"]));
    let still: Vec<String> = tab
        .selection_paths()
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(still, ["two.txt"], "the selection has to survive a re-read");
    assert!(tab.cursor.is_some(), "and the cursor has to go with it");

    // Something that has gone is simply not selected any more.
    tab.apply(listing(&folder, &["one.txt", "three.txt"]));
    assert_eq!(tab.selection_paths().len(), 0);

    // A different folder is a different set of files, even when a name matches.
    tab.apply(listing(&folder, &["one.txt", "two.txt"]));
    tab.select_only(position_of(&tab, "two.txt"));
    let elsewhere = PathBuf::from(r"C:\elsewhere");
    tab.go_to(elsewhere.clone());
    tab.apply(listing(&elsewhere, &["two.txt"]));
    assert_eq!(
        tab.selection_paths().len(),
        0,
        "a name that happens to match somewhere else is not the same file"
    );
}

/// A temp-free listing of whatever names you name, for the two tests below.
#[cfg(test)]
fn listing(folder: &std::path::Path, names: &[&str]) -> Arc<crate::fs::dir::Dir> {
    use crate::fs::dir::DirBuilder;
    let mut build = DirBuilder::new(folder.to_path_buf());
    for name in names {
        build.push(name, 1, 0, 0);
    }
    Arc::new(build.finish(0))
}

/// Where a leaf is in the display order, which is what the selection is indexed by.
#[cfg(test)]
fn leaf_at(tab: &Tab, leaf: &str) -> usize {
    (0..tab.order.len())
        .find(|&at| {
            tab.entry_at(at)
                .and_then(|e| tab.dir.as_ref().map(|d| d.leaf(e) == leaf))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("`{leaf}` is not in the listing"))
}

/// The leaf the open rename field is attached to.
#[cfg(test)]
fn renaming_leaf(tab: &Tab) -> String {
    let (entry, _) = tab.renaming.clone().expect("a rename in progress");
    tab.dir.as_ref().expect("a listing").leaf(entry).to_owned()
}

/// A rename in progress survives the folder being re-read, and stays on the same *file*.
///
/// The two halves of naming a newly made file collide by construction: the rename starts when
/// a re-read brings the file in, and making a file is exactly what has [`crate::watch`] asking
/// for another re-read. `renaming` holds an *entry index*, so left alone across that second one
/// the field is still on screen, still committing, and commits the typed name onto whichever
/// file has landed at that index.
#[test]
fn an_open_rename_follows_its_file_across_a_re_read() {
    let folder = PathBuf::from(r"C:\somewhere");
    let mut tab = Tab::new(folder.clone());
    tab.apply(listing(&folder, &["a.txt", "target.txt", "z.txt"]));
    tab.select_only(leaf_at(&tab, "target.txt"));
    tab.begin_rename();
    // Half-typed, which is the state that has something to lose.
    tab.renaming.as_mut().expect("a rename").1 = "half-typ".to_owned();
    tab.rename_fresh = false;

    // A re-read with a new file in front of it, which is what moves the index.
    tab.apply(listing(&folder, &["new.txt", "a.txt", "target.txt", "z.txt"]));
    assert_eq!(
        renaming_leaf(&tab),
        "target.txt",
        "the rename field ended up on a different file"
    );
    assert_eq!(
        tab.renaming.as_ref().expect("a rename").1,
        "half-typ",
        "what was typed into the field was lost"
    );
    assert!(
        tab.rename_fresh,
        "the field moved to another row -- a different egui id, with none of the old focus -- \
         and nothing asked for the caret back"
    );

    // A re-read that leaves it where it was asks for nothing, so a caret mid-name stays put.
    tab.rename_fresh = false;
    tab.apply(listing(&folder, &["new.txt", "a.txt", "target.txt", "z.txt"]));
    assert_eq!(renaming_leaf(&tab), "target.txt");
    assert!(
        !tab.rename_fresh,
        "the row did not move and the caret was thrown to the end anyway"
    );

    // And the file going takes the field with it: there is nothing left to commit onto.
    tab.apply(listing(&folder, &["new.txt", "a.txt", "z.txt"]));
    assert!(
        tab.renaming.is_none(),
        "a rename field left open over a file that has gone"
    );
}

/// The file the shell's `New >` just made arrives selected with its name open for editing.
///
/// Found by elimination, because the shell will not say what it made and the name it picks is
/// localised and may already have been taken. See [`Tab::name_the_new`].
#[test]
fn the_file_the_shell_just_made_opens_for_renaming() {
    let folder = PathBuf::from(r"C:\somewhere");
    let mut tab = Tab::new(folder.clone());
    tab.apply(listing(&folder, &["a.txt", "b.txt"]));
    tab.select_only(leaf_at(&tab, "a.txt"));

    // What `App::draw_menu` writes down before handing the verb to the shell.
    tab.name_the_new = Some(tab.names());

    // The watcher's re-read, with whatever the shell decided to call it.
    let made = "Nouveau document texte (2).txt";
    tab.apply(listing(&folder, &["a.txt", "b.txt", made]));
    assert_eq!(
        renaming_leaf(&tab),
        made,
        "the row that was not there before is the one to name"
    );
    assert_eq!(
        tab.renaming.as_ref().expect("a rename").1,
        made,
        "the field starts from the name the file actually has"
    );
    assert_eq!(
        tab.selected_count, 1,
        "and it is the only thing selected, whatever was selected before"
    );
    assert!(tab.scroll_to_cursor, "a new row can be row 500");
    assert!(
        tab.name_the_new.is_none(),
        "the snapshot has to be consumed, or the next thing any program writes here opens a \
         rename field of its own"
    );

    // A listing with nothing new in it consumes the snapshot too, rather than lying in wait.
    tab.name_the_new = Some(tab.names());
    tab.renaming = None;
    tab.apply(listing(&folder, &["a.txt", "b.txt", made]));
    assert!(tab.renaming.is_none(), "nothing was created and a rename opened");
    assert!(tab.name_the_new.is_none());
}

/// **The tiles do not follow you into the next folder**, and neither does the switch remember
/// anything: every folder opens in the details view.
///
/// The rule [`ViewMode`] argues for, pinned here because it is one line in [`Tab::go_to`] that
/// nothing else would notice going missing. The three navigations are tested separately because
/// they arrive by different routes and only `go_to` is common to them.
///
/// The other half is what must *not* reset: a **refresh** is the same folder read again — every
/// file operation ends in one, and a paste that dropped you back into rows would be the view
/// undoing itself under your hands — and a **flatten** is another question about the folder you are
/// already looking at.
#[test]
fn going_anywhere_puts_the_details_view_back() {
    let mut tab = Tab::new(r"C:\a");
    assert_eq!(tab.view_mode, ViewMode::Details, "and it starts there");

    tab.view_mode = ViewMode::Icons;
    tab.navigate(r"C:\a\b");
    assert_eq!(tab.view_mode, ViewMode::Details, "going down");

    tab.view_mode = ViewMode::Icons;
    tab.go_back();
    assert_eq!(tab.view_mode, ViewMode::Details, "going back");

    tab.view_mode = ViewMode::Icons;
    tab.go_up();
    assert_eq!(tab.view_mode, ViewMode::Details, "going up");

    tab.view_mode = ViewMode::Icons;
    tab.refresh();
    assert_eq!(tab.view_mode, ViewMode::Icons, "a refresh is the same folder");
    tab.toggle_flat(FlatMode::List, true);
    assert_eq!(tab.view_mode, ViewMode::Icons, "so is a flatten");
}

/// A snapshot means nothing once the tab is looking at something else.
#[test]
fn leaving_the_folder_drops_the_snapshot() {
    let folder = PathBuf::from(r"C:\somewhere");
    let mut tab = Tab::new(folder.clone());
    tab.apply(listing(&folder, &["a.txt"]));

    tab.name_the_new = Some(tab.names());
    let elsewhere = PathBuf::from(r"C:\elsewhere");
    tab.go_to(elsewhere.clone());
    assert!(tab.name_the_new.is_none(), "carried to another folder");
    // Where every row would otherwise have looked new.
    tab.apply(listing(&elsewhere, &["x.txt", "y.txt"]));
    assert!(tab.renaming.is_none());

    // The flat toggle is the other one: a name means `file.txt` on one side of it and
    // `sub\file.txt` on the other, so nothing would match and every row would look new.
    tab.name_the_new = Some(tab.names());
    tab.toggle_flat(FlatMode::Tree, false);
    assert!(tab.name_the_new.is_none(), "carried across the flat toggle");
}

#[test]
fn navigating_after_back_drops_the_future() {
    let mut tab = Tab::new("/a");
    tab.navigate("/a/b");
    tab.navigate("/a/b/c");
    assert!(tab.can_go_back());

    tab.go_back();
    assert_eq!(tab.path, PathBuf::from("/a/b"));
    assert!(tab.can_go_forward());

    tab.navigate("/a/b/d");
    assert!(
        !tab.can_go_forward(),
        "going somewhere new has to replace the forward trail"
    );
    assert_eq!(tab.history, ["/a", "/a/b", "/a/b/d"].map(PathBuf::from));
}

#[test]
fn walking_up_keeps_the_trail_on_the_breadcrumb() {
    let mut tab = Tab::new("/a/b/c");
    assert_eq!(tab.trail, PathBuf::from("/a/b/c"));

    // Up: the folder just left is still on the bar, which is the point.
    tab.go_up();
    assert_eq!(tab.path, PathBuf::from("/a/b"));
    assert_eq!(tab.trail, PathBuf::from("/a/b/c"));

    // And again -- two levels up still shows all three.
    tab.go_up();
    assert_eq!(tab.path, PathBuf::from("/a"));
    assert_eq!(tab.trail, PathBuf::from("/a/b/c"));

    // Back down onto the trail: the trail is unchanged, so nothing on the bar moves
    // while the bold segment walks along it.
    tab.navigate("/a/b");
    assert_eq!(tab.trail, PathBuf::from("/a/b/c"));

    // Deeper than the trail extends it.
    tab.navigate("/a/b/c/d");
    assert_eq!(tab.trail, PathBuf::from("/a/b/c/d"));
}

#[test]
fn stepping_off_the_trail_replaces_it() {
    let mut tab = Tab::new("/a/b/c");
    tab.go_up();

    // A sibling is not an ancestor, however much of the path it shares.
    tab.navigate("/a/b/x");
    assert_eq!(tab.trail, PathBuf::from("/a/b/x"));

    // Nor is a folder whose name merely starts the same way: `starts_with` compares
    // components, so `/a/bb` is not under `/a/b`.
    tab.navigate("/a/bb");
    assert_eq!(tab.trail, PathBuf::from("/a/bb"));
}

#[test]
fn this_pc_is_up_from_everywhere() {
    // The empty path is This PC, and the breadcrumb always starts there -- so going to
    // it is walking up, and the trail stays.
    let mut tab = Tab::new("/a/b");
    tab.navigate(PathBuf::new());
    assert!(tab.path.as_os_str().is_empty());
    assert_eq!(tab.trail, PathBuf::from("/a/b"));

    // Out of This PC to a different root: nothing shared, so the trail goes.
    tab.navigate("/z");
    assert_eq!(tab.trail, PathBuf::from("/z"));
}

#[test]
fn a_duplicate_shows_the_same_bar() {
    let mut tab = Tab::new("/a/b/c");
    tab.go_up();
    let copy = tab.duplicate();
    assert_eq!(copy.path, PathBuf::from("/a/b"));
    assert_eq!(
        copy.trail,
        PathBuf::from("/a/b/c"),
        "a duplicate that lost the trail would show a different bar from the tab it \
         was copied from"
    );
}

#[test]
fn navigating_to_where_you_are_is_not_history() {
    let mut tab = Tab::new("/a");
    tab.navigate("/a");
    assert_eq!(tab.history.len(), 1);
}

#[test]
fn history_stops_growing() {
    let mut tab = Tab::new("/0");
    for i in 1..400 {
        tab.navigate(format!("/{i}"));
    }
    assert_eq!(tab.history.len(), 256);
    assert_eq!(tab.at, 255, "the cursor has to follow the truncation");
    assert_eq!(tab.history[tab.at], tab.path);
}

#[test]
fn closing_the_last_tab_reports_the_pane_is_empty() {
    let mut pane = Pane::new(0, Tab::new("/a"));
    assert!(!pane.close_tab(0));
}

#[test]
fn closing_a_tab_keeps_the_active_one_active() {
    let mut pane = Pane::new(0, Tab::new("/a"));
    pane.tabs.push(Tab::new("/b"));
    pane.tabs.push(Tab::new("/c"));
    pane.active = 2;

    assert!(pane.close_tab(0));
    assert_eq!(pane.active, 1, "still looking at /c");
    assert_eq!(pane.tab().path, PathBuf::from("/c"));

    assert!(pane.close_tab(1));
    assert_eq!(pane.tab().path, PathBuf::from("/b"));
}
