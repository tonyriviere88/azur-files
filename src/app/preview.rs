//! Deciding what the preview panel should be showing, and collecting it when it arrives.
//!
//! The panel follows the keyboard, debounced by [`crate::ui::preview::FOLLOW_DELAY`] — holding
//! the down arrow through a folder of photographs must not decode thirty of them.
//!
//! The drop preview is here too — the highlight over the folder a drag would land in, and the ghost
//! and sentence that go with it. Same word, two different things: one is a panel showing a file,
//! the other is a promise about a gesture. They share this file because they are both "what the
//! window is about to show you".

use super::*;

/// How far the sentence sits from the pointer's hotspot, down and to the right.
///
/// [`azur_egui_theme::components::CURSOR_CLEARANCE`]'s reasoning and its value: the arrow's ink
/// hangs down and to the right of the point it names, so anything closer is drawn under the cursor.
///
/// The ghost goes *above* the hotspot — see [`crate::ui::drag_ghost`] — so the two are on opposite
/// sides of the cursor and neither has to make room for the other.
const CLEARANCE: f32 = azur_egui_theme::components::CURSOR_CLEARANCE;

impl App {
    /// Hand finished reads to the panels that asked, and keep each one pointed at its own
    /// keyboard.
    ///
    /// A panel **follows the selection** rather than being told once: you open it, then arrow
    /// down the folder and look at each file in turn without touching the shortcut again. That is
    /// only bearable because it waits — see [`crate::ui::preview::FOLLOW_DELAY`] — so holding the
    /// arrow key down decodes the file you stop on and not the thirty on the way past.
    ///
    /// **Every open panel is followed, not only the focused one.** Two panes side by side each
    /// showing a build of the same DLL is the case the panel is inside the pane *for*, and a
    /// preview that only tracked the pane with the keyboard would go stale in the other one the
    /// moment you clicked across to compare them.
    pub(super) fn collect_previews(&mut self, ctx: &egui::Context, now: f64) {
        // Collected first, so the reader is not borrowed while the panels are written to. The
        // payload goes to the one panel waiting for that token and nowhere else: it is a decoded
        // picture or a walked graph, so there is one of it and no copy to hand round.
        for loaded in self.previews.drain().collect::<Vec<_>>() {
            let wanted = self
                .panes
                .iter_mut()
                .flat_map(|pane| pane.tabs.iter_mut())
                .find(|tab| tab.preview.wants(loaded.token));
            if let Some(tab) = wanted {
                tab.preview.arrived(loaded.token, loaded.payload, ctx);
            }
        }

        // What each open panel should be looking at, worked out before anything is borrowed
        // mutably: `selected_preview` reads the tab, and asking for a read writes to it.
        type Wanted = (PaneId, usize, Option<crate::preview::Ask>);
        let wanted: Vec<Wanted> = self
            .panes
            .iter()
            .flat_map(|pane| {
                pane.tabs
                    .iter()
                    .enumerate()
                    .filter(|(at, tab)| tab.preview.open && *at == pane.active)
                    .map(|(at, tab)| (pane.id, at, Self::selected_preview(tab, self.preview.diff)))
                    .collect::<Vec<_>>()
            })
            .collect();

        let mut soonest: Option<f64> = None;
        for (id, at, what) in wanted {
            let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) else {
                continue;
            };
            let Some(tab) = pane.tabs.get_mut(at) else {
                continue;
            };
            tab.preview.follow(what, now);
            let (ready, left) = tab.preview.settle(now);
            if let Some(ask) = ready {
                let token = self.previews.request(&ask);
                if let Some(tab) = self
                    .panes
                    .iter_mut()
                    .find(|p| p.id == id)
                    .and_then(|p| p.tabs.get_mut(at))
                {
                    tab.preview.asked(ask, token);
                }
            }
            if let Some(left) = left {
                soonest = Some(soonest.map_or(left, |soonest: f64| soonest.min(left)));
            }
        }
        // And the frame that would notice the wait is up. Nothing else would ask for it: this
        // program is idle between events, so a deadline nobody books a frame for is a deadline
        // that arrives the next time something unrelated happens to want one. The *soonest* of
        // them, since one frame serves every panel that is waiting.
        if let Some(left) = soonest {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
    }

