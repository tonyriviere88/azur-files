use super::*;
use std::time::{Duration, Instant};

/// **One test at a time reaches the shell**, and this is not caution — it is a measurement.
///
/// Two `GetImage` calls in flight from one process answer *one of them* with a failure, whether or
/// not they are about the same file. Reproduced by letting the two tests below run in parallel, in
/// both orders and on both files: whichever pair overlapped, one of them came back with nothing.
///
/// It is a property of the tests rather than of the service, which has exactly one worker and
/// therefore never has two calls out. Saying so with a lock is honest; discovering it again as a
/// test that fails one run in three would not be. [`TRIES`] is what the *program* does about the
/// same fact when the other caller is Explorer.
///
/// Poisoning is stepped over deliberately: a failing test has a failure to report, and the second
/// one refusing to run because the first panicked would hide it.
static SHELL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    SHELL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The shell draws a picture for a file that is not a picture.
///
/// Which is the claim the grid rests on and the reason this module asks the shell rather than
/// decoding: one call has to cover both halves of a folder, or a tile of a `.dll` would be a
/// 16-point icon stretched to ninety-six.
///
/// Asked about this crate's own `Cargo.toml`, which is read and never written.
#[test]
fn the_shell_draws_a_picture_for_something_that_is_not_one() {
    let _one = one_at_a_time();
    crate::shell::init();
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let Got::Picture(image) = picture(&manifest) else {
        panic!("the shell has no picture for {}", manifest.display());
    };
    assert!(
        image.size[0] > 16 && image.size[1] > 16,
        "the picture came back {:?}, which is the small image list rather than a tile",
        image.size
    );
    assert!(image.size[0] <= CELL && image.size[1] <= CELL);
    let lit = image
        .pixels
        .iter()
        .filter(|p| p.a() > 0 && (p.r(), p.g(), p.b()) != (0, 0, 0))
        .count();
    assert!(
        lit > 64,
        "only {lit} pixels are both opaque and coloured -- the channel order or the alpha is \
         wrong, and the tile would draw as a black square"
    );
}

/// A picture is fetched off the UI thread, cached by path, and asked for once.
///
/// The shape is [`crate::shell::icons`]'s `a_bitmap_is_fetched_off_the_ui_thread` and the point
/// is the same: `get` answers `None` first and something later, because in between a worker did
/// the part that blocks. What is added here is the cache — the second ask for the same path must
/// not be a second shell call, or scrolling a folder of photographs would re-extract every one
/// of them on every frame.
#[test]
fn a_picture_is_fetched_once_and_off_the_ui_thread() {
    let _one = one_at_a_time();
    crate::shell::init();
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::new(&ctx);
    let exe = std::env::current_exe().expect("this test binary is an executable");
    thumbs.only(&[1]);

    assert!(
        thumbs.get(&exe, 7, 1).is_none(),
        "the first ask must not go to the shell"
    );
    assert_eq!(thumbs.fetched, 1);
    assert!(thumbs.get(&exe, 7, 1).is_none(), "and must not ask twice");
    assert_eq!(thumbs.fetched, 1, "a question in flight is not asked again");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        thumbs.poll();
        if thumbs.get(&exe, 7, 1).is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "the picture never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
    let asked = thumbs.fetched;
    for _ in 0..50 {
        let _ = thumbs.get(&exe, 7, 1);
    }
    assert_eq!(thumbs.fetched, asked, "a cached picture is not fetched again");

    // A different stamp is a different file as far as this is concerned, which is what makes an
    // edited picture come back rather than stay as it was.
    assert!(thumbs.get(&exe, 8, 1).is_none());
    assert_eq!(thumbs.fetched, asked + 1);
}

