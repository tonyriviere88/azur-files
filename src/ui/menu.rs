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

use crate::shell::menu::{Command, Entry, Kind};
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
    pub entries: Vec<Entry>,
    /// How much of a menu this is, carried through so that a command chosen here is resolved
    /// against a menu built the same way. See [`crate::shell::menu::invoke`].
    pub depth: crate::shell::menu::Depth,
    /// Which submenu chain is showing, as indices from the root.
    pub open: Vec<usize>,
    /// The keyboard highlight, as a path from the root.
    pub cursor: Option<Vec<usize>>,
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
    /// What the root level actually took on screen, last frame.
    ///
    /// Kept because it is the one thing worth asserting about a menu that lays itself out
    /// by arithmetic: it has to come out the size it said it would. See the test.
    pub drawn: Vec2,
}

impl Open {
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
            entries,
            depth,
            open: Vec::new(),
            cursor: None,
            token,
            fills: Vec::new(),
            asked: std::collections::HashSet::new(),
            textures: HashMap::new(),
            fresh: true,
            drawn: Vec2::ZERO,
        }
    }

    /// The entry at a path, if there is one.
    fn entry(&self, path: &[usize]) -> Option<&Entry> {
        let mut level = &self.entries;
        for (depth, index) in path.iter().enumerate() {
            let entry = level.get(*index)?;
            if depth + 1 == path.len() {
                return Some(entry);
            }
            match &entry.kind {
                Kind::Submenu { children, .. } => level = children,
                _ => return None,
            }
        }
        None
    }

    /// The entries at a level.
    fn level(&self, path: &[usize]) -> Option<&Vec<Entry>> {
        if path.is_empty() {
            return Some(&self.entries);
        }
        match self.entry(path)?.kind {
            Kind::Submenu { ref children, .. } => Some(children),
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
        put(&mut self.entries, id, &mut Some(children));
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

/// Draw the menu and every open submenu.
pub fn show(ui: &mut Ui, t: &Theme, menu: &mut Open) -> Outcome {
    let ctx = ui.ctx().clone();

    // ---- Keyboard ------------------------------------------------------
    if let Some(outcome) = keyboard(&ctx, menu) {
        return outcome;
    }

    // Textures for the shell's item bitmaps, uploaded once each.
    upload_icons(&ctx, menu, &mut Vec::new());

    // A submenu is empty until the extension that owns it is asked, and asking is the
    // expensive part — so it happens on the hover that opens it.
    menu.ask();

    let screen = ctx.viewport_rect();
    let mut chosen = None;
    let mut hovered_any = false;
    // The chain the pointer is currently over, which becomes the open chain so that
    // moving off a submenu and onto a sibling closes the old one.
    let mut wants_open: Option<Vec<usize>> = None;

    // Levels are drawn root-first, each anchored to the item that opened it.
    let depth = menu.open.len();
    let mut anchor = Rect::from_min_size(menu.at, Vec2::ZERO);
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
                popover_frame(t.azur())
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
                        let drawn = egui::ScrollArea::vertical()
                            .id_salt(("shell-menu-rows", level_depth))
                            .max_height(inner.y)
                            .auto_shrink([false, true])
                            .show(&mut rows, |ui| {
                                draw_level(
                                    ui,
                                    t,
                                    menu,
                                    &path,
                                    &entries,
                                    &mut chosen,
                                    &mut wants_open,
                                )
                            })
                            .inner;
                        // What the child used, so the frame wraps the rows rather than
                        // collapsing to nothing behind them.
                        ui.advance_cursor_after_rect(rows.min_rect());
                        drawn
                    })
                    .inner
            });
        if level_depth == 0 {
            menu.drawn = response.response.rect.size();
        }
        if response.response.contains_pointer() {
            hovered_any = true;
        }

        // The next level hangs off whichever item is open at this one.
        if level_depth < depth {
            let index = menu.open[level_depth];
            anchor = response
                .inner
                .get(&index)
                .copied()
                .unwrap_or(response.response.rect);
        }
    }

    if let Some(command) = chosen {
        return Outcome::Chose(command);
    }

    // Opening and closing submenus follows the pointer: hovering a submenu opens it,
    // hovering a sibling closes whatever was open beside it.
    if let Some(path) = wants_open {
        if menu.open != path {
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

/// One level's rows. Returns where each row ended up, so a submenu can be anchored.
fn draw_level(
    ui: &mut Ui,
    t: &Theme,
    menu: &Open,
    path: &[usize],
    entries: &[Entry],
    chosen: &mut Option<Command>,
    wants_open: &mut Option<Vec<usize>>,
) -> HashMap<usize, Rect> {
    let mut rects = HashMap::new();

    for (index, entry) in entries.iter().enumerate() {
        if matches!(entry.kind, Kind::Separator) {
            menu_divider(ui);
            continue;
        }

        let mut here = path.to_vec();
        here.push(index);
        let is_submenu = matches!(entry.kind, Kind::Submenu { .. });
        let highlighted = menu.cursor.as_deref() == Some(here.as_slice());

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

        let mut item = MenuItem::new(entry.label.clone())
            .shortcut(entry.shortcut.clone())
            .submenu(is_submenu)
            .selected(entry.checked);
        if texture.is_some() {
            item = item.icon(&paint_icon);
        }

        let response = ui.add_enabled(entry.enabled, item);
        rects.insert(index, response.rect);
        if highlighted {
            // The arrow keys can walk past the bottom of a scrolled level, and a cursor
            // you cannot see is a cursor you have lost.
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
            *wants_open = Some(if is_submenu {
                here.clone()
            } else {
                path.to_vec()
            });
        }
        if response.clicked() && entry.enabled {
            match &entry.kind {
                Kind::Command(command) => *chosen = Some(command.clone()),
                // Clicking a submenu row opens it rather than doing nothing, which is
                // what a pointer expects even though hovering already did it.
                Kind::Submenu { .. } => *wants_open = Some(here.clone()),
                Kind::Separator => {}
            }
        }
    }
    rects
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

    let height: f32 = entries
        .iter()
        .map(|entry| {
            if matches!(entry.kind, Kind::Separator) {
                separator_height()
            } else {
                row_height()
            }
        })
        .sum();
    let height = (height + space::S2 * 2.0).min(screen.height() - space::S3 * 2.0);
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
    let count = menu.level(path).map(Vec::len).unwrap_or(0);
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
        if matches!(menu.entry(path).map(|e| &e.kind), Some(Kind::Submenu { .. })) {
            upload_icons(ctx, menu, path);
        }
        path.pop();
    }
}

/// Arrow keys, Enter and Escape.
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
fn keyboard(ctx: &egui::Context, menu: &mut Open) -> Option<Outcome> {
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
            Some(Outcome::Closed)
        } else {
            menu.open.pop();
            menu.cursor = None;
            Some(Outcome::Open)
        };
    }

    if up || down {
        let level_path: Vec<usize> = menu.open.clone();
        let count = menu.level(&level_path).map(Vec::len).unwrap_or(0);
        if count == 0 {
            return None;
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
            let usable = menu
                .entry(&candidate)
                .is_some_and(|e| e.enabled && !matches!(e.kind, Kind::Separator));
            if usable {
                menu.cursor = Some(candidate);
                return Some(Outcome::Open);
            }
            next += step;
        }
        return Some(Outcome::Open);
    }

    if right {
        if let Some(cursor) = menu.cursor.clone() {
            if matches!(menu.entry(&cursor).map(|e| &e.kind), Some(Kind::Submenu { .. })) {
                menu.open = cursor;
                menu.cursor = None;
                return Some(Outcome::Open);
            }
        }
    }
    if left && !menu.open.is_empty() {
        menu.cursor = Some(menu.open.clone());
        menu.open.pop();
        return Some(Outcome::Open);
    }
    if enter {
        if let Some(cursor) = menu.cursor.clone() {
            match menu.entry(&cursor).map(|e| e.kind.clone()) {
                Some(Kind::Command(command)) => return Some(Outcome::Chose(command)),
                Some(Kind::Submenu { .. }) => {
                    menu.open = cursor;
                    menu.cursor = None;
                    return Some(Outcome::Open);
                }
                _ => {}
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::menu::Own;

    fn entry(label: &str) -> Entry {
        Entry {
            label: label.to_owned(),
            shortcut: String::new(),
            kind: Kind::Command(Command::Own(Own::CopyHere)),
            enabled: true,
            checked: false,
            icon: None,
        }
    }

    fn submenu(label: &str, children: Vec<Entry>) -> Entry {
        Entry {
            kind: Kind::complete(children),
            ..entry(label)
        }
    }

    /// A submenu row as the shell hands it over: known to be one, not yet asked about.
    fn unfilled(label: &str, source: u32) -> Entry {
        Entry {
            kind: Kind::unfilled(source),
            ..entry(label)
        }
    }

    fn menu(entries: Vec<Entry>) -> Open {
        Open::new(
            1,
            pos2(100.0, 100.0),
            Vec::new(),
            std::path::PathBuf::from(r"C:\x"),
            entries,
            crate::shell::menu::Depth::Full,
            1,
        )
    }

    /// A pass over a menu, for the tests that need one drawn.
    fn pass(open: &mut Open, screen: Rect, times: usize) -> egui::Context {
        let ctx = egui::Context::default();
        let theme = Theme::dark();
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().inner_rect = Some(screen);
        for _ in 0..times {
            let _ = ctx.run_ui(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = show(ui, &theme, open);
                });
            });
        }
        ctx
    }

    /// Opening an unfilled submenu asks for it once, by the id the shell handed over, and
    /// the level appears when the answer does -- not before, and not by asking again every
    /// frame.
    ///
    /// The id is the part worth holding down. It was a path of entry indices, and that broke
    /// the moment this program's own entries went above the shell's: the menu asked about
    /// index 8 for a submenu the shell had filed under index 1, every lookup missed, and
    /// every submenu in the program came back empty. Hence `unfilled("Send to", 41)` --
    /// deliberately not 1, so a version that went back to computing the index from the tree
    /// cannot pass.
    #[test]
    fn an_unfilled_submenu_is_asked_for_once_by_id_and_drawn_when_it_arrives() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let mut open = menu(vec![entry("Open"), unfilled("Send to", 41)]);

        // Nothing is asked for until it is opened.
        pass(&mut open, screen, 2);
        assert!(open.fills.is_empty(), "an unopened submenu was asked about");

        open.open = vec![1];
        pass(&mut open, screen, 3);
        assert_eq!(
            open.fills,
            vec![41],
            "opening it should have asked once, by the shell's id, across three frames"
        );
        // The caller sends it on; nothing more should accumulate.
        open.fills.clear();
        pass(&mut open, screen, 3);
        assert!(open.fills.is_empty(), "it was asked for twice");

        // The answer arrives and the level becomes drawable.
        assert!(open.level(&[1]).is_some_and(Vec::is_empty));
        open.filled(41, vec![entry("Desktop"), entry("Mail recipient")]);
        assert_eq!(open.level(&[1]).map(Vec::len), Some(2));
        assert!(open.entries[1].enabled);
    }

    /// An answer for a submenu nested inside another one finds its way in, and an id nobody
    /// is holding changes nothing.
    #[test]
    fn a_fill_finds_its_submenu_at_any_depth() {
        let mut open = menu(vec![
            entry("Open"),
            submenu("More", vec![entry("Here"), unfilled("Deeper", 9)]),
        ]);
        open.filled(9, vec![entry("Bottom")]);
        assert_eq!(open.level(&[1, 1]).map(Vec::len), Some(1));

        // An id from a menu that has already gone is not going to match anything, and must
        // not overwrite whatever is holding a different one.
        open.filled(1234, vec![entry("Wrong")]);
        assert_eq!(open.level(&[1, 1]).map(Vec::len), Some(1));
        assert_eq!(open.entries.len(), 2);
    }

    /// An extension that really has nothing leaves the row there and inert, rather than
    /// deleting it from under the pointer that is on it.
    #[test]
    fn a_submenu_that_fills_to_nothing_stops_being_usable() {
        let mut open = menu(vec![entry("Open"), unfilled("Nothing here", 3)]);
        open.filled(3, Vec::new());
        assert_eq!(open.entries.len(), 2, "the row stayed");
        assert!(!open.entries[1].enabled, "and stopped being usable");
        // Asked and answered: it must not be asked again.
        open.open = vec![1];
        pass(&mut open, Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), 2);
        assert!(open.fills.is_empty());
    }

    #[test]
    fn the_measured_row_heights_are_the_ones_the_components_allocate() {
        // `measure` adds these up to place the menu before a single row is drawn, so a
        // number that drifts from what the design system allocates puts the menu in the
        // wrong place — and the taller the menu, the further out. It drifted once already,
        // when the design system dropped its menu density from 36 points to 28, so this
        // asks the components rather than trusting either arithmetic.
        let ctx = egui::Context::default();
        let mut taken = 0.0;
        let _ = ctx.run_ui(Default::default(), |ctx| {
            egui::Area::new(Id::new("probe")).show(ctx, |ui| {
                ui.set_width(240.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                let top = ui.cursor().top();
                ui.add(MenuItem::new("one"));
                ui.add(MenuItem::new("two"));
                menu_divider(ui);
                taken = ui.cursor().top() - top;
            });
        });
        assert_eq!(taken, row_height() * 2.0 + separator_height());
    }

    #[test]
    fn a_menu_longer_than_the_screen_is_capped_and_drawn_the_size_it_measured() {
        // What this holds down: a menu with more entries than the window has room for is
        // capped to the screen — without which the entries past the edge are simply
        // unreachable — and what gets drawn is the height that was measured, since the
        // position was computed from it.
        //
        // It is not proof against the whole class of failure. The real one — an `Area`
        // whose `Ui` reports an available height derived from the area's own size last
        // frame, so that a scroll area sizing itself from it settled at 400 points against
        // a measured 584 — reproduces in a real window and not in a pass driven from here;
        // it was found by capturing the window and comparing. The fix is at the call site,
        // where the rows are given an explicit rect.
        let ctx = egui::Context::default();
        let theme = Theme::dark();
        let mut open = menu((0..12).map(|i| entry(&format!("entry {i}"))).collect());

        let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0));
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().inner_rect = Some(screen);

        for _ in 0..4 {
            let _ = ctx.run_ui(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = show(ui, &theme, &mut open);
                });
            });
        }

        let expected = measure(&ctx, &theme, &open.entries, screen);
        assert!(
            expected.y < row_height() * 12.0,
            "twelve rows should not fit in a 200-point window, or this proves nothing"
        );
        // Within the frame's own stroke.
        assert!(
            (open.drawn.y - expected.y).abs() <= 2.0,
            "measured {} and drew {}",
            expected.y,
            open.drawn.y
        );
        assert!(open.drawn.y <= screen.height(), "and it stays on screen");
    }

    #[test]
    fn levels_and_entries_resolve_by_path() {
        let m = menu(vec![
            entry("one"),
            submenu("more", vec![entry("deep"), submenu("deeper", vec![entry("bottom")])]),
        ]);
        assert_eq!(m.entry(&[0]).unwrap().label, "one");
        assert_eq!(m.entry(&[1]).unwrap().label, "more");
        assert_eq!(m.entry(&[1, 0]).unwrap().label, "deep");
        assert_eq!(m.entry(&[1, 1, 0]).unwrap().label, "bottom");
        assert!(m.entry(&[9]).is_none());
        // A path through a leaf is not a path.
        assert!(m.entry(&[0, 0]).is_none());

        assert_eq!(m.level(&[]).unwrap().len(), 2);
        assert_eq!(m.level(&[1]).unwrap().len(), 2);
        assert!(m.level(&[0]).is_none());
    }

    #[test]
    fn a_menu_that_would_run_off_the_right_flips() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
        let size = vec2(200.0, 300.0);

        // Room to the right: it hangs from the pointer.
        let anchor = Rect::from_min_size(pos2(100.0, 100.0), Vec2::ZERO);
        assert_eq!(place(anchor, size, screen, true), pos2(100.0, 100.0));

        // No room: it flips to the other side rather than sliding, so the pointer is not
        // left inside the menu it just opened.
        let anchor = Rect::from_min_size(pos2(950.0, 100.0), Vec2::ZERO);
        assert_eq!(place(anchor, size, screen, true), pos2(750.0, 100.0));
    }

    #[test]
    fn a_menu_that_would_run_off_the_bottom_flips_up() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
        let size = vec2(200.0, 300.0);
        let anchor = Rect::from_min_size(pos2(100.0, 700.0), Vec2::ZERO);
        assert_eq!(place(anchor, size, screen, true), pos2(100.0, 400.0));
    }

    #[test]
    fn a_menu_taller_than_the_screen_still_starts_on_it() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 400.0));
        let size = vec2(200.0, 900.0);
        let anchor = Rect::from_min_size(pos2(100.0, 300.0), Vec2::ZERO);
        let at = place(anchor, size, screen, true);
        assert_eq!(at, pos2(100.0, 0.0), "clamped to the top rather than off it");
    }

    #[test]
    fn a_submenu_hangs_off_the_right_of_its_row() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
        let row = Rect::from_min_size(pos2(100.0, 200.0), vec2(180.0, 36.0));
        let at = place(row, vec2(200.0, 100.0), screen, false);
        assert!(at.x > row.left(), "to the right of the row it came from");
        assert!(at.x <= row.right(), "with a small overlap so the pointer can cross");
    }
}
