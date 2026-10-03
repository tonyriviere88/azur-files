//! One frame, from the top: the title bar, the sidebar, the dock of panes, and the panels
//! inside each of them.
//!
//! Nothing here changes structure. Every gesture the drawing finds turns into a
//! [`super::Action`] and is performed afterwards — see [`super::App::apply`].

use super::*;

impl App {
    pub fn frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if !self.installed {
            azur::install(
                &ctx,
                self.theme.azur(),
                &azur::StyleOptions {
                    // A dense, table-heavy window, which is exactly the case the
                    // design system documents these two for.
                    control_height: azur::tokens::control::SMALL,
                    selectable_labels: false,
                    ..Default::default()
                },
            );
            self.installed = true;
        }
        // **Not while a video is filling the screen**, which is the same argument the size below
        // makes: filling it means the maximise bit comes off the window — see `win::fill_screen` —
        // so a window that was maximised reports itself restored for as long as the video is up.
        // Believing that loses the state to go back to, and a session left maximised and closed from
        // fullscreen reopened as a small window in a corner.
        if self.fullscreen_video.is_none() {
            self.maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        }
        self.close_on_blur(&ctx);
        // Keep painting while the window is being restored, resized or rescaled, so the
        // compositor never has to stretch a frame that was drawn for a different shape.
        let now = ctx.input(|i| i.time);
        if self.settling.observe(Shape::of(&ctx), now) {
            ctx.request_repaint();
        }
        // `--walk`: step to the next folder by itself, so a real window can be measured
        // browsing rather than sitting still.
        if let Some(next) = self.walk.as_mut().and_then(|walk| walk.step(now)) {
            let pane = self.focused;
            self.perform(&ctx, Action::Navigate { pane, path: next });
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(Walk::STEP));
        }
        // `--scroll`: keep the listing moving, so the painting is measured and not just the
        // reading.
        if self.scrolling.on {
            let span = self
                .panes
                .iter()
                .find(|p| p.id == self.focused)
                .map_or(0.0, |p| {
                    let tab = p.tab();
                    if tab.view_mode.is_icons() {
                        tab.grid.height()
                    } else {
                        tab.order.len() as f32 * crate::pane::ROW_HEIGHT
                    }
                });
            if let Some(to) = self.scrolling.step(span) {
                if let Some(pane) = self.panes.iter_mut().find(|p| p.id == self.focused) {
                    pane.tab_mut().scroll_to = Some(to);
                }
                ctx.request_repaint();
            }
        }
        // `--trace`: the memory figures, every few seconds, beside what the cache is holding.
        // Together they say whether growth is the cache filling up or something being kept.
        //
        // A repaint has to be booked for it, because this program is idle between events and
        // a report that only fires when something else asks for a frame is a report that
        // never fires while you sit and watch the number climb.
        if self.settling.trace {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(Settling::REPORT));
            if now - self.settling.reported_at > Settling::REPORT {
                self.settling.reported_at = now;
                let (private, gdi, user) = process_memory();
                let (dirs, entries) = self.loader.held();
                let (kinds, paths, textures) = self.icons.held();
                let (known, pictures, pages, spare) = self.thumbs.held();
                let (gaveup, queued) = self.thumbs.stuck();
                println!(
                    "{now:8.1}  private {:>7} KB   gdi {gdi:>5}   user {user:>4}   \
                     cache {dirs:>3} folders / {entries:>8} entries   \
                     icons {kinds:>4} kinds / {paths:>5} paths / {textures:>4} textures /                      {} bitmaps / {} uploads   \
                     thumbs {known:>4} known / {pictures:>4} drawn / {gaveup:>4} gaveup / {queued:>3} queued / {:>4} asks / {pages} pages / {spare:>4} spare / {} fetched / {} uploads",
                    private / 1024,
                    self.icons.bitmaps,
                    self.icons.uploads,
                    self.thumbs.asks,
                    self.thumbs.fetched,
                    self.thumbs.uploads,
                );
            }
        }
        // A drive may have been plugged in or ejected while the window was in the
        // background, and regaining focus is the one moment it is worth re-probing.
        if ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::WindowFocused(true)))
        }) {
            self.volumes.refresh(&ctx);
        }
        self.volumes.poll();
        self.icons.poll(&ctx);
        // Which listings are on screen, so the icon worker can drop the questions it still has
        // about folders nobody is looking at any more.
        let views: Vec<u64> = self
            .panes
            .iter()
            .flat_map(|pane| pane.tabs.iter())
            .map(|tab| tab.view)
            .collect();
        self.icons.only(&views);
        // Which folders the tiles' pictures may still be wanted for, for the same reason: one that has
        // been scrolled away from must not hold up the one in front of it.
        //
        // **Delivery is at the *end* of the frame** rather than here beside the icons' — see
        // [`crate::shell::thumbs::Thumbs::poll`], which is where that has to happen and why.
        self.thumbs.only(&views);
        self.deliver_icons();
        self.deliver_links();
        self.collect_previews(&ctx, now);
        // Both of these give memory back once nothing has wanted it for two minutes, so a
        // session that browsed a hundred folders and then settled does not hold the
        // high-water mark until the window closes.
        self.loader.sweep();
        self.clipboard_has_files = crate::shell::clipboard::has_files();
        self.collect_operations();
        // Registered on the window rather than at startup, because the handle does not
        // exist until the platform has made one.
        self.drops.attach(self.owner, &ctx);
        self.publish_drop_targets(&ctx);
        self.collect_drops(&ctx);
        self.pump_drag(&ctx);
        self.pump_asking(&ctx);
        self.collect_changes(&ctx);
        self.collect_modal();
        // **Nor while a video is filling the screen**, which is the same argument the maximised test
        // makes and a sharper case of it: fullscreen reports itself as neither maximised nor resized,
        // so without this the monitor's size would be written to the settings file as the window's and
        // the program would reopen filling the screen for good.
        if !self.maximized && self.fullscreen_video.is_none() {
            // Only a restored window's size is worth remembering; a maximised one is
            // described by the flag, and saving the screen size would pin the window
            // to this monitor.
            let size = ctx.viewport_rect().size();
            if size.x >= 640.0 && size.y >= 400.0 {
                self.window_size = Some([size.x, size.y]);
            }
            // Where it is, in physical pixels: the *outer* rect, because that is what the
            // platform places and what it will be given back. egui reports it in points, so
            // this multiplies by the same scale factor the restore divides by — which is what
            // makes the round trip exact whatever the monitor's DPI turns out to be.
            //
            // `None` while the window is minimised, when there is no position to have; the
            // last real one stays, which is the one worth reopening at.
            if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
                let scale = ctx.pixels_per_point();
                self.window_position = Some([outer.min.x * scale, outer.min.y * scale]);
            }
        }

        self.collect_scans(&ctx);
        // Before the next round of asking, so a share that has just been signed in to is re-read on
        // this frame rather than the one after — this is what turns the dialog's OK into a listing.
        self.collect_connections(&ctx);
        self.start_scans(&ctx, now);
        // After the scans, because a listing that landed this frame is a folder to ask about this
        // frame — the branch and the marks then arrive one answer later rather than one navigation
        // later.
        self.collect_git(now);
        // And after both, for the same reason: a listing that landed this frame is a folder to start
        // counting this frame, and a folder counted this frame is one whose bars are right on the
        // frame it appears in rather than one behind. See [`crate::sizes`].
        self.collect_sizes(&ctx, now);

        // Swapped out rather than borrowed, because the drawing code needs `&Theme`
        // and `&mut self` at the same time. The placeholder is the same side of the
        // palette, so nothing can read the wrong one.
        let placeholder = if self.theme.dark {
            Theme::dark()
        } else {
            Theme::light()
        };
        let theme = std::mem::replace(&mut self.theme, placeholder);

        // ---- Geometry, before anything is drawn -------------------------
        //
        // A pane's tabs sit directly above that pane, in the title bar or on a band of
        // their own, so where the panes are has to be known before the bar can be drawn —
        // and the bands take room off the top of the panes under them, so the panes cannot
        // be drawn until the bands are decided either. Everything here paints at explicit
        // rects rather than through egui's panels, which is what makes that ordering free
        // to choose: the title bar is drawn *last*, over canvas nothing else wanted.
        let screen = ctx.viewport_rect();

        // **A video filling the screen, and then nothing else at all.**
        //
        // Drawn instead of the window rather than over it, which is the whole reason this is here
        // rather than beside the drag and the menu at the bottom of this function. Over the top it
        // would be one layer above a listing that was still hit-testing every point of itself, so a
        // click meant for the scrubber would also land on whatever row happened to be under it — and
        // the panes would still be laying out, measuring columns and asking the shell for thumbnails
        // nobody can see. Instead of, there is nothing underneath to reach or to pay for.
        //
        // The keyboard still runs, because the way out is a key. See [`App::theatre`].
        if self.theatre(ui, &theme, screen) {
            self.keyboard(&ctx);
            self.apply(&ctx);
            return;
        }

        let bar = chrome::bar_rect(screen);
        let body = Rect::from_min_max(pos2(screen.left(), bar.bottom()), screen.max);
        let plan = self.plan_layout(body, bar);

        self.tab_slots.clear();
        self.body(ui, &theme, body, &plan);
        let in_bar = chrome::title_bar(
            ui,
            &theme,
            &self.panes,
            &plan.in_bar,
            self.focused,
            self.maximized,
            self.sidebar_shown,
            &self.drag,
            &mut self.icons,
            &mut self.actions,
        );
        self.tab_slots.extend(in_bar);

        // Resolved after the panes, so the rects the pointer is tested against are the
        // ones drawn this frame rather than last frame's.
        if self.drag.is_some() {
            let slots = std::mem::take(&mut self.tab_slots);
            let mut drag = self.drag.take();
            chrome::resolve_drag(ui, &theme, &self.panes, &slots, &mut drag, &mut self.actions);
            self.drag = drag;
            self.tab_slots = slots;
        }

        // Over everything, and in the root `Ui` so its coordinates are the screen's.
        self.draw_menu(ui, &theme);

        // The drag itself — the files under the pointer, and what letting go would do — over the
        // highlight that says where. After the panes, because it is the one thing on screen that
        // must not be behind them, and drawn from a *state* rather than from a hover because during
        // a drag the pointer belongs to OLE and egui has none.
        self.draw_the_drag(ui, &theme, screen);

        // Last, so they are on top of everything — though they are sized to sit in
        // canvas the panels do not reach, so there is nothing to be on top of.
        chrome::resize_borders(ui, self.maximized);

        // An undecorated window has no frame of its own, and on a dark desktop its
        // edge would be invisible. `stroke-default` is the line Azur gives a panel.
        let edge = ctx.viewport_rect();
        ui.painter().rect_stroke(
            edge,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, self.theme.stroke.default),
            egui::StrokeKind::Inside,
        );

        self.keyboard(&ctx);
        self.thumb_buttons(&ctx);
        self.apply(&ctx);

        // **The tiles' pictures are taken delivery of here, after the panes have been drawn**, and the
        // position in this function is the whole of what makes the atlas's eviction rule exact: a cell
        // is reusable precisely when no tile drew it in the frame that has just finished, and that is
        // only knowable once the frame has finished. Polled at the top of the frame, as everything else
        // here is, the newest information would be a frame old and "on screen" would have to be guessed
        // at. See [`crate::shell::thumbs::Thumbs::poll`].
        self.thumbs.poll();
    }

    /// The video filling the screen, if one is. Answers whether it drew — in which case that is the
    /// whole frame.
    ///
    /// **Four ways out, and they are all one action.** The button in the strip, a double click on the
    /// picture, `Escape`, and this: the player it was about is no longer there. The last one is not a
    /// corner case — closing the panel, switching tabs and a background scan moving the selection all
    /// reach it, and without it the window would be left fullscreen with nothing in it.
    fn theatre(&mut self, ui: &mut Ui, t: &Theme, screen: Rect) -> bool {
        let App {
            panes,
            preview,
            actions,
            fullscreen_video,
            ..
        } = self;
        let Some(id) = *fullscreen_video else {
            return false;
        };
        let drew = panes
            .iter_mut()
            .find(|pane| pane.id == id)
            .map(|pane| pane.tab_mut())
            .is_some_and(|tab| {
                crate::ui::preview::theatre(ui, t, screen, id, &mut tab.preview, preview, actions)
            });
        if !drew {
            // Through the action rather than by clearing the field, because the *window* has to be
            // given back as well and that is the action's job. One frame of an empty window is what
            // it costs, and the alternative is two places that both know how to leave fullscreen.
            actions.push(Action::ToggleVideoFullscreen(id));
        }
        drew
    }

    /// Divide the window up and lay the panes out, without drawing anything.
    ///
    /// Split out from [`App::body`] because the title bar needs the answer before it can
    /// place its tab strips, and because it is arithmetic worth being able to check on its
    /// own. Returns where every pane's tabs go, with the panes already reduced by the room
    /// their strip bands take.
    pub(super) fn plan_layout(&mut self, body: Rect, bar: Rect) -> chrome::StripPlan {
        let (_, panes_area) = Self::split_body(body, self.sidebar_width, self.sidebar_shown);
        self.layout
            .layout(panes_area, &mut self.pane_rects, &mut self.splitters);
        self.pane_order = self.pane_rects.iter().map(|(id, _)| *id).collect();
        chrome::plan_strips(&mut self.pane_rects, bar)
    }

    /// The sidebar and the area the panes divide between them.
    ///
    /// The body, whole: the panels reach the window's edges, and the only thing between a panel
    /// and the outside is the window's own one-pixel border painted over the top of it. They used
    /// to stop [`GUTTER`] short of it on every side, which put a band of canvas round the block
    /// and made it read as a tray of cards.
    ///
    /// [`crate::ui::SEAM`] between the two, and that is the only division in here.
    ///
    /// **With the panel hidden the seam goes too**, and the panes have the body whole: a one-point
    /// line down the left of a window with nothing to the left of it is a line dividing a thing from
    /// no thing. `Rect::NOTHING` for the sidebar then, which is what every caller tests rather than a
    /// second return value — a rect with no area is already the answer to "is there a panel", and it
    /// draws, hit-tests and clips as nothing wherever one is handed on by mistake.
    pub(super) fn split_body(body: Rect, sidebar_width: f32, shown: bool) -> (Rect, Rect) {
        let inner = body;
        if !shown {
            return (Rect::NOTHING, inner);
        }
        let width = sidebar_width.clamp(140.0, (inner.width() - 240.0).max(140.0));
        let sidebar = Rect::from_min_max(inner.min, pos2(inner.left() + width, inner.bottom()));
        let panes_area =
            Rect::from_min_max(pos2(sidebar.right() + crate::ui::SEAM, inner.top()), inner.max);
        (sidebar, panes_area)
    }

    /// The sidebar, the splitter between it and the panes, the tab bands, and the panes.
    pub(super) fn body(&mut self, ui: &mut Ui, t: &Theme, full: Rect, plan: &chrome::StripPlan) {
        if full.width() < 80.0 || full.height() < 40.0 {
            return;
        }

        let inner = full;
        let (sidebar, panes_area) = Self::split_body(full, self.sidebar_width, self.sidebar_shown);

        // ---- What shows through the seams ---------------------------------
        //
        // One fill behind the whole block, so every boundary between two panels is whatever
        // this leaves showing — and no panel has to know where its neighbours are. That
        // matters more than it sounds: panes come from a tree of splits, so "where the seams
        // are" is the layout's answer and it changes with every drag, whereas "the panels do
        // not quite cover this" is true for free.
        ui.painter()
            .rect_filled(inner, egui::CornerRadius::ZERO, crate::ui::seam(t));

        // ---- The sidebar --------------------------------------------------
        //
        // Its content fills it. There was a point of inset here, left from when the panel wore a
        // one-pixel ring and the content had to start inside it; with the ring gone it was a
        // point of nothing, on all four sides of both panels.
        //
        // `background-layer-alt`, not `self.surface`'s `background-layer`: the sidebar reads
        // as the same surface as the title bar and the status bar, not as a pane.
        //
        // **All of it is behind the switch**, the splitter below included: a grip beside a panel that
        // is not there would be four points of window that set a resize cursor and dragged nothing.
        // The two rects a drag is tested against are cleared for the same reason — see
        // [`App::bookmarks_preview`], which would otherwise go on offering a drop onto a group nobody
        // can see.
        if !self.sidebar_shown {
            self.bookmarks_rect = None;
            self.bookmark_rows.clear();
        } else {
            ui.painter()
                .rect_filled(sidebar, egui::CornerRadius::ZERO, t.bg.layer_alt);
            {
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(sidebar)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                child.set_clip_rect(sidebar.intersect(ui.clip_rect()));
                let current = self
                    .panes
                    .iter()
                    .find(|p| p.id == self.focused)
                    .map(|p| p.tab().path.clone())
                    .unwrap_or_default();
                let mut marks = sidebar::Marks {
                    list: &self.bookmarks,
                    editing: &mut self.bookmark_edit,
                    rows: &mut self.bookmark_rows,
                    // A drag from outside is over the window, so the `+` stands down — see
                    // [`sidebar::Marks::dragging`]. Read from the hover the OLE callbacks
                    // publish, which is the only thing that knows a drag is in flight at all.
                    dragging: self.drop_hover.is_some(),
                };
                self.bookmarks_rect = sidebar::show(
                    &mut child,
                    t,
                    self.volumes.all(),
                    self.volumes.shares(),
                    self.volumes.servers(),
                    self.volumes.found(),
                    self.volumes.finding(),
                    &mut marks,
                    &self.places,
                    &current,
                    self.focused,
                    &mut self.sections,
                    &mut self.icons,
                    &mut self.scratch,
                    &mut self.actions,
                );
                // A drag hovering over Bookmarks. Same highlight the listing gets, so pinning
                // reads as a drop rather than as nothing happening — and drawn here, after the
                // rows, for the same reason it is in the pane.
                if let Some(area) = self.bookmarks_preview(ui.ctx().pixels_per_point()) {
                    crate::ui::drop_target(child.painter(), area, t);
                }
            }

            // ---- The splitter beside it -----------------------------------
            let grip = Rect::from_min_max(
                pos2(sidebar.right() - 3.0, inner.top()),
                pos2(panes_area.left() + 3.0, inner.bottom()),
            );
            let response = ui.interact(
                grip,
                egui::Id::new("sidebar-grip"),
                egui::Sense::click_and_drag(),
            );
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if response.dragged() {
                self.sidebar_width =
                    (self.sidebar_width + response.drag_delta().x).clamp(140.0, 520.0);
                self.config_dirty = true;
            }
            // Double-clicking a splitter puts it back where it started, which is the same gesture
            // the column edges in the listing already answer to.
            if response.double_clicked() {
                self.sidebar_width = crate::config::SIDEBAR_WIDTH;
                self.config_dirty = true;
            }
        }

        // ---- The panes ----------------------------------------------------
        //
        // Already laid out by `plan_layout`, which had to run before the title bar.

        // ---- The tab bands, for the rows the title bar cannot reach --------
        //
        // Painted like the title bar, because that is what they are for the row below
        // them: `background-layer-alt` and a hairline along the bottom.
        for row in &plan.rows {
            ui.painter()
                .rect_filled(row.band, egui::CornerRadius::ZERO, t.bg.layer_alt);
            crate::ui::rule_below(ui.painter(), row.band, t);
        }

        let rects = self.pane_rects.clone();
        for (id, rect) in rects {
            self.pane(ui, t, id, rect);
        }

        // ---- The splitters, between the panes and the tabs ------------------
        //
        // **The order here is the priority, and it has to be exactly this.** Within a layer egui
        // gives a click to the *last* widget that asked for it ("in tie, pick last = topmost" in its
        // `hit_test`), so registering these decides what wins where they overlap — and they overlap
        // two things, in opposite directions.
        //
        // *After the panes*, because [`dock::GRAB`] reaches four points into the pane on each side and
        // the listing's vertical scrollbar is exactly those four points. Registered before the panes,
        // the scrollbar won and **two panes side by side could not be resized at all** — the pointer
        // over the divider was the scrollbar's. A stacked pair was never affected: there the grab
        // reaches into the pane's *bottom* edge, and a `ScrollArea::vertical` has nothing there.
        //
        // *Before the tab strips*, because the grab spans the split's full height, tab bands included.
        // Last would mean a splitter taking clicks off the tabs nearest a pane boundary.
        //
        // Nothing here paints, so none of this changes what is drawn.
        let mut ratios: Vec<(Vec<u8>, f32)> = Vec::new();
        for splitter in &self.splitters {
            let response = ui.interact(
                splitter.rect,
                egui::Id::new(("splitter", &splitter.route)),
                egui::Sense::click_and_drag(),
            );
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(if splitter.horizontal {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
            }
            if response.dragged() {
                let delta = if splitter.horizontal {
                    response.drag_delta().x / panes_area.width().max(1.0)
                } else {
                    response.drag_delta().y / panes_area.height().max(1.0)
                };
                ratios.push((splitter.route.clone(), delta));
            }
            // A double click evens the split up again.
            if response.double_clicked() {
                if let Some(ratio) = self.layout.ratio_at(&splitter.route) {
                    *ratio = 0.5;
                }
            }
        }
        for (route, delta) in ratios {
            if let Some(ratio) = self.layout.ratio_at(&route) {
                *ratio = (*ratio + delta).clamp(dock::RATIO_MIN, dock::RATIO_MAX);
            }
        }

        // The strips themselves after the panes, so a tab is never under a pane's card.
        for row in &plan.rows {
            for (id, strip) in &row.strips {
                let Some(index) = self.panes.iter().position(|p| p.id == *id) else {
                    continue;
                };
                let inner = Rect::from_min_max(
                    pos2(strip.left() + GUTTER, strip.top()),
                    pos2(strip.right() - GUTTER, strip.bottom()),
                );
                let focused = *id == self.focused;
                let slots = chrome::tab_strip(
                    ui,
                    t,
                    inner,
                    &self.panes[index],
                    focused,
                    &self.drag,
                    &mut self.icons,
                    &mut self.actions,
                );
                self.tab_slots.extend(slots);
            }
        }
    }

    /// One pane: its surface, its path bar, its listing.
    pub(super) fn pane(&mut self, ui: &mut Ui, t: &Theme, id: PaneId, rect: Rect) {
        let Some(index) = self.panes.iter().position(|p| p.id == id) else {
            return;
        };
        self.panes[index].rect = rect;
        // The card is drawn whatever the size, so a pane squeezed past the point of
        // usefulness still reads as a pane you can drag wider rather than as a hole
        // in the window.
        self.surface(ui, t, rect);
        if rect.width() < 96.0 || rect.height() < 48.0 {
            return;
        }

        // Which pane the keyboard is in is said by the accent under its active tab, and
        // only there. A ring around the whole card says the same thing ten times as
        // loudly, and in a two-pane window it turns every click into a visible change of
        // frame — so the pane itself stays quiet.
        // **Whether the *listing* has the keyboard**, which is not the same as whether the pane does:
        // this pane's console may be holding it, and then the arrow keys are the console's and the
        // rows have to say so. Asked of the panel — [`console::State::keeps_keys`] is the one
        // definition, and the same call the panel makes before it reads a key, so a row cannot draw
        // itself focused in a frame the keys were going somewhere else.
        let console_open = self.panes[index].console_open;
        let console_has_keys =
            console_open && self.panes[index].console_state.keeps_keys(ui.ctx(), id);
        let focused = id == self.focused && !console_has_keys;

        // The whole pane, with nothing held back: the path bar reaches the seams on both sides
        // and the listing reaches the bottom edge. The point of inset that used to be here was
        // room for the ring the panel no longer wears.
        let inside = rect;
        let bar = Rect::from_min_size(inside.min, vec2(inside.width(), breadcrumb::HEIGHT));
        // **The status line is the floor of the pane**, across its whole width, and everything else is
        // stacked on top of it. It is taken off *first* for that reason: it is the one piece of
        // furniture that is about the pane rather than about a panel inside it, and a pane whose bottom
        // edge is a status line under one half and a preview panel under the other has two floors.
        //
        // Which is also why it is not the listing's to place any more. It was, and it looked right for
        // as long as the preview panel went along the bottom — where it lands *above* the line either
        // way — and wrong the moment the panel was docked to the right, where it ran down past the
        // line's left end to the pane's own edge.
        let floor = Rect::from_min_max(
            pos2(inside.left(), inside.bottom() - crate::ui::filelist::STATUS_HEIGHT),
            inside.max,
        );
        // Everything between the path bar and the floor, which the listing and this folder's preview
        // panel divide between them. The panel is *inside* the pane — see [`crate::ui::preview`] — so
        // it is this shape that `Where::Auto` reads, and both rects come out of one split.
        let body = Rect::from_min_max(bar.left_bottom(), pos2(inside.right(), floor.top()));
        let open = self.panes[index].tab().preview.open;
        let (list, panel) = crate::ui::preview::split(body, open, self.preview);

        // **The console is in the stack, not over it.** It takes its band off the bottom of the
        // *listing's* region, so the order down a pane is rows, console, status — and nothing about
        // where the preview panel goes changes.
        //
        // The listing still gets the whole of `list` to draw in. What it gets told is how much of the
        // bottom to leave alone, because a rect that reaches under the console is one whose rows are
        // laid out and hit-tested there — the console painting over them is what makes it look like an
        // overlay.
        let (rows, console) =
            crate::ui::console::split(list, self.panes[index].console_open, self.console_share);
        let reserved = (list.bottom() - rows.bottom()).max(0.0);

        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inside)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(inside.intersect(ui.clip_rect()));

        // A click anywhere in the pane moves the keyboard here.
        let claim = child.interact(rect, egui::Id::new(("pane-claim", id)), egui::Sense::click());
        if claim.clicked() || claim.secondary_clicked() {
            self.actions.push(Action::Focus(id));
        }

        // The path bar paints its own surface, in `breadcrumb::show`. It used to be
        // `background-layer` like the rest of the pane with a `stroke-subtle` hairline under it
        // to say where it ended; now it is [`crate::ui::seam`], the fill says it, and a hairline
        // between two fills that already differ is a third line nobody asked for.

        let Self {
            panes,
            crumbs,
            complete,
            loader,
            actions,
            scratch,
            zone,
            icons,
            thumbs,
            links,
            cut,
            notice,
            ops,
            preview,
            flat_mode,
            regroup,
            forward_slashes,
            auto_tiles,
            providers,
            ..
        } = self;
        let (flat_mode, regroup, slashes) = (*flat_mode, *regroup, *forward_slashes);
        let auto_tiles = *auto_tiles;
        // A copy in progress, or the last thing that went wrong: whichever there is,
        // the pane's status line says so instead of counting files.
        let status = ops.in_progress().or(notice.as_deref());
        let pane = &mut panes[index];
        let tab = pane.tab_mut();

        breadcrumb::show(
            &mut child, t, bar, id, tab, crumbs, complete, loader, icons, preview, flat_mode,
            regroup, slashes, actions,
        );
        let outcome = filelist::show(
            &mut child, t, zone, list, floor, id, tab, focused, icons, links, thumbs, cut, status,
            console_open, auto_tiles, providers, reserved, scratch, actions,
        );
        // After the listing, so the panel's surface is over it rather than under: the listing
        // reaches for the whole body when it measures its own columns, and a panel drawn first
        // would have a row's fill painted across it.
        if let Some(panel) = panel {
            // Whether git has a different version of what the panel is about, which is what decides
            // whether a *picture* is offered the diff toggle. A text file answers that from its own
            // payload — the diff came back with it — and a picture cannot: the comparison is what the
            // toggle asks for, so with it off there is nothing in the panel that knows.
            let changed = Self::changed_here(tab);
            crate::ui::preview::show(
                &mut child,
                t,
                panel,
                id,
                &mut tab.preview,
                preview,
                body,
                changed,
                scratch,
                actions,
            );
        }

        // Last of the three, and after the listing for the same reason the preview panel is: the
        // listing paints a row's fill across the whole width of its body, so anything drawn under
        // it comes out with a highlight through it.
        if let Some(console) = console {
            self.console_panel(&mut child, t, id, index, console, list);
        }

        self.panes[index].drop_rows = outcome.drop_rows;
        self.panes[index].drop_area = outcome.drop_area;
        if let Some(path) = outcome.prefetch {
            self.loader.prefetch(&path);
        }

        // Where a drag hovering over this pane would land. Painted *after* the listing: a
        // selected row's fill is an accent surface drawn over anything underneath it, so a
        // highlight drawn first disappeared under the one row most likely to be dropped on.
        if let Some(area) = self.preview_rect(id, ui.ctx().pixels_per_point()) {
            crate::ui::drop_target(ui.painter(), area, t);
        }
    }

    /// Draw a pane's console, and do what it asked for.
    ///
    /// **The shell is started here rather than when the panel is toggled**, and lazily: starting one
    /// needs somewhere to start it, and the folder is the pane's. It is also where a `Shift+Tab`
    /// lands — a different kind means a different process, so the old one is ended and the new one
    /// **keeps the log**, because throwing away what a session printed is not what changing shell
    /// was asked to do.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn console_panel(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        id: PaneId,
        index: usize,
        rect: Rect,
        body: Rect,
    ) {
        let ctx = ui.ctx().clone();
        let here = self.panes[index].tab().path.clone();
        let want = self.panes[index].console_state.kind();

        let stale = match &self.panes[index].console {
            Some(session) => session.kind != want,
            None => self.panes[index].console_failed != Some(want),
        };
        if stale {
            // What the old session printed, minus anything still running — a block whose sentinel
            // is coming from a shell that no longer exists would never close.
            let kept = self
                .panes[index]
                .console
                .take()
                .map_or_else(Vec::new, |mut old| {
                    let mut blocks = std::mem::take(&mut old.blocks);
                    blocks.retain(|block| !block.running());
                    blocks
                });
            match crate::console::Session::start(want, &here, &ctx) {
                Ok(mut session) => {
                    session.blocks = kept;
                    self.panes[index].console = Some(session);
                    self.panes[index].console_failed = None;
                }
                Err(what) => {
                    self.panes[index].console_failed = Some(want);
                    self.notice = Some(what);
                }
            }
        }
        if let Some(session) = &mut self.panes[index].console {
            session.poll();
            // Whatever `--console` asked for, now that there is something to ask.
            for command in std::mem::take(&mut self.panes[index].console_queue) {
                if let Some(session) = &mut self.panes[index].console {
                    session.send(Some(&here), &command);
                }
            }
        }

        let share = self.console_share;
        let out = {
            let pane = &mut self.panes[index];
            crate::ui::console::show(
                ui,
                t,
                rect,
                id,
                &mut pane.console_state,
                pane.console.as_mut(),
                Some(here.as_path()),
                body,
                share,
            )
        };

        let mut orphan = None;
        if let Some(session) = &mut self.panes[index].console {
            if let Some(command) = &out.send {
                session.send(Some(&here), command);
            }
            if out.stop && session.running() {
                session.stop(&ctx);
            }
            if out.clear {
                session.clear();
            }
        } else {
            // Nothing to run it in, which is only reachable while a shell is refusing to start.
            orphan = out.send.clone();
        }
        if let Some(command) = orphan {
            self.notice = Some(format!("no {} to run `{command}` in", want.label()));
        }
        if out.claimed {
            self.actions.push(Action::Focus(id));
        }
        if let Some(text) = out.copy {
            ctx.copy_text(text);
        }
        if let Some(share) = out.share {
            self.console_share = share;
            self.config_dirty = true;
        }
        // Whichever shell was just picked is the one the next console opens on, in this pane or any
        // other, this session or the next.
        if let Some(kind) = out.swap {
            self.console_shell = kind;
            self.config_dirty = true;
        }
        // A command moved the shell, so the pane goes with it. Through the action queue like every
        // other navigation, which is what gives it the history entry and the scan.
        if let Some(path) = out.cwd {
            self.actions.push(Action::Navigate { pane: id, path });
        }
    }

    /// The surface every panel sits on: a plain square fill, and nothing else.
    ///
    /// It was a card — `radius-medium` and a `stroke-subtle` ring — which is the right
    /// treatment for a card floating on canvas and the wrong one for a panel that is part of
    /// the window's structure. Two of them side by side gave every boundary two rings and a
    /// channel of canvas between them, so the sidebar and the panes read as separate windows
    /// that happened to be next to each other.
    ///
    /// The ring is gone rather than kept-and-thinned because the seam does its job: panels are
    /// [`crate::ui::SEAM`] apart, and [`crate::ui::seam`] is already painted behind them, so
    /// the one line between two panels is a line neither of them draws.
    pub(super) fn surface(&self, ui: &Ui, t: &Theme, rect: Rect) {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::ZERO, t.bg.layer);
    }
}
