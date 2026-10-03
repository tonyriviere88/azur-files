//! Drawing the context menu.
//!
//! The content is the shell's — see [`crate::shell::menu`] — but the menu itself is
//! this program's: Azur's `MenuItem` on Azur's popover surface, in the same palette as
//! everything else, with the same focus ring and the same row states. A native
//! `TrackPopupMenu` would have been less work and would have looked like a different
//! application had opened.
//!
//! # Position
//!
//! At the pointer, and flipped rather than clipped when it would run off an edge —
//! which is why the size is computed before anything is drawn rather than measured
//! after. An `egui::Area` sizes itself to its content, so a menu that discovered its
//! own height a frame late would appear in the wrong place and then jump.
//!
//! # Submenus
//!
//! One [`egui::Area`] per open level, each anchored to the item that opened it, opening
//! to the right unless there is no room. `open` is the chain of indices currently
//! showing, so the whole state of an arbitrarily deep menu is one `Vec<usize>`.
//!
//! A shell submenu arrives empty — filling it means asking an extension, which is slow
//! enough to be worth not doing until somebody opens it. So opening one puts its id in
//! [`Open::fills`], and the level appears when the answer does. See
//! [`crate::shell::menu`].
//!
//! # A level that scrolls, and the part of it that does not
//!
//! A machine with a dozen shell extensions installed has a menu taller than the window, so
//! each level's rows go in a scroll area — without which the entries past the edge would
//! simply be unreachable. Two things follow from that, and both are decided here rather than
//! left to `egui`: `Properties` and this program's `Copy path(s)` beside it are drawn *below* the
//! scrolling part so they are always the last thing on the menu (see [`pinned_from`]), and a level
//! goes back to the top on the frame it appears rather than inheriting whatever the last menu was
//! left scrolled to (see [`Open::shown`]).
//!
//! # Nothing here reflows
//!
//! The shell's half of the menu takes between a sixth of a second and most of a second to
//! arrive, and none of that waiting happens on screen: [`Open`] is not built until the
//! entries are all there, so the menu appears once, at its final size, and never grows
//! under the pointer. An earlier version opened immediately with this program's own entries
//! and a `Loading…` row and let the shell's land underneath — which kept the window
//! responsive and did not read as a context menu at all.

use std::collections::HashMap;

use azur_egui_theme::components::{menu_divider, popover_frame, MenuItem};
use azur_egui_theme::tokens::{radius, space};
use egui::{pos2, vec2, Color32, Id, Order, Pos2, Rect, TextureHandle, Ui, Vec2};

use crate::shell::menu::{Command, Entry, Kind, Moves, Own};
use crate::theme::Theme;

/// A menu on screen.
pub struct Open {
    /// The pane it was raised from.
    pub pane: crate::pane::PaneId,
    /// Where the pointer was, in points.
    pub at: Pos2,
    /// What it is for. Empty means the folder's own menu.
    pub items: Vec<std::path::PathBuf>,
    pub folder: std::path::PathBuf,
    /// What is drawn: [`Open::raw`] banded by `regroup` with this program's own entries put in.
    /// Rebuilt by [`Open::arrange`] and never assigned anywhere else.
    pub entries: Vec<Entry>,
    /// The shell's own entries, flat, exactly as they were read.
    ///
    /// Kept for the whole life of the menu so that a right click can rearrange it without asking
    /// the shell anything — which is the difference between a rearrangement that happens on the
    /// click and one that takes the sixth of a second to most of a second the module header
    /// measures. It is also the only copy that is still in the shell's own order, and so the only
    /// thing `regroup` can be re-run against: its output is not its own input.
    raw: Vec<Entry>,
    /// Who registered each verb, for naming the groups. From the builder thread; see
    /// [`crate::shell::menu::Handlers`].
    handlers: crate::shell::menu::Handlers,
    /// Whether this program's Paste goes on this menu enabled — a background menu with files on the
    /// clipboard. Remembered rather than re-read so that [`Open::arrange`] does not take the
    /// desktop's one clipboard on every right click.
    can_paste: bool,
    /// How much of a menu this is, carried through so that a command chosen here is resolved
    /// against a menu built the same way. See [`crate::shell::menu::invoke`].
    pub depth: crate::shell::menu::Depth,
    /// Which submenu chain is showing, as indices from the root.
    pub open: Vec<usize>,
    /// The keyboard highlight, as a path from the root.
    pub cursor: Option<Vec<usize>>,
    /// Which tile of the tile row the keyboard is on, when [`Open::cursor`] is on that row.
    ///
    /// Beside the cursor rather than inside it — a path of one index further down would have been
    /// the obvious thing — because the tile row is one *row*: Up and Down step over it as a unit,
    /// and every piece of code that reads `cursor` as "an index into this level" would otherwise
    /// have had to learn about a path that is one longer than the level it is in. Left and Right are
    /// the only keys that touch this.
    tile: usize,
    /// Which build of the shell's menu this is; see [`crate::shell::menu::Builder`].
    pub token: u64,
    /// Submenus the shell should be asked to fill, for the caller to drain and send on.
    pub fills: Vec<u32>,
    /// Submenus already asked about, so a hover asks once rather than once a frame.
    asked: std::collections::HashSet<u32>,
    /// Item bitmaps, uploaded once and keyed by the path to their entry.
    textures: HashMap<Vec<usize>, TextureHandle>,
    /// Set on the frame it opens, so the click that opened it does not also close it.
    fresh: bool,
    /// The keyboard is driving, so a pointer merely resting on the menu does not undo it.
    ///
    /// Cleared by the first real pointer movement — see [`show`], where it is set and where it
    /// gates the hover.
    by_key: bool,
    /// The levels that were on screen last frame, by path.
    ///
    /// What it is for is the scroll offset. A `ScrollArea` keeps its offset in `egui`'s memory
    /// under an id that outlives the menu, so a right click, a scroll to the bottom and a
    /// dismissal left the *next* menu opening halfway down itself — showing `Properties` where
    /// its first entry should be. A level absent from this set is a level appearing this frame,
    /// and it is put back to the top. Everything after that frame is the user's own scrolling
    /// and is left alone.
    ///
    /// Per level rather than per menu, because sibling submenus share one scroll id: hovering a
    /// long `Open with` and then a short `Send to` inherited the first one's offset.
    shown: std::collections::HashSet<Vec<usize>>,
    /// What the root level actually took on screen, last frame.
    ///
    /// Kept because it is the one thing worth asserting about a menu that lays itself out
    /// by arithmetic: it has to come out the size it said it would. See the test.
    pub drawn: Vec2,
    /// And what its rows were scrolled by, for the same reason: a menu opens at its first
    /// entry, and the only way to know it did is to look. See the test.
    pub scrolled: f32,
}

