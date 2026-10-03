//! Explorer's own thumbnails, for the tiles in [`crate::ui::grid`].
//!
//! # Why the shell and not a decoder
//!
//! This crate already has `image` and `resvg` in it, and the preview panel decodes with both — so
//! the obvious thing is to reuse [`crate::preview`] at a small size. That is the wrong answer here,
//! and for three reasons that are all about *what a folder actually holds*:
//!
//! 1. **The shell has already done it.** Windows keeps a thumbnail cache per user, populated by
//!    Explorer, and `IShellItemImageFactory::GetImage` reads it. A folder you have looked at in
//!    Explorer comes back instantly; one you have not is extracted once and cached for both
//!    programs.
//! 2. **It answers for everything, not just the formats linked in.** RAW files from a camera, HEIC
//!    from a phone, PSDs, PDFs, `.mp4` — every one of those has a thumbnail provider registered by
//!    whatever produced it, and none of them is a codec this program would be right to embed.
//! 3. **A file with no thumbnail still needs a picture**, and the same call gives it: the shell
//!    falls back to the file's icon at the size asked for, which is the *large* icon — a 96-point
//!    tile drawn from the 16-point image list behind the details view would be a blur. One call
//!    covers the picture and the icon, which is what makes the grid one code path.
//!
//! What it costs is a shell call **per file** rather than the per-*type* answer
//! [`crate::shell::icons`] is built around — see that module's header for why the distinction
//! matters so much there. It is affordable here for one reason: a tile is 96 points, so a screenful
//! is a hundred-odd of them rather than the forty *rows* × nothing that a listing is. Only cells
//! actually on screen are ever asked about.
//!
//! # A page at a time, and why it is not one texture
//!
//! Tiles are drawn from an atlas rather than from a texture each, for the reason
//! [`crate::shell::icons::Icons::uv`] gives at length: egui begins a new draw call whenever the
//! texture changes between primitives, and `egui_glow`'s painter leaks per draw call — so a texture
//! per thumbnail would break a frame's primitive stream at every tile.
//!
//! **The atlas is a stack of pages, grown one at a time.** One fixed texture was the first design and
//! it has a floor it cannot get under: the cache's size *is* the atlas's, so a window showing more
//! tiles than there are cells has tiles that can never hold a picture — and every one of those cells
//! is being drawn, so there is nothing to evict either. It shows up as a band of plain glyphs that
//! fills in only when you scroll, which is exactly what a fixed 324 cells did on a maximised 4K
//! panel.
//!
//! So capacity follows use instead of being guessed at. A page is [`PAGE_COLUMNS`] × [`PAGE_ROWS`]
//! cells of [`CELL`] pixels — [`PER_PAGE`] of them, 5.3 MB — and one is allocated only when a cell on
//! it is first claimed. A window standing still therefore pays for what it is *showing*:
//!
//! | window, maximised | tiles at once | pages | texture |
//! | --- | --- | --- | --- |
//! | 1920 × 1080 | 112 | 1 | 5.3 MB |
//! | 2560 × 1392 | 190 | 2 | 10.6 MB |
//! | 3440 × 1440 | 260 | 2 | 10.6 MB |
//! | 3840 × 2160 | 450 | 4 | 21.2 MB |
//!
//! Those are the floor rather than the ceiling, because **scrolling claims cells too**: go through a
//! folder of ten thousand pictures and all six pages fill, since every cell is holding a thumbnail
//! that scrolling back would want again. That is the cache doing its job, and it is where [`MAX_PAGES`]
//! comes in — measured with `--trace --scroll`, it stops at six and stays there. Past the last page the
//! surplus tiles keep their painted glyphs rather than taking a cell from a tile that is on screen; see
//! [`Thumbs::claim_slot`]. A session that never leaves the details view allocates none of it.
//!
//! **What the pages cost is one draw call each**, and only because the tiles are drawn in one batch
//! that [`crate::ui::grid`] sorts by texture before it paints. Unsorted, a screenful spread over
//! three pages would break the stream wherever two neighbours happened to land on different ones.
//!
//! Keyed by **path**, unlike everything in [`crate::shell::icons`] — and bounded, which is the whole
//! of what made a path key wrong there: that module's per-file map was unbounded and grew a
//! heap-allocated path per executable ever seen. This one cannot pass [`HELD`] entries however long
//! the session runs, and a thumbnail genuinely does belong to a path rather than to a folder — two
//! panes on the same folder, and the same folder revisited, draw the same picture.
//!
//! An entry carries the file's modified stamp, so a picture edited under the window is re-fetched
//! rather than shown as it was.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[cfg(windows)]
#[path = "../../windows/thumbs.rs"]
mod win;
#[cfg(windows)]
use win::picture;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::{ColorImage, TextureHandle, TextureOptions};

/// The shell call on its own, for the benchmark in `app::click_tests::memory`.
///
/// The service around it is what a grid uses and what the tests here drive; this is the floor
/// underneath it — the one cost nothing in this program can make smaller, only ask for less often.
#[cfg(test)]
pub(crate) fn picture_for_tests(path: &Path) -> bool {
    matches!(picture(path), Got::Picture(_))
}