/// **A cell that is on screen is never taken from the tile drawing it.**
///
/// The bug this is the fix for, and it is worth stating as a sequence because reading the code
/// does not make it obvious. Show more tiles than the atlas has cells. Every visible tile is drawn
/// every frame, so every entry is equally recent — plain least-recently-used therefore picks one of
/// them, that tile loses its picture and goes back to its glyph, it asks again, its answer takes a
/// cell off another visible tile, and round it goes every frame for ever. On screen it is a wall of
/// thumbnails flickering as though each were being replaced by its neighbour.
///
/// So: with the atlas full of cells drawn in this frame there is nothing to claim and no room to ask
/// for more; let the frame turn over with one tile no longer drawing its cell — it scrolled off —
/// and *that* is the cell that gets taken, with every live one left alone.
///
/// One frame and not two, which is the other half of the rule: `poll` runs at the *end* of a frame,
/// so "nothing drew this cell" is a fact rather than a guess. The version that polled at the top
/// had to wait two quiet frames to be safe, and a window that paints on demand does not
/// necessarily *have* a second quiet frame — see [`Thumbs::poll`].
///
/// No shell in it. The entries are placed by hand, which is what lets this cover every cell in a
/// millisecond instead of needing that many files with thumbnails.
#[test]
fn a_visible_cell_is_never_taken_from_the_tile_drawing_it() {
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::new(&ctx);

    let named = |i: usize| PathBuf::from(format!("C:\\{i}.png"));
    for i in 0..SLOTS {
        let slot = thumbs.claim_slot().expect("an empty atlas has room for every cell");
        thumbs.known.insert(
            named(i),
            Held {
                slot: Some(slot),
                size: [CELL as u32, CELL as u32],
                stamp: 1,
                used: thumbs.frame,
                tries: 0,
                retry_at: None,
            },
        );
    }

    assert!(
        thumbs.claim_slot().is_none(),
        "a cell was taken while every one of them was being drawn"
    );
    assert_eq!(thumbs.room(), 0, "and there is no room to ask for another picture");

    // Nothing is asked for either, which is the other half: a surplus tile that asked anyway would
    // be a shell call per frame for a picture with nowhere to go.
    thumbs.spare = thumbs.room();
    let before = thumbs.fetched;
    assert!(thumbs.get(Path::new("C:\\new.png"), 1, 1).is_none());
    assert_eq!(
        thumbs.fetched, before,
        "a tile with nowhere to put its answer still went to the shell"
    );

    // The frame turns over, and one tile scrolls off — every other entry keeps being drawn.
    thumbs.frame += 1;
    let gone = named(7);
    let wanted = thumbs.known[&gone].slot;
    for (path, held) in thumbs.known.iter_mut() {
        if *path != gone {
            held.used = thumbs.frame;
        }
    }

    assert_eq!(thumbs.room(), 1, "exactly one cell is free to take");
    assert_eq!(
        thumbs.claim_slot(),
        wanted,
        "the cell taken was not the one nobody is drawing"
    );
    assert!(!thumbs.known.contains_key(&gone), "its entry has to go with it");
    assert_eq!(
        thumbs.known.len(),
        SLOTS - 1,
        "and nothing else was evicted along with it"
    );
    assert!(thumbs.claim_slot().is_none(), "there is nothing left to take");
}