impl Open {
    /// A menu whose entries are already exactly what should be drawn.
    ///
    /// Which is every menu that did not come from the shell: a right-button drop's four entries are
    /// this program's own and there is nothing to band. A shell menu is this followed by
    /// [`Open::banded`].
    pub fn new(
        pane: crate::pane::PaneId,
        at: Pos2,
        items: Vec<std::path::PathBuf>,
        folder: std::path::PathBuf,
        entries: Vec<Entry>,
        depth: crate::shell::menu::Depth,
        token: u64,
    ) -> Self {
        Self {
            pane,
            at,
            items,
            folder,
            raw: entries.clone(),
            entries,
            handlers: crate::shell::menu::Handlers::new(),
            can_paste: false,
            depth,
            open: Vec::new(),
            cursor: None,
            tile: 0,
            token,
            fills: Vec::new(),
            asked: std::collections::HashSet::new(),
            textures: HashMap::new(),
            fresh: true,
            by_key: false,
            shown: std::collections::HashSet::new(),
            drawn: Vec2::ZERO,
            scrolled: 0.0,
        }
    }

    /// The same menu, banded: Windows' flat list turned into the arrangement this program shows.
    ///
    /// Chained onto [`Open::new`] by the one caller that has a shell menu to show —
    /// `crate::app::App::pump_menu`. Everything it needs that a plain `new` has no use for goes in
    /// here rather than into seven more parameters on a constructor that five other places call.
    ///
    /// `can_paste` is passed in rather than read because reading it takes the desktop's one
    /// clipboard; see the caller.
    pub fn banded(
        mut self,
        handlers: crate::shell::menu::Handlers,
        can_paste: bool,
        moves: &Moves,
    ) -> Self {
        self.handlers = handlers;
        self.can_paste = can_paste;
        self.arrange(moves);
        self
    }

    /// Band the shell's entries and put this program's own two in.
    ///
    /// **The one place a menu's contents are decided**, called by [`Open::new`] and again by every
    /// right click that moves an entry. Two code paths that each arranged the menu their own way is
    /// exactly how the menu you get by opening one and the menu you get by rearranging one would
    /// come to differ, and the second is the one nobody would test.
    ///
    /// Every path into this menu changes when it is called — the indices the open chain, the
    /// keyboard cursor, the texture keys and the scroll memory are all made of shift as soon as an
    /// entry moves between levels — so all of them are dropped. `asked` is *not*: a submenu's fill
    /// id is the shell's own and rearranging the menu does not change which submenu it names, so
    /// keeping it is what stops an already-filled `Send to` from being asked for a second time.
    pub fn arrange(&mut self, moves: &Moves) {
        let entries = crate::shell::menu::regroup(self.raw.clone(), &self.handlers, moves);
        // An empty selection is the folder's *background* menu, and that is the one menu the shell
        // hands over with a gap in it — no Paste. See `crate::shell::menu::Own::Paste`.
        let entries = if self.items.is_empty() {
            crate::shell::menu::with_our_paste(entries, self.can_paste)
        } else {
            entries
        };
        // And `Copy path(s)`, on both menus, just above Properties.
        self.entries = crate::shell::menu::with_our_copy_paths(entries);

        self.open.clear();
        self.cursor = None;
        self.tile = 0;
        self.textures.clear();
        self.shown.clear();
    }

    /// The entry at a path, if there is one.
    ///
    /// Walks into a tile row as well as into a submenu, so a tile has a path like everything else —
    /// which is what lets the texture map and the keyboard cursor address one. [`Open::level`] does
    /// *not*, and the difference is deliberate: a tile row is a row and not a level, and a version
    /// of this that let one be opened as a level would hang an `egui::Area` off it.
    fn entry(&self, path: &[usize]) -> Option<&Entry> {
        let mut level = &self.entries;
        for (depth, index) in path.iter().enumerate() {
            let entry = level.get(*index)?;
            if depth + 1 == path.len() {
                return Some(entry);
            }
            match &entry.kind {
                Kind::Submenu { children, .. } | Kind::Tiles(children) => level = children,
                _ => return None,
            }
        }
        None
    }

    /// The entries at a level — a level being something that gets an `egui::Area` of its own.
    ///
    /// A tile row is not one; see [`Open::children`] for the walk that includes it.
    fn level(&self, path: &[usize]) -> Option<&Vec<Entry>> {
        if path.is_empty() {
            return Some(&self.entries);
        }
        match self.entry(path)?.kind {
            Kind::Submenu { ref children, .. } => Some(children),
            _ => None,
        }
    }

    /// Whatever entries hang off a path, tile rows included.
    ///
    /// For the walks that are about *entries* rather than about levels — uploading the item
    /// bitmaps, and nothing else so far. A tile's icon has to be uploaded like any other, and it is
    /// the one child that [`Open::level`] deliberately will not hand over.
    fn children(&self, path: &[usize]) -> Option<&Vec<Entry>> {
        if path.is_empty() {
            return Some(&self.entries);
        }
        match self.entry(path)?.kind {
            Kind::Submenu { ref children, .. } | Kind::Tiles(ref children) => Some(children),
            _ => None,
        }
    }

    /// One submenu's contents have arrived.
    ///
    /// An empty answer is a real answer — the extension had nothing — so the row stays where
    /// it is and stops being usable, rather than vanishing from under the pointer.
    pub fn filled(&mut self, id: u32, children: Vec<Entry>) {
        fn put(entries: &mut [Entry], id: u32, children: &mut Option<Vec<Entry>>) {
            for entry in entries.iter_mut() {
                if entry.kind.unasked() == Some(id) {
                    if let Some(children) = children.take() {
                        entry.enabled = !children.is_empty();
                        entry.kind = Kind::complete(children);
                    }
                    return;
                }
                if let Kind::Submenu { children: deeper, .. } = &mut entry.kind {
                    put(deeper, id, children);
                    if children.is_none() {
                        return;
                    }
                }
            }
        }
        // Into the arrangement on screen, so the submenu the user is hovering appears on this frame.
        put(&mut self.entries, id, &mut Some(children.clone()));
        // And into the shell's own copy, which is what [`Open::arrange`] rebuilds from. Without
        // this, moving an entry would throw away every submenu that had been filled and the shell
        // would be asked for them all over again — up to a tenth of a second each for `Open with`.
        put(&mut self.raw, id, &mut Some(children));
    }

