//! The picture canvas: one image, or three being compared.
//!
//! The zoom and the pan are shared across the frames on purpose — see [`Picture`].

use super::*;

/// One image on the canvas.
pub(super) struct Frame {
    pub(super) texture: egui::TextureHandle,
    /// Its own size in pixels, which need not be the group's — two files being compared can be
    /// different shapes.
    pub(super) pixels: Vec2,
    /// What the caption above it says. Empty when there is only one frame and no caption.
    pub(super) label: String,
    /// It is a difference mask rather than a picture, so it is drawn in the status hue over the
    /// board rather than as it is. See [`crate::preview::Diff::mask`].
    pub(super) mask: bool,
}

/// One or three images, and how they are being looked at.
///
/// Three when two files are being compared: the two of them and the difference. The zoom and the
/// pan are **shared**, which is the whole point of a comparison — three views that scrolled
/// independently would be three views of nothing in particular.
pub(super) struct Picture {
    pub(super) frames: Vec<Frame>,
    /// The size all frames are placed against: the larger of the two, so the same pixel lands at
    /// the same offset in each view.
    pub(super) pixels: Vec2,
    /// What the first file is on disk, for the bar.
    pub(super) natural: [u32; 2],
    /// And the second's, when there is one and it differs.
    pub(super) other: Option<[u32; 2]>,
    pub(super) scaled: bool,
    pub(super) vector: bool,
    /// Windows' registered visualizer drew this, because nothing here could — see
    /// [`crate::preview::visual`]. What it changes is the bar: `natural` is the render's size rather
    /// than the file's, so it is reported as the preview's and takes the slot that gives way first.
    pub(super) shell: bool,
    /// The share of pixels that differ, for a comparison.
    pub(super) differing: Option<f32>,
    /// Show both sources as well as the difference. Only meaningful with three frames.
    pub(super) all: bool,
    /// `None` is *fit*, recomputed from the canvas every frame; `Some` is an absolute scale
    /// somebody chose. That distinction is the whole zoom model: resizing the pane while fitted
    /// re-fits, and resizing it while zoomed leaves the zoom alone.
    pub(super) zoom: Option<f32>,
    /// The scale that fits the canvas, as of the last frame drawn.
    ///
    /// Derived rather than chosen, and stored only so the bar's buttons and field have a number to
    /// work from — they run before the canvas is measured. Until a frame has been drawn it is 1.0,
    /// which is a sane picture rather than a blank one.
    pub(super) fit: f32,
    /// How far the images are dragged from where they would sit, in points.
    pub(super) pan: Vec2,
    /// The zoom field's text, and which preset was last taken from its list.
    ///
    /// The text is the field's to own while it has focus — that is what makes typing `137` possible
    /// — and is rewritten from [`Picture::percent`] on every frame it does not.
    pub(super) zoom_text: String,
    pub(super) zoom_pick: Option<usize>,
}

impl Picture {
    /// Fill the zoom field in from what the canvas is about to show.
    ///
    /// Called when the picture is built, so the field has a value on the frame it first appears
    /// rather than on every frame after it. `fit` is 1.0 until a frame has been drawn, so this is a
    /// plausible number that the first real frame corrects — which is a great deal better than a
    /// blank field, and the difference is visible if the window happens not to be asked for another
    /// frame straight away.
    pub(super) fn labelled(mut self) -> Self {
        self.zoom_text = format!("{:.0}%", self.percent());
        self
    }

    /// The fields that are the same however many frames there are.
    pub(super) fn fresh() -> Self {
        Self {
            frames: Vec::new(),
            pixels: Vec2::ZERO,
            natural: [0, 0],
            other: None,
            scaled: false,
            vector: false,
            shell: false,
            differing: None,
            all: true,
            zoom: None,
            fit: 1.0,
            pan: Vec2::ZERO,
            zoom_text: String::new(),
            zoom_pick: None,
        }
    }

    /// The scale the canvas is showing it at, fit included.
    ///
    /// `fit` is not the stored answer: it depends on the canvas, the canvas depends on the pane,
    /// and a stored one would be a frame stale every time anything moved. So it is recomputed and
    /// the *choice* — fitted, or a number somebody picked — is what is kept.
    pub(super) fn scale(&self) -> f32 {
        self.zoom.unwrap_or(self.fit)
    }

