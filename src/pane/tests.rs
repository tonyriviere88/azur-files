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

/// A folder of pictures opens as tiles; **the same folder read again does not go back to tiles.**
///
/// The second half is the whole of what "only when opening a folder" means, and it is the half that
/// cannot be read off the source with any confidence: a listing arrives for four different reasons
/// and three of them are the same folder again. So this drives all four — an opening, a refresh, a
/// watcher-style re-read, and the folder revisited after going somewhere else — and each time asks
/// whether the view is the one the *reader* last chose. See [`Tab::opening`].
#[test]
fn a_folder_of_pictures_opens_as_tiles_and_a_refresh_leaves_the_view_alone() {
    use crate::fs::dir::DirBuilder;

    /// Eight pictures and two files that are not: 80%, which is over the default threshold and
    /// under a hundred, so a rule that had come to mean "all of them" would fail here too.
    fn gallery() -> Arc<Dir> {
        let mut builder = DirBuilder::new(r"C:\photos");
        for i in 0..8 {
            builder.push(&format!("shot-{i}.jpg"), 1, 0, 0);
        }
        builder.push("notes.txt", 1, 0, 0);
        builder.push("index.html", 1, 0, 0);
        Arc::new(builder.finish(0))
    }

    let auto = AutoTiles { on: true, ..AutoTiles::default() };
    // The fixture's `.txt` and `.html` are not pictures by name, so this machine is asked about them
    // — see `asking_the_machine_can_only_add_to_the_count`. Whatever it answers, the share is between
    // 80% and 100%, and every assertion below is about a threshold of 60.
    let mut providers = crate::shell::providers::Providers::new();
    let mut tab = Tab::new(r"C:\photos");
    assert!(tab.opening, "a tab that has shown nothing yet is opening");
    tab.apply(gallery());
    let (pictures, rows) = tab.picture_rows(&mut providers);
    assert!(
        (8..=10).contains(&pictures) && rows == 10,
        "the fixture reads as {pictures} of {rows}, and eight of its ten rows are `.jpg`"
    );
    tab.choose_view(auto, &mut providers);
    assert_eq!(
        tab.view_mode,
        ViewMode::Icons,
        "a folder that is four fifths pictures did not open as tiles"
    );
    assert!(!tab.opening, "the judgement is once per opening, not once per listing");

    // The reader wants the rows after all. Every re-read from here has to leave that alone.
    tab.view_mode = ViewMode::Details;

    // `F5`: the listing is dropped and the same folder comes back.
    tab.refresh();
    tab.apply(gallery());
    tab.choose_view(auto, &mut providers);
    assert_eq!(
        tab.view_mode,
        ViewMode::Details,
        "a refresh put the tiles back over somebody who had just switched to rows"
    );

    // The watcher, which does not drop the listing first — a build, or another program writing
    // into the folder. The most dangerous of the four, because nobody asked for it.
    tab.apply(gallery());
    tab.choose_view(auto, &mut providers);
    assert_eq!(
        tab.view_mode,
        ViewMode::Details,
        "something else touching the folder switched the view underneath the reader"
    );

    // And going away and coming back **is** an opening, so it is judged afresh: the answer is
    // about the folder, and nothing about the last visit is remembered.
    tab.navigate(r"C:\src");
    tab.navigate(r"C:\photos");
    assert!(tab.opening);
    tab.apply(gallery());
    tab.choose_view(auto, &mut providers);
    assert_eq!(tab.view_mode, ViewMode::Icons);
}

