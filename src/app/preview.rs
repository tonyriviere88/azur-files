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
        // mutably: `selected_previews` reads the tab, and asking for a read writes to it. **A list
        // rather than an option**, one entry per tile — see [`Self::selected_previews`].
        type Wanted = (PaneId, usize, Vec<crate::preview::Ask>, bool);
        let wanted: Vec<Wanted> = self
            .panes
            .iter()
            .flat_map(|pane| {
                pane.tabs
                    .iter()
                    .enumerate()
                    .filter(|(at, tab)| tab.preview.open && *at == pane.active)
                    .map(|(at, tab)| {
                        (
                            pane.id,
                            at,
                            Self::selected_previews(
                                tab,
                                self.preview.diff,
                                tab.preview.comparing(),
                            ),
                            Self::can_compare(tab),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        // **A tab that has stopped being the active one is told**, before anything else, and it is
        // the one thing here that has to reach the panels the loop above filtered out. An inactive
        // tab is not drawn and not followed — so a video in one would go on playing, unseen and
        // audible, until the tab was closed. Nothing else is affected: every other view is a still
        // thing that costs nothing to be holding out of sight.
        for pane in self.panes.iter_mut() {
            let active = pane.active;
            for (at, tab) in pane.tabs.iter_mut().enumerate() {
                if at != active {
                    tab.preview.out_of_sight();
                }
            }
        }

        let mut soonest: Option<f64> = None;
        for (id, at, asks, can_compare) in wanted {
            let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) else {
                continue;
            };
            let Some(tab) = pane.tabs.get_mut(at) else {
                continue;
            };
            // The blend is only on offer while the selection is the two pictures it was asked about,
            // so this is where a latch that has outlived them is dropped.
            tab.preview.allow_compare(can_compare);
            tab.preview.follow_all(asks, now);
            let (ready, left) = tab.preview.settle_all(now);
            // Read out of the panel before the loop below borrows `self` again for the muting
            // decision, which is the whole reason it is a local.
            let focused = tab.preview.focused_at();
            let quiet_all = self.preview.muted;
            // **One request per tile that is ready**, and the tile's index travels with it: the
            // answer has to come back to the slot that asked, and by the time it does the selection
            // may well have moved.
            for (slot, ask) in ready {
                // **A video is opened here rather than asked for.** It is a player and not a read —
                // see [`crate::preview::Kind::Video`] — so there is no worker, no token and no
                // payload: the panel is handed the thing itself. Which is also why this is the one
                // request that can fail on the spot, and says so where a decoder's complaint would
                // have gone.
                //
                // Opened muted unless it is the focused tile's — every clip plays, one is audible,
                // and [`crate::ui::preview::show`] keeps that true as the focus moves.
                let player = match &ask {
                    // **The one preview an archive does not get.** Every other kind is read on a
                    // worker, where [`crate::archive::extract`] can put the bytes on a disk first —
                    // but a player is opened here, on the UI thread, and extracting a film to do it
                    // would freeze the window for as long as the film took to decompress. Media
                    // Foundation would refuse the path anyway; refusing it here is the same outcome
                    // with a sentence that says what to do about it.
                    crate::preview::Ask::One(path, crate::preview::Kind::Video)
                        if crate::archive::is_virtual(path) =>
                    {
                        Some(Err(
                            "A video inside an archive cannot be played. Copy it out first."
                                .to_owned(),
                        ))
                    }
                    crate::preview::Ask::One(path, crate::preview::Kind::Video) => {
                        Some(crate::preview::Player::open(
                            path,
                            quiet_all || slot != focused,
                            ctx,
                        ))
                    }
                    _ => None,
                };
                let token = player.is_none().then(|| self.previews.request(&ask));
                if let Some(tab) = self
                    .panes
                    .iter_mut()
                    .find(|p| p.id == id)
                    .and_then(|p| p.tabs.get_mut(at))
                {
                    match (player, token) {
                        (Some(Ok(player)), _) => tab.preview.plays(slot, ask, player),
                        (Some(Err(why)), _) => tab.preview.refused(slot, ask, why),
                        (None, Some(token)) => tab.preview.asked(slot, ask, token),
                        (None, None) => {}
                    }
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

    /// What this tab's preview panel should be showing: one [`Ask`] per tile, in listing order.
    ///
    /// **A selection of two, three or four files is that many previews**, side by side — see
    /// [`crate::ui::preview::tiles`] for the shapes. Selecting a handful of files is a deliberate act
    /// with an obvious meaning, and it is the one case where the *selection* rather than the cursor
    /// decides. Past [`crate::ui::preview::MOST`] it stops being deliberate — a select-all is not a
    /// request for forty previews — so a wider selection falls back to the cursor's one file, which
    /// is also what a selection of one is.
    ///
    /// The cursor is the answer everywhere else: it is where the keyboard is, and it follows a click.
    ///
    /// # The two comparisons, which are one file's worth of tile each
    ///
    /// Both are a *blend* — three views of the same picture in one tile — rather than two tiles, and
    /// both are asked for rather than implied:
    ///
    /// - **Two selected pictures**, when `comparing` is on. That is the panel's diff button, which is
    ///   what it means while exactly two pictures are selected; the default is off, because tiling is
    ///   what selecting two files now means. It used to be the *only* answer for two pictures, and
    ///   the button is what keeps it reachable.
    /// - **One picture git has a different version of**, when `diffing` is on. A `.png` has no lines
    ///   to put a red band behind, so "show me what changed" for one is the two versions and the
    ///   difference between them.
    ///
    /// Both toggles are read here rather than in the panel because they change *what is read*:
    /// turning either off has to produce a different [`Ask`], or the panel would go on showing the
    /// comparison it is already holding.
    /// What a row would be previewed as, from its name alone.
    ///
    /// Costs nothing to ask about a row, and a file that passes here and turns out to be something
    /// else says so in the panel rather than being refused. `None` is a row with no preview: a folder,
    /// or a name nothing has a view for.
    ///
    /// Out here because four places wanted it — the two that decide what the panel shows, and the two
    /// capture flags that stand in for the mouse — and each had written the same three arguments out
    /// for itself.
    pub(super) fn preview_kind_at(tab: &Tab, row: usize) -> Option<crate::preview::Kind> {
        let dir = tab.dir.as_ref()?;
        let entry = tab.entry_at(row)?;
        crate::preview::kind_of(
            dir.leaf(entry),
            dir.ext(entry),
            dir.entries[entry].is_dir(),
        )
    }

    pub(super) fn selected_previews(
        tab: &Tab,
        diffing: bool,
        comparing: bool,
    ) -> Vec<crate::preview::Ask> {
        use crate::preview::{Ask, Kind};

        let kind_at = |row: usize| Self::preview_kind_at(tab, row);

        // In display order, so the left-hand or upper tile is the upper row — the panel reads the way
        // the listing above it does rather than the way the clicks happened to land.
        let picked: Vec<usize> = if (2..=crate::ui::preview::MOST).contains(&tab.selected_count) {
            (0..tab.order.len())
                .filter(|&row| tab.is_selected(row))
                .collect()
        } else {
            Vec::new()
        };

        // Two pictures, blended, because the diff button asked for it. One tile, not two.
        if comparing && picked.len() == 2 {
            if let [a, b] = picked[..] {
                if kind_at(a) == Some(Kind::Picture) && kind_at(b) == Some(Kind::Picture) {
                    if let (Some(a), Some(b)) = (tab.target_at(a), tab.target_at(b)) {
                        return vec![Ask::Pair(a, b)];
                    }
                }
            }
        }

        if !picked.is_empty() {
            // A row with nothing to preview is left out rather than given an empty tile: the tiling
            // is about the files that *have* a preview, and a folder among four selected files should
            // not cost one of the four its place.
            let asks: Vec<Ask> = picked
                .iter()
                .filter_map(|&row| Some(Ask::One(tab.target_at(row)?, kind_at(row)?)))
                .collect();
            if !asks.is_empty() {
                return asks;
            }
        }

        let Some(at) = tab.cursor else {
            return Vec::new();
        };
        let Some(kind) = kind_at(at) else {
            return Vec::new();
        };
        let Some(path) = tab.target_at(at) else {
            return Vec::new();
        };
        if diffing && kind == Kind::Picture && Self::changed_here(tab) {
            return vec![Ask::AgainstHead(path)];
        }
        vec![Ask::One(path, kind)]
    }

    /// Whether the diff button should be offering the two-picture blend rather than git's changes.
    ///
    /// Exactly two selected pictures and nothing else. Asked so that the header can label its button
    /// for what pressing it will actually do, and so that [`crate::ui::preview::Preview::compare`] is
    /// dropped the moment the selection stops being the one it was about — a latch that outlived its
    /// two files would blend the next two the moment they were picked.
    pub(super) fn can_compare(tab: &Tab) -> bool {
        if tab.selected_count != 2 {
            return false;
        }
        let rows: Vec<usize> = (0..tab.order.len()).filter(|&row| tab.is_selected(row)).collect();
        rows.len() == 2
            && rows.iter().all(|&row| {
                Self::preview_kind_at(tab, row) == Some(crate::preview::Kind::Picture)
            })
    }

    /// Fill the monitor with this window, or put it back exactly as it was.
    ///
    /// Windows only, and off it there is nothing to do rather than something missing: a video is only
    /// ever [`crate::preview::Kind::Video`] on Windows, so there is no player anywhere else to fill a
    /// screen with. See `win::fill_screen` for why the window is moved rather than asked.
    pub(super) fn fill_screen(&self, on: bool) {
        #[cfg(windows)]
        crate::win::fill_screen(self.owner, on);
        #[cfg(not(windows))]
        let _ = on;
    }

    /// Spread the window across every monitor. Answers whether it did.
    ///
    /// Beside [`Self::fill_screen`] because it is the same kind of thing said to the same layer, and
    /// off Windows it is the same kind of nothing: the rectangle is a union of monitor work areas in
    /// physical pixels, which is a question no portable layer here can ask. Answering `false` is what
    /// keeps the caller from recording a shape the window was never given.
    pub(super) fn span_screens(&self) -> bool {
        #[cfg(windows)]
        return crate::win::span_screens(self.owner);
        #[cfg(not(windows))]
        false
    }

    /// Whether a video is filling the screen. For the tests; the frame reads the field.
    #[cfg(test)]
    pub fn fullscreen_for_tests(&self) -> bool {
        self.fullscreen_video.is_some()
    }

    /// The window size that would be written to the settings file. For the test that checks the
    /// monitor's size is not recorded as the window's while a video is filling it.
    #[cfg(test)]
    pub fn window_size_for_tests(&self) -> Option<[f32; 2]> {
        self.window_size
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
