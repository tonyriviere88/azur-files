//! Explorer's own file icons, at Explorer's speed.
//!
//! # The problem this has to solve
//!
//! The obvious way to get a file's icon is `SHGetFileInfoW` with `SHGFI_ICON` for
//! each row. [`crate::fs::scan`]'s benchmark measures that path at **over a
//! millisecond per file** — 67 seconds for a folder of 60,000 — because it opens the
//! file, consults its association, may extract a resource from a DLL, and returns a
//! fresh `HICON` to be destroyed. It is exactly what makes a shell-backed file
//! manager show an empty window for a second before its rows appear.
//!
//! Three things together make this cost nothing instead:
//!
//! 1. **`SHGFI_USEFILEATTRIBUTES`.** With it, the shell answers from the *name and
//!    the attribute word* and never touches the file. That removes the I/O, and with
//!    it the reason a folder of 60,000 was slow.
//! 2. **`SHGFI_SYSICONINDEX`, not `SHGFI_ICON`.** An index into the one system image
//!    list, rather than a handle to own and free. Nothing is allocated per row.
//! 3. **A cache keyed by extension.** Every `.rs` in a folder resolves to the same
//!    index, so a folder of ten thousand source files performs *one* lookup. This is
//!    the step that turns a per-file cost into a per-*type* one, and there are only
//!    ever a few dozen types on screen.
//!
//! The bitmap itself is then pulled out of the system image list once per icon index
//! and uploaded to one egui texture atlas, so a row costs a textured quad — the same
//! as the painted glyphs it replaces.
//!
//! # What still has to be per-file
//!
//! Executables, shortcuts and anything with its own embedded icon: `.exe` files do
//! not share an icon the way `.txt` files do. Those are resolved per file, off the
//! UI thread, and until the answer arrives the row shows the generic icon for its
//! kind. A folder full of executables therefore fills in over a few frames rather
//! than all at once, which is the same thing Explorer does and for the same reason.
//!
//! # Where a per-file answer lives, and why it matters
//!
//! **In the view that asked, keyed by row.** Not here, and not keyed by path.
//!
//! It used to be a `HashMap<PathBuf, i32>` on this struct, which is the obvious design and
//! is a memory leak with a cache's manners: scrolling `C:\Windows\System32` put **4,345
//! entries** in it — a heap-allocated path per executable file ever seen — and none of them
//! went away when you left the folder. Twenty folders like that is 87,000 paths, and the
//! shell-side cost of every question behind them.
//!
//! So a request carries the *view* that asked and the *row* it asked about, and the answer
//! comes back addressed the same way. [`crate::pane::Tab`] holds a `Vec<i32>` — four bytes a
//! row, sized when the listing lands, dropped when the tab moves. Leave the folder and
//! everything the folder cost goes with it; an answer that arrives afterwards finds no view
//! to belong to and is thrown away. Measured, the same scroll of System32 went from
//! **+8.7 MB to +1.4 MB**.
//!
//! What stays here is only what is genuinely shared and genuinely small: one entry per file
//! *type*, one per sidebar *place*, and the uploaded bitmaps — of which a folder of 4,910
//! executables produced twenty, because thousands of files share a handful of icons.
//!
//! The questions go to **one worker thread with a bounded queue**. That used to be a thread
//! per question, so the same scroll spawned 4,910 of them, each with a stack; the burst was
//! visible as megabytes arriving and leaving. A dropped question costs a row the generic icon
//! until it comes round again, which is cheaper than remembering every path in order to
//! avoid asking twice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(windows)]
#[path = "../../windows/icons.rs"]
mod win;
#[cfg(windows)]
pub(crate) use win::{bitmap, bitmap_of, bitmap_premultiplied, index_of, index_of_place};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::{ColorImage, TextureHandle, TextureOptions};

/// The size the shell's small image list is drawn at.
///
/// 16×16 on every Windows to date. Only the tests name it — the drawing code takes
/// whatever size the bitmap turns out to be and scales it into the row, so a future
/// Windows that changes this needs no code change here.
#[cfg_attr(not(test), allow(dead_code))]
const SMALL: usize = 16;