/// **No cell is ever lost**, however many answers arrive for the same file.
///
/// This is the one that mattered, and the failure it guards is the reason the grid stopped filling
/// in at all. Two answers for one file — which the window between the worker releasing its claim and
/// `poll` writing the answer down used to allow on *every* answer — replaced the entry, and the cell
/// the old entry was holding became unreachable: referenced by nothing, not on the free list, gone.
/// Measured in the wild: **864 cells handed out, 71 entries left**, and a screenful of tiles that
/// could never be given a picture again because there was nowhere to put one.
///
/// So the invariant is asserted rather than the symptom: every cell is held by an entry, on the free
/// list, or never handed out, and that stays true across a picture, a duplicate picture, a refusal
/// over the top of a picture, and an eviction. Any of the four leaking is a slow death for the view
/// and none of them looks wrong at the call site.
#[test]
fn no_cell_is_ever_lost() {
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::new(&ctx);
    let path = PathBuf::from("C:\\one.png");
    let tiny = || ColorImage::filled([8, 8], egui::Color32::RED);

    let answer = |thumbs: &mut Thumbs, stamp: u64, got: Got| {
        thumbs
            .tx
            .send(Ready {
                path: path.clone(),
                stamp,
                got,
            })
            .expect("the channel is ours");
        thumbs.poll();
    };

    assert_eq!(thumbs.accounted(), SLOTS, "an empty atlas is all cells and no entries");

    // One picture: one cell held, none free, the rest never handed out.
    answer(&mut thumbs, 1, Got::Picture(tiny()));
    assert_eq!(thumbs.known.len(), 1);
    assert_eq!(thumbs.accounted(), SLOTS, "after one picture");

    // **A second answer for the same file**, which is the case that leaked. The cell the first was
    // holding has to come back, not vanish.
    answer(&mut thumbs, 1, Got::Picture(tiny()));
    assert_eq!(thumbs.known.len(), 1, "one file is one entry");
    assert_eq!(thumbs.accounted(), SLOTS, "a duplicate picture lost a cell");

    // Twenty more of them, because one leak per answer is what this was: 864 cells and a few
    // hundred answers is the whole of how the atlas came to be exhausted.
    for _ in 0..20 {
        answer(&mut thumbs, 1, Got::Picture(tiny()));
    }
    assert_eq!(thumbs.accounted(), SLOTS, "twenty duplicates lost {} cells", SLOTS - thumbs.accounted());
    assert!(
        thumbs.next_slot as usize <= 21,
        "each duplicate took a fresh cell instead of reusing the freed one: {} handed out",
        thumbs.next_slot
    );

    // A refusal landing over the top of a picture — a re-fetch of an edited file that then failed —
    // takes the entry's cell away, so that cell has to be freed too.
    answer(&mut thumbs, 2, Got::Later);
    assert!(thumbs.known[&path].slot.is_none());
    assert_eq!(thumbs.accounted(), SLOTS, "a refusal over a picture lost its cell");

    // And an eviction, which is the path that always worked — pinned so it stays that way.
    let mut thumbs = Thumbs::new(&ctx);
    for i in 0..SLOTS + 10 {
        let each = PathBuf::from(format!("C:\\{i}.png"));
        thumbs
            .tx
            .send(Ready {
                path: each,
                stamp: 1,
                got: Got::Picture(tiny()),
            })
            .expect("the channel is ours");
        // A frame per answer, so each one is placed with the last no longer being drawn.
        thumbs.poll();
    }
    assert_eq!(thumbs.accounted(), SLOTS, "eviction lost cells");
    assert_eq!(thumbs.next_slot as usize, SLOTS, "and the atlas did fill up");
}