    /// Ask for the open submenu's contents, once.
    fn ask(&mut self) {
        if self.open.is_empty() {
            return;
        }
        let Some(id) = self.entry(&self.open.clone()).and_then(|e| e.kind.unasked()) else {
            return;
        };
        if self.asked.insert(id) {
            self.fills.push(id);
        }
    }
}

/// What happened to the menu this frame.
pub enum Outcome {
    /// Still open.
    Open,
    /// Dismissed without choosing anything.
    Closed,
    /// This was chosen.
    Chose(Command),
    /// A right click asked for these entries to be moved between their group and the main menu.
    ///
    /// The menu stays open and is rearranged in place — see `crate::app::App::draw_menu`. Several
    /// keys because a right click on a group's own row moves everything in it, which is how a group
    /// that should not have been collapsed is undone in one gesture rather than five.
    Move {
        keys: Vec<String>,
        /// Where they are going. `false` is out onto the main menu.
        into_group: bool,
    },
}

/// One row's height, from the design system rather than from arithmetic repeated here.
///
/// It has to be the real number: [`measure`] adds these up to place the menu *before*
/// anything is drawn, so a value that disagrees with what `MenuItem` allocates puts the
/// menu in the wrong place — the taller the menu, the further out.
fn row_height() -> f32 {
    azur_egui_theme::components::menu_item_height()
}

/// A separator's, asked for rather than derived — the same reason [`row_height`] is.
///
/// This one was `space::S2 * 2.0 + 1.0`, which is what the divider allocated when it was
/// written; the design system has since tightened it to `space-1` either side, and every menu
/// with a divider in it was being placed four points out per divider. The test at the bottom
/// of this file is what caught it.
fn separator_height() -> f32 {
    azur_egui_theme::components::menu_divider_height()
}

/// How tall shell32's block is when it is drawn as one row of tiles: an icon over a caption, with
/// `space-2` above and below and `space-1` between the two.
///
/// Stated here beside [`draw_tiles`], which allocates it, for the reason [`separator_height`]
/// carries at length: [`measure`] adds these up to place the menu before anything is drawn, so a
/// height derived anywhere else drifts the moment the drawing changes and puts the whole menu out.
fn tile_row_height() -> f32 {
    space::S2 * 2.0 + TILE_ICON + space::S1 + azur_egui_theme::tokens::typography::LINE_CAPTION
}

/// A tile's icon, at the size the shell's `hbmpItem` actually is.
///
/// 16 and not the 20-something Windows 11 draws, because a menu item bitmap is a 16×16 bitmap and
/// there is nothing else to have: scaling it up would blur the one part of this row that has to
/// read as Windows' own icon.
const TILE_ICON: f32 = 16.0;

/// What a run of entries takes, rows and dividers alike.
///
/// The one place the heights are added up, so [`measure`] and the split between the
/// scrolling part of a level and its pinned tail cannot disagree about what a level is worth.
fn stack_height(entries: &[Entry]) -> f32 {
    entries
        .iter()
        .map(|entry| match entry.kind {
            Kind::Separator => separator_height(),
            Kind::Tiles(_) => tile_row_height(),
            _ => row_height(),
        })
        .sum()
}

/// Where a level's pinned tail starts: the first entry drawn *below* the scroll area rather
/// than inside it. `entries.len()` when there is nothing to pin, which is every submenu and
/// every menu of this program's own entries.
///
/// **Properties is the entry that has to be reachable without scrolling.** It is where a
/// shell menu ends, it is what people go to the bottom of one *for*, and on a machine with a
/// dozen extensions installed the bottom of the menu is past the edge of the window — so the
/// entry with the furthest to scroll to is the one most often wanted. Pinned, it is always the
/// last thing on the menu whatever the scroll is doing above it.
///
/// **Which row Properties is** is not decided here. [`crate::shell::menu::properties_at`] says, by
/// verb and never by label, and the code that puts an entry beside it asks the same function — so the
/// two cannot come to different answers.
///
/// **Nothing is moved *here*.** The tail is a suffix of the entries in whatever order they arrive
/// in, so if an extension has put something below Properties it is pinned too.
///
/// This used to go on to say that lifting an entry out of the middle of the menu and re-hanging it
/// somewhere else would show the menu in an order Explorer does not, which is worse than a menu that
/// scrolls. **That has been reversed, deliberately, one layer up**:
/// [`crate::shell::menu::regroup`] now bands the menu and hoists `openas` to the second row. The
/// argument that changed is that a 45-row menu whose useful half is below the fold is not "Explorer's
/// order" in any sense that helps anybody, and Windows 11's own menu hoists the same entry. What is
/// still true is that *this* function moves nothing: it decides where a suffix begins.
///
/// **This program's own `Copy path(s)` comes with it**, because that is where it was put — between
/// Properties and the divider above them both, see [`crate::shell::menu::with_our_copy_paths`]. A
/// tail that began at Properties would leave the one entry in the menu that is *this program's*
/// scrolling away above a pinned row, which is the opposite of what pinning is for.
///
/// The divider above them comes too. It belongs to what is below rather than to whatever is
/// above — left in the scrolling part it would slide away and leave a pinned row sitting under
/// the last of the entries with no rule between them.
fn pinned_from(entries: &[Entry]) -> usize {
    // Properties, or — on a menu the shell gave none for — the last row, when that is the entry
    // `with_our_copy_paths` put there instead. `Copy path(s)` being reachable without scrolling
    // should not rest on the shell having offered a Properties to hang it off.
    let anchor = crate::shell::menu::properties_at(entries).or_else(|| {
        entries
            .len()
            .checked_sub(1)
            .filter(|&at| ours_beside_properties(&entries[at]))
    });
    // The tile row is pinned whether or not there is a Properties to hang it off, and it is the
    // *reason* to pin on a menu that has one: Cut, Copy, Rename and Delete are what people go to a
    // context menu for, and the whole point of the row is that they are one glance away rather than
    // one scroll. It sits above `link`, `Copy path(s)` and Properties — see
    // `crate::shell::menu::regroup` — so it is the earliest row the tail can begin at.
    let tiles = crate::shell::menu::tiles_at(entries);
    let Some(index) = anchor.or(tiles) else {
        return entries.len();
    };
    // Ours first, then the tile row above them, then the divider above the lot.
    let mut from = index;
    while from > 0 && ours_beside_properties(&entries[from - 1]) {
        from -= 1;
    }
    if let Some(tiles) = tiles {
        from = from.min(tiles);
    }
    if from > 0 && matches!(entries[from - 1].kind, Kind::Separator) {
        from - 1
    } else {
        from
    }
}

