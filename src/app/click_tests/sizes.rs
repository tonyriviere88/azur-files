//! Measuring the folders on show: the button on the status line, and what lands in the Size column.
//!
//! Driven through the real button and real frames rather than through `perform`, because the two
//! things most likely to be wrong are the ones only that can see: a button nothing can reach, and a
//! cell that is filled in the model and painted nowhere.

use super::*;
use crate::ui::filelist::STATUS_HEIGHT;

/// Run frames until nothing is being counted any more.
///
/// Not a fixed number: the totals come back from worker threads that are walking real directories,
/// and a test process sharing a machine with seven other test threads is not a clock. So it waits
/// for the thing it is waiting for, exactly as [`Harness::settle`] does for a listing.
fn counted(h: &mut Harness) {
    for attempt in 0..400 {
        h.frame(Vec::new());
        if attempt >= 2 && h.app.panes[0].tab().sizes.waiting() == 0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!(
        "the folders were still being counted after 400 frames: {} outstanding",
        h.app.panes[0].tab().sizes.waiting()
    );
}

/// The button, the numbers it puts in the Size column, and the bars under them.
///
/// `src` rather than the crate root, because the root has `target` in it: a few hundred thousand
/// build artefacts would be measured correctly and prove nothing this does not.
#[test]
fn measuring_fills_the_folders_size_cells_and_bars_them() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: sources,
        },
    );
    h.settle();

    let rect_of = |h: &Harness, id: Id| {
        h.ctx
            .read_response(id)
            .map(|r| r.rect)
            .unwrap_or_else(|| panic!("{id:?} was not laid out"))
    };

    // ---- Where the button is -------------------------------------------
    //
    // On the status line, at the far end of the pane's group of switches: the console's first, then
    // the view, then this. That order is the order `Ctrl+²`, `Ctrl+1` and `Ctrl+2` are printed in on
    // the number row, so the run of buttons and the run of keys read the same way — and this one is
    // last because `Ctrl+2` is the last of the three keys. Asserted by geometry rather than by reading
    // the source, because the order of three rects laid out from one running `x` is exactly the thing
    // a source change can get wrong silently.
    let button = rect_of(&h, Id::new(("sizes", pane)));
    let view = rect_of(&h, Id::new(("view-switch", pane)));
    let console = rect_of(&h, Id::new(("console-switch", pane)));
    assert!(
        console.right() <= view.left() && view.right() <= button.left(),
        "the switches are not in their keys' order: {console:?}, {view:?}, {button:?}"
    );
    assert!(
        (button.center().y - view.center().y).abs() < 0.5
            && (button.height() - view.height()).abs() < 0.5,
        "it is not on the same line, or not the same size, as the switch beside it"
    );
    // And it is on the status line rather than in the listing above it.
    let floor = h.pane_rect(0);
    assert!(
        button.bottom() <= floor.bottom() + 0.5 && button.top() >= floor.bottom() - STATUS_HEIGHT,
        "the measure button is not on the status band: {button:?} in {floor:?}"
    );
    // Nothing of it is left on the path bar, which is where it used to be.
    assert!(
        button.top() > rect_of(&h, Id::new(("flatten", pane))).bottom(),
        "the measure button is still up on the path bar"
    );

    // ---- Nothing before it is pressed ------------------------------------
    //
    // A folder's Size cell is blank, which is what it has always been: a directory's own byte count
    // is noise. Counted off the painted shapes rather than off the model, because "the model says
    // so" is the half of this that reading the source already proves.
    let track = h.app.theme.gauge_track;
    // Inside the pane, because the sidebar has bars of its own: a drive's capacity gauge is the
    // same bar in the same `stroke-subtle`, which is the point — one window, one way of saying
    // "this much of that", and one constant for its height.
    let listing = h.pane_rect(0);
    let bars = move |h: &Harness| -> usize {
        h.rects()
            .into_iter()
            .filter(|(rect, _, fill)| {
                *fill == track
                    && (rect.height() - crate::ui::sidebar::GAUGE_HEIGHT).abs() < 0.01
                    && listing.contains(rect.center())
            })
            .count()
    };
    assert_eq!(bars(&h), 0, "there are bars in the Size column already");

    // ---- Pressing it ----------------------------------------------------
    let done = h.click_at(button.center());
    assert!(
        done.contains(&"ToggleSizes"),
        "the button did nothing, got {done:?}"
    );
    assert!(h.app.panes[0].tab().sizes.on, "the measurement is not on");
    counted(&mut h);

    // Every folder on show has a total now, and `src` has no empty folders in it — so every one of
    // them is a real number rather than the zero an empty folder would honestly report.
    let tab = h.app.panes[0].tab();
    let dir = tab.dir.clone().expect("a listing");
    let mut folders = 0;
    let mut biggest = 0u64;
    for &row in &tab.order {
        let entry = row as usize;
        if !dir.entries[entry].is_dir() {
            continue;
        }
        folders += 1;
        let bytes = tab
            .size_shown(entry)
            .unwrap_or_else(|| panic!("`{}` has no total", dir.name(entry)));
        assert!(bytes > 0, "`{}` came back empty", dir.name(entry));
        biggest = biggest.max(bytes);
    }
    assert!(folders > 3, "the fixture has no subfolders to measure");
    // Every row's bar is a share of the same total, and that total is the sum of the rows on show —
        // asked through `share`, since the figure itself is the measurement's to keep.
    let shares: f64 = tab
        .order
        .iter()
        .filter_map(|&row| tab.size_shown(row as usize))
        .filter_map(|bytes| tab.sizes.share(bytes))
        .map(f64::from)
        .sum();
    assert!(
        (shares - 1.0).abs() < 1e-5,
        "the bars come to {shares} of the listing, not 1 — the total is not what the rows add up to"
    );

    // ---- What is on screen ----------------------------------------------
    //
    // The biggest folder's figure is painted, and there is a bar per row that has a size. Both are
    // asked of the frame rather than of the tab: a number the model knows and the column does not
    // draw is the whole failure this file exists to catch.
    let mut figure = String::new();
    crate::fs::fmt::size(biggest, &mut figure);
    assert!(
        h.texts().iter().any(|(_, text)| *text == figure),
        "the largest folder's total, `{figure}`, was not painted anywhere"
    );
    let drawn = bars(&h);
    assert!(
        drawn >= folders,
        "only {drawn} bars for {folders} measured folders and every file besides"
    );

    // ---- And what it cost, on the status line ----------------------------
    //
    // Read off the painted text rather than off the tab, because the figure existing and the figure
    // being drawn are different claims — and it is the second one that was asked for. See
    // `filelist::status_line` for why the duration is on that bar at all.
    let tab = h.app.panes[0].tab();
    assert!(
        tab.sizes.micros() > 0,
        "the measurement finished without timing itself"
    );
    let figure = format!("counted in {}", crate::ui::filelist::took(tab.sizes.micros()));
    assert!(
        h.texts().iter().any(|(_, text)| *text == figure),
        "`{figure}` is not on the status line; it has {:?}",
        h.texts()
            .iter()
            .filter(|(at, _)| at.y > h.pane_rect(0).bottom() - STATUS_HEIGHT)
            .map(|(_, text)| text.clone())
            .collect::<Vec<_>>()
    );

    // ---- And off again ---------------------------------------------------
    //
    // The bars go, and so do the folders' figures. The files keep theirs, because a file's size was
    // never part of this.
    let done = h.click_at(button.center());
    assert!(done.contains(&"ToggleSizes"), "the button would not turn off");
    h.frame(Vec::new());
    assert!(!h.app.panes[0].tab().sizes.on);
    assert_eq!(bars(&h), 0, "the bars outlived the button");
    let tab = h.app.panes[0].tab();
    let folder = tab
        .order
        .iter()
        .map(|&row| row as usize)
        .find(|&entry| dir.entries[entry].is_dir())
        .expect("the fixture has subfolders");
    assert_eq!(
        tab.size_shown(folder),
        None,
        "a folder's cell should be blank again"
    );
    assert!(
        tab.order
            .iter()
            .map(|&row| row as usize)
            .filter(|&entry| !dir.entries[entry].is_dir())
            .all(|entry| tab.size_shown(entry).is_some()),
        "turning the measurement off took the files' own sizes with it"
    );
}