/// **Asking the machine can only ever add to the count, and never a folder.**
///
/// What it *answers* is a fact about the machine the test is running on — whether a `.pdf` reader and
/// a `.3dr` viewer are installed — so nothing here asserts on which types come back:
/// `shell::providers::tests::providers_on_this_machine` prints that instead. What is testable is the
/// shape, and the shape is what a bug would break.
///
/// The table's own answer is computed here rather than asked for, because there is no longer a way to
/// ask [`Tab::picture_rows`] for it: both questions are always put. That is the point of the bound —
/// whatever the machine says, the count cannot come out *below* what the table already claimed.
///
/// And the registry is asked once per **type**: four here, not five rows, because the photograph never
/// reaches it — the table answers that one first, which is what keeps a folder of pictures free.
#[test]
fn asking_the_machine_can_only_add_to_the_count() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR};

    let mut builder = DirBuilder::new(r"C:\work");
    builder.push("sub", 0, 0, FLAG_DIR);
    builder.push("plan.pdf", 1, 0, 0);
    builder.push("model.3dr", 1, 0, 0);
    builder.push("sheet.xlsx", 1, 0, 0);
    builder.push("photo.jpg", 1, 0, 0);
    builder.push("build.log", 1, 0, 0);
    let mut tab = Tab::new(r"C:\work");
    tab.apply(Arc::new(builder.finish(0)));

    // What this program's own table makes of the same rows, walked the same way.
    let dir = tab.dir.clone().expect("a listing");
    let by_name = tab
        .order
        .iter()
        .filter(|&&row| {
            let entry = row as usize;
            crate::fs::fmt::shows_a_picture(dir.ext(entry), dir.entries[entry].is_dir())
        })
        .count();
    assert_eq!(by_name, 1, "only the photograph is a picture by name");

    let mut providers = crate::shell::providers::Providers::new();
    let (pictures, rows) = tab.picture_rows(&mut providers);
    assert_eq!(rows, 6, "the machine changed how many rows there are");
    assert!(
        pictures >= by_name,
        "the count came out below the {by_name} the table had already claimed"
    );
    assert!(
        pictures <= 5,
        "{pictures} of 6 rows counted, and one of them is a folder"
    );
    assert_eq!(
        providers.asked(),
        4,
        "{} types asked about, and there are four the table will not claim",
        providers.asked()
    );
    let _ = tab.picture_rows(&mut providers);
    assert_eq!(
        providers.asked(),
        4,
        "a second pass over the same folder asked the registry again"
    );
}

/// The rule, at its edges. Each of these was a decision rather than an accident.
#[test]
fn the_tiles_rule_counts_the_rows_on_show() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR, FLAG_HIDDEN};

    // A fresh profile has the rule **on** — see [`AutoTiles`], which is one of the two defaults in
    // this program that are not the quieter option.
    let fresh = AutoTiles::default();
    assert!(fresh.on, "the rule is on out of the box");
    assert_eq!(fresh.threshold, TILES_THRESHOLD);
    let off = AutoTiles { on: false, ..fresh };
    assert!(!off.reached(9, 10), "off means off, whatever is in the folder");

    let on = AutoTiles { on: true, threshold: 60.0 };
    assert!(on.reached(6, 10), "the threshold is a floor, not a fence");
    assert!(!on.reached(5, 10));
    assert!(!on.reached(0, 0), "an empty folder is not a folder of pictures");
    // The bottom of the slider means "as soon as there is one", not "always" — which is the
    // difference between a useful setting and one that turns every empty folder into a grid.
    let any = AutoTiles { on: true, threshold: 0.0 };
    assert!(any.reached(1, 400));
    assert!(!any.reached(0, 400));
    assert!(!any.reached(0, 0));
    // And the top means all of them.
    let all = AutoTiles { on: true, threshold: 100.0 };
    assert!(all.reached(4, 4));
    assert!(!all.reached(3, 4));

    // ---- What counts as a row, and what counts as a picture -------------
    let mut builder = DirBuilder::new(r"C:\mixed");
    builder.push("sub", 0, 0, FLAG_DIR);
    builder.push("clip.mp4", 1, 0, 0);
    builder.push("photo.JPG", 1, 0, 0);
    builder.push("scan.pdf", 1, 0, 0);
    builder.push("Thumbs.db", 1, 0, FLAG_HIDDEN);
    let mut tab = Tab::new(r"C:\mixed");
    tab.apply(Arc::new(builder.finish(0)));

    // Asked of the table alone, walked the way [`Tab::picture_rows`] walks: what a `.pdf` counts as is
    // this machine's business, and `asking_the_machine_can_only_add_to_the_count` is where that half
    // is pinned. What is being checked here is which *rows* are counted at all.
    let dir = tab.dir.clone().expect("a listing");
    let by_name = tab
        .order
        .iter()
        .filter(|&&row| {
            let entry = row as usize;
            crate::fs::fmt::shows_a_picture(dir.ext(entry), dir.entries[entry].is_dir())
        })
        .count();
    assert_eq!(
        (by_name, tab.order.len()),
        (2, 4),
        "the video counts, the extension's case does not, the `.pdf` is not a picture by name, the \
         hidden row is not on show at all — and the folder is a row and not a picture"
    );
    // Which is the point of counting rows rather than files: a folder is not a picture, so a
    // folder full of folders cannot pass the rule on the strength of one photograph.
    let mut builder = DirBuilder::new(r"C:\tree");
    for i in 0..99 {
        builder.push(&format!("f{i:02}"), 0, 0, FLAG_DIR);
    }
    builder.push("cover.png", 1, 0, 0);
    let mut tab = Tab::new(r"C:\tree");
    tab.apply(Arc::new(builder.finish(0)));
    tab.choose_view(on, &mut crate::shell::providers::Providers::new());
    assert_eq!(
        tab.view_mode,
        ViewMode::Details,
        "one picture among ninety-nine folders opened a grid of folder tiles"
    );
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