/// Whether this is one of this program's own entries that belongs to the pinned tail.
///
/// Named rather than "any [`Command::Own`]", because most of them are not: `Copy here`, `Move here`
/// and `Cancel` are the whole of a right-button drop's menu, which has no Properties and nothing to
/// pin, and [`Own::Paste`] goes at the *top* of a background menu.
fn ours_beside_properties(entry: &Entry) -> bool {
    matches!(entry.kind, Kind::Command(Command::Own(Own::CopyPaths)))
}

/// Draw the menu and every open submenu.
pub fn show(ui: &mut Ui, t: &Theme, menu: &mut Open) -> Outcome {
    let ctx = ui.ctx().clone();

    // ---- Keyboard ------------------------------------------------------
    //
    // Read before the levels are drawn, because a key changes what is about to be drawn — but
    // only *returned* from when the key ends the menu. A key that merely moves the cursor or
    // opens a submenu falls through and the same frame draws the menu in its new state.
    //
    // It used to return `Outcome::Open` here, which left a frame with nothing drawn on it: an
    // arrow key blinked the whole menu out and back, and an arrow held down strobed it.
    let keys = keyboard(&ctx, menu);
    if let Keys::Done(outcome) = keys {
        return outcome;
    }
    // Who is in charge of the open chain, decided before the rows are drawn and read again
    // below, once the hover is known. A key this frame takes it from the pointer; a pointer
    // that actually moved takes it back.
    if matches!(keys, Keys::Moved) {
        menu.by_key = true;
    } else if ctx.input(|i| i.pointer.delta() != Vec2::ZERO) {
        menu.by_key = false;
    }

    // Textures for the shell's item bitmaps, uploaded once each.
    upload_icons(&ctx, menu, &mut Vec::new());

    // A submenu is empty until the extension that owns it is asked, and asking is the
    // expensive part — so it happens on the hover that opens it.
    menu.ask();

    let screen = ctx.viewport_rect();
    let mut out = Out::default();
    let mut hovered_any = false;

    // Levels are drawn root-first, each anchored to the item that opened it.
    let depth = menu.open.len();
    let mut anchor = Rect::from_min_size(menu.at, Vec2::ZERO);
    // The levels this pass puts on screen, which becomes `Open::shown` at the end of it — so a
    // level that was not there last frame is one appearing now, and gets its scroll reset.
    let mut on_screen: Vec<Vec<usize>> = Vec::new();
    for level_depth in 0..=depth {
        let path: Vec<usize> = menu.open[..level_depth].to_vec();
        let Some(entries) = menu.level(&path).cloned() else {
            break;
        };
        // A level with nothing in it is either a submenu still being filled or one that
        // turned out to be empty; either way there is nothing to hang an area on.
        if entries.is_empty() {
            break;
        }

        let size = measure(&ctx, t, &entries, screen);
        let origin = place(anchor, size, screen, level_depth == 0);
        let appearing = !menu.shown.contains(&path);
        on_screen.push(path.clone());
        // Where the scrolling part of this level ends and its pinned tail begins.
        let pin = pinned_from(&entries);
        let pinned_height = stack_height(&entries[pin..]);

        let response = egui::Area::new(Id::new(("shell-menu", level_depth)))
            .order(Order::Foreground)
            .fixed_pos(origin)
            // Never movable: an `Area` that can be dragged is an `Area` that will be,
            // and a menu that slides is a menu that is broken.
            .movable(false)
            // No fade. An `Area` fades in over a tenth of a second by default, which is
            // right for a tooltip that appeared on its own and wrong for a menu the user
            // asked for and is already reading.
            .fade_in(false)
            .show(&ctx, |ui| {
                // Square corners, which is `menu_frame`'s decision rather than an invention here:
                // the design system rounds a popover — something that floats near a control — and
                // squares a *menu*, because the corner is the loudest signal of which era a menu
                // belongs to and this one is a desktop menu hung off a right click.
                //
                // Only the corner is taken from it. `menu_frame`'s margins are horizontal 0 and
                // vertical `space-1`, and `measure` below is written to `popover_frame`'s `space-2`
                // either side — a menu measured against one frame and drawn in another is a menu
                // placed a few points out, which is the failure the whole of `measure` exists to
                // avoid.
                popover_frame(t.azur())
                    .corner_radius(egui::CornerRadius::ZERO)
                    .show(ui, |ui| {
                        // An explicit rect for the rows, rather than whatever the `Ui`
                        // says is available.
                        //
                        // The `Ui` an `Area` hands over reports an available height
                        // derived from the size the area had *last* frame, so a scroll
                        // area that sizes itself from it feeds its own output back in and
                        // settles at whatever height it first happened to be — measured
                        // 584, drawn 400, every frame. It is the same trap as the one
                        // documented at length on [`crate::ui::chrome::resize_borders`],
                        // and the same answer: use the rect this code already computed.
                        let inner = vec2(
                            size.x - space::S2 * 2.0,
                            size.y - space::S2 * 2.0,
                        );
                        let mut rows = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(Rect::from_min_size(ui.max_rect().min, inner))
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        // Rows touch. A gap between menu entries is a gap the pointer can
                        // cross into nothing, and over twenty entries — which a shell menu
                        // with a few extensions installed easily reaches — it is a whole
                        // screen of menu that did not need to exist.
                        rows.spacing_mut().item_spacing.y = 0.0;
                        // A menu with several extensions installed is taller than a
                        // 600-point window, and the entries past the edge would simply be
                        // unreachable. `auto_shrink` vertically, so a short menu is still
                        // its own height rather than the whole screen.
                        //
                        // Everything but the pinned tail, which is drawn under it and keeps
                        // its own height out of what the scrolling part may take — so the
                        // two together come to the height that was measured and the menu is
                        // still the size its position was computed from.
                        let mut area = egui::ScrollArea::vertical()
                            .id_salt(("shell-menu-rows", level_depth))
                            .max_height((inner.y - pinned_height).max(row_height()))
                            .auto_shrink([false, true]);
                        // Back to the top on the frame this level appears, and never
                        // afterwards: past that it is the user's own scrolling. See
                        // [`Open::shown`].
                        if appearing {
                            area = area.vertical_scroll_offset(0.0);
                        }
                        let scrolling = area
                            .show(&mut rows, |ui| {
                                draw_level(
                                    ui,
                                    t,
                                    menu,
                                    &path,
                                    Run {
                                        entries: &entries[..pin],
                                        first: 0,
                                        scrolls: true,
                                    },
                                    &mut out,
                                )
                            });
                        let mut drawn = scrolling.inner;
                        // And the tail, in the same column and outside the scroll area, so
                        // it stays put whatever is happening above it.
                        drawn.extend(draw_level(
                            &mut rows,
                            t,
                            menu,
                            &path,
                            Run {
                                entries: &entries[pin..],
                                first: pin,
                                scrolls: false,
                            },
                            &mut out,
                        ));
                        // What the child used, so the frame wraps the rows rather than
                        // collapsing to nothing behind them.
                        ui.advance_cursor_after_rect(rows.min_rect());
                        (drawn, scrolling.state.offset.y)
                    })
                    .inner
            });
        let (rects, offset) = response.inner;
        if level_depth == 0 {
            menu.drawn = response.response.rect.size();
            menu.scrolled = offset;
        }
        if response.response.contains_pointer() {
            hovered_any = true;
        }

        // The next level hangs off whichever item is open at this one.
        if level_depth < depth {
            let index = menu.open[level_depth];
            anchor = rects
                .get(&index)
                .copied()
                .unwrap_or(response.response.rect);
        }
    }
    // What is on screen now, so the next frame can tell an appearing level from one that has
    // been there and been scrolled. Set from what this pass drew rather than from `menu.open`,
    // which is a level that has been *asked* for: a submenu the shell has not filled yet has no
    // area on screen, and must still count as appearing on the frame it finally gets one.
    menu.shown = on_screen.into_iter().collect();

    if let Some(command) = out.chosen {
        return Outcome::Chose(command);
    }
    // Before the dismissal check below, which would otherwise take the same right click as a click
    // outside the menu and close it.
    if let Some((keys, into_group)) = out.moved {
        return Outcome::Move { keys, into_group };
    }

    // Opening and closing submenus follows the pointer: hovering a submenu opens it,
    // hovering a sibling closes whatever was open beside it.
    //
    // Unless the keyboard is the one driving. A pointer that is merely sitting there is still
    // `hovered()`, and a menu opens *under* the pointer — so the row it happened to land on top
    // of would take the open chain straight back off the arrow keys, and Right would never get a
    // submenu open at all. The pointer takes over again the moment it actually moves.
    if let Some(path) = out.wants_open {
        if !menu.by_key && menu.open != path {
            menu.open = path;
        }
    }

    // ---- Dismissal -----------------------------------------------------
    let clicked = ctx.input(|i| i.pointer.any_click());
    if menu.fresh {
        // The release of the right click that opened it arrives on the next frame.
        menu.fresh = false;
    } else if clicked && !hovered_any {
        return Outcome::Closed;
    }
    Outcome::Open
}