    /// What this tab's preview panel should be showing, if anything.
    ///
    /// **Two pictures selected at once is a comparison**, and that is the one case where the
    /// *selection* rather than the cursor decides: picking a second image is a deliberate act with
    /// an obvious meaning, and no other pair of files has one. Everything else is the cursor's
    /// answer — the cursor is where the keyboard is, it follows a click as well, and a preview is
    /// about one file, so a selection of thirty has nothing to show.
    ///
    /// **And a picture git has a different version of is a comparison too**, when the panel's diff
    /// toggle is on: a `.png` has no lines to put a red band behind, so what "show me what changed"
    /// means for one is the two versions and the difference between them — which is the view two
    /// selected pictures already get. `diffing` is that toggle, and it is read here rather than in the
    /// panel because it changes *what is read*: turning it off has to be a different [`Ask`], or the
    /// panel would go on showing the comparison it is holding.
    pub(super) fn selected_preview(tab: &Tab, diffing: bool) -> Option<crate::preview::Ask> {
        use crate::preview::{kind_of, Ask, Kind};

        let dir = tab.dir.as_ref()?;
        let kind_at = |row: usize| -> Option<Kind> {
            let entry = tab.entry_at(row)?;
            // From the name alone, which costs nothing to ask about a row. A file that passes and
            // turns out to be something else says so in the panel rather than being refused here.
            kind_of(
                dir.leaf(entry),
                dir.ext(entry),
                dir.entries[entry].is_dir(),
            )
        };

        if tab.selected_count == 2 {
            // In display order, so the left-hand or upper view is the upper row — the pair reads
            // the way the listing above it does rather than the way the clicks happened to land.
            let two: Vec<usize> = (0..tab.order.len())
                .filter(|&row| tab.is_selected(row))
                .take(3)
                .collect();
            if let [a, b] = two[..] {
                if kind_at(a) == Some(Kind::Picture) && kind_at(b) == Some(Kind::Picture) {
                    return Some(Ask::Pair(tab.target_at(a)?, tab.target_at(b)?));
                }
            }
        }
        let at = tab.cursor?;
        let kind = kind_at(at)?;
        let path = tab.target_at(at)?;
        if diffing && kind == Kind::Picture && Self::changed_here(tab) {
            return Some(Ask::AgainstHead(path));
        }
        Some(Ask::One(path, kind))
    }

    /// Whether any preview has been asked for and has not come back yet.
    ///
    /// For `--shot --preview`, which would otherwise photograph the word `Reading…`: a decode or a
    /// dependency walk takes tens of milliseconds, which is several frames.
    pub fn preview_pending(&self) -> bool {
        self.panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .any(|tab| tab.preview.busy())
    }

    /// What the drop highlight should cover in this pane, if anything.
    ///
    /// The folder row under the pointer when there is one, and the listing otherwise — which
    /// mirrors where the drop will actually go, since a row is published as a drop zone of its own
    /// and wins over the listing it sits in. A listing-wide highlight over a subfolder would
    /// promise the wrong destination.
    pub(super) fn preview_rect(&self, pane: PaneId, scale: f32) -> Option<Rect> {
        if self.refusing() {
            return None;
        }
        let (x, y) = self.drop_hover?;
        let pane = self.panes.iter().find(|p| p.id == pane)?;
        if pane.tab().path.as_os_str().is_empty() {
            return None;
        }
        // The hover point arrives in physical pixels, as the drop zones are published.
        let scale = scale.max(0.01);
        let at = egui::pos2(x as f32 / scale, y as f32 / scale);
        if !pane.drop_area.contains(at) {
            return None;
        }
        Some(
            pane.drop_rows
                .iter()
                .find(|(row, _)| row.contains(at))
                .map(|(row, _)| row.intersect(pane.drop_area))
                .unwrap_or(pane.drop_area),
        )
    }

    /// [`App::preview_rect`], for the test that checks which rect it picks.
    #[cfg(test)]
    pub fn preview_rect_for_tests(&self, pane: PaneId, scale: f32) -> Option<Rect> {
        self.preview_rect(pane, scale)
    }

    /// Whether the drag over the window is one this program will not take.
    ///
    /// **The destination highlight stands down for it, and that is the point of asking.** The
    /// highlight is a promise — an accent wash over the folder a drop is about to land in — and a
    /// place lighting up to accept what it is refusing is the one thing worse than saying nothing.
    /// What appears instead is the sentence and its mark: see [`crate::ui::drag_saying`], and
    /// [`crate::shell::dnd::refuses`] for which drops these are.
    ///
    /// Or nothing appears at all, which is the second half: a hushed drop — see
    /// [`crate::shell::dnd::Shared::silent`] — has no sentence to put in the highlight's place, and
    /// a folder lit up for a move that would do nothing is the promise this exists to prevent.
    fn refusing(&self) -> bool {
        self.drop_silent
            || self
                .drop_telling
                .as_ref()
                .is_some_and(|told| told.refused.is_some())
    }