/// One atlas cell, in pixels — and the size a thumbnail is asked for.
///
/// The same number as [`crate::ui::grid::TILE`]'s points, so a tile on a 1× display is drawn at
/// exactly the pixels the shell produced. On a scaled display it is resampled up, which is the same
/// trade the 16-point icon atlas makes and says so: the alternative is four times the texture for a
/// sharpness nobody has asked about, and a photograph resampled by 1.5 is a photograph.
pub const CELL: usize = 96;

/// One page of the atlas: 144 cells in a 1152-square texture, which is 5.3 MB.
///
/// Twelve square rather than one big grid, because a page is the unit that gets *allocated* — see the
/// module header. Small enough that a laptop window pays for one, big enough that a 4K one is four
/// rather than fourteen.
const PAGE_COLUMNS: usize = 12;
const PAGE_ROWS: usize = 12;
const PER_PAGE: usize = PAGE_COLUMNS * PAGE_ROWS;

/// How many pages there may ever be.
///
/// **A backstop, not a budget.** Pages are allocated on demand, so what a session actually holds is
/// what its window has shown at once — see the table in the module header. This is only the point past
/// which the surplus tiles keep their glyphs instead: 6 pages is 864 cells, which is a window showing
/// nearly nine hundred 96-point tiles at the same time. There is no such window; the number is here so
/// that a fault somewhere else cannot turn into unbounded texture allocation.
const MAX_PAGES: usize = 6;

/// How many thumbnails can be held at the very most, which is also how many could be on screen.
///
/// A tile's slot is [`crate::ui::grid::CELL_W`] × [`crate::ui::grid::CELL_H`] — 120 × 156 — so a
/// maximised window asks for `columns × lines` of them, and the module header has the arithmetic for
/// the four sizes that matter.
const SLOTS: usize = PER_PAGE * MAX_PAGES;
/// How many entries the cache holds, against [`SLOTS`] that can carry a picture.
///
/// Larger than the atlas because an entry is also how a file the shell **would not draw** is
/// remembered, and those have no cell: how many attempts it has had, and when the next one is due —
/// see [`BACKOFF`]. Without room for them, a folder of files the shell keeps refusing would be asked
/// about again on every frame.
const HELD: usize = SLOTS * 4;

/// How many questions may be outstanding at once.
///
/// A scroll can bring a hundred new tiles into view in one frame and the answers are wanted in the
/// order they were asked, so the queue is short on purpose: what does not fit is simply not asked
/// this frame, and the tile draws its painted glyph until the cell comes round again. The same trade
/// [`crate::shell::icons::Icons::request_file`] makes, and for the same reason — the alternative is
/// remembering every path ever asked about in order to avoid asking twice.
const QUEUE_CAP: usize = 48;

/// What is known about one file's picture.
struct Held {
    /// Where in the atlas it is, or `None` for a file the shell had no picture for.
    slot: Option<u32>,
    /// The size of the image *inside* the cell, which is not the cell: the shell preserves the
    /// picture's aspect, so a 16:9 photograph asked for at 96 comes back 96×54.
    size: [u32; 2],
    /// The file's modified time when this was fetched, so an edited picture is noticed.
    stamp: u64,
    /// **Which frame a tile last drew it in**, for the eviction below.
    ///
    /// A frame number rather than an `Instant`, and that is the whole of what makes
    /// [`Thumbs::claim_slot`] able to say "this one is on screen": a timestamp can be compared for
    /// age but not for *liveness*, and the question that matters is not which cell is oldest, it is
    /// which cells are being drawn right now.
    used: u64,
    /// How many times the shell has failed to draw this file. See [`BACKOFF`].
    tries: u8,
    /// When it is worth asking again. `None` once the shell has been given every chance it is going
    /// to get, and the tile settles for its painted glyph.
    retry_at: Option<std::time::Instant>,
}

/// How long to wait before asking the shell again about a file it would not draw, per attempt.
///
/// # Why a failure is retried at all, and why the waiting is the point
///
/// **The shell's answer is not a fact about the file.** `GetImage` fails under contention — for a
/// thumbnail Explorer is extracting at that moment, for one its own COM surrogate is busy with, for
/// a cache it is in the middle of writing — and it does not restrict itself to `E_PENDING` when it
/// does. `shell::thumbs`' own tests measured it: two calls in flight from one process, and one of
/// them comes back with a failure, on files that draw perfectly a moment later.
///
/// So a failure cached as "this file has no picture" is a tile that keeps its painted glyph for as
/// long as the folder is open, and there is nothing the user can do about it. Which is exactly what
/// it did, and what made it *accumulate*: every scroll-and-switch asks a few hundred files at once,
/// a slice of them fail while the shell is busy, and each of those is written off for good. Round
/// again and another slice goes. What you see is a grid where the files asked first have pictures
/// and everything asked since is a wall of glyphs.
///
/// **The delays are what make retrying work rather than merely re-fail.** Three attempts inside three
/// consecutive frames is fifty milliseconds, all of it inside the same busy window that caused the
/// first failure — so it is one attempt with extra steps. Spread over seconds it outlasts the burst:
/// a tenth of a second, then half, then a second and a half, then five. A file the shell genuinely
/// cannot draw costs five calls in total and then nothing.
///
/// The window paints on demand, so a tile waiting for its next attempt books a repaint for the
/// moment it comes due — see [`Thumbs::get`]. Nothing else would ask for that frame.
const BACKOFF: [u64; 4] = [100, 500, 1_500, 5_000];