/// One of the two runs a level is drawn in — the part that scrolls, and the pinned tail under
/// it. See [`pinned_from`].
///
/// `first` is the index `entries` starts at in the whole level, and it is the reason this is a
/// struct rather than a slice: every path the menu is keyed by is an index into the *level*, so
/// without it the second run would put the keyboard cursor, the icon textures and the submenu
/// anchors on the wrong entries.
struct Run<'a> {
    entries: &'a [Entry],
    first: usize,
    scrolls: bool,
}

/// What a pass over the rows found, gathered so that the two runs of a level and the several levels
/// of a menu all report into one place.
#[derive(Default)]
struct Out {
    /// A command was activated, and the menu is over.
    chosen: Option<Command>,
    /// The chain the pointer is over, which becomes the open chain.
    wants_open: Option<Vec<usize>>,
    /// A right click asked for these keys to be moved. See [`Outcome::Move`].
    moved: Option<(Vec<String>, bool)>,
}

/// A run of one level's rows. Returns where each row ended up, so a submenu can be anchored.
fn draw_level(
    ui: &mut Ui,
    t: &Theme,
    menu: &Open,
    path: &[usize],
    run: Run<'_>,
    out: &mut Out,
) -> HashMap<usize, Rect> {
    let Run { entries, first, scrolls } = run;
    let mut rects = HashMap::new();

    for (offset, entry) in entries.iter().enumerate() {
        let index = first + offset;
        if matches!(entry.kind, Kind::Separator) {
            menu_divider(ui);
            continue;
        }

        let mut here = path.to_vec();
        here.push(index);
        let highlighted = menu.cursor.as_deref() == Some(here.as_slice());

        // shell32's block, as one row rather than five. Its own function because nothing below
        // applies to it: it has no label, no shortcut, no submenu arrow and no single response.
        if let Kind::Tiles(tiles) = &entry.kind {
            let rect = draw_tiles(ui, t, menu, &here, tiles, highlighted, &mut out.chosen);
            rects.insert(index, rect);
            if ui.rect_contains_pointer(rect) {
                // Hovering the row closes whatever submenu was open beside it, like any other row.
                out.wants_open = Some(path.to_vec());
            }
            continue;
        }

        let is_submenu = matches!(entry.kind, Kind::Submenu { .. });

        // The shell's own bitmap for the row, drawn through Azur's icon slot so the
        // layout is the component's rather than something invented here.
        let texture = menu.textures.get(&here).map(|handle| handle.id());
        let paint_icon = move |painter: &egui::Painter, rect: Rect, _tint: Color32| {
            if let Some(texture) = texture {
                painter.image(
                    texture,
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        };

        // The arrow-key cursor, painted under the row rather than through `MenuItem`:
        // the component fills for a hover and for keyboard focus, neither of which this
        // is, and `selected` is not it either — that is the tick a checked entry gets,
        // and using it would both show a false tick and shift the label sideways as the
        // cursor moved. The rect is the one the row is about to take, which is knowable
        // because the rows are a top-down column with no spacing between them.
        if highlighted && entry.enabled {
            let rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), row_height()));
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(radius::SMALL),
                t.bg.control_hover,
            );
        }

        // A group this program had to invent a name for — `More apps`, `More actions` — is set one
        // tier quieter than the rest of the menu, because its label is a stand-in and not a name.
        // Only those two: a group `name_of` could call `7-Zip` names something the user installed
        // and reads as confidently as any shell row.
        let mut item = MenuItem::new(entry.label.clone())
            .shortcut(entry.shortcut.clone())
            .submenu(is_submenu)
            .selected(entry.checked)
            .secondary(crate::shell::menu::is_generic_group(entry));
        if texture.is_some() {
            item = item.icon(&paint_icon);
        }

        let response = ui.add_enabled(entry.enabled, item);
        rects.insert(index, response.rect);
        if highlighted && scrolls {
            // The arrow keys can walk past the bottom of a scrolled level, and a cursor
            // you cannot see is a cursor you have lost. Only asked for inside the scroll
            // area: a pinned row is on screen already, and a scroll target set outside one
            // is a target the next scroll area in the frame would take instead.
            response.scroll_to_me(None);
        }

        // The default entry — what a double click would have done — is *not* marked.
        //
        // It was, with the 2px accent bar a selected row gets. Which is a real thing to say and
        // the wrong place to say it: the default entry is almost always the first one, so every
        // context menu in the program opened with a blue bar down the side of its top row, where
        // it read as a selection nobody had made rather than as a hint about double-clicking.
        // `entry.default` is still read off `MFS_DEFAULT` — the read of the shell's menu stays
        // faithful whether or not this program draws it.

        if response.hovered() {
            out.wants_open = Some(if is_submenu {
                here.clone()
            } else {
                path.to_vec()
            });
        }
        if response.clicked() && entry.enabled {
            match &entry.kind {
                Kind::Command(command) => out.chosen = Some(command.clone()),
                // Clicking a submenu row opens it rather than doing nothing, which is
                // what a pointer expects even though hovering already did it.
                Kind::Submenu { .. } => out.wants_open = Some(here.clone()),
                Kind::Separator | Kind::Tiles(_) => {}
            }
        }
        // A right click moves the entry between its group and the main menu — the one gesture in
        // this menu that rearranges it rather than running something. Disabled rows included: a
        // greyed entry is still one you may want out of a submenu, and nothing is invoked either
        // way.
        if response.secondary_clicked() {
            if let Some(asked) = movable(menu, &here) {
                out.moved = Some(asked);
            }
        }
    }
    rects
}