/// **The measurement's whole state machine, from the tab's side.**
///
/// The walk itself is [`crate::sizes`]' and is tested there. What lives here is everything the Size
/// column reads, and every part of it is a way to get a wrong number onto the screen:
///
/// - A folder with no total is a **blank cell** and never a `0 B` — see [`Tab::size_shown`], which
///   has to tell "not counted" from "counted, and empty".
/// - A folder is asked about **once**, so a listing that has been counted asks for nothing on the
///   next frame — which is what `Measurement`'s per-row "asked" state is for.
/// - The **share** is of what is on show, and in a folder's own listing the rows partition it: the
///   shares have to add up to one, or the bars are drawn against a total that is not the one the
///   column adds up to.
/// - A **link** is given no number rather than a zero, because the walk would not follow it.
#[test]
fn measuring_a_folder_fills_its_cells_once_and_shares_out_the_whole_listing() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR, FLAG_LINK};

    let mut builder = DirBuilder::new(r"C:\here");
    builder.push("a.txt", 250, 0, 0);
    builder.push("big", 4096, 0, FLAG_DIR);
    builder.push("empty", 4096, 0, FLAG_DIR);
    builder.push("junction", 0, 0, FLAG_DIR | FLAG_LINK);
    let mut tab = Tab::new(r"C:\here");
    tab.apply(Arc::new(builder.finish(0)));
    let (a, big, empty, junction) = (
        entry_of(&tab, "a.txt"),
        entry_of(&tab, "big"),
        entry_of(&tab, "empty"),
        entry_of(&tab, "junction"),
    );

    // ---- Off: the column says what it has always said -------------------
    assert_eq!(tab.size_shown(a), Some(250), "a file has its own bytes");
    assert_eq!(
        tab.size_shown(big),
        None,
        "a folder's own byte count is noise and must not reach the cell"
    );
    assert_eq!(tab.sizes.share(250), None, "no bars while the button is off");
    assert!(walked(&mut tab).is_empty(), "nothing was asked for");

    // ---- On: two folders asked about, and two rows that are not ----------
    tab.set_sizes(true);
    let asked = walked(&mut tab);
    let rows: Vec<u32> = asked.iter().map(|&(row, _)| row).collect();
    assert_eq!(
        rows,
        [big as u32, empty as u32],
        "the link should not have been asked about, and a file is not a folder"
    );
    assert_eq!(
        asked[0].1,
        PathBuf::from(r"C:\here\big"),
        "asked about by the path the row leads to"
    );
    assert_eq!(tab.sizes.waiting(), 2);
    assert!(
        walked(&mut tab).is_empty(),
        "a folder already asked about was asked again — the column would ask on every frame"
    );
    // A cell with a question outstanding is blank, exactly as it was with the button off: it is a
    // folder that has not answered, not a folder of nothing.
    assert_eq!(tab.size_shown(big), None);
    assert_eq!(
        tab.size_shown(junction),
        None,
        "a junction is not followed, so `0 B` would be a claim rather than a silence"
    );

    // ---- The totals land -------------------------------------------------
    assert!(answer(&mut tab, big as u32, 750));
    assert!(answer(&mut tab, empty as u32, 0));
    assert_eq!(tab.sizes.waiting(), 0, "nothing is outstanding now");
    tab.settle_sizes(1.0);

    assert_eq!(tab.size_shown(big), Some(750));
    assert_eq!(
        tab.size_shown(empty),
        Some(0),
        "a folder with nothing in it has been counted, and the answer is zero"
    );
    // The rows partition the folder, so their shares add to one — which is the property that makes
    // a bar in a cell readable as "this much of what is on show".
    assert_eq!(shares(&tab), Some(1.0));
    assert_eq!(
        tab.sizes.share(250),
        Some(0.25),
        "250 of the 1000 bytes on show: 250 in the file and 750 under `big`"
    );

    // ---- A filter narrows what the shares are of -------------------------
    tab.filter.push_str("big");
    tab.rebuild_order();
    tab.settle_sizes(2.0);
    assert_eq!(tab.order.len(), 1, "only `big` survives the filter");
    assert_eq!(
        tab.sizes.share(750),
        Some(1.0),
        "the total is of what is displayed, so the only row on show is all of it"
    );
}

