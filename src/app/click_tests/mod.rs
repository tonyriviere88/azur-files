/// Driving the interface with real pointer events.
///
/// Everything else in this program can be checked by reading it. Click targets
/// cannot: an interaction rect that another one happens to cover reads perfectly
/// correctly at the call site and simply does not respond, and the only way to know is
/// to press the pointer down at a coordinate and see what moves. So these tests run
/// whole frames through a real [`egui::Context`] with synthetic input and assert on
/// what the application actually did.
use super::*;
use egui::{pos2, vec2, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect};

// One file per part of the window. [`Harness`] and the few helpers every group needs stay
// here; the tests themselves are grouped by what they press.
//
// | module | what it drives |
// | --- | --- |
// | [`details`] | rows, columns, and the space under the last one |
// | [`path_bar`] | history, Up, Refresh, and the crumbs |
// | [`breadcrumb`] | the chevrons and their dropdowns |
// | [`title_bar`] | the mark, the tabs, the window buttons, the switches |
// | [`sidebar`] | places, drives, bookmarks, the splitter |
// | [`band`] | the rubber band |
// | [`panes`] | the dock: focus, splits, seams |
// | [`path_field`] | the path field and every other text field |
// | [`flatten`] | the whole tree in one listing |
// | [`filter`] | the filter box and the shortcuts past it |
// | [`preview`] | the preview panel, every view of it |
// | [`grid`] | the tiles view |
// | [`menu`] | the context menu, this program's half and the shell's |
// | [`transfer`] | the clipboard, and drag and drop |
// | [`rescan`] | a folder changing underneath the listing |
// | [`memory`] | does browsing let go of what it read? |
mod band;
mod breadcrumb;
mod details;
mod filter;
mod flatten;
mod grid;
mod memory;
mod menu;
mod panes;
mod path_bar;
mod path_field;
mod preview;
mod rescan;
mod sidebar;
mod title_bar;
mod transfer;

struct Harness {
    app: App,
    ctx: egui::Context,
    size: egui::Vec2,
    /// The harness drives the clock rather than letting it drift. egui counts
    /// clicks that arrive within 300ms of each other as a double or a triple, so
    /// two gestures in quick succession would run together — which is a property
    /// of the test, not of the program.
    time: f64,
    /// Held modifiers. `Event::PointerButton` carries a copy, but the code under
    /// test reads `InputState::modifiers`, which comes from this.
    modifiers: Modifiers,
    /// Whether the window has the platform's focus. `true` as `RawInput`'s own default is,
    /// so every test but the one about losing it is unaffected.
    focused: bool,
    /// What the last frame asked the pointer to look like.
    cursor: egui::CursorIcon,
    /// Everything the frames so far have asked the platform to do to the window, in order.
    commands: Vec<egui::ViewportCommand>,
    /// Every shape the last frame painted, in paint order.
    ///
    /// Replaced each frame rather than accumulated, because the question these answer is
    /// "what does the window look like now". They are how a test can assert on a *fill* —
    /// a colour is not a click target, and reading the source only proves the source says
    /// what it says.
    shapes: Vec<egui::Shape>,
}