    /// Draw the drag itself: what it is carrying, and what letting go would do.
    ///
    /// Only while a drag is over the window, which is what [`App::drop_hover`] says — and it is the
    /// only thing that says it, because a drag holds the pointer and egui's own is wherever the
    /// gesture left it.
    ///
    /// **The ghost is only drawn for a drag this window started.** What another program is carrying
    /// is that program's to picture, and Explorer already does — a second ghost drawn over its own
    /// is not this window's to add. The sentence and the highlight still appear for those.
    pub(super) fn draw_the_drag(&mut self, ui: &mut egui::Ui, t: &Theme, screen: Rect) {
        let Some((x, y)) = self.drop_hover else {
            return;
        };
        let scale = ui.ctx().pixels_per_point().max(0.01);
        let at = egui::pos2(x as f32 / scale, y as f32 / scale);

        // What it is carrying, above the pointer; what letting go would do, below it.
        if let Some((icons, count)) = self.ghost_icons(ui.ctx()) {
            crate::ui::drag_ghost(ui.painter(), t, at, screen, &icons, count);
        }
        // The ghost stays even with nothing to promise — the files are still in the air over a
        // place that will not take them, and a pointer that dropped them for a moment would lie.
        let Some(told) = self.saying() else {
            return;
        };
        crate::ui::drag_saying(
            ui.painter(),
            t,
            at + egui::vec2(CLEARANCE, CLEARANCE),
            screen,
            told,
        );
    }

    /// The sentence to draw beside the pointer, if there is one.
    ///
    /// `None` over a place that would take no drop, and `None` for the one refusal not worth saying
    /// — see [`crate::shell::dnd::Shared::silent`], which the drop target decides and the cursor
    /// obeys as well. Asked in one place because the drawing and the test that measures it have to
    /// get the same answer.
    fn saying(&self) -> Option<&crate::shell::dnd::Told> {
        self.drop_telling.as_ref().filter(|_| !self.drop_silent)
    }

    /// The icons for the ghost, and how many files there really are.
    ///
    /// `None` unless this window started the drag: see [`App::draw_the_drag`]. The icons are the
    /// per-*type* ones the listing draws for most of its rows — see [`super::Dragging::ghost`] for
    /// why the type and not the item — out of the same atlas, so the ghost costs one textured quad
    /// an icon and no new lookup.
    fn ghost_icons(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<(Vec<(egui::TextureId, Rect)>, usize)> {
        let dragging = self.file_drag.as_ref()?;
        let count = dragging.items.len();
        if count == 0 {
            return None;
        }
        // Back to front, so the item that was picked up first is the one on top of the stack.
        // Cloned out first because both halves of the lookup want the cache: asking for an icon
        // can queue a question, and getting at the atlas can upload the answer.
        let wanted: Vec<(String, bool)> = dragging.ghost.iter().rev().cloned().collect();
        let icons: Vec<(egui::TextureId, Rect)> = wanted
            .iter()
            .filter_map(|(ext, is_dir)| {
                let icon = self.icons.kind(ext, *is_dir)?;
                self.icons.uv(ctx, icon)
            })
            .collect();
        (!icons.is_empty()).then_some((icons, count))
    }

    /// Where [`App::draw_the_drag`] would put the sentence, without drawing it.
    #[cfg(test)]
    pub fn drag_saying_for_tests(
        &self,
        ctx: &egui::Context,
        screen: Rect,
    ) -> Option<(String, Rect)> {
        let (x, y) = self.drop_hover?;
        let scale = ctx.pixels_per_point().max(0.01);
        let at = egui::pos2(x as f32 / scale, y as f32 / scale);
        let told = self.saying()?.clone();
        // Into a throwaway layer: what is wanted is the arithmetic, and the painter is what does
        // it. Nothing reads this layer, and the context it is on is the test's own.
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Debug,
            egui::Id::new("drag-saying-measure"),
        ));
        let words = at + egui::vec2(CLEARANCE, CLEARANCE);
        let rect = crate::ui::drag_saying(&painter, &self.theme, words, screen, &told);
        Some((told.sentence(), rect))
    }

    /// What the drop highlight should cover in the Bookmarks section, when a drag is over it.
    ///
    /// **The group under the pointer when there is one, and the whole section otherwise** —
    /// which mirrors where the drop will actually go, since a group is published as a zone of its
    /// own and wins over the section it sits in. The same rule, and the same reason, as
    /// [`Self::preview_rect`] in a listing: a section-wide highlight over a group would promise
    /// the wrong destination.
    ///
    /// A group means its name *and* the bookmarks under it, which is what makes the highlight a
    /// picture of the thing being joined rather than of the row it was aimed at.
    ///
    /// Between two bookmarks there is nothing to promise — pinning appends — which is also why
    /// the pointer is answered `LINK` rather than copy or move.
    pub(super) fn bookmarks_preview(&self, scale: f32) -> Option<Rect> {
        if self.refusing() {
            return None;
        }
        let (x, y) = self.drop_hover?;
        let rect = self.bookmarks_rect?;
        let at = egui::pos2(x as f32 / scale.max(0.01), y as f32 / scale.max(0.01));
        if !rect.contains(at) {
            return None;
        }
        Some(
            self.bookmark_rows
                .iter()
                .find(|(row, _)| row.contains(at))
                .map(|(row, _)| row.intersect(rect))
                .unwrap_or(rect),
        )
    }
}