/// **A Size sort is redone as the totals land — on a deadline — and then settles.**
///
/// Three failures in one test, because they are three sides of one line. Without the re-sort, a
/// listing ordered by Size shows the order it had when nothing was counted, for as long as the
/// counting takes — minutes, on a drive. Without the *deadline*, it rebuilds once per answer, which
/// because every answer wakes the window is once per frame: the same pass [`super::FILTER_DELAY`]
/// measures at 220–240 ms on a large listing. And without the round's end overriding the deadline,
/// what you are finally left looking at is a quarter-second stale.
#[test]
fn the_size_sort_follows_the_totals_on_a_deadline_and_then_settles() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR};

    let mut builder = DirBuilder::new(r"C:\here");
    builder.push("huge", 4096, 0, FLAG_DIR);
    builder.push("later", 4096, 0, FLAG_DIR);
    builder.push("z.bin", 500, 0, 0);
    let mut tab = Tab::new(r"C:\here");
    tab.sort_by = Column::Size;
    tab.ascending = false;
    tab.apply(Arc::new(builder.finish(0)));

    tab.set_sizes(true);
    assert_eq!(walked(&mut tab).len(), 2, "both folders were asked about");
    // By name, not by where they came back: at this point the order is still the one `apply` built
    // with the button off, which is folders-first in *descending name* order — so `asked[0]` is
    // `later`, and binding positionally would answer for the wrong folder and still look plausible.
    let (huge, later) = (
        entry_of(&tab, "huge") as u32,
        entry_of(&tab, "later") as u32,
    );
    // Turning the button on re-sorts at once — the listing was ordered without the folders in it and
    // now they are in it — and an uncounted folder keys as zero, so both wait at the quiet end. They
    // tie there, and the name tie-break follows the sort's own direction like every other one here.
    assert_eq!(tab.settle_sizes(0.0), None, "the press is due at once");
    assert_eq!(names(&tab), ["z.bin", "later", "huge"]);

    // ---- One answer, with another still outstanding: owed, not due -------
    answer(&mut tab, huge, 9_000);
    let left = tab.settle_sizes(0.0);
    assert!(
        matches!(left, Some(left) if left > 0.0),
        "a re-sort was taken immediately, so every answer would rebuild the order: {left:?}"
    );
    assert_eq!(
        names(&tab),
        ["z.bin", "later", "huge"],
        "the order moved before the deadline"
    );
    // The bars, though, are right on this frame: the total settles every time even while the order
    // waits, or the listing would be drawn against a denominator it no longer sums to.
    assert_eq!(shares(&tab), Some(1.0));

    // ---- The deadline passes ---------------------------------------------
    assert_eq!(tab.settle_sizes(crate::sizes::RESORT_DELAY + 0.01), None);
    assert_eq!(
        names(&tab),
        ["huge", "z.bin", "later"],
        "the sort did not follow the total once the deadline had passed"
    );
    assert_eq!(tab.sizes.waiting(), 1, "`later` has still not answered");

    // ---- The last answer overrides the deadline --------------------------
    answer(&mut tab, later, 20_000);
    assert_eq!(tab.sizes.waiting(), 0, "the round is over");
    assert_eq!(
        tab.settle_sizes(crate::sizes::RESORT_DELAY + 0.02),
        None,
        "the last answer of a round is due at once, whatever the deadline says"
    );
    assert_eq!(names(&tab), ["later", "huge", "z.bin"]);

    // ---- And it stops ----------------------------------------------------
    //
    // The re-sort rebuilds the order, which invalidates the total, which is what the same call then
    // pays. Owing itself another one would mean rebuilding the order on every frame for ever.
    assert_eq!(tab.settle_sizes(9.0), None);
    let before = tab.order_gen;
    assert_eq!(tab.settle_sizes(9.0), None);
    assert_eq!(
        tab.order_gen, before,
        "settling twice rebuilt the order twice, so it would rebuild on every frame"
    );
    assert_eq!(shares(&tab), Some(1.0));
}