impl Harness {
    /// One pane per requested count, all showing directories that exist and have
    /// something in them to click on.
    fn with_panes(count: usize) -> Self {
        let ctx = egui::Context::default();
        azur_egui_theme::fonts::install(&ctx);
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let open: Vec<PathBuf> = (0..count)
            .map(|i| if i == 0 { here.clone() } else { here.join("src") })
            .collect();
        let mut app = App::opening(&ctx, Config::default(), open, Side::Right);
        app.journal = Some(Vec::new());

        let mut harness = Harness {
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
        // The listing arrives by channel, so rects that depend on it do not exist
        // until a few frames have gone by.
        harness.settle();
        harness
    }

    fn new() -> Self {
        Self::with_panes(1)
    }

    /// Run frames until the listings have arrived.
    ///
    /// Not a fixed count: a scan comes back by channel from a worker thread, and eight
    /// frames of a test process sharing a machine with seven other test threads is
    /// sometimes not long enough — which showed up as this suite failing one run in
    /// three on an assertion about the *listing* rather than about anything the test
    /// was for. So it waits for the thing it is waiting for.
    fn settle(&mut self) {
        for attempt in 0..200 {
            self.frame(Vec::new());
            let arrived = self
                .app
                .panes
                .iter()
                .all(|pane| pane.tab().dir.is_some());
            if arrived && attempt >= 4 {
                break;
            }
            if attempt >= 4 {
                // Frames are free here; the disk is not.
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        self.take_journal();
    }

    /// One pass, with whatever the platform brought and whatever the app had to add.
    ///
    /// The injected events matter as much as the given ones and are appended in the same
    /// order the real input hook appends them. A harness that skipped them would be testing
    /// a program that does not exist — which is how "the second drag does nothing" survived
    /// a suite that was green.
    fn frame(&mut self, events: Vec<Event>) {
        let mut events = events;
        events.extend(self.app.take_injected());
        self.time += 1.0 / 60.0;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.time),
            modifiers: self.modifiers,
            focused: self.focused,
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let out = self.ctx.run_ui(input, |ui| app.frame(ui));
        // Taken from the frame's own output rather than read back off the context
        // afterwards: `run_ui` moves the platform output out on the way through, so a test
        // that asked the context what the cursor was got whatever the *next* frame had
        // accumulated so far, which is nothing.
        self.cursor = out.platform_output.cursor_icon;
        // What the frame asked the platform to do to the window. Collected rather than
        // sampled, because the interesting thing about a pair of these is their order.
        for output in out.viewport_output.values() {
            self.commands.extend(output.commands.iter().cloned());
        }
        self.shapes.clear();
        self.shapes
            .extend(out.shapes.into_iter().map(|clipped| clipped.shape));
    }

    /// Every rectangle the last frame filled, innermost last, as `(rect, corner, fill)`.
    ///
    /// Flattened out of the nested `Shape::Vec`s egui builds, because a shape's depth in
    /// that tree is an artefact of which `Ui` painted it.
    fn rects(&self) -> Vec<(Rect, egui::CornerRadius, egui::Color32)> {
        fn walk(shape: &egui::Shape, into: &mut Vec<(Rect, egui::CornerRadius, egui::Color32)>) {
            match shape {
                egui::Shape::Rect(r) => into.push((r.rect, r.corner_radius, r.fill)),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &self.shapes {
            walk(shape, &mut out);
        }
        out
    }

    /// Every straight line the last frame drew, as `(the two ends, its colour)`.
    ///
    /// [`Self::rects`] for the things painted as strokes rather than as shapes. A dashed outline
    /// is a run of these — one segment per dash — which is why a test asks for the union of the
    /// ones inside a rect rather than for a single line.
    #[allow(dead_code)]
    fn segments(&self) -> Vec<([Pos2; 2], egui::Color32)> {
        fn walk(shape: &egui::Shape, into: &mut Vec<([Pos2; 2], egui::Color32)>) {
            match shape {
                egui::Shape::LineSegment { points, stroke } => into.push((*points, stroke.color)),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &self.shapes {
            walk(shape, &mut out);
        }
        out
    }

    /// Where the last frame put each run of text, as `(top-left, the string)`.
    ///
    /// The counterpart to [`Self::rects`], and what lets a test assert that a word is in the
    /// same place in two different frames without re-deriving where either frame put it.
    fn texts(&self) -> Vec<(egui::Pos2, String)> {
        fn walk(shape: &egui::Shape, into: &mut Vec<(egui::Pos2, String)>) {
            match shape {
                egui::Shape::Text(text) => {
                    into.push((text.pos, text.galley.text().to_owned()));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &self.shapes {
            walk(shape, &mut out);
        }
        out
    }

    /// Where the tooltip that is up ended up, frame and all, or `None` if none is.
    ///
    /// A tooltip is an `Area` in the `Tooltip` layer order, and its state is what knows the
    /// rect — the shapes only know where the *text* went, and the padding and the shadow are
    /// exactly the part a test about overlapping the cursor is asking after.
    fn tooltip_rect(&self) -> Option<Rect> {
        let layer = self.ctx.memory(|m| {
            m.areas()
                .visible_layer_ids()
                .into_iter()
                .find(|layer| layer.order == egui::Order::Tooltip)
        })?;
        egui::AreaState::load(&self.ctx, layer.id).map(|area| area.rect())
    }

    /// Every run of text the last frame painted, as `(left edge and **baseline**, the string)`.
    ///
    /// The counterpart to [`Self::texts`], which gives where a galley's *box* was put. A box
    /// says nothing about whether two texts are level: two fonts have different line heights
    /// and different ascents, so galleys centred in the same rect end up with their baselines
    /// a point or two apart — which is exactly the fault that is invisible in the source and
    /// obvious on screen. This asks the question the eye asks.
    ///
    /// The x is the galley's left edge, unchanged, so a caller can pick out the runs inside
    /// one panel with `rect.contains`.
    fn baselines(&self) -> Vec<(Pos2, String)> {
        fn walk(shape: &egui::Shape, into: &mut Vec<(Pos2, String)>) {
            match shape {
                egui::Shape::Text(text) => into.push((
                    pos2(
                        text.pos.x,
                        text.pos.y + azur::components::galley_baseline(&text.galley),
                    ),
                    text.galley.text().to_owned(),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &self.shapes {
            walk(shape, &mut out);
        }
        out
    }

    /// The text of every run the last frame painted **that did not fit**, as the string it was
    /// asked to draw.
    ///
    /// Necessary because `Galley::text` hands back the *source* string and not the glyphs: a
    /// galley elided to `Fi…` still reports `Fit`, so a test that looked for an ellipsis in the
    /// drawn text was asking a question nothing could answer no. `Galley::elided` is the flag
    /// that knows, and it is the only way from out here to tell a label from a cropped one.
    fn cropped(&self) -> Vec<String> {
        fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) if text.galley.elided => {
                    into.push(text.galley.text().to_owned());
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &self.shapes {
            walk(shape, &mut out);
        }
        out
    }

    /// Every outlined rectangle the last frame painted, as `(rect, the stroke's colour)`.
    ///
    /// The counterpart to [`Self::rects`], for the things that are a border and not a fill —
    /// a field's outline says what it is with its edge, and a test that only looked at fills
    /// could not tell it from nothing at all.
    fn outlines(&self) -> Vec<(Rect, egui::Color32)> {
        fn walk(shape: &egui::Shape, into: &mut Vec<(Rect, egui::Color32)>) {
            match shape {
                egui::Shape::Rect(r) if r.stroke.width > 0.0 => {
                    into.push((r.rect, r.stroke.color));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &self.shapes {
            walk(shape, &mut out);
        }
        out
    }

    /// The fill of every polygon the last frame painted inside `rect`.
    ///
    /// How a painted glyph is asserted on: this program's icons are convex polygons, so
    /// "there is a glyph here, in this ink" is a question about the polygons whose ink lands
    /// in a given box.
    fn glyph_inks(&self, rect: Rect) -> Vec<egui::Color32> {
        self.glyph_shapes(rect)
            .into_iter()
            .map(|(_, fill)| fill)
            .collect()
    }

    /// Where a glyph's ink actually is: the union of the polygons painted inside `rect`.
    ///
    /// The rect a glyph is *handed* says nothing about where the drawing inside it ended up,
    /// which is exactly how a glyph in a row comes out misaligned while every measurement in
    /// the source reads `center()`.
    fn glyph_bounds(&self, rect: Rect) -> Option<Rect> {
        self.glyph_shapes(rect)
            .into_iter()
            .map(|(bounds, _)| bounds)
            .reduce(|a, b| a.union(b))
    }

    fn glyph_shapes(&self, rect: Rect) -> Vec<(Rect, egui::Color32)> {
        fn walk(shape: &egui::Shape, want: Rect, into: &mut Vec<(Rect, egui::Color32)>) {
            match shape {
                egui::Shape::Path(path) => {
                    let bounds = path.visual_bounding_rect();
                    if want.contains_rect(bounds) {
                        into.push((bounds, path.fill));
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, want, into);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &self.shapes {
            walk(shape, rect, &mut out);
        }
        out
    }

    /// The last fill painted at `rect`, to within half a point on every edge.
    ///
    /// The *last*, because that is the one you can see.
    fn fill_at(&self, rect: Rect) -> Option<(egui::CornerRadius, egui::Color32)> {
        self.rects()
            .into_iter()
            .rev()
            .find(|(painted, _, _)| {
                painted.min.distance(rect.min) < 0.5 && painted.max.distance(rect.max) < 0.5
            })
            .map(|(_, corner, fill)| (corner, fill))
    }

    /// Run frames until the window stops asking for more, and say whether it did.
    ///
    /// A window with animations still running or icons still arriving asks for a repaint
    /// every frame, which makes "did anything ask for a repaint" an assertion that passes
    /// on its own. This is how a test gets a silent window to measure against.
    fn quiesce(&mut self) -> bool {
        for _ in 0..240 {
            self.frame(Vec::new());
            if !self.ctx.has_requested_repaint() {
                self.take_journal();
                return true;
            }
            self.time += 1.0 / 60.0;
        }
        false
    }

    /// Let enough time pass that the next click starts a fresh gesture.
    fn wait(&mut self) {
        self.time += 1.0;
        self.frame(Vec::new());
    }

    /// Move the pointer there and report whether `id` is the widget under it.
    ///
    /// This is the assertion that actually matters: a widget the pointer cannot
    /// reach is a widget that does not work, however correct its rect looks.
    fn hovers(&mut self, id: Id, at: Pos2) -> bool {
        self.frame(vec![Event::PointerMoved(at)]);
        self.ctx
            .read_response(id)
            .is_some_and(|response| response.hovered())
    }

    /// Sweep down a column of the window looking for `id`, and return where it
    /// was found. Beats hard-coding a y that any layout change invalidates.
    fn find(&mut self, id: Id, x: f32, ys: std::ops::Range<i32>) -> Option<Pos2> {
        for y in ys.step_by(2) {
            let at = pos2(x, y as f32);
            if self.hovers(id, at) {
                return Some(at);
            }
        }
        None
    }

    /// Move, press, release — three frames, which is how a real click arrives.
    fn click_at(&mut self, at: Pos2) -> Vec<&'static str> {
        self.click_with(at, PointerButton::Primary, Modifiers::NONE)
    }

    fn click_with(
        &mut self,
        at: Pos2,
        button: PointerButton,
        modifiers: Modifiers,
    ) -> Vec<&'static str> {
        self.take_journal();
        self.modifiers = modifiers;
        self.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.frame(vec![Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers,
            }]);
        }
        // One more, so an action queued by the release is applied and drawn.
        self.frame(Vec::new());
        self.modifiers = Modifiers::NONE;
        self.take_journal()
    }

    fn double_click_at(&mut self, at: Pos2) -> Vec<&'static str> {
        // A fresh gesture: without this, the clicks of the previous one are still
        // inside egui's double-click window and these two count as a triple and a
        // quadruple.
        self.wait();
        self.take_journal();
        self.frame(vec![Event::PointerMoved(at)]);
        for _ in 0..2 {
            self.frame(vec![
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                },
            ]);
        }
        self.frame(Vec::new());
        self.take_journal()
    }

    /// Press, travel, release.
    fn drag(&mut self, from: Pos2, to: Pos2) -> Vec<&'static str> {
        self.take_journal();
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        // Several steps, because a drag threshold is a distance travelled.
        for step in 1..=6 {
            let t = step as f32 / 6.0;
            self.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
        }
        self.frame(vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.frame(Vec::new());
        self.take_journal()
    }

    /// Press and travel, and never release — which is what an OLE drag looks like from
    /// here. `DoDragDrop` takes the capture and its own loop swallows the button-up, so
    /// the release genuinely never arrives.
    fn drag_and_hold(&mut self, from: Pos2, to: Pos2) -> Vec<&'static str> {
        self.take_journal();
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        for step in 1..=6 {
            let t = step as f32 / 6.0;
            self.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
        }
        self.take_journal()
    }

    fn take_journal(&mut self) -> Vec<&'static str> {
        let journal = self.app.journal.take().unwrap_or_default();
        self.app.journal = Some(Vec::new());
        journal
    }

    fn pane_rect(&self, index: usize) -> Rect {
        self.app.panes[index].rect
    }

    /// Where a pane's own content starts: its path bar, one hairline in.
    fn pane_content_top(&self, index: usize) -> f32 {
        self.pane_rect(index).top() + 1.0
    }

    /// The vertical middle of the path bar of a pane.
    fn path_bar_y(&self, index: usize) -> f32 {
        self.pane_content_top(index) + crate::ui::breadcrumb::HEIGHT * 0.5
    }

    /// The vertical middle of the column header of a pane.
    fn header_y(&self, index: usize) -> f32 {
        self.pane_content_top(index)
            + crate::ui::breadcrumb::HEIGHT
            + crate::ui::filelist::HEADER_HEIGHT * 0.5
    }

    /// The middle of row `row` of a pane's listing.
    fn row_center(&self, index: usize, row: usize) -> Pos2 {
        let pane = self.pane_rect(index);
        let top = self.pane_content_top(index)
            + crate::ui::breadcrumb::HEIGHT
            + crate::ui::filelist::HEADER_HEIGHT;
        pos2(
            pane.center().x,
            top + crate::pane::ROW_HEIGHT * (row as f32 + 0.5),
        )
    }

    fn tab(&self, index: usize) -> &Tab {
        self.app.panes[index].tab()
    }

    /// How many textured quads the last frame drew.
    ///
    /// In the large-icon view that *is* the number of tiles showing a picture: a tile with one draws
    /// an image out of [`crate::shell::thumbs`]' atlas, and a tile without draws a painted glyph,
    /// which is shapes out of the font atlas rather than an image. Counting the paint rather than
    /// asking the service is deliberate — the question is what is on screen.
    fn images(&self) -> usize {
        fn walk(shape: &egui::Shape, found: &mut usize) {
            match shape {
                // `Painter::image` builds a one-quad `Mesh` carrying the texture, which is what
                // makes this countable at all; text is still a `Text` shape at this stage and only
                // becomes a mesh in the tessellator. The font atlas is `Managed(0)`, so anything
                // else is an atlas quad — a thumbnail, or a shell icon on a tree's folder row.
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    *found += 1;
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = 0;
        for shape in &self.shapes {
            walk(shape, &mut found);
        }
        found
    }

    /// Run frames **only while the window asks for them**, and answer how many pictures ended up on
    /// screen.
    ///
    /// The "only while it asks" is the whole point and it took a wrong version of this to see why.
    /// A loop that simply runs six hundred frames cannot catch the failure it was written for: this
    /// program **paints on demand**, and the bug was a view that could not fill itself in *without*
    /// frames it never asked for. Handed frames for free it filled in perfectly, and the test passed
    /// against the broken code and the fixed one alike — which is worse than no test.
    ///
    /// So this waits instead. A frame is drawn while egui says one is wanted; when nothing is, it
    /// sits for [`QUIET`] and only carries on if something asks — a worker landing an answer calls
    /// `request_repaint`, so real work still wakes it. Nothing asking for [`QUIET`] means the window
    /// is genuinely idle and whatever is on screen is what the user would be looking at.
    fn pictures_once_settled(&mut self) -> usize {
        /// Longer than the heartbeat `Thumbs::request` books when it has nowhere to put an answer,
        /// so a view that recovers slowly is counted as recovering rather than as stuck.
        const QUIET: std::time::Duration = std::time::Duration::from_millis(900);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut best = 0;
        while std::time::Instant::now() < deadline {
            self.frame(Vec::new());
            best = best.max(self.images());
            if self.ctx.has_requested_repaint() {
                continue;
            }
            let quiet = std::time::Instant::now() + QUIET;
            while std::time::Instant::now() < quiet && !self.ctx.has_requested_repaint() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            if !self.ctx.has_requested_repaint() {
                break;
            }
        }
        best
    }
}

/// Which widget is actually on top at a position, and which merely contain it.
///
/// A blocked click is invisible in the source, so this asks egui directly: any id
/// that `contains_pointer` but is not `hovered` has something over it, and the one
/// that is `hovered` is what took the click.
#[test]
#[ignore = "diagnostic; run explicitly"]
fn what_is_under_the_pointer() {
    let mut h = Harness::with_panes(1);
    let pane = h.app.panes[0].id;
    let rect = h.pane_rect(0);
    println!("window {:?}  pane {:?}", h.size, rect);

    let probes: Vec<(String, Id)> = vec![
        ("rows-hit".into(), Id::new(("rows-hit", pane))),
        ("pane-claim".into(), Id::new(("pane-claim", pane))),
        ("caption-drag".into(), Id::new("caption-drag")),
        ("sidebar-grip".into(), Id::new("sidebar-grip")),
        ("app-menu".into(), Id::new("app-menu")),
        ("th0".into(), Id::new(("th", pane, 0usize))),
        ("th1".into(), Id::new(("th", pane, 1usize))),
        ("th2".into(), Id::new(("th", pane, 2usize))),
        ("th3".into(), Id::new(("th", pane, 3usize))),
        ("refresh".into(), Id::new(("refresh", pane))),
        ("bookmark".into(), Id::new(("bookmark-toggle", pane))),
        ("resize-n".into(), Id::new(("yafe-resize", "n"))),
        ("resize-s".into(), Id::new(("yafe-resize", "s"))),
        ("resize-e".into(), Id::new(("yafe-resize", "e"))),
        ("resize-w".into(), Id::new(("yafe-resize", "w"))),
        ("resize-nw".into(), Id::new(("yafe-resize", "nw"))),
        ("resize-ne".into(), Id::new(("yafe-resize", "ne"))),
        ("resize-sw".into(), Id::new(("yafe-resize", "sw"))),
        ("resize-se".into(), Id::new(("yafe-resize", "se"))),
    ];

    let row_y = h.row_center(0, 0).y;
    let header_y = h.header_y(0);
    let bar_y = h.path_bar_y(0);
    let spots = [
        ("row left", pos2(rect.left() + 14.0, row_y)),
        ("row name", pos2(rect.left() + 120.0, row_y)),
        ("row middle", pos2(rect.center().x, row_y)),
        ("row right", pos2(rect.right() - 12.0, row_y)),
        ("header left", pos2(rect.left() + 60.0, header_y)),
        ("header right", pos2(rect.right() - 60.0, header_y)),
        ("path bar right", pos2(rect.right() - 30.0, bar_y)),
        ("title bar left", pos2(16.0, crate::ui::chrome::HEIGHT * 0.5)),
        ("sidebar row", pos2(crate::ui::GUTTER + 80.0, 360.0)),
    ];

    for (label, at) in spots {
        h.frame(vec![Event::PointerMoved(at)]);
        let mut hovered = Vec::new();
        let mut blocked = Vec::new();
        for (name, id) in &probes {
            if let Some(r) = h.ctx.read_response(*id) {
                if r.hovered() {
                    hovered.push(name.clone());
                } else if r.contains_pointer() {
                    blocked.push(name.clone());
                }
            }
        }
        println!(
            "{at:?} {label:<16} hovered={hovered:?}  blocked={blocked:?}"
        );
    }

    println!("
--- rects ---");
    h.frame(vec![Event::PointerMoved(pos2(rect.center().x, row_y))]);
    for (name, id) in &probes {
        if let Some(r) = h.ctx.read_response(*id) {
            println!(
                "{name:<14} rect={:?}  interact={:?}",
                r.rect, r.interact_rect
            );
        }
    }
}

#[test]
fn the_harness_has_something_to_click() {
    let h = Harness::new();
    assert!(h.tab(0).dir.is_some(), "the listing has to have arrived");
    assert!(!h.tab(0).order.is_empty());
    assert!(h.pane_rect(0).width() > 200.0, "and the pane has to be laid out");
}

/// The rows a tab is showing, in display order, by the name each one carries.
///
/// Which is the whole of what "flattened" means from the outside: the same listing, with
/// relative paths in it instead of bare names.
fn shown_names(h: &Harness, pane: usize) -> Vec<String> {
    let tab = h.app.panes[pane].tab();
    let dir = tab.dir.as_ref().expect("the listing has not arrived");
    tab.order
        .iter()
        .map(|&i| dir.name(i as usize).to_owned())
        .collect()
}

/// Everything under the focused pane's path bar, which the listing and the panel share.
fn pane_body(h: &Harness) -> Rect {
    let pane = h.pane_rect(0);
    Rect::from_min_max(
        pos2(
            pane.left(),
            h.pane_content_top(0) + crate::ui::breadcrumb::HEIGHT,
        ),
        pane.max,
    )
}

/// One key, with nothing held.
fn tap(key: egui::Key) -> Vec<Event> {
    vec![Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]
}