/// A navigation keeps the button on and counts the folder it arrives in.
///
/// **The one view setting that follows you**, and deliberately: the whole gesture is to find the
/// folder that is taking the space, open it, and ask the same question of what is inside — so a mode
/// that switched itself off at every step would have to be pressed once per level. The flatten does
/// the opposite, for its own reasons; see [`crate::pane::Tab::sizes`].
///
/// What must *not* follow is the numbers, which are indices into a listing that has gone.
#[test]
fn the_measurement_follows_a_navigation_and_its_numbers_do_not() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let sources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: sources.clone(),
        },
    );
    h.settle();
    let button = h
        .ctx
        .read_response(Id::new(("sizes", pane)))
        .map(|r| r.rect)
        .expect("the measure button was not laid out");
    h.click_at(button.center());
    counted(&mut h);
    let was = h.app.panes[0].tab().sizes.gen();

    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: sources.join("ui"),
        },
    );
    h.settle();
    let tab = h.app.panes[0].tab();
    assert!(tab.sizes.on, "the measurement did not come along");
    assert_ne!(
        tab.sizes.gen(), was,
        "the same generation would let a total meant for the folder we left land on this one"
    );

    counted(&mut h);
    let tab = h.app.panes[0].tab();
    let dir = tab.dir.clone().expect("a listing");
    assert!(
        tab.order
            .iter()
            .map(|&row| row as usize)
            .filter(|&entry| dir.entries[entry].is_dir())
            .all(|entry| tab.size_shown(entry).is_some()),
        "the folder we arrived in was not counted"
    );
}

/// On This PC the button is drawn disabled, and a click on it does nothing.
///
/// See `filelist::status_line` for why there is nothing there to count. Checked through the pointer
/// rather than by reading, because "disabled" in this window means `Sense::hover`, and a rect that
/// still senses clicks reads perfectly correctly in the source.
#[test]
fn there_is_nothing_to_measure_on_this_pc() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    h.app.perform(
        &h.ctx.clone(),
        Action::Navigate {
            pane,
            path: PathBuf::new(),
        },
    );
    h.settle();
    let button = h
        .ctx
        .read_response(Id::new(("sizes", pane)))
        .map(|r| r.rect)
        .expect("the measure button was not laid out");

    let done = h.click_at(button.center());
    assert!(
        !done.contains(&"ToggleSizes"),
        "the button answered on This PC, got {done:?}"
    );
    assert!(!h.app.panes[0].tab().sizes.on, "This PC is being measured");
}