/// A rebuilt order does **not** owe a re-sort, so a settled keystroke is not paid for twice.
///
/// `rows_moved` invalidates the total and deliberately nothing else. Arming the re-sort there instead
/// would have every filter keystroke, `Ctrl+H` and column click rebuild the listing, then rebuild it
/// identically one frame later — for an order that cannot have changed, since no total moved in
/// between. On a large listing that is a second 240 ms stall per interaction, for nothing.
#[test]
fn a_rebuilt_order_does_not_owe_itself_a_second_rebuild() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR};

    let mut builder = DirBuilder::new(r"C:\here");
    builder.push("one", 4096, 0, FLAG_DIR);
    builder.push("z.bin", 500, 0, 0);
    let mut tab = Tab::new(r"C:\here");
    tab.sort_by = Column::Size;
    tab.apply(Arc::new(builder.finish(0)));
    tab.set_sizes(true);
    let asked = walked(&mut tab);
    answer(&mut tab, asked[0].0, 900);
    tab.settle_sizes(0.0);
    tab.settle_sizes(crate::sizes::RESORT_DELAY + 0.01);

    // A user's own rebuild — a column click, say — and then a frame.
    tab.rebuild_order();
    let after_theirs = tab.order_gen;
    assert_eq!(tab.settle_sizes(9.0), None, "nothing is owed but the total");
    assert_eq!(
        tab.order_gen, after_theirs,
        "the order was rebuilt again on the next frame, doubling the cost of the click"
    );
    assert_eq!(shares(&tab), Some(1.0), "and the total was still settled");
}