    /// What the bar reports: the scale against the file's own pixels, not against the texture's —
    /// which are not the same thing for a picture scaled down to fit memory, or for vector art
    /// rasterised at a size of this program's choosing.
    pub(super) fn percent(&self) -> f32 {
        self.scale() * self.pixels.x / self.natural[0].max(1) as f32 * 100.0
    }

    /// And the inverse, for a percentage somebody typed or picked.
    pub(super) fn scale_for(&self, percent: f32) -> f32 {
        percent / 100.0 * self.natural[0].max(1) as f32 / self.pixels.x.max(1.0)
    }

    /// The frames on show: all of them, or the difference alone.
    pub(super) fn showing(&self) -> &[Frame] {
        if self.frames.len() == 3 && !self.all {
            &self.frames[2..]
        } else {
            &self.frames
        }
    }
}

/// One picture, or three, on a checkerboard.
///
/// The interactions are the ones every image viewer has: **the wheel zooms about the pointer**,
/// **dragging pans**, and **a double click goes back to fit**. Zooming about the pointer rather
/// than the middle is the one that matters — it is what makes it possible to get to a corner of a
/// large image without a dozen alternating zooms and drags.
///
/// With three frames all of that is **shared**: one zoom, one pan, one gesture over the whole
/// canvas. Three views that scrolled independently would be three views of nothing in particular.
pub(super) fn pictures(ui: &mut Ui, t: &Theme, canvas: Rect, pane: PaneId, picture: &mut Picture) {
    let count = picture.showing().len().max(1);
    let n = count as f32;
    // Along the canvas's longer axis, so three views of a wide panel are three columns and three
    // of a tall one are three rows — the same question `Where::Auto` answers, one level down.
    let across = canvas.width() >= canvas.height();
    let cells: Vec<Rect> = (0..count)
        .map(|i| {
            let at = i as f32;
            if across {
                let w = (canvas.width() - SEAM * (n - 1.0)) / n;
                Rect::from_min_size(
                    pos2(canvas.left() + at * (w + SEAM), canvas.top()),
                    vec2(w, canvas.height()),
                )
            } else {
                let h = (canvas.height() - SEAM * (n - 1.0)) / n;
                Rect::from_min_size(
                    pos2(canvas.left(), canvas.top() + at * (h + SEAM)),
                    vec2(canvas.width(), h),
                )
            }
        })
        .collect();

    // One interaction over the whole canvas, so a drag anywhere moves every view together.
    let response = ui.interact(
        canvas,
        Id::new(("preview-canvas", pane)),
        Sense::click_and_drag(),
    );

    // Where the images go inside each cell, once the captions have had their strip.
    let captioned = count > 1;
    let areas: Vec<Rect> = cells
        .iter()
        .map(|cell| {
            if captioned {
                Rect::from_min_max(pos2(cell.left(), cell.top() + CAPTION), cell.max)
            } else {
                *cell
            }
        })
        .collect();

    // Fit: recomputed every frame, because the canvas moves — and against the *smallest* area, so
    // every view fits rather than the first one fitting and the rest overflowing. **Never enlarged
    // past 1:1**: a 16-pixel icon blown up to fill a 400-point panel is not a preview of it, it is
    // a mosaic, and fitting is an upper bound.
    let smallest = areas
        .iter()
        .fold(Vec2::splat(f32::INFINITY), |acc, area| acc.min(area.size()));
    picture.fit = (smallest.x / picture.pixels.x.max(1.0))
        .min(smallest.y / picture.pixels.y.max(1.0))
        .min(1.0);

    // ---- The gestures ----
    if response.dragged() {
        picture.pan += response.drag_delta();
        // A drag is a choice to look at part of it, so it pins the scale as well: without this,
        // panning a fitted image would move something that cannot move.
        picture.zoom = Some(picture.scale());
    }
    if response.double_clicked() {
        picture.zoom = None;
        picture.pan = Vec2::ZERO;
    }
    let wheel = ui.input(|i| i.smooth_scroll_delta.y);
    if response.hovered() && wheel != 0.0 {
        let was = picture.scale();
        let now = (was * ZOOM_STEP.powf(wheel / 50.0)).clamp(ZOOM_MIN, ZOOM_MAX);
        // **Zoom about the pointer**: the point under the cursor stays under it. The images sit at
        // their cell's centre plus the pan, so the offset from that centre to the pointer scales
        // with them and the pan takes up the difference. Measured against the cell the pointer is
        // in, which is what makes it work in a three-view comparison too.
        if let Some(at) = response.hover_pos() {
            let cell = areas
                .iter()
                .find(|area| area.contains(at))
                .copied()
                .unwrap_or(canvas);
            let middle = cell.center() + picture.pan;
            picture.pan += (middle - at) * (now / was - 1.0);
        }
        picture.zoom = Some(now);
    }

    let scale = picture.scale();
    let group = picture.pixels * scale;
    // Panning is bounded so the images cannot be dragged out of view altogether: at least a
    // quarter stays. One smaller than its cell is simply centred — there is nowhere for it to go,
    // and letting it wander would be a gesture with no meaning.
    let slack = ((group - smallest) * 0.5 + smallest * 0.25).max(Vec2::ZERO);
    picture.pan = picture.pan.clamp(-slack, slack);

    let checker = checkerboard(ui.ctx(), t);
    for (frame, (cell, area)) in picture.showing().iter().zip(cells.iter().zip(&areas)) {
        if captioned {
            let strip = Rect::from_min_max(cell.min, pos2(cell.right(), cell.top() + CAPTION));
            let galley = truncated(
                ui.painter(),
                &frame.label,
                t.fonts.caption.clone(),
                t.text.secondary,
                (strip.width() - PAD * 2.0).max(0.0),
            );
            let baseline =
                ink_baseline(ui.painter(), &t.fonts.caption, strip.top(), strip.height());
            galley_on_baseline(ui.painter(), strip.left() + PAD, baseline, galley);
        }

        // The checkerboard, so an alpha channel is visible as absence rather than as whatever the
        // panel's surface happens to be. One tiled quad, not a grid of rects: a 400-point canvas
        // at 8-point squares would be two and a half thousand rectangles a frame.
        ui.painter().add(egui::Shape::image(
            checker.id(),
            *area,
            Rect::from_min_size(pos2(0.0, 0.0), area.size() / (CHECKER * 2.0)),
            Color32::WHITE,
        ));

        // Placed against the *group's* rect and not its own, so the same pixel of two files of
        // different shapes lands at the same offset in each view — which is the only way a
        // comparison means anything.
        let whole = Rect::from_center_size(area.center() + picture.pan, group);
        let where_ = Rect::from_min_size(whole.min, frame.pixels * scale);
        let painter = ui.painter().with_clip_rect(*area);
        painter.add(egui::Shape::image(
            frame.texture.id(),
            where_,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            // A mask is tinted here rather than coloured on the worker, which is what keeps the
            // rule that no colour is chosen outside the theme: what came back is a measurement.
            if frame.mask {
                t.status.danger
            } else {
                Color32::WHITE
            },
        ));
    }

    if response.hovered() {
        ui.ctx()
            .set_cursor_icon(if group.x > smallest.x || group.y > smallest.y {
                egui::CursorIcon::Grab
            } else {
                egui::CursorIcon::Default
            });
    }
}

