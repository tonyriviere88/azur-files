use super::*;
use std::time::{Duration, Instant};

/// A file that carries an icon of its own, on any Windows.
fn notepad() -> PathBuf {
    PathBuf::from(r"C:\Windows\System32
otepad.exe")
}

#[test]
fn a_type_icon_costs_no_io() {
    crate::shell::init();
    // `SHGFI_USEFILEATTRIBUTES` means the shell answers from the name alone, so a
    // path that does not exist still resolves — which is the whole point, and the
    // reason a folder of 60,000 files does not pay per file.
    let index = index_of(Path::new(r"C:\nothing-here.txt"), false, true);
    assert!(
        index.is_some(),
        "a type icon has to resolve without the file existing"
    );

    let started = Instant::now();
    for _ in 0..200 {
        let _ = index_of(Path::new(r"C:\nothing-here.txt"), false, true);
    }
    let each = started.elapsed() / 200;
    assert!(
        each < Duration::from_micros(500),
        "{each:?} per type lookup -- this is meant to be the cheap path"
    );
}

#[test]
fn a_file_with_its_own_icon_gets_its_own_index() {
    crate::shell::init();
    let notepad = notepad();
    if !notepad.exists() {
        return;
    }
    let own = index_of(&notepad, false, false).expect("notepad has an icon");
    let generic = index_of(Path::new(r"C:\anything.exe"), false, true)
        .expect("the generic application icon");
    assert_ne!(
        own, generic,
        "an executable carries its own icon, which is why it is worth a per-file \
         lookup at all"
    );
}

#[test]
fn an_icon_index_yields_a_visible_bitmap() {
    crate::shell::init();
    let index = index_of(Path::new(r"C:\folder"), true, true).expect("a folder icon");
    let image = bitmap(index).expect("the system image list has a bitmap for it");
    assert_eq!(image.size, [SMALL, SMALL], "the small list is 16 square");

    let lit = image
        .pixels
        .iter()
        .filter(|p| p.a() > 0 && (p.r(), p.g(), p.b()) != (0, 0, 0))
        .count();
    assert!(
        lit > 20,
        "only {lit} pixels are both opaque and coloured -- the mask or the channel \
         order is wrong, and the icon would draw as a black square"
    );
    let clear = image.pixels.iter().filter(|p| p.a() == 0).count();
    assert!(
        clear > 20,
        "nothing is transparent, so the icon would draw as a filled block"
    );
}

/// Time every shell call an icon costs, against a folder given on the command line.
///
/// `YAFE_PROBE=H:\some\folder cargo test probe_icon_costs -- --ignored --nocapture`
///
/// Not a test of anything: a measurement, kept because the answer is entirely different on
/// a network share and guessing which of these calls is the slow one is how a whole
/// afternoon gets spent on the wrong one.
#[test]
#[ignore]
#[cfg(windows)]
fn probe_icon_costs() {
    let Some(folder) = std::env::var_os("YAFE_PROBE") else {
        eprintln!("set YAFE_PROBE to a folder");
        return;
    };
    crate::shell::init();
    let folder = std::path::PathBuf::from(folder);
    let mut entries: Vec<(std::path::PathBuf, bool)> = Vec::new();
    for entry in std::fs::read_dir(&folder).expect("readable").flatten() {
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        entries.push((entry.path(), is_dir));
    }
    eprintln!("{} entries in {}", entries.len(), folder.display());

    let timed = |label: &str, f: &mut dyn FnMut() -> Option<i32>| {
        let at = Instant::now();
        let got = f();
        (label.to_owned(), at.elapsed(), got)
    };

    // 1. The type lookup, which is what every ordinary row goes through.
    let mut rows: Vec<(String, Duration, Option<i32>)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (path, is_dir) in &entries {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !seen.insert((ext.clone(), *is_dir)) {
            continue;
        }
        let p = path.clone();
        rows.push(timed(
            &format!("kind .{ext}{}", if *is_dir { " (dir)" } else { "" }),
            &mut || index_of(&p, *is_dir, true),
        ));
    }

    // 2. The per-file lookup, for the extensions that carry their own icon.
    for (path, is_dir) in entries.iter().filter(|(_, d)| !d).take(12) {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !has_own_icon(&ext) {
            continue;
        }
        let p = path.clone();
        rows.push(timed(
            &format!("file {}", p.file_name().unwrap_or_default().to_string_lossy()),
            &mut || index_of(&p, *is_dir, false),
        ));
    }

    // 3. The place lookup, which is what the sidebar and every tab does.
    for path in std::iter::once(folder.clone())
        .chain(entries.iter().filter(|(_, d)| *d).map(|(p, _)| p.clone()).take(6))
    {
        let p = path.clone();
        rows.push(timed(
            &format!("place {}", p.display()),
            &mut || index_of_place(&p),
        ));
    }

    for (label, took, got) in &rows {
        eprintln!("{:>9.2?}  {label} -> {got:?}", took);
    }

    // 4. Pulling the bitmap out of the image list, which happens on the UI thread.
    let mut indices: Vec<i32> = rows.iter().filter_map(|(_, _, got)| *got).collect();
    indices.sort_unstable();
    indices.dedup();
    eprintln!("-- bitmaps, {} distinct indices --", indices.len());
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    for index in &indices {
        let at = Instant::now();
        let got = bitmap(*index);
        let took = at.elapsed();
        total += took;
        worst = worst.max(took);
        eprintln!("{:>9.2?}  bitmap {index} -> {}", took, got.is_some());
    }
    eprintln!("bitmaps: {total:.2?} total, {worst:.2?} worst");
}

/// A bitmap is fetched off the UI thread, and drawn once it arrives.
///
/// The shape of this test is the point. `uv` answers `None` first and something later,
/// because in between a worker thread did the only part of this that can block — pulling
/// the icon out of the shell's image list, which reaches the network for an icon that came
/// from a file on a share and was measured at **2.54 seconds** for one `.ico`. It used to
/// happen here, in the frame, and that is what froze the window.
#[test]
fn a_bitmap_is_fetched_off_the_ui_thread() {
    crate::shell::init();
    let ctx = egui::Context::default();
    let mut icons = Icons::new();

    // A real image-list index to ask about: the folder icon.
    let deadline = Instant::now() + Duration::from_secs(10);
    let icon = loop {
        icons.poll(&ctx);
        if let Some(icon) = icons.kind("", true) {
            break icon;
        }
        assert!(Instant::now() < deadline, "the folder icon never arrived");
        std::thread::sleep(Duration::from_millis(10));
    };

    // Asked here, not fetched here.
    assert!(
        icons.uv(&ctx, icon).is_none(),
        "the first ask must not go to the shell"
    );
    assert_eq!(icons.bitmaps, 1, "and it must not ask twice");
    assert!(icons.uv(&ctx, icon).is_none());
    assert_eq!(icons.bitmaps, 1);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        icons.poll(&ctx);
        if icons.uv(&ctx, icon).is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "the bitmap never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A question about a folder nobody is looking at any more is dropped, not answered.
#[test]
fn leaving_a_folder_cancels_its_questions() {
    crate::shell::init();
    let ctx = egui::Context::default();
    let mut icons = Icons::new();
    let exe = std::env::current_exe().expect("this test binary is an executable");

    // View 1 asks, and then goes away before the worker gets to it.
    icons.only(&[1]);
    assert!(icons.request_file(1, 0, exe.clone()));
    icons.only(&[2]);

    // Nothing addressed to view 1 comes back. Waited on rather than asserted at once,
    // because the point is that the worker *reached* the job and skipped it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        icons.poll(&ctx);
        assert!(
            icons.answers().is_empty(),
            "a question from a view that has gone should not be answered"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // And a live view still gets its answer, so the skip is about the view and not about
    // the worker having stopped.
    assert!(icons.request_file(2, 0, exe));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = Vec::new();
    while Instant::now() < deadline && got.is_empty() {
        icons.poll(&ctx);
        got = icons.answers();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        got.iter().any(|(view, _, _)| *view == 2),
        "the live view's question should be answered: {got:?}"
    );
}

#[test]
fn the_cache_asks_once_per_type() {
    crate::shell::init();
    let mut icons = Icons::new();

    // First sighting: nothing yet, and a request queued.
    assert!(icons.kind("", true).is_none());

    // Give the worker a moment, then it is cached and every later ask is free.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        icons.poll(&egui::Context::default());
        if icons.kind("", true).is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        icons.kind("", true).is_some(),
        "the folder icon never arrived"
    );
    let held = icons.len();
    for _ in 0..100 {
        let _ = icons.kind("", true);
    }
    assert_eq!(icons.len(), held, "a cached type is not looked up again");
}