/// A re-read of the same folder throws the totals away, and an answer to the old question cannot
/// land on the new listing.
///
/// **This is the one way this feature could put a real number on the wrong file.** A total names a
/// *row*, and a row is an index into the listing that was on screen when it was asked for — so after
/// an `F5`, a file operation, or [`crate::watch`] noticing a write, entry 40 is very likely a
/// different file. The generation is what makes that impossible, and it is deliberately not
/// [`Tab::view`]: a view survives a re-read of its own folder, which is exactly the case that has to
/// be told apart.
#[test]
fn a_re_read_will_not_take_an_answer_meant_for_the_listing_it_replaced() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR};

    let listing = |names: &[&str]| {
        let mut builder = DirBuilder::new(r"C:\here");
        for name in names {
            builder.push(name, 0, 0, FLAG_DIR);
        }
        Arc::new(builder.finish(0))
    };

    let mut tab = Tab::new(r"C:\here");
    tab.apply(listing(&["one", "two"]));
    tab.set_sizes(true);
    let asked = walked(&mut tab);
    assert_eq!(asked.len(), 2);
    let stale = crate::sizes::Answer {
        gen: tab.sizes.gen(),
        row: asked[0].0,
        bytes: 4_000,
    };

    // The folder is read again, with a row inserted at the front of it — so every index has moved.
    tab.apply(listing(&["aaa", "one", "two"]));
    assert!(
        !tab.sizes.take(&stale, 0.0),
        "an answer to the listing that was replaced was taken for this one"
    );
    assert_eq!(tab.sizes.waiting(), 0, "and so was what it was waiting for");
    assert_eq!(
        tab.size_shown(entry_of(&tab, "one")),
        None,
        "a total from the previous listing survived the read that replaced it"
    );
    // The view is *not* what tells the two listings apart, which is why the generation exists.
    let view = tab.view;
    tab.apply(listing(&["aaa", "one", "two"]));
    assert_eq!(
        tab.view, view,
        "a re-read is the same view of the same folder"
    );
}

/// Pressing the button twice keeps what was already counted, and puts back what was outstanding.
///
/// Turning it off is a cancellation — the generation stops being reported and the walks are
/// abandoned — so the folders that were waiting are never going to be answered. Left marked as asked
/// they would stay blank for ever. The answers that did land are still true of the same listing and
/// are kept, which is what makes the second press cheap.
#[test]
fn turning_the_measurement_off_and_on_keeps_the_answers_and_re_asks_the_rest() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR};

    let mut builder = DirBuilder::new(r"C:\here");
    builder.push("done", 0, 0, FLAG_DIR);
    builder.push("waiting", 0, 0, FLAG_DIR);
    let mut tab = Tab::new(r"C:\here");
    tab.apply(Arc::new(builder.finish(0)));
    tab.set_sizes(true);

    let asked = walked(&mut tab);
    assert_eq!(asked.len(), 2);
    let done = asked[0].0;
    answer(&mut tab, done, 512);
    assert_eq!(tab.sizes.waiting(), 1, "one is still outstanding");

    tab.set_sizes(false);
    let off = tab.sizes.gen();
    assert_eq!(
        tab.size_shown(done as usize),
        None,
        "the column stops showing folder sizes with the button off"
    );

    tab.set_sizes(true);
    assert_ne!(
        tab.sizes.gen(),
        off,
        "the same generation would let a straggler from the last press land"
    );
    assert_eq!(
        tab.size_shown(done as usize),
        Some(512),
        "an answer that landed is still true of this listing and should not be thrown away"
    );
    let again = walked(&mut tab);
    assert_eq!(
        again.len(),
        1,
        "the folder left waiting was not asked again, so its cell would stay blank for ever"
    );
    assert_ne!(again[0].0, done, "and the one already answered was re-asked");
}