/// **A shell that would not draw a file this time is asked again, later — and the waits grow.**
///
/// The failure this is the fix for is the one that *accumulates*, and it is worth stating as the
/// user's own recipe: scroll a grid of ten thousand pictures, switch the flatten mode, scroll,
/// switch, waiting a few seconds each time. Every cycle asks a few hundred files at once, a slice of
/// them fail while the shell is busy with the others — and cached as "this file has no picture" each
/// of those tiles is written off for the rest of the session. Round again and another slice goes.
/// What you end up looking at is a grid where the files asked *first* have pictures and everything
/// asked since is a wall of painted glyphs, which is exactly what the report showed.
///
/// So there is no such answer any more, and this holds the two halves of what replaced it. Failing
/// leaves an entry that is **due to be asked again**, not one that is finished; and the delay before
/// each attempt **grows**, because three attempts inside three frames is fifty milliseconds — all of
/// it inside the same busy window that caused the first failure, which is one attempt with extra
/// steps.
///
/// Driven through `poll` by hand rather than through the shell: the answer being tested is a
/// *failure*, and a test that needed the real shell to fail on demand would be a test of the shell.
#[test]
fn a_file_the_shell_would_not_draw_is_asked_again_and_the_waits_grow() {
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::new(&ctx);
    let path = PathBuf::from("C:\\busy.png");

    // Every attempt in `BACKOFF`, then the one past the end of it.
    let mut waits = Vec::new();
    for attempt in 1..=BACKOFF.len() + 1 {
        thumbs
            .tx
            .send(Ready {
                path: path.clone(),
                stamp: 1,
                got: Got::Later,
            })
            .expect("the channel is ours");
        thumbs.poll();
        let held = thumbs.known.get(&path).expect("a refusal is remembered");
        assert_eq!(held.tries as usize, attempt, "the count has to survive the round trip");
        assert!(held.slot.is_none());
        waits.push(
            held.retry_at
                .map(|at| at.saturating_duration_since(std::time::Instant::now())),
        );
    }

    // Growing, and then over. The comparison is on the *stored* deadlines rather than on `BACKOFF`
    // itself, so a table edited into the wrong order would fail here rather than ship.
    let due: Vec<std::time::Duration> = waits.iter().take(BACKOFF.len()).map(|w| {
        w.expect("an attempt was left with nothing to wait for")
    }).collect();
    for pair in due.windows(2) {
        assert!(
            pair[1] > pair[0],
            "the waits do not grow: {due:?} — three attempts in three frames is one attempt"
        );
    }
    assert!(
        due[0] >= std::time::Duration::from_millis(50),
        "the first wait is {:?}, which is inside the frame that just failed",
        due[0]
    );
    assert!(
        waits[BACKOFF.len()].is_none(),
        "the attempts never run out, so a file the shell truly cannot draw is asked about for ever"
    );

    // And once they have run out the tile stops asking: it draws its glyph and costs nothing.
    let spent = thumbs.fetched;
    for _ in 0..20 {
        assert!(thumbs.get(&path, 1, 1).is_none());
    }
    assert_eq!(thumbs.fetched, spent, "a file that has run out of attempts is still being asked");
}

/// Every slot is its own patch of its own page, and every page is used from its first cell.
///
/// Arithmetic, not the shell — and the arithmetic that would fail *quietly*: a `uv` that overlapped
/// its neighbour draws half of one picture on another tile, and a slot whose page index and whose
/// cell disagree draws the right patch of the wrong texture. Both look like a caching bug from the
/// outside.
#[test]
fn every_slot_is_its_own_patch_of_its_own_page() {
    let full = [CELL as u32, CELL as u32];
    assert_eq!(Thumbs::image_uv(0, full).min, egui::pos2(0.0, 0.0));

    for slot in 0..SLOTS as u32 {
        let uv = Thumbs::image_uv(slot, full);
        let within = slot as usize % PER_PAGE;
        let column = (within % PAGE_COLUMNS) as f32 / PAGE_COLUMNS as f32;
        let row = (within / PAGE_COLUMNS) as f32 / PAGE_ROWS as f32;
        assert!((uv.min.x - column).abs() < 1e-6, "slot {slot} is at {uv:?}");
        assert!((uv.min.y - row).abs() < 1e-6, "slot {slot} is at {uv:?}");
        assert!(
            uv.max.x <= 1.001 && uv.max.y <= 1.001,
            "slot {slot} leaves its page at {uv:?}"
        );
    }

    // The first slot of every page is that page's own top-left corner, which is the one thing a
    // single-atlas arithmetic left behind would get wrong: it would keep walking down one texture.
    for page in 0..MAX_PAGES {
        let uv = Thumbs::image_uv((page * PER_PAGE) as u32, full);
        assert_eq!(uv.min, egui::pos2(0.0, 0.0), "page {page} does not start at its corner");
    }

    // And a picture narrower than its cell is cropped to the pixels it filled, or a 16:9
    // photograph would be drawn with a strip of the transparent cell beside it.
    let wide = Thumbs::image_uv(0, [CELL as u32, 54]);
    assert!(wide.height() < wide.width(), "{wide:?} did not keep the aspect");
}
