//! Does browsing let go of what it read?
//!
//! An LRU filling up to its budget and something that never lets go look identical from outside
//! the process. `crate::counting` makes the difference a number, and these are the tests that
//! read it — so a cache is allowed to grow and a leak is not.

use super::*;

/// Live heap bytes: every Rust allocation, minus every free. See [`crate::counting`].
fn live_heap() -> isize {
    crate::counting::LIVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Directories under `from`, breadth-first, up to `want` of them.
///
/// Real folders with real contents: a synthetic tree of empty directories would exercise
/// none of the per-entry storage this is about.
fn folders(from: &Path, want: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut queue = std::collections::VecDeque::from([from.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        if found.len() >= want {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                queue.push_back(entry.path());
                found.push(entry.path());
            }
        }
    }
    found
}

#[test]
#[ignore = "measures the whole process; run explicitly, single-threaded"]
fn scrolling_the_same_folder_up_and_down_costs_nothing() {
    // The reported case, exactly: one folder, scrolled up and down, the same rows drawn
    // over and over. Nothing about that is new work — every name, every icon and every
    // formatted size has been seen before — so anything that grows here grows *per frame*
    // rather than per folder, which is why leaving the folder would not give it back.
    let dir = PathBuf::from(r"C:\Windows");
    if !dir.is_dir() {
        println!("no C:\\Windows; skipping");
        return;
    }
    let mut h = Harness::new();
    h.app.panes[0].tab_mut().navigate(dir);
    h.settle();
    let rows = h.tab(0).order.len();
    assert!(rows > 40, "need a folder taller than the window");

    // One full pass first, so every icon, galley and column width is already resolved:
    // whatever the first sweep costs is work, not growth.
    let sweep = |h: &mut Harness| {
        for top in (0..rows).step_by(11) {
            h.app.panes[0].tab_mut().scroll_to = Some(top as f32 * crate::pane::ROW_HEIGHT);
            h.frame(Vec::new());
            h.app.icons.poll(&h.ctx.clone());
            h.app.deliver_icons();
        }
        for top in (0..rows).step_by(11).rev() {
            h.app.panes[0].tab_mut().scroll_to = Some(top as f32 * crate::pane::ROW_HEIGHT);
            h.frame(Vec::new());
        }
    };
    sweep(&mut h);
    h.settle();

    let heap0 = live_heap();
    let (private0, gdi0, _) = process_memory();
    println!("after one pass: heap {} KB, private {} KB, gdi {gdi0}", heap0 / 1024, private0 / 1024);

    for pass in 1..=10 {
        sweep(&mut h);
        let (private, gdi, _) = process_memory();
        println!(
            "pass {pass:>2}:  heap {:+7} KB   private {:+7} KB   gdi {gdi:>4}",
            (live_heap() - heap0) / 1024,
            (private as isize - private0 as isize) / 1024
        );
    }

    // Ten more passes over rows that were already drawn ten times must cost nothing.
    // A megabyte of slack for egui's own per-frame reuse.
    assert!(
        live_heap() - heap0 < (1 << 20),
        "scrolling the same folder grew the heap by {:+} KB -- something allocates per \
         frame and keeps it",
        (live_heap() - heap0) / 1024
    );
}

#[test]
#[ignore = "measures the whole process; run explicitly, single-threaded"]
fn what_a_very_large_folder_costs() {
    // A tab holds its listing, its display order and a selection flag per entry, and the
    // cache holds the listing again until it is evicted. This is what one folder of
    // 27,000 comes to, and it is the number to multiply by if a session keeps several
    // such folders open in tabs.
    let dir = PathBuf::from(r"C:\Windows\WinSxS");
    if !dir.is_dir() {
        println!("no WinSxS; skipping");
        return;
    }
    let mut h = Harness::new();
    h.settle();
    let (private0, _, _) = process_memory();
    let heap0 = live_heap();

    h.app.panes[0].tab_mut().navigate(dir);
    h.settle();
    let rows = h.tab(0).order.len();
    let (private, _, _) = process_memory();
    println!(
        "{rows} entries:  heap {:+} KB   private {:+} KB   = {} bytes an entry (heap)",
        (live_heap() - heap0) / 1024,
        (private as isize - private0 as isize) / 1024,
        (live_heap() - heap0) / rows.max(1) as isize
    );

    // Leave it, and see what comes back once neither the tab nor the cache holds it.
    h.app.panes[0].tab_mut().navigate(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    h.settle();
    h.app.loader.invalidate(Path::new(r"C:\Windows\WinSxS"));
    h.settle();
    println!(
        "after leaving and dropping it from the cache: heap {:+} KB",
        (live_heap() - heap0) / 1024
    );
}

#[test]
#[ignore = "measures the whole process; run explicitly, single-threaded"]
fn what_scrolling_a_folder_of_executables_costs() {
    // The worst case for icons, and the one a synthetic walk of small folders never
    // reaches: a folder of thousands of files that each carry their own icon. Every one
    // that comes on screen is a separate question to the shell, which opens the file and
    // reads its resources — so this is where "memory grows as I browse" would come from.
    let dir = PathBuf::from(r"C:\Windows\System32");
    if !dir.is_dir() {
        println!("no System32; skipping");
        return;
    }

    let mut h = Harness::new();
    h.app.panes[0].tab_mut().navigate(dir);
    h.settle();
    let rows = h.tab(0).order.len();
    println!("{rows} rows");

    let (private0, gdi0, _) = process_memory();
    let heap0 = live_heap();
    // Scroll the whole listing past, a screenful at a time, so every row is drawn once.
    let step = 20;
    for top in (0..rows).step_by(step) {
        h.app.panes[0].tab_mut().scroll_to = Some(top as f32 * crate::pane::ROW_HEIGHT);
        for _ in 0..3 {
            h.frame(Vec::new());
        }
        h.app.icons.poll(&h.ctx.clone());
        h.app.deliver_icons();
        if top % (step * 40) == 0 {
            let (private, gdi, _) = process_memory();
            let (kinds, paths, textures) = h.app.icons.held();
            println!(
                "row {top:>5}:  private {:+8} KB   heap {:+7} KB   gdi {gdi:>5}   \
                 icons {kinds:>4}/{paths:>5}/{textures:>4}",
                (private as isize - private0 as isize) / 1024,
                (live_heap() - heap0) / 1024
            );
            let _ = gdi0;
        }
    }
    let (private, gdi, _) = process_memory();
    let (kinds, paths, textures) = h.app.icons.held();
    println!(
        "after the whole listing: private {:+} KB   gdi {gdi}   icons {kinds}/{paths}/{textures}",
        (private as isize - private0 as isize) / 1024
    );
}

#[test]
#[ignore = "measures the whole process; run explicitly, single-threaded"]
fn browsing_hundreds_of_folders_settles_rather_than_grows() {
    // Somewhere else with `YAFE_WALK`, which is how the *ceiling* gets measured: the
    // caches are sized in entries, so a walk of this repository's small folders and a
    // walk of `C:\Windows` settle at very different heights.
    let root = std::env::var_os("YAFE_WALK")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    let dirs = folders(&root, 600);
    println!("walking {} ({} folders)", root.display(), dirs.len());
    assert!(
        dirs.len() > 120,
        "need a few hundred real folders to see a plateau, found {}",
        dirs.len()
    );

    let mut h = Harness::new();
    let browse = |h: &mut Harness, dir: &Path| {
        h.app.panes[0].tab_mut().navigate(dir.to_path_buf());
        h.settle();
    };

    // The first stretch is setup, not growth: the fonts, the glyph atlas, the loader's
    // threads and the shell's own caches are all paid for once.
    for dir in dirs.iter().take(20) {
        browse(&mut h, dir);
    }
    let (heap0, private0, gdi0, user0) = {
        let (p, g, u) = process_memory();
        (live_heap(), p, g, u)
    };
    println!(
        "baseline at 20 folders: heap {:>7} KB   private {:>7} KB   gdi {gdi0:>5}   user {user0:>4}",
        heap0 / 1024,
        private0 / 1024
    );

    let mut samples = Vec::new();
    for (i, dir) in dirs.iter().enumerate().skip(20) {
        browse(&mut h, dir);
        if (i + 1) % 50 == 0 {
            let heap = live_heap();
            let (private, gdi, user) = process_memory();
            let (dirs, entries) = h.app.loader.held();
            samples.push((heap, private as isize));
            println!(
                "{:>4} folders:  heap {:+8} KB   private {:+8} KB   gdi {gdi:>5}   \
                 user {user:>4}   cache {dirs:>3}/{entries:>7} = {:>4} B/entry",
                i + 1,
                (heap - heap0) / 1024,
                (private as isize - private0 as isize) / 1024,
                if entries > 0 {
                    (heap - heap0) / entries as isize
                } else {
                    0
                }
            );
        }
    }

    // The caches have budgets and are meant to reach them. What must not happen is the
    // second half of the walk costing as much as the first — that is the signature of
    // something that never lets go.
    let mid = samples.len() / 2;
    let (heap_mid, private_mid) = samples[mid];
    let (heap_end, private_end) = samples[samples.len() - 1];
    let folders_after = (samples.len() - mid) * 50;
    println!(
        "over the last {folders_after} folders: heap {:+} KB, private {:+} KB",
        (heap_end - heap_mid) / 1024,
        (private_end - private_mid) / 1024
    );

    // A megabyte of slack over hundreds of folders, which is arena reuse and allocator
    // fragmentation rather than anything held.
    let slack = 1 << 20;
    assert!(
        heap_end - heap_mid < slack,
        "the heap is still growing after the cache should have settled: {:+} KB over \
         {folders_after} folders",
        (heap_end - heap_mid) / 1024
    );
    assert!(
        private_end - private_mid < 4 * slack,
        "the process is still growing after the caches should have settled: {:+} KB \
         over {folders_after} folders -- something outside the Rust heap is being kept",
        (private_end - private_mid) / 1024
    );
}

#[test]
#[ignore = "measures the whole process; run explicitly, single-threaded"]
fn what_the_shell_menu_costs() {
    // Not an assertion — a measurement, and the answer to where the memory in this
    // process actually is. Opening a folder's context menu makes Windows load every
    // installed shell extension into *this* process: an archiver, a screenshot tool, a
    // cloud client, a rename tool, whatever else. Each is a DLL with its own heap, none
    // of them is ever unloaded, and none of it is visible to the Rust allocator counter.
    let mut h = Harness::new();
    h.settle();
    let (private_before, gdi_before, user_before) = process_memory();
    let heap_before = live_heap();
    println!(
        "before the menu: private {:>7} KB   heap {:>7} KB   gdi {gdi_before:>4}   \
         user {user_before:>4}",
        private_before / 1024,
        heap_before / 1024
    );

    // Five times over, because the answer that matters is whether it *repeats*: a DLL
    // loads once and stays, so a one-off cost of a few megabytes is very different from
    // a few megabytes every time somebody right-clicks.
    let mut last = private_before as isize;
    for round in 1..=5 {
        h.app.open_folder_menu(&h.ctx.clone());
        // The shell's entries come from a thread now, and it is exactly the extensions
        // this test is weighing that make it slow — so wait for them rather than
        // measuring a menu that never got any.
        let waited = std::time::Instant::now();
        while h.app.menu.is_none() {
            h.frame(Vec::new());
            if waited.elapsed() > std::time::Duration::from_secs(20) {
                panic!("the menu builder never answered");
            }
        }
        for _ in 0..20 {
            h.frame(Vec::new());
        }
        h.settle();
        h.app.close_menu();
        h.frame(Vec::new());

        let (private, gdi, user) = process_memory();
        println!(
            "menu {round}:  private {:>7} KB  ({:+} KB this time)   gdi {gdi:>4}   user {user:>4}",
            private / 1024,
            (private as isize - last) / 1024
        );
        last = private as isize;
    }

    let (private, gdi, user) = process_memory();
    let heap = live_heap();
    println!(
        "after the menus: private {:>7} KB   heap {:>7} KB   gdi {gdi:>4}   user {user:>4}",
        private / 1024,
        heap / 1024
    );
    println!(
        "the menu cost:   private {:+} KB   heap {:+} KB   gdi {:+}   user {:+}",
        (private as isize - private_before as isize) / 1024,
        (heap - heap_before) / 1024,
        gdi as i64 - gdi_before as i64,
        user as i64 - user_before as i64
    );
}

#[test]
#[ignore = "measures the whole process; run explicitly, single-threaded"]
fn closing_a_tab_gives_its_listing_back() {
    // The cache has a budget; a *tab* does not. Every open tab pins its own listing
    // through an `Arc`, which is why the cache's own figure understates what is held —
    // and it is the one shape of browsing that grows without a bound: a tab per folder.
    //
    // That much is by design. What would be a leak is a tab that is closed and does not
    // give the memory back, so this opens a pile of them, closes them all, and looks.
    let dirs = folders(Path::new(env!("CARGO_MANIFEST_DIR")), 40);
    assert!(dirs.len() >= 20, "need folders to open tabs on");

    let mut h = Harness::new();
    h.settle();
    let before = live_heap();

    let pane = h.app.panes[0].id;
    for dir in &dirs {
        h.app.perform(&h.ctx.clone(), Action::NavigateNewTab {
            pane,
            path: dir.clone(),
        });
        h.settle();
    }
    let tabs = h.app.panes[0].tabs.len();
    let open = live_heap();
    println!(
        "{tabs} tabs open: {:+} KB  ({} KB a tab)",
        (open - before) / 1024,
        (open - before) / 1024 / tabs as isize
    );

    // Close them from the back, leaving the one the window started with.
    while h.app.panes[0].tabs.len() > 1 {
        let last = h.app.panes[0].tabs.len() - 1;
        h.app
            .perform(&h.ctx.clone(), Action::CloseTab { pane, tab: last });
        h.settle();
    }
    // And empty the cache, which legitimately still holds what the tabs were showing.
    for dir in &dirs {
        h.app.loader.invalidate(dir);
    }
    h.settle();
    let closed = live_heap();
    println!(
        "after closing: {:+} KB on the baseline (was {:+} KB with {tabs} tabs open)",
        (closed - before) / 1024,
        (open - before) / 1024
    );

    // A tab's listing, its display order and its selection are the whole cost, and all
    // three go with it. Half a megabyte of slack for the cache's own bookkeeping.
    assert!(
        closed - before < (1 << 19),
        "closing every tab left {:+} KB behind -- something is holding listings after \
         their tab is gone",
        (closed - before) / 1024
    );
}

/// What starting up costs, phase by phase.
///
/// ```text
/// cargo test --release -- --ignored --nocapture startup_phases
/// ```
///
/// The window's own creation is eframe's and the graphics device is wgpu's, so neither is
/// reachable from here — what *is* reachable is everything this program does between being handed
/// a context and having a listing on screen, which is the part it can do something about. Four
/// phases, and they are separated because they fail differently: fonts are a fixed cost paid once,
/// [`App::opening`] is where the settings, the theme, the dock and the workers come from, the
/// first frame is layout with no listing in it, and the wait afterwards is the disk.
///
/// Run against this crate's own root, which is a folder of a few dozen entries — deliberately, so
/// the figure is the *fixed* cost of coming up rather than the cost of whatever folder was last
/// open. `what_a_very_large_folder_costs` is the other end of that.
#[test]
#[ignore = "a benchmark, not a test"]
fn startup_phases() {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Fonts, once per process: every run after the first would report zero, so it is measured
    // on its own context rather than inside the loop below.
    let fresh = egui::Context::default();
    let at = std::time::Instant::now();
    azur_egui_theme::fonts::install(&fresh);
    let fonts = at.elapsed();

    let mut opening = std::time::Duration::MAX;
    let mut first = std::time::Duration::MAX;
    let mut listing = std::time::Duration::MAX;
    let mut frames_waited = 0;
    let mut steady = std::time::Duration::MAX;

    for _ in 0..5 {
        let ctx = egui::Context::default();
        azur_egui_theme::fonts::install(&ctx);

        let at = std::time::Instant::now();
        let app = App::opening(&ctx, Config::default(), vec![here.clone()], Side::Right);
        opening = opening.min(at.elapsed());

        let mut h = Harness {
            app,
            ctx,
            size: vec2(1024.0, 650.0),
            time: 1.0,
            modifiers: Modifiers::NONE,
            focused: true,
            cursor: egui::CursorIcon::Default,
            commands: Vec::new(),
            shapes: Vec::new(),
        };

        let at = std::time::Instant::now();
        h.frame(Vec::new());
        first = first.min(at.elapsed());

        // Until the listing is on screen, which is when the window is worth looking at.
        let at = std::time::Instant::now();
        let mut waited = 1;
        for _ in 0..400 {
            if h.app.panes[0].tab().dir.is_some() {
                break;
            }
            h.frame(Vec::new());
            waited += 1;
        }
        listing = listing.min(at.elapsed());
        frames_waited = waited;

        // And a frame once everything has arrived, which is what every frame after startup costs.
        h.settle();
        let at = std::time::Instant::now();
        for _ in 0..20 {
            h.frame(Vec::new());
        }
        steady = steady.min(at.elapsed() / 20);
    }

    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    println!("  install the fonts            {:>8.2} ms  (once per process)", ms(fonts));
    println!("  App::opening                 {:>8.2} ms", ms(opening));
    println!("  the first frame              {:>8.2} ms", ms(first));
    println!("  until the listing is on it   {:>8.2} ms  ({frames_waited} frames)", ms(listing));
    println!("  a frame once settled         {:>8.2} ms", ms(steady));
    println!(
        "  ------------------------------------\n  to a listing on screen       {:>8.2} ms  (+ fonts, once)",
        ms(opening) + ms(first) + ms(listing)
    );
}

/// What a grid of thumbnails costs, and where the time goes.
///
/// ```text
/// cargo test --release -- --ignored --nocapture thumbnail_speed
/// ```
///
/// Point `YAFE_THUMBS` at a folder of pictures; without it this builds one in the sandbox, which
/// measures the *machinery* rather than the shell's decoder and is worth having either way — a
/// regression in the queue, the cache or the atlas shows up in it just as well.
///
/// Three numbers, because they are three different problems. **The shell call** is
/// `IShellItemImageFactory` per file and is the floor: nothing here can make it quicker, only ask
/// for it less. **The wait for a screenful** is what somebody switching to tiles actually sees.
/// **A frame while they arrive** is the one that must stay under a frame's budget, because the
/// alternative is a grid that stutters as it fills.
///
/// **Windows caches thumbnails, so read the first figure knowing which side of that cache it is
/// on.** Measured on files it had never been asked about, the shell's call came to **12.95 ms** a
/// picture; on the same files afterwards, **2.1 ms**. Both are real — the first is what somebody
/// opening a folder of holiday photographs waits for, the second is what they get on the way back
/// to it — and a run of this benchmark reports whichever applies to the sandbox as it stands. To
/// see the cold number again, point `YAFE_THUMBS` at a folder of pictures nothing has browsed.
#[test]
#[ignore = "a benchmark, not a test"]
fn thumbnail_speed() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let folder = match std::env::var_os("YAFE_THUMBS") {
        Some(given) => PathBuf::from(given),
        None => {
            // A folder of real PNGs in the sandbox, each a different size so no two thumbnails
            // are the same piece of work.
            let root = crate::sandbox::dir("thumbs-bench");
            for i in 0..96u32 {
                let path = root.join(format!("picture_{i:03}.png"));
                if path.exists() {
                    continue;
                }
                let side = 64 + (i % 8) * 32;
                let mut pixels = Vec::with_capacity((side * side * 3) as usize);
                for y in 0..side {
                    for x in 0..side {
                        pixels.push((x ^ y) as u8);
                        pixels.push((x.wrapping_add(i)) as u8);
                        pixels.push((y.wrapping_mul(3)) as u8);
                    }
                }
                let _ = image::save_buffer(
                    &path,
                    &pixels,
                    side,
                    side,
                    image::ExtendedColorType::Rgb8,
                );
            }
            root
        }
    };
    if !folder.is_dir() {
        println!("no {}; skipping", folder.display());
        return;
    }

    let pictures: Vec<PathBuf> = std::fs::read_dir(&folder)
        .expect("readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| crate::preview::kind_of("x", e, false) == Some(crate::preview::Kind::Picture))
        })
        .collect();
    println!("{} pictures in {}", pictures.len(), folder.display());
    if pictures.is_empty() {
        return;
    }

    // 1. The shell call itself, on the thread this test is on, one file at a time.
    let want = pictures.len().min(48);
    let at = std::time::Instant::now();
    let mut drawn = 0;
    for path in pictures.iter().take(want) {
        if crate::shell::thumbs::picture_for_tests(path) {
            drawn += 1;
        }
    }
    let each = at.elapsed().as_secs_f64() * 1000.0 / want as f64;
    println!("  the shell's own call         {each:>8.2} ms per picture  ({drawn}/{want} drawn)");

    // 2. And through the service, which is what the grid uses: a screenful asked for at once,
    //    fetched on workers, with a frame run while they land.
    //
    // **Warm**, and it has to be said out loud: the loop above has just asked the shell for these
    // same files, and the shell keeps a thumbnail cache of its own. So this figure is the
    // machinery — the queue, the cache, the atlas, the frame — over answers that come back
    // quickly, which is the right thing to watch for a regression and *not* what somebody
    // switching to tiles on a cold folder waits for. That number is the line above times the
    // number of pictures, divided by however many workers the fetch runs on.
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let ctx = h.ctx.clone();
    h.app.perform(&ctx, Action::Navigate { pane, path: folder.clone() });
    h.settle();
    h.app.show_tiles_here();

    let at = std::time::Instant::now();
    let mut frames = 0u32;
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..4000 {
        let frame_at = std::time::Instant::now();
        h.frame(Vec::new());
        worst = worst.max(frame_at.elapsed());
        frames += 1;
        if !h.app.thumbs_pending() && frames > 4 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    println!(
        "  a screenful through the grid  {:>7.1} ms  ({frames} frames, worst {:.2} ms, warm)",
        at.elapsed().as_secs_f64() * 1000.0,
        worst.as_secs_f64() * 1000.0
    );

    // 3. A frame with the grid full and nothing left to fetch, which is the steady state.
    let at = std::time::Instant::now();
    for _ in 0..30 {
        h.frame(Vec::new());
    }
    println!(
        "  a settled frame of tiles      {:>7.2} ms",
        at.elapsed().as_secs_f64() * 1000.0 / 30.0
    );
}