/// A flattened listing counts itself, with no disk at all, and its shares are of the whole tree.
///
/// Two things at once, and the second is why [`Tab::settle_sizes`] has two branches. A folder's own
/// listing partitions itself; a flattened one does not — a folder and the files inside it are both
/// rows — so summing the rows there counts every byte once per level and every bar would be a
/// fraction of a number that means nothing.
///
/// It goes through [`Tab::wanted_sizes`] like every other listing, which is the point of that method
/// existing: this used to reimplement `App`'s flat branch by hand, so the timing half of it was
/// untested and the tab it left behind would have re-counted on a real frame.
#[test]
fn a_flattened_listing_counts_itself_against_the_whole_tree() {
    use crate::fs::dir::{DirBuilder, FLAG_DIR};

    let mut builder = DirBuilder::new(r"C:\x");
    builder.push("top.txt", 250, 0, 0);
    builder.push("sub", 0, 0, FLAG_DIR);
    builder.push(r"sub\deep.bin", 750, 0, 0);
    let dir = Arc::new(builder.finish(0));
    assert_eq!(dir.total_size, 1000, "every file under the folder, once");

    let mut tab = Tab::new(r"C:\x");
    tab.flat = true;
    tab.apply(dir);
    tab.set_sizes(true);
    // No disk at all, and no walk to wait for: the answer is the listing.
    assert!(matches!(tab.wanted_sizes(), crate::sizes::Work::Done));
    assert_eq!(tab.sizes.waiting(), 0, "nothing was handed to the service");
    assert!(
        matches!(tab.wanted_sizes(), crate::sizes::Work::None),
        "a flattened listing was counted twice"
    );
    assert!(
        tab.sizes.micros() > 0,
        "the flat pass did not time itself, so the status line has nothing to report"
    );
    tab.settle_sizes(1.0);

    assert_eq!(tab.size_shown(entry_of(&tab, "sub")), Some(750));
    // The nested file and the folder holding it are both three quarters of the tree, which is the
    // right answer for both — and only possible because the denominator is the tree rather than the
    // sum of rows that overlap.
    assert_eq!(tab.sizes.share(750), Some(0.75));
    assert_eq!(
        tab.sizes.share(1000),
        Some(1.0),
        "and the tree's own total is all of it"
    );
}

// ---- What the measurement tests say in one line each ----------------------
//
// `Measurement` owns its state and hands nothing out, so these four are how a test says "ask about
// the folders on show" and "one answered" without naming a field.

/// Which entry a name is at, in the listing the tab is showing.
fn entry_of(tab: &Tab, want: &str) -> usize {
    let dir = tab.dir.as_ref().expect("a listing");
    (0..dir.len())
        .find(|&i| dir.name(i) == want)
        .expect("pushed above")
}

/// The rows the measurement wants walked, which is what `App::collect_sizes` hands to the service.
fn walked(tab: &mut Tab) -> Vec<(u32, PathBuf)> {
    match tab.wanted_sizes() {
        crate::sizes::Work::Walk(folders) => folders,
        crate::sizes::Work::Done | crate::sizes::Work::None => Vec::new(),
    }
}

/// One folder answering, the way the service's answer arrives.
fn answer(tab: &mut Tab, row: u32, bytes: u64) -> bool {
    let gen = tab.sizes.gen();
    tab.sizes
        .take(&crate::sizes::Answer { gen, row, bytes }, 0.0)
}

/// The rows on show, in order.
fn names(tab: &Tab) -> Vec<String> {
    let dir = tab.dir.as_ref().expect("a listing");
    tab.order
        .iter()
        .map(|&i| dir.name(i as usize).to_owned())
        .collect()
}

/// Every bar the listing draws, added up — which for rows that partition the folder has to be one.
///
/// Rows with no number draw no bar and are not in the total either, so they are skipped rather than
/// failing the sum: a junction is the case, and `0 B` for one would be the wrong answer twice over.
/// Rounded, because these are `f32` shares of a `u64`.
fn shares(tab: &Tab) -> Option<f32> {
    let mut total = 0.0;
    for &row in &tab.order {
        let Some(bytes) = tab.size_shown(row as usize) else {
            continue;
        };
        total += tab.sizes.share(bytes)? as f64;
    }
    Some(((total * 1e5).round() / 1e5) as f32)
}