/// Extensions whose icon is stored in the file rather than shared by the type.
///
/// A `.exe` carries its own; a `.txt` does not. Only these pay a per-file lookup.
/// Dropping `dll` and `ocx` from this list was tried, on the grounds that scrolling all of
/// System32 asked about 4,345 files and got 20 distinct icons back. It cut the questions to
/// 710 and did not measurably cut the memory — the cost is the shell's own cache, which it
/// keeps whether or not this program does — so the fidelity was not worth spending. The
/// [`Recent`] cap is what bounds it instead.
#[cfg(windows)]
fn has_own_icon(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "exe" | "dll" | "ico" | "lnk" | "url" | "cpl" | "msc" | "scr" | "ocx" | "msi"
    )
}

/// One atlas cell, and the grid: 16x16 icons, 32 across and 8 down.
const CELL: usize = 16;
const ATLAS_COLUMNS: usize = 32;
const ATLAS_ROWS: usize = 8;
const ATLAS_SLOTS: usize = ATLAS_COLUMNS * ATLAS_ROWS;

/// A map that remembers when each entry was last read, so the least useful can go when
/// there are too many of them or when nothing has wanted them for a while.
///
/// The two caches below are keyed by *path* and by *texture*, and both are unbounded in the
/// thing that grows: files, and GL objects. Measured, scrolling `C:\Windows\System32` — 4,910
/// files, nearly all of them `.dll` — put **4,345 entries** in the path map and cost 8.7 MB,
/// and twenty folders like it would be most of a memory budget. The same measurement is the
/// argument for the size of the caps: those 4,345 lookups produced **20 distinct icons**, so
/// the path map is a lookup table for a handful of answers and holding thousands of its keys
/// buys nothing.
///
/// A plain `HashMap` plus a stamp rather than an intrusive list: an eviction happens once
/// every few hundred lookups, and one sort of a thousand stamps is cheaper to run than a
/// doubly-linked list is to maintain — or to read.
struct Recent<K, V> {
    items: HashMap<K, (V, std::time::Instant)>,
    /// Trim back to three-quarters of this when it is passed.
    cap: usize,
}

impl<K: std::hash::Hash + Eq + Clone, V> Recent<K, V> {
    fn new(cap: usize) -> Self {
        Self {
            items: HashMap::new(),
            cap,
        }
    }

    /// Read an entry, marking it as used now.
    fn get(&mut self, key: &K) -> Option<&V> {
        let (value, used) = self.items.get_mut(key)?;
        *used = std::time::Instant::now();
        Some(value)
    }