/// One square of the checkerboard, in points.
pub(super) const CHECKER: f32 = 8.0;

/// The two-by-two texture the checkerboard is tiled from.
///
/// Built once per theme and kept in the context's own cache: it is one 2×2 image, it never
/// changes, and a texture uploaded per frame would be a texture uploaded per frame.
///
/// `Repeat` is the whole trick — it is what lets one quad with a `uv` of many tiles stand in for
/// a grid of rectangles.
pub(super) fn checkerboard(ctx: &egui::Context, t: &Theme) -> egui::TextureHandle {
    let id = Id::new(("preview-checker", t.dark));
    if let Some(cached) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return cached;
    }
    // Two surfaces from the ramp rather than two greys named here, and *this* pair rather than an
    // adjacent one, because the two themes have to agree: measured, `canvas`/`control` is 11.5 ΔL*
    // apart in the dark theme and 3.6 in the light one, where the board all but disappeared and
    // with it the whole point of having one. `layer`/`control-active` is 15.4 and 13.9 — balanced,
    // and about what Photoshop's white-and-light-grey board measures, which is the value everyone
    // already reads as "nothing here". See `the_checkerboard_reads_as_a_checkerboard`.
    let (a, b) = (t.bg.layer, t.bg.control_active);
    let image = egui::ColorImage {
        size: [2, 2],
        pixels: vec![a, b, b, a],
        source_size: vec2(2.0, 2.0),
    };
    let handle = ctx.load_texture(
        "preview-checker",
        image,
        egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Nearest,
            wrap_mode: egui::TextureWrapMode::Repeat,
            mipmap_mode: None,
        },
    );
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    handle
}