/// How long to wait before attempt number `tries`, or `None` when there are none left.
fn backoff(tries: u8) -> Option<std::time::Duration> {
    BACKOFF
        .get(tries.saturating_sub(1) as usize)
        .map(|&ms| std::time::Duration::from_millis(ms))
}

/// A picture to go and get.
struct Job {
    /// The view that wants it, so leaving the folder cancels the question rather than queueing
    /// behind it — see [`Thumbs::only`].
    view: u64,
    path: PathBuf,
    stamp: u64,
}

/// What the shell said. **Two answers, and "no" is not one of them.**
///
/// There is deliberately no "this file has no picture" — see [`BACKOFF`]. The shell fails for reasons
/// that have nothing to do with the file, and it does not label them reliably, so every failure is a
/// *not yet* that the attempt count is what bounds. A file that really cannot be drawn is one that
/// answers `Later` until the attempts run out, which costs five calls and reads the same on screen.
enum Got {
    /// It drew one.
    Picture(ColorImage),
    /// It did not. Worth asking again, after a wait — a few times, and then not.
    Later,
}

/// And the answer on its way back to the tile that asked.
struct Ready {
    path: PathBuf,
    stamp: u64,
    got: Got,
}

/// Where a tile's picture is: the one texture, the patch of it to draw, and the picture's own
/// aspect inside that patch.
#[derive(Clone, Copy)]
pub struct Thumb {
    pub texture: egui::TextureId,
    pub uv: egui::Rect,
    /// The image's pixel size, for fitting it into the tile without stretching it.
    pub size: [u32; 2],
}

/// The thumbnail service. One per application.
pub struct Thumbs {
    known: HashMap<PathBuf, Held>,
    /// The pages, in slot order: page `n` holds slots `n * PER_PAGE ..`. Grown by one whenever a slot
    /// on a page that does not exist yet is claimed, and never shrunk — a page that has been filled
    /// once is a page this window is going to want again.
    pages: Vec<TextureHandle>,
    /// The next never-used slot, across every page.
    next_slot: u32,
    /// Slots whose entry has gone, ready to be filled again.
    free: Vec<u32>,
    /// Questions in flight, so a tile does not re-ask on every frame while one is out.
    pending: Arc<Mutex<HashSet<PathBuf>>>,
    /// The views on screen. See [`Thumbs::only`].
    live: Arc<Mutex<HashSet<u64>>>,
    jobs: Option<Sender<Job>>,
    queued: Arc<AtomicUsize>,
    tx: Sender<Ready>,
    rx: Receiver<Ready>,
    ctx: egui::Context,
    /// Which frame this is, bumped once per [`Thumbs::poll`] — which the application calls once per
    /// pass. What [`Held::used`] records, and what tells a cell that is on screen from one that is not.
    frame: u64,
    /// How many cells could still be given to a new picture this frame. Counted in `poll` and spent by
    /// `request`, so a screenful of tiles larger than the atlas asks for what will fit and no more —
    /// see [`Thumbs::claim_slot`].
    spare: usize,
    /// Diagnostics for `--trace`: how many pictures have been fetched and how many uploaded. Both
    /// should stop climbing as soon as a folder has settled.
    pub fetched: u64,
    pub uploads: u64,
    /// How many tiles asked in the frame that just finished — which should be a screenful and nothing
    /// like the size of the folder. See `Thumbs::asks`.
    pub asks: usize,
    asking: usize,
}