/// What a right click on this row would move, and where to. `None` for a row that is not the
/// user's to rearrange.
///
/// Four kinds of row are refused, and each for its own reason:
///
/// - **This program's own entries** and separators and the tile row — [`Moves::key`] has no name
///   for them, because none of them came from a run.
/// - **Windows' own submenus and everything inside one.** `Send to > Documents` is Windows'
///   arrangement of Windows' entries; offering to promote one would be this program rearranging a
///   menu it does not own. Told apart from a group of ours by [`Kind::is_ours`].
/// - **The anchored verbs.** `crate::shell::menu::is_anchored` names the ones `regroup` lifts into a
///   band of their own, and a preference recorded about one of those would be inert — a right click
///   that appears to do something and does nothing is worse than one that does nothing visibly.
fn movable(menu: &Open, path: &[usize]) -> Option<(Vec<String>, bool)> {
    let entry = menu.entry(path)?;

    // A group's own row: the whole group comes out. One gesture to undo a run that should not have
    // been collapsed, rather than one per entry.
    if let Kind::Submenu { children, ours: true, .. } = &entry.kind {
        let keys: Vec<String> = children.iter().filter_map(Moves::key).collect();
        return (!keys.is_empty()).then_some((keys, false));
    }

    let key = Moves::key(entry)?;
    match path.len() {
        // Inside something. Out of it, but only if the something is ours.
        2.. => {
            let parent = menu.entry(&path[..path.len() - 1])?;
            parent.kind.is_ours().then_some((vec![key], false))
        }
        // On the main menu, so into the group its run would have made — unless it is a band.
        _ => (!crate::shell::menu::is_anchored(entry)).then_some((vec![key], true)),
    }
}

/// shell32's Cut / Copy / Rename / Share / Delete as one row of icon tiles, the way Windows 11's
/// own menu shows them.
///
/// Returns the row's rect, which is what a hover is tested against — the row is several responses
/// and the caller needs one shape.
///
/// The tiles divide the row evenly rather than each taking its caption's width. Five equal cells
/// read as one control; five ragged ones read as five buttons that happen to be adjacent, and the
/// width they would need is not knowable before [`measure`] has already placed the menu.
fn draw_tiles(
    ui: &mut Ui,
    t: &Theme,
    menu: &Open,
    path: &[usize],
    tiles: &[Entry],
    highlighted: bool,
    chosen: &mut Option<Command>,
) -> Rect {
    let full = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(full, tile_row_height()), egui::Sense::hover());
    let each = full / tiles.len() as f32;
    // One buffer for the row rather than one per tile: the texture map is keyed by path, and this is
    // a draw loop. Pushed and popped per tile, like `upload_icons` walks the tree.
    let mut here = path.to_vec();

    for (index, tile) in tiles.iter().enumerate() {
        let cell = Rect::from_min_size(
            pos2(row.left() + each * index as f32, row.top()),
            vec2(each, row.height()),
        );
        // One response per tile, placed by hand: `allocate_exact_size` down a column cannot
        // produce a row, and `horizontal()` would size the cells to their contents.
        let response = ui.interact(
            cell,
            ui.id().with(("menu-tile", path, index)),
            egui::Sense::click(),
        );
        let lit = tile.enabled && (response.hovered() || (highlighted && menu.tile == index));
        if lit {
            ui.painter().rect_filled(
                cell.shrink(space::S1),
                egui::CornerRadius::same(radius::SMALL),
                t.bg.control_hover,
            );
        }

        // The icon, centred on the cell and sitting on the top padding. Centred by its box and not
        // by its ink, which is right for a bitmap: the shell drew it into a 16×16 with whatever
        // margins it wanted, and second-guessing those would misalign it against the same icon
        // drawn in Explorer.
        here.push(index);
        let texture = menu.textures.get(&here);
        here.pop();
        let at = Rect::from_min_size(
            pos2(cell.center().x - TILE_ICON / 2.0, cell.top() + space::S2),
            Vec2::splat(TILE_ICON),
        );
        if let Some(texture) = texture {
            ui.painter().image(
                texture.id(),
                at,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                if tile.enabled {
                    Color32::WHITE
                } else {
                    // Half alpha rather than a tint: the bitmap carries its own colours and a
                    // greyed entry has to read as the same icon, dimmed.
                    Color32::from_white_alpha(96)
                },
            );
        } else if let Some(glyph) = crate::shell::menu::tile_glyph(tile) {
            // Windows gives these five no bitmap at all — see `tile_glyph`, which is where the
            // measurement is — so this is not a fallback for an unusual machine, it is the normal
            // path. A painted glyph rather than an image, so it takes the row's own colour and is
            // sharp at whatever the row height happens to be.
            glyph(
                ui.painter(),
                at,
                if tile.enabled {
                    t.text.primary
                } else {
                    t.text.disabled
                },
            );
        }

        // And the caption under it, centred, on its baseline — `Align2::CENTER_TOP` puts the
        // galley's box there, which for a single line of one font is the same thing and is the one
        // egui offers.
        ui.painter().text(
            pos2(
                cell.center().x,
                cell.top() + space::S2 + TILE_ICON + space::S1,
            ),
            egui::Align2::CENTER_TOP,
            &tile.label,
            t.fonts.caption.clone(),
            if tile.enabled {
                t.text.primary
            } else {
                t.text.disabled
            },
        );

        if response.clicked() && tile.enabled {
            if let Kind::Command(command) = &tile.kind {
                *chosen = Some(command.clone());
            }
        }
    }
    row
}