    fn insert(&mut self, key: K, value: V) {
        self.items.insert(key, (value, std::time::Instant::now()));
        if self.items.len() > self.cap {
            self.evict_oldest(self.items.len() - self.cap * 3 / 4);
        }
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    /// Drop the `count` least recently read entries.
    fn evict_oldest(&mut self, count: usize) {
        let mut stamps: Vec<(std::time::Instant, K)> = self
            .items
            .iter()
            .map(|(key, (_, used))| (*used, key.clone()))
            .collect();
        stamps.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in stamps.into_iter().take(count) {
            self.items.remove(&key);
        }
    }

}

/// What a row needs to draw an icon: which texture, and where in it.
#[derive(Clone, Copy, Debug)]
pub struct Icon {
    /// Index into the system image list. The cache maps this to a texture region.
    pub index: i32,
}

/// An answer from the worker.
enum Ready {
    /// An extension (or `"\0dir"`) now known to map to this image-list index.
    Kind { key: String, index: i32 },
    /// One file's own icon, addressed by the view that asked and the row it asked about.
    ///
    /// No path comes back. The question was "row 412 of view 9", and the answer is stored in
    /// view 9's own column — so when that view is gone, so is everything it collected. This
    /// is the whole of the folder-scoped rule for icons: nothing here is keyed by a path,
    /// because a path is a per-file allocation that outlives the folder it came from.
    File { view: u64, row: u32, index: i32 },
    /// A *place* — a sidebar row. Keyed by path, and deliberately: there are a dozen of them
    /// for the life of the window and they belong to no folder.
    Place { path: PathBuf, index: i32 },
    /// One icon's pixels, pulled out of the shell's image list.
    ///
    /// **This is the one that used to freeze the window.** Getting a bitmap out of the image
    /// list looks like a local operation and is not: for an icon the shell resolved from a file
    /// on a network share, `IImageList::GetIcon` goes back over the network to extract it.
    /// Measured on a mapped share: **2.54 seconds**, on the UI thread, for one `.ico` file —
    /// with the other four icons in the same folder costing 2 to 31 ms each. The window sat
    /// there until it returned.
    Bitmap { index: i32, image: ColorImage },
}

/// What the worker is asked to find out.
enum Job {
    Kind { key: String, is_dir: bool },
    File { view: u64, row: u32, path: PathBuf },
}

/// A bitmap to fetch, on a thread of its own. See [`Ready::Bitmap`].
struct Wanted(i32);

/// A row's icon has not been asked about yet.
pub const UNASKED: i32 = -1;
/// It has been asked about and the answer has not come back.
pub const ASKED: i32 = -2;

/// The icon service.
///
/// Lookups are answered from memory when they can be and requested in the
/// background when they cannot, so drawing a row never blocks on the shell.
pub struct Icons {
    /// Extension → image-list index. `"\0dir"` is the folder entry.
    ///
    /// Not bounded, because it cannot grow: it is one entry per *type*, and a machine has a
    /// few hundred file types on it. Measured at 29 after scrolling all of System32.
    kinds: HashMap<String, i32>,
    /// Sidebar places, which are keyed by path because they are not part of any folder:
    /// the drives, the shell's own folders, the Recycle Bin. A dozen or so, for the session.
    places_seen: HashMap<PathBuf, i32>,
    /// File answers waiting to be handed to the view that asked. Drained every frame.
    answers: Vec<(u64, u32, i32)>,
    /// The one worker thread, and how many jobs are outstanding on it.
    ///
    /// It used to be a thread *per file*: scrolling a folder of 4,910 executables spawned
    /// 4,910 of them, each with its own stack, and the burst was visible as megabytes
    /// arriving and leaving. One thread with a queue does the same work with one stack.
    jobs: Option<Sender<Job>>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
    /// Diagnostics for `--trace`: how many bitmaps have been pulled out of the image list and
    /// how many textures uploaded. Both should stop climbing almost immediately.
    pub bitmaps: u64,
    pub uploads: u64,
    /// Image-list index → its slot in the atlas, least recently drawn first.
    slots: Recent<i32, u32>,
    /// The one texture every shell icon is drawn from. See [`Icons::uv`].
    atlas: Option<TextureHandle>,
    /// The next never-used slot, until the grid is full.
    next_slot: u32,
    /// Requests already in flight, so a folder of ten thousand `.exe` files does not
    /// queue ten thousand duplicates per frame.
    pending: Arc<Mutex<std::collections::HashSet<String>>>,
    tx: Sender<Ready>,
    rx: Receiver<Ready>,
    /// Turned off when the shell cannot be reached at all, so nothing retries per row
    /// for the rest of the session.
    available: bool,
    /// The worker that answers *place* lookups, started on first use. One thread for the
    /// session, and the only lookup here that needs a thread of its own — see
    /// [`place_worker`].
    places: Option<Sender<PathBuf>>,
    /// The worker that pulls bitmaps out of the image list. A thread of its own rather than a
    /// share of the lookup thread, because one slow extraction must not hold up the type
    /// lookups every ordinary row is waiting on — and on a network share one of them took
    /// two and a half seconds.
    pixels: Option<Sender<Wanted>>,
    /// Bitmaps already asked for, so a row does not re-ask every frame while one is in flight.
    /// Bounded by the number of distinct icons on screen, which is what the atlas is sized for.
    asked_bitmaps: std::collections::HashSet<i32>,
    /// The views on screen. A per-file question for a view that has gone is dropped by the
    /// worker before it costs anything — which is what makes changing folder cancel the old
    /// folder's questions rather than queue behind them.
    live: Arc<Mutex<std::collections::HashSet<u64>>>,
    /// To wake the window when an answer lands. See [`Self::nudge`].
    ctx: egui::Context,
}

impl Icons {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            ctx: ctx.clone(),
            kinds: HashMap::new(),
            places_seen: HashMap::new(),
            answers: Vec::new(),
            jobs: None,
            queued: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            bitmaps: 0,
            uploads: 0,
            // As many as the atlas has slots. Shared across folders, because an icon is not
            // folder-scoped: one bitmap serves every `.dll` on the machine.
            slots: Recent::new(ATLAS_SLOTS),
            atlas: None,
            next_slot: 0,
            pending: Arc::new(Mutex::new(std::collections::HashSet::new())),
            tx,
            rx,
            available: cfg!(windows),
            places: None,
            pixels: None,
            asked_bitmaps: std::collections::HashSet::new(),
            live: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }

    /// Take delivery of anything resolved since the last frame, and let go of anything
    /// nothing has asked about for a while.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(ready) = self.rx.try_recv() {
            match ready {
                Ready::Kind { key, index } => {
                    self.kinds.insert(key, index);
                }
                Ready::File { view, row, index } => self.answers.push((view, row, index)),
                Ready::Place { path, index } => {
                    self.places_seen.insert(path, index);
                }
                Ready::Bitmap { index, image } => {
                    // The upload is the only part of this that has to be here: it is a memcpy
                    // into a texture, with none of the shell behind it.
                    self.asked_bitmaps.remove(&index);
                    let slot = self.claim_slot(index);
                    self.blit(ctx, slot, &image);
                    self.uploads += 1;
                }
            }
        }
        // The atlas is one texture for the session and its slots are reused, so there is
        // nothing here to sweep.
    }

    /// File answers that arrived, for the views that asked. Drained.
    ///
    /// The application hands each to the tab whose `view` matches and drops the rest, which
    /// is what makes a folder you have left cost nothing: its answers are not stored
    /// anywhere on the way past.
    pub fn answers(&mut self) -> Vec<(u64, u32, i32)> {
        std::mem::take(&mut self.answers)
    }

    /// The icon for a file *type* — one answer shared by every file of that extension.
    ///
    /// Returns `None` on the first sighting of a type and asks about it; the row draws its
    /// painted glyph for a frame or two instead. That is the whole trade: never block a
    /// frame, and accept that a brand-new type is generic for an instant.
    pub fn kind(&mut self, ext: &str, is_dir: bool) -> Option<Icon> {
        if !self.available {
            return None;
        }
        let key = if is_dir {
            "\0dir"
        } else if ext.is_empty() {
            "\0none"
        } else {
            // The only allocation on this path, and only for a type never seen before.
            return self.kind_owned(ext.to_ascii_lowercase(), is_dir);
        };
        if let Some(&index) = self.kinds.get(key) {
            return Some(Icon { index });
        }
        self.request_kind(key.to_owned(), is_dir);
        None
    }

    fn kind_owned(&mut self, key: String, is_dir: bool) -> Option<Icon> {
        if let Some(&index) = self.kinds.get(&key) {
            return Some(Icon { index });
        }
        self.request_kind(key, is_dir);
        None
    }

    /// Whether a file of this extension carries an icon of its own.
    ///
    /// Public so the listing can decide whether a row is worth a per-file question before
    /// building the path to ask it with.
    pub fn is_per_file(ext: &str) -> bool {
        #[cfg(windows)]
        return has_own_icon(ext);
        #[cfg(not(windows))]
        {
            let _ = ext;
            false
        }
    }

    /// Ask for one file's own icon, to be delivered to `view` at `row`.
    ///
    /// The queue is capped rather than the requests deduplicated: a dropped question costs a
    /// row the generic icon for as long as it takes to come round again, and the alternative
    /// is a set of every path ever asked about — which is the per-file allocation this whole
    /// arrangement exists to avoid.
    pub fn request_file(&mut self, view: u64, row: u32, path: PathBuf) -> bool {
        if !self.available {
            return false;
        }
        const QUEUE_CAP: usize = 64;
        if self.queued.load(std::sync::atomic::Ordering::Relaxed) >= QUEUE_CAP {
            return false;
        }
        // **A view that is asking is live, by definition, and saying so here is what makes the
        // question survive the frame it was asked in.**
        //
        // The worker drops a job whose view is not in [`Self::only`]'s set — that is what makes
        // leaving a folder cancel its questions instead of queueing behind them. But `only` is
        // published near the top of the frame and a listing is applied further down it, so on the
        // frame a folder's listing lands, the view the rows are about to ask with does not yet
        // exist in that set. Every per-file question in that frame was therefore thrown away by the
        // worker — and thrown away *silently*, because `request_file` had already answered `true`
        // and the caller had already marked the row as asked. A row holding `ASKED` is never asked
        // again, so no executable, shortcut or `.dll` in a freshly read folder ever got its own icon.
        //
        // Registering it here rather than reordering the frame: the frame's order is not this
        // module's to depend on, and "asking about a view keeps it alive" is true whatever that
        // order becomes. `only` still prunes it the moment no tab holds it.
        if let Ok(mut live) = self.live.lock() {
            live.insert(view);
        }
        let jobs = self.worker().clone();
        self.queued
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if jobs.send(Job::File { view, row, path }).is_err() {
            self.queued
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            return false;
        }
        true
    }

    /// The icon for a *place* — a sidebar row rather than a listing row.
    ///
    /// Different from [`Icons::lookup`] in two ways that matter, and both are the reason
    /// the sidebar cannot just use it:
    ///
    /// - **The answer is per path, never per type.** The whole point is that Downloads
    ///   does not look like a folder, and it only does not because the shell is allowed
    ///   to read that folder's own `desktop.ini`. Asking about the *type* "directory"
    ///   would give the generic folder for every row.
    /// - **A place need not be a file at all.** The Recycle Bin and This PC are shell
    ///   namespace items with no path behind them; those go through a PIDL.
    ///
    /// There are a dozen or so of these for a session, all resolved once, so the
    /// per-path cost the listing goes to such lengths to avoid is not a cost here.
    pub fn place(&mut self, path: &Path) -> Option<Icon> {
        if !self.available {
            return None;
        }
        if let Some(&index) = self.places_seen.get(path) {
            return Some(Icon { index });
        }
        self.request_place(path);
        None
    }

    /// Where an icon lives in the atlas: the one texture, and the patch of it to draw.
    ///
    /// **One texture for every shell icon in the window**, which is not an optimisation but a
    /// correctness fix. egui begins a new draw call whenever the texture changes between
    /// primitives, and `egui_glow` leaks memory per draw call — megabytes a second at 60fps,
    /// reproduced with none of this program in the frame by `examples/spin.rs`. A texture per
    /// icon meant a frame's primitive stream broke at every distinct icon on screen; a single
    /// atlas means it breaks once.
    ///
    /// A slot is 16×16, the size of the shell's small image list, in a 32×8 grid — 256 of
    /// them, which is more distinct icons than a window has ever shown at once. Slots are
    /// reused least-recently-drawn first, so a long session overwrites rather than grows, and
    /// the texture is one 512×128 upload for the life of the process.
    /// Returns `None` until the bitmap has arrived, the same as [`Icons::kind`] does for a type
    /// never seen before: the caller draws its painted glyph for a frame or two. That is not a
    /// nicety — pulling the bitmap out of the image list is a shell call that reaches the
    /// network, and doing it here is what froze the window for two and a half seconds on a
    /// mapped share. See [`Ready::Bitmap`].
    pub fn uv(&mut self, ctx: &egui::Context, icon: Icon) -> Option<(egui::TextureId, egui::Rect)> {
        let _ = ctx;
        let slot = match self.slots.get(&icon.index) {
            Some(&slot) => slot,
            None => {
                self.request_bitmap(icon.index);
                return None;
            }
        };
        let atlas = self.atlas.as_ref()?;
        Some((atlas.id(), Self::slot_uv(slot)))
    }

    /// Ask for one icon's pixels, once.
    ///
    /// Once per index for the life of the process, including when the answer never comes: an
    /// index the shell cannot produce a bitmap for sends nothing back and stays in
    /// `asked_bitmaps`, so the row keeps its painted glyph rather than asking again on every
    /// frame forever. On a share that is the difference between one failed extraction and one
    /// per frame.
    fn request_bitmap(&mut self, index: i32) {
        if !self.available || !self.asked_bitmaps.insert(index) {
            return;
        }
        self.bitmaps += 1;
        if self.pixels().send(Wanted(index)).is_err() {
            self.asked_bitmaps.remove(&index);
        }
    }

    /// Which views are on screen, so the worker can drop questions from the ones that are not.
    ///
    /// Called every frame. Changing folder leaves a queue of per-file questions about files
    /// nothing is showing any more, and each of them is a blocking shell call — on a network
    /// share, a slow one. Skipping them is the difference between a new folder's icons arriving
    /// now and arriving after the old folder's have all been answered.
    pub fn only(&mut self, views: &[u64]) {
        if let Ok(mut live) = self.live.lock() {
            if live.len() == views.len() && views.iter().all(|view| live.contains(view)) {
                return;
            }
            live.clear();
            live.extend(views.iter().copied());
        }
    }

    /// The next slot for an icon: an unused one, or the one drawn longest ago.
    fn claim_slot(&mut self, index: i32) -> u32 {
        let slot = if (self.next_slot as usize) < ATLAS_SLOTS {
            let slot = self.next_slot;
            self.next_slot += 1;
            slot
        } else {
            // Full: take the slot of the icon nobody has drawn for the longest, and let its
            // owner ask again if it reappears.
            let victim = self
                .slots
                .items
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, (slot, _))| (*key, *slot));
            match victim {
                Some((key, slot)) => {
                    self.slots.items.remove(&key);
                    slot
                }
                // Cannot happen: the grid is full, so something is in it.
                None => 0,
            }
        };
        self.slots.insert(index, slot);
        slot
    }

    /// Copy one icon into its slot, creating the atlas on first use.
    fn blit(&mut self, ctx: &egui::Context, slot: u32, image: &ColorImage) {
        let atlas = self.atlas.get_or_insert_with(|| {
            ctx.load_texture(
                "shell-icons",
                ColorImage::filled(
                    [ATLAS_COLUMNS * CELL, ATLAS_ROWS * CELL],
                    egui::Color32::TRANSPARENT,
                ),
                // Linear, because a 16px icon drawn at 15.8 device pixels on a scaled
                // display is the normal case and nearest would shimmer.
                TextureOptions::LINEAR,
            )
        });
        // Padded into the cell rather than scaled: the shell's small list is 16×16, and a
        // bitmap of another size is rare enough that clipping it beats resampling every one.
        let mut cell = ColorImage::filled([CELL, CELL], egui::Color32::TRANSPARENT);
        let [w, h] = image.size;
        for y in 0..h.min(CELL) {
            for x in 0..w.min(CELL) {
                cell[(x, y)] = image[(x, y)];
            }
        }
        let (column, row) = ((slot as usize) % ATLAS_COLUMNS, (slot as usize) / ATLAS_COLUMNS);
        atlas.set_partial([column * CELL, row * CELL], cell, TextureOptions::LINEAR);
    }

    /// The patch of the atlas a slot occupies, in 0..1 texture coordinates.
    fn slot_uv(slot: u32) -> egui::Rect {
        let (column, row) = ((slot as usize) % ATLAS_COLUMNS, (slot as usize) / ATLAS_COLUMNS);
        let (w, h) = (
            1.0 / ATLAS_COLUMNS as f32,
            1.0 / ATLAS_ROWS as f32,
        );
        egui::Rect::from_min_size(
            egui::pos2(column as f32 * w, row as f32 * h),
            egui::vec2(w, h),
        )
    }

    /// How many distinct icons are held. Read by the tests, which is how the
    /// cache-once-per-type promise is checked.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.kinds.len() + self.places_seen.len()
    }

    /// What the cache is holding: known types, known paths, and uploaded textures.
    ///
    /// For `--trace`. A texture is the expensive one — it is a GL object with the driver's
    /// own per-object overhead behind it, which is invisible to a Rust allocator counter and
    /// is exactly the kind of thing that turns "memory grows as I browse" into a number
    /// nobody can find.
    pub fn held(&self) -> (usize, usize, usize) {
        (self.kinds.len(), self.places_seen.len(), self.slots.len())
    }

    fn request_kind(&mut self, key: String, is_dir: bool) {
        // One type is asked about once, and there are a few dozen of them in a session, so
        // this set is the one place a claim is still worth keeping.
        if !self.claim(&key) {
            return;
        }
        let jobs = self.worker();
        let _ = jobs.send(Job::Kind { key, is_dir });
    }

    fn request_place(&mut self, path: &Path) {
        if !self.claim(&place_key(path)) {
            return;
        }
        let worker = self
            .places
            .get_or_insert_with(|| {
                place_worker(self.tx.clone(), self.pending.clone(), self.ctx.clone())
            });
        let _ = worker.send(path.to_path_buf());
    }

    /// The worker that answers type and file lookups, started on first use.
    ///
    /// One thread for the session. It used to be one thread per question — 4,910 of them for
    /// a folder of that many executables, each with a stack of its own — which is both the
    /// spike this program was accused of and pure waste: the work is a blocking syscall, and
    /// a queue in front of one thread serialises it just as well.
    fn worker(&mut self) -> &Sender<Job> {
        let tx = self.tx.clone();
        let pending = self.pending.clone();
        let queued = self.queued.clone();
        let live = self.live.clone();
        // Its own clone: the closure outlives this borrow. See [`nudge`] for why a worker that does
        // not ask for a frame is a worker whose answers are not drawn.
        let ctx = self.ctx.clone();
        self.jobs.get_or_insert_with(|| {
            let (send, receive) = channel::<Job>();
            let _ = std::thread::Builder::new()
                .name("shell-icons".to_owned())
                .spawn(move || {
                    // Any lookup can touch an empty removable drive, and the syscall would
                    // otherwise raise "Please insert a disk into drive E:" from inside it.
                    crate::fs::scan::silence_device_dialogs();
                    while let Ok(job) = receive.recv() {
                        match job {
                            Job::Kind { key, is_dir } => {
                                // A representative name of that type. The shell is told not
                                // to look at the file, so nothing of this name need exist.
                                let sample = if is_dir {
                                    PathBuf::from("C:\\folder")
                                } else if key.starts_with('\0') {
                                    PathBuf::from("C:\\file")
                                } else {
                                    PathBuf::from(format!("C:\\file.{key}"))
                                };
                                if let Some(index) = index_of(&sample, is_dir, true) {
                                    let _ = tx.send(Ready::Kind {
                                        key: key.clone(),
                                        index,
                                    });
                                    nudge(&ctx);
                                }
                                if let Ok(mut pending) = pending.lock() {
                                    pending.remove(&key);
                                }
                            }
                            Job::File { view, row, path } => {
                                // Nothing is showing that view any more: the folder was left
                                // while this was in the queue. Dropped here rather than
                                // answered and discarded, because the answering is the
                                // expensive part — it opens the file, and on a network share
                                // that is the whole of the cost.
                                let wanted = live
                                    .lock()
                                    .map(|live| live.contains(&view))
                                    .unwrap_or(true);
                                if wanted {
                                    // Not `use_attributes`: the whole point of this branch is
                                    // that the icon is inside the file, so it has to be opened.
                                    if let Some(index) = index_of(&path, false, false) {
                                        let _ = tx.send(Ready::File { view, row, index });
                                        nudge(&ctx);
                                    }
                                }
                                queued.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                });
            send
        })
    }

    /// The thread that pulls bitmaps out of the shell's image list, started on first use.
    ///
    /// Its own thread, and its own apartment. `SHGetImageList` hands back a COM interface, so
    /// this needs one — and it is entered once for the life of the thread rather than around
    /// each call, for the reason set out on [`place_worker`]: the last `CoUninitialize` in a
    /// process frees shell state other threads are still using.
    fn pixels(&mut self) -> &Sender<Wanted> {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.pixels.get_or_insert_with(|| {
            let (send, receive) = channel::<Wanted>();
            let _ = std::thread::Builder::new()
                .name("shell-bitmaps".to_owned())
                .spawn(move || {
                    crate::shell::init();
                    crate::fs::scan::silence_device_dialogs();
                    while let Ok(Wanted(index)) = receive.recv() {
                        if let Some(image) = bitmap(index) {
                            let _ = tx.send(Ready::Bitmap { index, image });
                            // See [`nudge`]: the bitmap is what a row actually draws, so an answer
                            // nothing comes to collect is a generic glyph on screen.
                            nudge(&ctx);
                        }
                    }
                });
            send
        })
    }

    /// Reserve a key, returning whether this caller got it.
    fn claim(&self, key: &str) -> bool {
        self.pending
            .lock()
            .map(|mut pending| pending.insert(key.to_owned()))
            .unwrap_or(false)
    }
}

/// Ask for a frame, because an answer that lands with nothing to draw it is not an answer.
///
/// **This was missing, and these were the one background service in the program with no way to wake
/// the window.** Every sibling does it — the loader, git, sizes, thumbs, links, the preview and the
/// console all book a repaint when they have something. The case it shows in is Back, or re-entering
/// a folder: that takes the loader's synchronous cache path, so the frame is drawn without anything
/// having been asked for asynchronously and nothing books a repaint. The rows then ask for their
/// per-file icons *during* that frame, the answers arrive a few milliseconds later, and every `.exe`
/// and `.lnk` kept its generic type glyph until the pointer happened to move.
///
/// A free function rather than a method on [`Icons`], because the callers are the worker threads:
/// each holds a `Context` of its own and none of them can reach the `Icons` that started it.
///
/// Delayed rather than immediate, as `crate::shell::thumbs::Thumbs::nudge` does it: answers arrive
/// in bursts of up to sixty-four, and one frame after the burst is worth sixty-four during it.
fn nudge(ctx: &egui::Context) {
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

/// The key a place lookup is claimed under.
///
/// Apart from the key space [`Icons::request_path`] uses, so a folder that is both a place
/// and a listed file cannot answer one question with the other's icon.
fn place_key(path: &Path) -> String {
    format!("\0place\0{}", path.to_string_lossy())
}

/// The thread that answers place lookups, with an apartment of its own that lasts.
///
/// **Why a thread at all.** `SHParseDisplayName` — the only way to reach something that is
/// not a file, which This PC and the Recycle Bin both are — goes through the desktop's
/// `IShellFolder`, and on a thread with no apartment it fails with `CO_E_NOTINITIALIZED`.
/// That failure is silent and indistinguishable from "the shell has no icon for this", which
/// is how those two rows came to draw their painted glyph while every real folder beside
/// them had the shell's icon. Every other lookup here calls `SHGetFileInfoW`, which is
/// documented as needing no apartment, and gets none.
///
/// **Why one thread, and why it never uninitialises.** The obvious fix — COM around each
/// lookup on its own short-lived thread — is worse than the bug: the *last*
/// `CoUninitialize` in a process frees shell state other threads are still using, and it
/// showed up at once as `SHGetFileInfoW` on an unrelated thread returning nothing. So the
/// apartment is entered once, on a thread that outlives the requests, exactly as
/// [`crate::shell::Modal`] does.
fn place_worker(
    tx: Sender<Ready>,
    pending: Arc<Mutex<std::collections::HashSet<String>>>,
    ctx: egui::Context,
) -> Sender<PathBuf> {
    let (send, requests) = channel::<PathBuf>();
    let spawned = std::thread::Builder::new()
        .name("shell-place".to_owned())
        .spawn(move || {
            crate::shell::init();
            // Ends when the `Icons` holding the sender goes away.
            while let Ok(path) = requests.recv() {
                if let Some(index) = index_of_place(&path) {
                    let _ = tx.send(Ready::Place {
                        path: path.clone(),
                        index,
                    });
                    // See [`nudge`]. The sidebar's own rows are what this answers for, and they sat
                    // with a painted glyph until unrelated input produced a frame.
                    nudge(&ctx);
                }
                if let Ok(mut pending) = pending.lock() {
                    pending.remove(&place_key(&path));
                }
            }
        });
    let _ = spawned;
    send
}

// ---------------------------------------------------------------------------
// Everywhere else
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn index_of(_path: &Path, _is_dir: bool, _use_attributes: bool) -> Option<i32> {
    None
}

#[cfg(not(windows))]
fn index_of_place(_path: &Path) -> Option<i32> {
    None
}

#[cfg(not(windows))]
fn bitmap(_index: i32) -> Option<ColorImage> {
    None
}

#[cfg(all(test, windows))]
mod tests;