impl Thumbs {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            known: HashMap::new(),
            pages: Vec::new(),
            next_slot: 0,
            free: Vec::new(),
            pending: Arc::new(Mutex::new(HashSet::new())),
            live: Arc::new(Mutex::new(HashSet::new())),
            jobs: None,
            queued: Arc::new(AtomicUsize::new(0)),
            tx,
            rx,
            ctx: ctx.clone(),
            frame: 1,
            spare: SLOTS,
            fetched: 0,
            uploads: 0,
            asks: 0,
            asking: 0,
        }
    }

    /// Take delivery of whatever came back, place it, and work out what next frame may ask for.
    ///
    /// # It runs at the **end** of the frame, and that is not arbitrary
    ///
    /// Everything else this program polls — the scans, the icons, git, the previews — is polled at the
    /// top of a frame, because an answer is wanted on screen as soon as it exists. This one is
    /// different because placing an answer means *taking a cell*, and which cells are takeable is a
    /// question about what was on screen. At the end of a frame that is known exactly: a cell was drawn
    /// in this frame or it was not. At the top of one it can only be guessed at from the frame before,
    /// which is what the first version did — and it had to call a cell reusable only after **two**
    /// quiet frames to be safe, because one quiet frame is indistinguishable from "about to be drawn
    /// again".
    ///
    /// Those two frames were a bug rather than a cost, and the shape of it is worth keeping: this
    /// program paints **on demand**. Switch a scrolled listing from a tree to a list and the tiles are
    /// all new, all their cells are held by the files you scrolled past, and none of those has been
    /// quiet for two frames yet — so nothing is asked for, nothing arrives, nothing asks for a repaint,
    /// and the window sits there with a grid of painted glyphs until you scroll and force some frames.
    /// Which is exactly what it did.
    pub fn poll(&mut self) {
        let used = self.frame;
        while let Ok(Ready { path, stamp, got }) = self.rx.try_recv() {
            // The worker's claim on this path, released now that the answer is about to be written down
            // rather than when the worker let go of it — see the worker for the window that closes.
            // Before the `continue` below as well as after it: a dropped answer is still an answer this
            // path is no longer waiting for, and holding the claim would mean never asking again.
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&path);
            }
            // How many refusals this file has already had, which only a `Later` adds to.
            let before = self
                .known
                .get(&path)
                .filter(|held| held.stamp == stamp)
                .map_or(0, |held| held.tries);
            let held = match got {
                // The upload is the only part of this that has to be on this thread: it is a memcpy
                // into a texture, with none of the shell behind it.
                Got::Picture(image) => {
                    let Some(slot) = self.claim_slot() else {
                        // Every cell in the atlas is being drawn, so there is nowhere to put this
                        // without taking a picture off a tile that is on screen. Dropped, and
                        // deliberately **not remembered**: the file is not "without a picture", it is
                        // waiting for room, and `request` will not ask again until there is some.
                        continue;
                    };
                    let size = self.blit(slot, &image);
                    self.uploads += 1;
                    Held {
                        slot: Some(slot),
                        size,
                        stamp,
                        used,
                        tries: 0,
                        retry_at: None,
                    }
                }
                // Not this time. Kept, so the *count* survives — and with the moment the next attempt
                // comes due on it, which is what `get` reads. See [`BACKOFF`].
                Got::Later => {
                    let tries = before.saturating_add(1);
                    Held {
                        slot: None,
                        size: [0, 0],
                        stamp,
                        used,
                        tries,
                        retry_at: backoff(tries).map(|wait| std::time::Instant::now() + wait),
                    }
                }
            };
            self.remember(path, held);
        }
        // How many tiles asked in the frame that has just finished. **A screenful, or something is
        // drawing the whole folder**: every tile that asks marks its cell as live and spends a share of
        // the queue, so a view that asks about rows nobody can see would starve the ones they can.
        self.asks = std::mem::take(&mut self.asking);
        // What the *next* frame's tiles may ask for, counted now that this frame's are known — and then
        // the frame moves on, so a `get` during the next one marks its cell as drawn in that one.
        self.spare = self.room();
        self.frame = self.frame.wrapping_add(1);
    }

    /// The picture for a file, or `None` until there is one — and then the question is asked.
    ///
    /// `stamp` is the file's modified time out of the listing, which costs nothing to pass and is
    /// what makes a picture edited under the window come back rather than stay as it was.
    ///
    /// Answering `None` on the first ask is the whole design, the same as
    /// [`crate::shell::icons::Icons::kind`]: the tile draws its painted glyph for a frame or two and
    /// nothing ever waits on the shell inside a frame.
    pub fn get(&mut self, path: &Path, stamp: u64, view: u64) -> Option<Thumb> {
        self.asking += 1;
        // Four answers, in one pass over the entry: it has a picture; it is waiting for its next
        // attempt, which may or may not be due; or it has run out of attempts.
        let frame = self.frame;
        enum Answer {
            /// Drawable, out of this cell.
            Here(u32, [u32; 2]),
            /// Nothing yet, and the shell is worth asking.
            Ask,
            /// Nothing yet, and the next attempt is not due until then.
            Wait(std::time::Instant),
            /// Nothing, and nothing more to try.
            Never,
        }
        let answer = match self.known.get_mut(path) {
            Some(held) if held.stamp == stamp => {
                held.used = frame;
                match (held.slot, held.retry_at) {
                    (Some(slot), _) => Answer::Here(slot, held.size),
                    (None, Some(at)) if at <= std::time::Instant::now() => Answer::Ask,
                    (None, Some(at)) => Answer::Wait(at),
                    (None, None) => Answer::Never,
                }
            }
            // Not known, or known about a version of the file that has been written over since.
            _ => Answer::Ask,
        };
        let (slot, size) = match answer {
            Answer::Here(slot, size) => (slot, size),
            Answer::Ask => {
                self.request(path, stamp, view);
                return None;
            }
            // **A frame is booked for the moment it comes due**, because nothing else would ask for one:
            // the window paints on demand, and a tile sitting out a wait is a tile with nothing else
            // happening to it. Without this the retry would land whenever something unrelated next
            // wanted a frame, which for a folder somebody is reading is never.
            Answer::Wait(at) => {
                self.ctx
                    .request_repaint_after(at.saturating_duration_since(std::time::Instant::now()));
                return None;
            }
            Answer::Never => return None,
        };
        let page = self.pages.get(slot as usize / PER_PAGE)?;
        Some(Thumb {
            texture: page.id(),
            uv: Self::image_uv(slot, size),
            size,
        })
    }

    /// Which views are on screen, so the worker can drop the questions from the ones that are not.
    ///
    /// Called every frame, exactly as [`crate::shell::icons::Icons::only`] is and for the same
    /// reason: scrolling away leaves a queue of questions about tiles nothing is drawing, and each
    /// of them is a blocking shell call.
    pub fn only(&mut self, views: &[u64]) {
        if let Ok(mut live) = self.live.lock() {
            if live.len() == views.len() && views.iter().all(|view| live.contains(view)) {
                return;
            }
            live.clear();
            live.extend(views.iter().copied());
        }
    }

    /// Whether any tile is still waiting for its picture.
    ///
    /// For the capture runs, which have to wait for the same reason `App::git_pending` makes them
    /// wait: a screenshot of a grid of placeholder glyphs says nothing about the grid. Nothing in the
    /// window reads this — a tile that has not been answered draws its glyph and asks again.
    pub fn pending(&self) -> bool {
        self.queued.load(Ordering::Relaxed) > 0
            || self.pending.lock().map(|held| !held.is_empty()).unwrap_or(false)
    }

    /// How many files have run out of attempts and will not be asked about again, and how many
    /// questions are outstanding. For `--trace`, and between them they say which end is stuck.
    ///
    /// - `gave up` climbing means the shell is refusing and the retries are not outlasting it.
    /// - `queued` pinned at [`QUEUE_CAP`] with nothing arriving means the worker is wedged behind one
    ///   file, and every tile behind it is waiting on that.
    /// - both at zero while tiles show glyphs means nothing has been *asked*, which is a third thing
    ///   again.
    pub fn stuck(&self) -> (usize, usize) {
        (
            self.known
                .values()
                .filter(|held| held.slot.is_none() && held.retry_at.is_none())
                .count(),
            self.queued.load(Ordering::Relaxed),
        )
    }

    /// What is held, for `--trace`: entries, how many carry a picture, how many pages that took, and
    /// **how many cells this frame's tiles may still be given**.
    ///
    /// The last of those is the diagnostic that matters. A grid where tiles sit on their painted glyph
    /// is one of two things, and the number says which: `spare` at zero means every cell is held by a
    /// tile that is on screen, and anything else means the pictures are merely still on their way.
    ///
    /// It is [`Thumbs::spare`] and deliberately not [`Thumbs::room`]. `room` is only meaningful at the
    /// end of a frame, which is where `poll` reads it; asked from anywhere else — and `--trace` reports
    /// from the top of a frame — every cell looks reusable, because nothing has been drawn yet. A
    /// figure that always reads "plenty" is worse than no figure at all, and it read that way once.
    pub fn held(&self) -> (usize, usize, usize, usize) {
        (
            self.known.len(),
            self.known.values().filter(|held| held.slot.is_some()).count(),
            self.pages.len(),
            self.spare,
        )
    }

    fn request(&mut self, path: &Path, stamp: u64, view: u64) {
        // Nothing to ask off Windows, so nothing is ever asked: `picture` has no answer there and a
        // tile keeps its painted glyph.
        if !cfg!(windows) {
            return;
        }
        // **Every way of saying "not now" books a frame to say it in again.**
        //
        // This is the rule the whole service hangs on, and it was learned the hard way: a tile that is
        // refused and does not book a frame is a tile that waits for somebody else to want one, and in a
        // window that paints on demand there is nobody else. A grid of glyphs that fills in the moment
        // you scroll is what that looks like, and it looks the same whichever of the three refusals
        // below did it — so all three do the same thing, rather than each being reasoned about
        // separately and one of them being missed.
        //
        // It cannot spin: the delay throttles it, and it is only reached while a tile on screen is
        // actually waiting. A tile that has run out of attempts never gets here at all — `get` answers
        // `Never` and asks for nothing.
        if self.queued.load(Ordering::Relaxed) >= QUEUE_CAP {
            self.nudge();
            return;
        }
        // **Nowhere to put the answer is a reason not to ask the question.** A window showing more
        // tiles than the atlas has cells would otherwise spend a shell call per surplus tile per
        // frame, for pictures that could only be stored by taking one off a tile that is on screen.
        // Spent rather than tested, so the tiles that *do* fit are the first ones asked.
        //
        if self.spare == 0 {
            self.nudge();
            return;
        }
        // Claimed before the send, so the tile behind this one in the same frame does not ask again.
        let claimed = self
            .pending
            .lock()
            .map(|mut pending| pending.insert(path.to_path_buf()))
            .unwrap_or(false);
        if !claimed {
            // Somebody is already asking — which is not a reason to stop expecting an answer.
            self.nudge();
            return;
        }
        self.fetched += 1;
        self.spare -= 1;
        self.queued.fetch_add(1, Ordering::Relaxed);
        let job = Job {
            view,
            path: path.to_path_buf(),
            stamp,
        };
        if self.worker().send(job).is_err() {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(path);
            }
            // **The worker is gone, so the next request starts a new one.** Nothing here can send down
            // a channel whose other end has been dropped, and a service that answered nothing for the
            // rest of the session because one thread ended is a worse failure than the one that ended
            // it. `worker` is `get_or_insert_with`, so clearing this is all it takes.
            self.jobs = None;
        }
    }

    /// Book a frame, because a tile that has just been refused has to be able to ask again.
    ///
    /// A tenth of a second: fast enough that a queue draining or a cell freeing is picked up while the
    /// eye is still on the tile, slow enough that a hundred refused tiles are one wake-up rather than a
    /// hundred. It replaces nothing — before this, three of the four ways a request could be refused
    /// simply returned, and whether the tile was ever asked about again depended on something else in
    /// the window happening to want a frame.
    fn nudge(&self) {
        self.ctx
            .request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// The one worker, started on first use.
    ///
    /// A thread of its own with an apartment that lasts, for the reason `shell::icons`'
    /// `place_worker` sets out at length: `SHCreateItemFromParsingName` goes through the shell
    /// namespace and needs COM, and the *last* `CoUninitialize` in a process frees shell state other
    /// threads are still using — so the apartment is entered once on a thread that outlives the
    /// requests.
    ///
    /// One thread and not a pool. The queue is short by design and the answers are wanted in the
    /// order they were asked: a pool would return the bottom of the screen before the top.
    fn worker(&mut self) -> &Sender<Job> {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let pending = self.pending.clone();
        let queued = self.queued.clone();
        let live = self.live.clone();
        self.jobs.get_or_insert_with(|| {
            let (send, receive) = channel::<Job>();
            let _ = std::thread::Builder::new()
                .name("shell-thumbs".to_owned())
                .spawn(move || {
                    crate::shell::init();
                    // Any of this can touch an empty removable drive, and the call would otherwise
                    // raise "Please insert a disk into drive E:" from inside it.
                    crate::fs::scan::silence_device_dialogs();
                    while let Ok(Job { view, path, stamp }) = receive.recv() {
                        // Nothing is showing that view any more: the folder was left while this was
                        // in the queue. Dropped here rather than fetched and discarded, because the
                        // fetching is the whole of the cost.
                        let wanted = live
                            .lock()
                            .map(|live| live.contains(&view))
                            .unwrap_or(true);
                        // Skipped for a dead view, and then nothing is sent at all — see below. The
                        // placeholder is never looked at.
                        //
                        // **Caught, because the thing being called is other people's code.** `GetImage`
                        // runs whichever thumbnail provider is registered for the file, in this process:
                        // a broken one is somebody else's DLL panicking on this thread. Unguarded, that
                        // ends the worker, and a service with no worker answers nothing for the rest of
                        // the session — every tile in every folder, from one bad file. Guarded, it costs
                        // that one tile its picture.
                        let got = if wanted {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                picture(&path)
                            }))
                            .unwrap_or(Got::Later)
                        } else {
                            Got::Later
                        };
                        queued.fetch_sub(1, Ordering::Relaxed);
                        // **The claim on this path is held until the answer has been *recorded*.**
                        //
                        // Released here, as it used to be, there is a window one frame wide between the
                        // worker finishing with a file and `poll` writing the answer down — and a tile
                        // that asks inside that window finds no entry, sees no claim, and starts a second
                        // job for the same file. Both answers then arrive and the second overwrites the
                        // first, which is where the leak in `remember` came from. With 140 tiles asking
                        // every frame, every single answer was exposed to it.
                        //
                        // So `poll` releases it — except on the two paths where there will be no answer to
                        // record, which have to release it here or the file is never asked about again:
                        // a job skipped for a folder nobody is looking at, and a reply nothing received.
                        let released = if wanted {
                            tx.send(Ready { path: path.clone(), stamp, got })
                                .inspect(|()| ctx.request_repaint())
                                .is_ok()
                        } else {
                            false
                        };
                        if !released {
                            if let Ok(mut pending) = pending.lock() {
                                pending.remove(&path);
                            }
                        }
                    }
                });
            send
        })
    }

    /// Record an answer, trimming the cache when it has grown past [`HELD`].
    ///
    /// # Whatever this replaces gives its cell back
    ///
    /// **The bug that made the grid stop filling in at all**, and it is one line's worth of leak with a
    /// symptom nothing like its cause. A second answer for a file that already has a cell replaces its
    /// entry — and the cell the old entry was holding is then referenced by nothing and is not on the
    /// free list either. It is simply gone.
    ///
    /// Measured in the wild before this line existed: **864 cells handed out, 71 entries left**, six
    /// pages allocated and `spare` at zero. The atlas exhausted while holding seventy-one pictures, so
    /// nothing new could ever be placed, so a screenful of tiles kept their painted glyphs for ever —
    /// and a scroll made a couple of the drawn cells stale, which is why scrolling loaded "some but not
    /// all". Every symptom of the report falls out of that one number.
    ///
    /// A duplicate answer is easy to come by, which is the other half of the fix — see the worker, where
    /// the claim on a path is now released once the answer is *recorded* rather than as the worker
    /// finishes with it. But a cache that leaks its own storage whenever a key is written twice is wrong
    /// however hard the duplicates are to produce, so the containment is here and the cause is there.
    fn remember(&mut self, path: PathBuf, held: Held) {
        let taking = held.slot;
        if let Some(displaced) = self.known.insert(path, held) {
            // Not when the new entry is using the very cell the old one had, which happens whenever a
            // re-fetch is placed in the cell its own eviction freed.
            if let Some(slot) = displaced.slot.filter(|&slot| Some(slot) != taking) {
                self.free.push(slot);
            }
        }
        if self.known.len() <= HELD {
            return;
        }
        // Back to three quarters, oldest first — and every cell that goes with them is put back on the
        // free list rather than lost, or the atlas would fill up with cells nothing can reach.
        //
        // By the frame numbers alone. Sorting `(used, path)` pairs meant cloning every key in the map to
        // decide which quarter to drop — three and a half thousand `PathBuf`s per trim, and a trim comes
        // round once per atlas-worth of pictures placed, which is repeatedly during a long scroll.
        // `select_nth_unstable` finds the cutoff without ordering the rest, and `retain` needs no keys
        // at all.
        let excess = self.known.len() - HELD * 3 / 4;
        let mut ages: Vec<u64> = self.known.values().map(|held| held.used).collect();
        let (_, &mut cutoff, _) = ages.select_nth_unstable(excess - 1);
        let free = &mut self.free;
        self.known.retain(|_, held| {
            if held.used > cutoff {
                return true;
            }
            if let Some(slot) = held.slot {
                free.push(slot);
            }
            false
        });
    }

    /// The next cell for a picture: a free one, a never-used one, or the one **nobody is drawing**.
    ///
    /// # A cell that is on screen is never taken
    ///
    /// This is the whole of the rule, and it is the difference between running out of room looking
    /// like a few plain tiles and looking like a broken window.
    ///
    /// Plain least-recently-used is not enough, and the failure is not subtle once you have seen it.
    /// Suppose a window shows more tiles than the atlas has cells. Every visible tile is drawn every
    /// frame, so every entry's `used` is the current frame and *all* of them are equally recent — so
    /// LRU picks one of them, an on-screen tile loses its picture and goes back to its glyph, that tile
    /// asks again, its answer takes a cell off another on-screen tile, and round it goes. Every frame,
    /// for ever. What it looks like is a wall of thumbnails flickering as though each were being
    /// replaced by its neighbour, which is exactly what is happening.
    ///
    /// So the victim has to be a cell **no tile drew in the frame that has just finished** — which is
    /// exactly knowable, because [`Thumbs::poll`] runs at the end of one. That is the whole of the rule
    /// and it needs no slack: a cell is on screen or it is not.
    ///
    /// Which leaves the case the rule creates: a window with more tiles than cells has *no* reusable
    /// cell, so the surplus never gets a picture at all until something moves. That is what
    /// [`MAX_PAGES`] is for — the atlas grows a page rather than the tiles going without, and the rule
    /// only bites at the end of the last page. See the module header.
    ///
    /// `None` means every cell on every page is live and there is no page left to add. The caller drops
    /// the answer rather than stealing a picture off a tile that is on screen; [`Thumbs::room`] normally
    /// stops it getting that far.
    ///
    /// The victim is chosen by a scan of the cache, which is a cost worth naming: it happens per placed
    /// answer, so up to a queue's worth in one frame, and only once the atlas is full. It is deliberately
    /// **not** folded into `room`'s once-a-frame pass — that pass could free every off-screen cell at once
    /// and cheaply, and freeing them is exactly what must not happen: an off-screen cell is a thumbnail
    /// that scrolling back would otherwise have to fetch again. Taking the oldest one at a time is what
    /// makes this a cache rather than a screenful.
    fn claim_slot(&mut self) -> Option<u32> {
        if let Some(slot) = self.free.pop() {
            return Some(slot);
        }
        if (self.next_slot as usize) < SLOTS {
            let slot = self.next_slot;
            self.next_slot += 1;
            return Some(slot);
        }
        // Not drawn in the frame that has just finished, oldest first.
        let victim = self
            .known
            .iter()
            .filter(|(_, held)| held.slot.is_some() && held.used < self.frame)
            .min_by_key(|(_, held)| held.used)
            .map(|(path, held)| (path.clone(), held.slot.unwrap_or(0)));
        let (path, slot) = victim?;
        // Removed rather than left pointing at a cell somebody else now owns, so the file asks again
        // if it comes back into view.
        self.known.remove(&path);
        Some(slot)
    }

    /// **Every cell is in exactly one place**: held by an entry, on the free list, or never handed out.
    ///
    /// The invariant the atlas rests on, written down because breaking it is invisible until the whole
    /// view stops working. A cell that belongs to nothing and is not free cannot be reached and cannot
    /// be reused — it is simply gone, and there is no symptom until enough of them have gone that the
    /// atlas is exhausted. That is what happened: 864 handed out, 71 reachable, and a grid that had
    /// stopped filling in for good. `no_cell_is_ever_lost` is this, asserted.
    #[cfg(test)]
    fn accounted(&self) -> usize {
        let held = self.known.values().filter(|held| held.slot.is_some()).count();
        held + self.free.len() + (SLOTS - self.next_slot as usize)
    }

    /// How many cells could be given to a new picture: the free ones, the ones on pages not made yet,
    /// and the ones [`Thumbs::claim_slot`] would be willing to take.
    ///
    /// Counted once per frame at the end of [`Thumbs::poll`] rather than per request, because the scan is
    /// over every entry and a screenful of tiles asks a few hundred times.
    ///
    /// **And not even that, once the cheap cells already cover what one frame could spend.** A frame can
    /// place at most [`QUEUE_CAP`] answers, so past that the exact figure changes nothing — and the
    /// early-out is what keeps a window that is *not* showing tiles from paying anything at all. Without
    /// it, a session that visited one tiled folder would walk a few thousand entries on every frame for
    /// the rest of its life to compute a budget nothing was going to spend.
    fn room(&self) -> usize {
        let cheap = self.free.len() + (SLOTS - self.next_slot as usize);
        if cheap >= QUEUE_CAP {
            return cheap;
        }
        let evictable = self
            .known
            .values()
            .filter(|held| held.slot.is_some() && held.used < self.frame)
            .count();
        cheap + evictable
    }

    /// Copy one picture into its cell, adding the page it lives on if this is the first slot claimed
    /// on it. Answers the size it went in at, which is what the draw needs to keep the aspect.
    fn blit(&mut self, slot: u32, image: &ColorImage) -> [u32; 2] {
        // Pages are added in slot order and never removed, so a slot's page is either the next one or
        // one that is already here. The loop rather than an index so that a `free` slot from a page
        // that somehow has not been made yet cannot panic.
        while self.pages.len() <= slot as usize / PER_PAGE {
            let page = self.pages.len();
            self.pages.push(self.ctx.load_texture(
                format!("shell-thumbs-{page}"),
                ColorImage::filled(
                    [PAGE_COLUMNS * CELL, PAGE_ROWS * CELL],
                    egui::Color32::TRANSPARENT,
                ),
                // Linear, because a 96-pixel picture drawn at 96 *points* is not 96 device pixels
                // on any scaled display, and nearest would shimmer as it scrolled.
                TextureOptions::LINEAR,
            ));
        }
        // The whole cell is written every time, not only the part the picture covers: a slot is
        // reused, and a smaller picture arriving in it would otherwise be drawn over the corner of
        // whatever was there before.
        // A row at a time rather than a pixel at a time: a full cell is 9,216 bounds-checked index
        // pairs, and a scroll places up to a queue's worth of them in one frame.
        let mut cell = ColorImage::filled([CELL, CELL], egui::Color32::TRANSPARENT);
        let [across, h] = image.size;
        let (w, h) = (across.min(CELL), h.min(CELL));
        for y in 0..h {
            let from = &image.pixels[y * across..y * across + w];
            cell.pixels[y * CELL..y * CELL + w].copy_from_slice(from);
        }
        let (column, row) = Self::cell_of(slot);
        self.pages[slot as usize / PER_PAGE]
            .set_partial([column * CELL, row * CELL], cell, TextureOptions::LINEAR);
        [w as u32, h as u32]
    }

    /// Where in its own page a slot's cell is, in cells.
    fn cell_of(slot: u32) -> (usize, usize) {
        let within = slot as usize % PER_PAGE;
        (within % PAGE_COLUMNS, within / PAGE_COLUMNS)
    }

    /// The patch of its page a picture occupies: its cell, cropped to the pixels the picture actually
    /// filled.
    fn image_uv(slot: u32, size: [u32; 2]) -> egui::Rect {
        let (column, row) = Self::cell_of(slot);
        let (across, down) = ((PAGE_COLUMNS * CELL) as f32, (PAGE_ROWS * CELL) as f32);
        let min = egui::pos2((column * CELL) as f32 / across, (row * CELL) as f32 / down);
        egui::Rect::from_min_size(
            min,
            egui::vec2(size[0] as f32 / across, size[1] as f32 / down),
        )
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Unreachable: `Thumbs::available` is false off Windows, so nothing is ever asked for.
#[cfg(not(windows))]
fn picture(_path: &Path) -> Got {
    Got::Later
}

#[cfg(all(test, windows))]
mod tests;