/// How big a level will be, before anything is drawn.
///
/// Computed rather than measured because the position depends on it: a menu that
/// learned its own height a frame late would appear in the wrong place and jump.
///
/// Capped at the screen, which is also what makes the level scroll: the rows go in a
/// scroll area of exactly this height, so a menu longer than the window keeps its last
/// entry reachable instead of drawing it past the edge.
fn measure(ctx: &egui::Context, t: &Theme, entries: &[Entry], screen: Rect) -> Vec2 {
    // `Menu { min-width: 180px }`, and a cap so one long "Open with" entry does not
    // stretch the menu across the window.
    const MIN: f32 = 180.0;
    const MAX: f32 = 420.0;

    // What a row puts around its text, asked for rather than repeated here. This was
    // repeated here, and it was wrong in two places at once: the leading gutter was added
    // only for the entries that had an icon, when `MenuItem` reserves it for every entry,
    // and the gap before a shortcut was `space-5` where the component uses `space-3`. The
    // two errors cancelled out often enough to go unnoticed, because a shell menu always
    // has one long "Restore previous versions" in it that pushes the width to `MAX`
    // anyway. Then this program's own six entries were shown on their own, while the shell
    // was still being asked, and the menu came up 16 points short with `Copy pa…` in it.
    let row = azur_egui_theme::components::menu_item_metrics();

    let width = ctx.fonts_mut(|fonts| {
        let mut widest: f32 = 0.0;
        for entry in entries {
            if matches!(entry.kind, Kind::Separator) {
                continue;
            }
            // The tile row is five captions side by side rather than one label, so it wants the sum
            // and not the widest. Asked for here rather than left to the row to discover, for the
            // reason the whole of this function exists: the menu is placed from this number before
            // anything is drawn, and a row that turned out wider than the menu would be clipped
            // rather than fitted.
            if let Kind::Tiles(tiles) = &entry.kind {
                let mut row = 0.0;
                for tile in tiles {
                    let caption = fonts
                        .layout_no_wrap(tile.label.clone(), t.fonts.caption.clone(), Color32::WHITE)
                        .size()
                        .x;
                    // `space-3` either side, not `space-2`. The captions are centred in cells of
                    // equal width, so the gap between two of them is whatever is left over after the
                    // longer one — and on a French Windows the row is `Couper Copier Renommer
                    // Partager Supprimer`, where `Renommer` and `Partager` are within a few points
                    // of the cell and ended up almost touching. This is the one number that buys
                    // them air, because the row's share of the menu's width is decided here.
                    row += caption + space::S3 * 2.0;
                }
                widest = widest.max(row);
                continue;
            }
            let label = fonts
                .layout_no_wrap(entry.label.clone(), t.fonts.body.clone(), Color32::WHITE)
                .size()
                .x;
            let shortcut = if entry.shortcut.is_empty() {
                0.0
            } else {
                fonts
                    .layout_no_wrap(
                        entry.shortcut.clone(),
                        t.fonts.caption.clone(),
                        Color32::WHITE,
                    )
                    .size()
                    .x
            };
            let submenu = matches!(entry.kind, Kind::Submenu { .. });
            widest = widest.max(row.width(label, shortcut, submenu));
        }
        widest
    });

    // Plus the frame's own `space-2` either side.
    //
    // Rounded up for the same reason the design system rounds its own: handing the widest
    // entry exactly its galley width leaves it a fraction short, and it ellipsizes.
    let width = (width + space::S2 * 2.0).ceil().clamp(MIN, MAX);

    let height = (stack_height(entries) + space::S2 * 2.0).min(screen.height() - space::S3 * 2.0);
    vec2(width, height)
}

/// Where a level goes: at the anchor, flipped rather than clipped.
fn place(anchor: Rect, size: Vec2, screen: Rect, root: bool) -> Pos2 {
    // A root menu hangs from the pointer; a submenu from the right edge of its row, with
    // a small overlap so the pointer does not cross a gap on the way in.
    let (mut x, mut y) = if root {
        (anchor.left(), anchor.top())
    } else {
        (anchor.right() - space::S1, anchor.top() - space::S2)
    };

    if x + size.x > screen.right() {
        // Flip to the other side of the anchor rather than merely sliding left, which is
        // what would put a submenu on top of its own parent.
        x = if root {
            anchor.left() - size.x
        } else {
            anchor.left() - size.x + space::S1
        };
    }
    if y + size.y > screen.bottom() {
        y = if root {
            anchor.top() - size.y
        } else {
            screen.bottom() - size.y
        };
    }
    pos2(
        x.clamp(screen.left(), (screen.right() - size.x).max(screen.left())),
        y.clamp(screen.top(), (screen.bottom() - size.y).max(screen.top())),
    )
}

/// Upload the shell's item bitmaps, once each.
fn upload_icons(ctx: &egui::Context, menu: &mut Open, path: &mut Vec<usize>) {
    // Walked by index rather than by reference so the tree can be read while the texture
    // map is written.
    let count = menu.children(path).map(Vec::len).unwrap_or(0);
    for index in 0..count {
        path.push(index);
        let image = menu
            .entry(path)
            .and_then(|entry| entry.icon.clone())
            .filter(|_| !menu.textures.contains_key(path));
        if let Some(image) = image {
            let handle = ctx.load_texture(
                format!("menu-icon-{path:?}"),
                image,
                egui::TextureOptions::LINEAR,
            );
            menu.textures.insert(path.clone(), handle);
        }
        // Into a tile row as well as into a submenu: a tile is drawn from the shell's own bitmap
        // and there is nothing else to draw it with.
        if matches!(
            menu.entry(path).map(|e| &e.kind),
            Some(Kind::Submenu { .. } | Kind::Tiles(_))
        ) {
            upload_icons(ctx, menu, path);
        }
        path.pop();
    }
}

/// What a frame's keys did to the menu. See [`keyboard`].
enum Keys {
    /// Nothing that concerns the menu.
    Idle,
    /// The cursor or the open chain moved. The menu is still up and still has to be drawn on
    /// this frame, and the pointer must not undo what the key just did.
    Moved,
    /// The menu is over, one way or the other.
    Done(Outcome),
}

/// Arrow keys, Enter and Escape.
///
/// **Nothing here draws or skips a frame.** Everything but [`Keys::Done`] is a menu that is still
/// up: the caller goes on to draw it in whatever state this left it. Returning early for a key
/// that only moved the cursor is what made an arrow key flicker the menu.
///
/// **Taken rather than read.** `App::keyboard` keeps the listing's own keys off while a menu is up
/// — see the `typing` gate there — but it runs *after* the menu is drawn in the same frame, and by
/// then Enter has already chosen an entry and closed the menu. So the gate is open, and the same
/// still-pressed Enter reaches the listing and opens whatever row was selected.
///
/// Which is what it did: driving `New > Folder` from the keyboard created the folder, and then
/// navigated into it — taking the rename that was the rest of the gesture with it, since going
/// somewhere else drops it. `consume_key` removes the event instead, so nothing downstream sees a
/// key the menu has already acted on. All six, not only the two that close a menu: the others are
/// harmless today purely because the gate happens to still be shut for them, and that is not a
/// property worth depending on.
fn keyboard(ctx: &egui::Context, menu: &mut Open) -> Keys {
    use egui::Key;

    let (up, down, left, right, enter, escape) = ctx.input_mut(|i| {
        let none = egui::Modifiers::NONE;
        (
            i.consume_key(none, Key::ArrowUp),
            i.consume_key(none, Key::ArrowDown),
            i.consume_key(none, Key::ArrowLeft),
            i.consume_key(none, Key::ArrowRight),
            i.consume_key(none, Key::Enter),
            i.consume_key(none, Key::Escape),
        )
    });
    if escape {
        // Escape closes one level at a time, and the whole menu from the root.
        return if menu.open.is_empty() {
            Keys::Done(Outcome::Closed)
        } else {
            menu.open.pop();
            menu.cursor = None;
            Keys::Moved
        };
    }

    if up || down {
        let level_path: Vec<usize> = menu.open.clone();
        let count = menu.level(&level_path).map(Vec::len).unwrap_or(0);
        if count == 0 {
            return Keys::Idle;
        }
        let current = menu
            .cursor
            .as_ref()
            .filter(|c| c.len() == level_path.len() + 1 && c.starts_with(&level_path))
            .and_then(|c| c.last().copied());
        let step: isize = if down { 1 } else { -1 };
        let mut next = match current {
            Some(index) => index as isize + step,
            None if down => 0,
            None => count as isize - 1,
        };
        // Skip separators and anything disabled, and wrap.
        for _ in 0..count * 2 {
            let wrapped = next.rem_euclid(count as isize) as usize;
            let mut candidate = level_path.clone();
            candidate.push(wrapped);
            let usable = menu.entry(&candidate).is_some_and(|e| match &e.kind {
                Kind::Separator => false,
                // A row of five whose every tile is greyed — a selection nothing can be done to —
                // is a row there is no point stopping on.
                Kind::Tiles(tiles) => tiles.iter().any(|tile| tile.enabled),
                _ => e.enabled,
            });
            if usable {
                // Landing on the tile row starts at its first usable tile rather than at whatever
                // tile was last under the cursor, which may since have been greyed.
                if let Some(Kind::Tiles(tiles)) = menu.entry(&candidate).map(|e| &e.kind) {
                    menu.tile = tiles.iter().position(|tile| tile.enabled).unwrap_or(0);
                }
                menu.cursor = Some(candidate);
                return Keys::Moved;
            }
            next += step;
        }
        return Keys::Moved;
    }

    // Left and Right walk the tile row before they mean anything about submenus — the row is one
    // cursor stop and moving along it is what those keys are for while it is the stop. Wrapping
    // rather than clamping, because a row of five is a ring and Left from the first tile plainly
    // means the last.
    if left || right {
        if let Some(cursor) = menu.cursor.clone() {
            if let Some(Kind::Tiles(tiles)) = menu.entry(&cursor).map(|e| &e.kind) {
                let count = tiles.len();
                let step: isize = if right { 1 } else { -1 };
                let mut next = menu.tile as isize + step;
                for _ in 0..count {
                    let wrapped = next.rem_euclid(count as isize) as usize;
                    if tiles[wrapped].enabled {
                        menu.tile = wrapped;
                        break;
                    }
                    next += step;
                }
                return Keys::Moved;
            }
        }
    }

    if right {
        if let Some(cursor) = menu.cursor.clone() {
            if matches!(menu.entry(&cursor).map(|e| &e.kind), Some(Kind::Submenu { .. })) {
                menu.open = cursor;
                menu.cursor = None;
                return Keys::Moved;
            }
        }
    }
    if left && !menu.open.is_empty() {
        menu.cursor = Some(menu.open.clone());
        menu.open.pop();
        return Keys::Moved;
    }
    if enter {
        if let Some(cursor) = menu.cursor.clone() {
            match menu.entry(&cursor).map(|e| e.kind.clone()) {
                Some(Kind::Command(command)) => return Keys::Done(Outcome::Chose(command)),
                Some(Kind::Submenu { .. }) => {
                    menu.open = cursor;
                    menu.cursor = None;
                    return Keys::Moved;
                }
                // Whichever tile the row is on.
                Some(Kind::Tiles(tiles)) => {
                    if let Some(Kind::Command(command)) =
                        tiles.get(menu.tile).map(|tile| &tile.kind)
                    {
                        return Keys::Done(Outcome::Chose(command.clone()));
                    }
                }
                _ => {}
            }
        }
    }
    Keys::Idle
}

#[cfg(test)]
mod tests;
