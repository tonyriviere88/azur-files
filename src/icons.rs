//! The application's glyphs, painted rather than loaded.
//!
//! Same shape as [`azur_egui_theme::icons`] — `fn(&Painter, Rect, Color32)` — so
//! these are interchangeable with the design system's own and can be handed to any
//! Azur component that takes an icon.
//!
//! Painted, for the same reason the theme paints its own: nothing to embed,
//! nothing to rasterise, and a glyph that is sharp at any size. It also sidesteps
//! the thing that makes shell-based file managers slow — asking the operating
//! system for a per-file icon is a registry walk, an image load and a cache miss
//! per row, and it is why Explorer's window can be visible for a second before its
//! icons are.
//!
//! # The one convention
//!
//! **Folders are filled; files are outlined.** A folder is a place you can go and
//! a file is a thing that is there, and at 14 pixels that difference has to be
//! legible before any colour is. Every glyph is laid out on a 16-unit grid mapped
//! onto whatever rect it is handed, so the whole set stays visually consistent as
//! the row height changes.

use egui::{pos2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

/// A 16-unit design grid mapped onto a rect.
struct Grid {
    origin: Pos2,
    scale: f32,
}

impl Grid {
    fn new(rect: Rect) -> Self {
        // The largest centred square, so a glyph in a non-square box is not
        // stretched.
        let side = rect.width().min(rect.height());
        let square = Rect::from_center_size(rect.center(), Vec2::splat(side));
        Self {
            origin: square.min,
            scale: side / 16.0,
        }
    }

    /// A point in grid units.
    #[inline]
    fn at(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.origin.x + x * self.scale, self.origin.y + y * self.scale)
    }

    /// A rect in grid units.
    #[inline]
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.at(x0, y0), self.at(x1, y1))
    }

    /// Corner radius in grid units, as egui's `u8` of pixels.
    #[inline]
    fn radius(&self, units: f32) -> CornerRadius {
        CornerRadius::same((units * self.scale).round().clamp(0.0, 255.0) as u8)
    }

    /// The stroke an outlined glyph is drawn with: 1.5 units, floored at one
    /// device pixel so a small icon stays visible rather than fading.
    #[inline]
    fn stroke(&self, color: Color32) -> Stroke {
        Stroke::new((self.scale * 1.5).max(1.0), color)
    }

    /// A thinner line, for interior detail that must not compete with the outline.
    #[inline]
    fn hairline(&self, color: Color32) -> Stroke {
        Stroke::new((self.scale * 1.15).max(1.0), color)
    }
}

fn path(p: &Painter, points: Vec<Pos2>, stroke: Stroke) {
    p.add(Shape::line(points, stroke));
}

fn closed(p: &Painter, points: Vec<Pos2>, stroke: Stroke) {
    p.add(Shape::closed_line(points, stroke));
}

fn fill(p: &Painter, points: Vec<Pos2>, color: Color32) {
    p.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

// ---------------------------------------------------------------------------
// Folders — filled, because a place is not a thing
// ---------------------------------------------------------------------------

/// A folder: the tab behind, the body in front.
pub fn folder(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    // The tab, drawn first so the body's top edge cuts across it cleanly.
    p.rect_filled(g.rect(1.5, 2.6, 7.5, 5.6), g.radius(0.9), color);
    p.rect_filled(g.rect(1.0, 4.4, 15.0, 13.6), g.radius(1.3), color);
}

/// An open folder — the front face tilted forward. For the folder you are in.
pub fn folder_open(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    p.rect_filled(g.rect(1.5, 2.6, 7.5, 5.6), g.radius(0.9), color);
    p.rect_filled(g.rect(1.0, 4.4, 13.0, 11.0), g.radius(1.3), color);
    // The front flap, sheared right so it reads as leaning open.
    fill(
        p,
        vec![
            g.at(3.0, 7.0),
            g.at(16.0, 7.0),
            g.at(13.6, 13.6),
            g.at(0.6, 13.6),
        ],
        color,
    );
}

/// A folder with a link badge — a junction or a symlinked directory.
pub fn folder_link(p: &Painter, rect: Rect, color: Color32) {
    folder(p, rect, color);
    let g = Grid::new(rect);
    // An arrow in the bottom-right corner, knocked through the fill.
    let stroke = Stroke::new((g.scale * 1.6).max(1.2), color);
    path(
        p,
        vec![g.at(9.0, 12.0), g.at(14.0, 12.0), g.at(14.0, 7.0)],
        stroke,
    );
}

// ---------------------------------------------------------------------------
// Files — outlined
// ---------------------------------------------------------------------------

/// The page every file glyph is built on: a sheet with the corner turned down.
fn page(p: &Painter, g: &Grid, color: Color32) {
    let stroke = g.stroke(color);
    closed(
        p,
        vec![
            g.at(3.5, 1.5),
            g.at(9.5, 1.5),
            g.at(12.5, 4.5),
            g.at(12.5, 14.5),
            g.at(3.5, 14.5),
        ],
        stroke,
    );
    // The fold, which is what stops the shape reading as a plain rectangle.
    path(
        p,
        vec![g.at(9.5, 1.5), g.at(9.5, 4.5), g.at(12.5, 4.5)],
        g.hairline(color),
    );
}

/// A file of no particular kind.
pub fn file(p: &Painter, rect: Rect, color: Color32) {
    page(p, &Grid::new(rect), color);
}

/// A document: a page with writing on it.
pub fn document(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    page(p, &g, color);
    let stroke = g.hairline(color);
    for (i, width) in [4.5, 4.5, 3.0].into_iter().enumerate() {
        let y = 7.5 + i as f32 * 2.3;
        p.line_segment([g.at(5.5, y), g.at(5.5 + width, y)], stroke);
    }
}

/// An image: a frame with a horizon and a sun.
pub fn image(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.rect_stroke(
        g.rect(1.5, 2.5, 14.5, 13.5),
        g.radius(1.3),
        stroke,
        StrokeKind::Middle,
    );
    p.circle_filled(g.at(5.4, 6.2), g.scale * 1.15, color);
    // The hill, clipped by the frame it sits in.
    path(
        p,
        vec![
            g.at(2.0, 12.0),
            g.at(6.2, 8.2),
            g.at(9.4, 11.0),
            g.at(11.4, 9.2),
            g.at(14.0, 11.8),
        ],
        g.hairline(color),
    );
}

/// A quaver: the note head and its stem.
pub fn audio(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.circle_stroke(g.at(5.0, 11.6), g.scale * 2.2, stroke);
    path(p, vec![g.at(7.2, 11.6), g.at(7.2, 3.0)], stroke);
    // The flag, which is what makes it a note rather than a lollipop.
    path(
        p,
        vec![g.at(7.2, 3.0), g.at(12.6, 4.6), g.at(12.6, 7.0)],
        g.hairline(color),
    );
}

/// Video: a play triangle in a frame.
pub fn video(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    p.rect_stroke(
        g.rect(1.5, 3.0, 14.5, 13.0),
        g.radius(1.3),
        g.stroke(color),
        StrokeKind::Middle,
    );
    fill(
        p,
        vec![g.at(6.6, 5.6), g.at(11.0, 8.0), g.at(6.6, 10.4)],
        color,
    );
}

/// An archive: a box with a band and a clasp.
pub fn archive(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.rect_stroke(
        g.rect(2.0, 2.5, 14.0, 13.5),
        g.radius(1.3),
        stroke,
        StrokeKind::Middle,
    );
    p.line_segment([g.at(2.0, 6.2), g.at(14.0, 6.2)], g.hairline(color));
    p.rect_filled(g.rect(7.0, 8.2, 9.0, 10.6), g.radius(0.5), color);
}

/// Code: the angle brackets, which is what a source file looks like.
pub fn code(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    path(
        p,
        vec![g.at(5.6, 4.6), g.at(1.8, 8.0), g.at(5.6, 11.4)],
        stroke,
    );
    path(
        p,
        vec![g.at(10.4, 4.6), g.at(14.2, 8.0), g.at(10.4, 11.4)],
        stroke,
    );
    p.line_segment([g.at(9.2, 3.4), g.at(6.8, 12.6)], g.hairline(color));
}

/// An executable: a chip, legs and all.
pub fn executable(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.rect_stroke(
        g.rect(4.0, 4.0, 12.0, 12.0),
        g.radius(1.0),
        stroke,
        StrokeKind::Middle,
    );
    p.rect_filled(g.rect(6.6, 6.6, 9.4, 9.4), g.radius(0.4), color);
    let leg = g.hairline(color);
    for offset in [6.0, 8.0, 10.0] {
        p.line_segment([g.at(offset, 2.0), g.at(offset, 4.0)], leg);
        p.line_segment([g.at(offset, 12.0), g.at(offset, 14.0)], leg);
        p.line_segment([g.at(2.0, offset), g.at(4.0, offset)], leg);
        p.line_segment([g.at(12.0, offset), g.at(14.0, offset)], leg);
    }
}

/// A font: a serifed A on a baseline.
pub fn font(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    path(p, vec![g.at(4.0, 12.0), g.at(8.0, 3.2), g.at(12.0, 12.0)], stroke);
    p.line_segment([g.at(5.6, 9.4), g.at(10.4, 9.4)], g.hairline(color));
    p.line_segment([g.at(2.6, 14.2), g.at(13.4, 14.2)], g.hairline(color));
}

/// A model: a wireframe cube. Meshes, point clouds, CAD.
pub fn model(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    // The silhouette is a hexagon; the three interior edges make it a solid.
    closed(
        p,
        vec![
            g.at(8.0, 1.8),
            g.at(14.2, 5.2),
            g.at(14.2, 10.8),
            g.at(8.0, 14.2),
            g.at(1.8, 10.8),
            g.at(1.8, 5.2),
        ],
        stroke,
    );
    let inner = g.hairline(color);
    p.line_segment([g.at(8.0, 8.0), g.at(8.0, 14.2)], inner);
    p.line_segment([g.at(8.0, 8.0), g.at(1.8, 5.2)], inner);
    p.line_segment([g.at(8.0, 8.0), g.at(14.2, 5.2)], inner);
}

/// Data: the database cylinder.
pub fn data(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    let radius = g.scale * 5.0;
    for y in [3.6, 8.0, 12.4] {
        // A flat ellipse out of a scaled circle, which is cheaper than a path and
        // indistinguishable at this size.
        ellipse(p, g.at(8.0, y), Vec2::new(radius, g.scale * 1.7), stroke);
    }
    p.line_segment([g.at(3.0, 3.6), g.at(3.0, 12.4)], stroke);
    p.line_segment([g.at(13.0, 3.6), g.at(13.0, 12.4)], stroke);
}

fn ellipse(p: &Painter, center: Pos2, radii: Vec2, stroke: Stroke) {
    const STEPS: usize = 20;
    let points = (0..=STEPS)
        .map(|i| {
            let a = i as f32 / STEPS as f32 * std::f32::consts::TAU;
            center + Vec2::new(a.cos() * radii.x, a.sin() * radii.y)
        })
        .collect();
    p.add(Shape::line(points, stroke));
}

/// The glyph for a file kind.
pub fn for_kind(kind: crate::fs::fmt::Kind) -> azur_egui_theme::icons::Icon<'static> {
    use crate::fs::fmt::Kind;
    match kind {
        Kind::Folder => &folder,
        Kind::Image => &image,
        Kind::Audio => &audio,
        Kind::Video => &video,
        Kind::Archive => &archive,
        Kind::Code => &code,
        Kind::Document => &document,
        Kind::Executable => &executable,
        Kind::Font => &font,
        Kind::Model => &model,
        Kind::Data => &data,
        Kind::Other => &file,
    }
}

// ---------------------------------------------------------------------------
// Places and volumes
// ---------------------------------------------------------------------------

/// This PC: a desktop tower.
pub fn this_pc(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.rect_stroke(
        g.rect(4.0, 1.8, 12.0, 14.2),
        g.radius(1.2),
        stroke,
        StrokeKind::Middle,
    );
    let detail = g.hairline(color);
    p.line_segment([g.at(6.2, 4.4), g.at(9.8, 4.4)], detail);
    p.line_segment([g.at(6.2, 6.4), g.at(9.8, 6.4)], detail);
    p.circle_filled(g.at(8.0, 11.4), g.scale * 1.1, color);
}

/// A monitor on a stand.
pub fn desktop(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.rect_stroke(
        g.rect(1.6, 2.6, 14.4, 11.0),
        g.radius(1.2),
        stroke,
        StrokeKind::Middle,
    );
    p.line_segment([g.at(8.0, 11.0), g.at(8.0, 13.4)], g.hairline(color));
    p.line_segment([g.at(5.0, 13.6), g.at(11.0, 13.6)], stroke);
}

/// A house.
pub fn home(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    path(
        p,
        vec![
            g.at(1.8, 7.6),
            g.at(8.0, 2.2),
            g.at(14.2, 7.6),
        ],
        stroke,
    );
    path(
        p,
        vec![
            g.at(3.4, 7.0),
            g.at(3.4, 14.0),
            g.at(12.6, 14.0),
            g.at(12.6, 7.0),
        ],
        stroke,
    );
    p.line_segment([g.at(6.6, 14.0), g.at(6.6, 10.2)], g.hairline(color));
    p.line_segment([g.at(6.6, 10.2), g.at(9.4, 10.2)], g.hairline(color));
    p.line_segment([g.at(9.4, 10.2), g.at(9.4, 14.0)], g.hairline(color));
}

/// An arrow into a tray.
pub fn downloads(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.line_segment([g.at(8.0, 1.8), g.at(8.0, 9.6)], stroke);
    path(
        p,
        vec![g.at(4.8, 6.6), g.at(8.0, 9.8), g.at(11.2, 6.6)],
        stroke,
    );
    path(
        p,
        vec![
            g.at(2.2, 11.0),
            g.at(2.2, 14.0),
            g.at(13.8, 14.0),
            g.at(13.8, 11.0),
        ],
        stroke,
    );
}

/// A waste bin.
pub fn trash(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.line_segment([g.at(2.2, 4.4), g.at(13.8, 4.4)], stroke);
    path(
        p,
        vec![g.at(6.2, 4.4), g.at(6.2, 2.4), g.at(9.8, 2.4), g.at(9.8, 4.4)],
        g.hairline(color),
    );
    path(
        p,
        vec![
            g.at(3.6, 4.4),
            g.at(4.4, 14.0),
            g.at(11.6, 14.0),
            g.at(12.4, 4.4),
        ],
        stroke,
    );
    let bar = g.hairline(color);
    p.line_segment([g.at(6.6, 6.8), g.at(6.9, 11.8)], bar);
    p.line_segment([g.at(9.4, 6.8), g.at(9.1, 11.8)], bar);
}

// ---------------------------------------------------------------------------
// The context menu's tile row
// ---------------------------------------------------------------------------
//
// Cut, Copy, Rename, Share and Delete, for the row `crate::shell::menu::regroup` makes out of
// shell32's own block — see `crate::ui::menu::draw_tiles`.
//
// **They have to be drawn here because Windows does not supply them.** The obvious source is the
// menu item's own bitmap, which is what every other row in that menu uses: `hbmpItem`, read by
// `shell::menu::win::menu_bitmap`. shell32's verbs have none — measured, on a real menu: `Couper`,
// `Copier`, `Renommer` and `Supprimer` all come back with an empty `hbmpItem`, because Windows 11
// draws that row from its own Segoe Fluent glyphs rather than through the menu API. So a tile row
// fed only by the shell is a row of five captions with a hole above each one.
//
// And they are here rather than in `azur_egui_theme::icons`, whose header is explicit about the
// division: that module is "only about the glyphs the *system* needs — chevrons, checks, window
// buttons, status marks. Domain icons stay in the application." A pair of scissors is a domain icon.
//
// Delete reuses [`trash`], which the Recycle Bin place already needed.

/// Scissors, for Cut.
pub fn cut(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.hairline(color);
    // The two blades, crossing a little above the middle so the pivot reads as a pivot.
    p.line_segment([g.at(4.6, 2.2), g.at(10.4, 11.0)], stroke);
    p.line_segment([g.at(11.4, 2.2), g.at(5.6, 11.0)], stroke);
    // And the finger loops under them, outlined so they do not read as two blobs.
    let r = g.scale * 1.9;
    p.circle_stroke(g.at(5.0, 12.6), r, stroke);
    p.circle_stroke(g.at(11.0, 12.6), r, stroke);
}

/// Two sheets, one behind the other, for Copy.
pub fn copy(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    // The sheet behind, drawn first and only where it shows: an L of two segments rather than a
    // whole rectangle, so there is no line running through the front sheet.
    path(
        p,
        vec![g.at(5.2, 3.4), g.at(11.6, 3.4), g.at(11.6, 4.6)],
        g.hairline(color),
    );
    path(
        p,
        vec![g.at(4.0, 4.6), g.at(4.0, 3.4), g.at(5.2, 3.4)],
        g.hairline(color),
    );
    // And the sheet in front, whole.
    p.rect_stroke(
        g.rect(4.0, 5.6, 11.0, 13.4),
        g.radius(1.2),
        stroke,
        StrokeKind::Inside,
    );
}

/// A pencil over a baseline, for Rename.
///
/// Not [`azur_egui_theme::icons::pencil`], which is the edit affordance the path bar uses: this one
/// sits on a rule, which is what says "the name, edited" rather than "edit something".
pub fn rename(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.hairline(color);
    // The body, as a quadrilateral from the nib up to the flat end.
    closed(
        p,
        vec![
            g.at(2.6, 10.4),
            g.at(9.8, 3.2),
            g.at(12.0, 5.4),
            g.at(4.8, 12.6),
        ],
        stroke,
    );
    // The ferrule, so the flat end does not read as a second nib.
    p.line_segment([g.at(8.6, 4.4), g.at(10.8, 6.6)], stroke);
    // And the line being written on.
    p.line_segment([g.at(2.4, 14.2), g.at(13.6, 14.2)], g.hairline(color));
}

/// A box with an arrow leaving it, for Share.
pub fn share(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    // Three sides of a tray — open at the top, which is where the arrow goes out.
    path(
        p,
        vec![
            g.at(5.4, 5.0),
            g.at(3.0, 5.0),
            g.at(3.0, 13.6),
            g.at(13.0, 13.6),
            g.at(13.0, 5.0),
            g.at(10.6, 5.0),
        ],
        stroke,
    );
    // The arrow: a shaft up the middle and a head on it.
    p.line_segment([g.at(8.0, 10.0), g.at(8.0, 2.6)], stroke);
    path(
        p,
        vec![g.at(5.4, 5.2), g.at(8.0, 2.6), g.at(10.6, 5.2)],
        stroke,
    );
}

/// A five-pointed star, for a bookmark. `filled` is the bookmarked state.
fn star_shape(p: &Painter, rect: Rect, color: Color32, filled: bool) {
    let g = Grid::new(rect);
    let center = g.at(8.0, 8.4);
    let (outer, inner) = (g.scale * 6.4, g.scale * 2.6);
    let points: Vec<Pos2> = (0..10)
        .map(|i| {
            // Start at the top, then alternate outer and inner vertices.
            let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let radius = if i % 2 == 0 { outer } else { inner };
            center + Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect();
    if filled {
        // A star is not convex, so it has to be triangulated by hand: a fan from
        // the centre, which is exact for a regular star polygon.
        for pair in points.windows(2) {
            p.add(Shape::convex_polygon(
                vec![center, pair[0], pair[1]],
                color,
                Stroke::NONE,
            ));
        }
        p.add(Shape::convex_polygon(
            vec![center, points[9], points[0]],
            color,
            Stroke::NONE,
        ));
    } else {
        closed(p, points, g.stroke(color));
    }
}

/// An outlined star — not bookmarked.
pub fn star(p: &Painter, rect: Rect, color: Color32) {
    star_shape(p, rect, color, false);
}

/// A filled star — bookmarked.
pub fn star_filled(p: &Painter, rect: Rect, color: Color32) {
    star_shape(p, rect, color, true);
}

/// A group of bookmarks: a folder outline with a star in it.
///
/// **Outlined, where every other folder in this program is filled**, and that is the one thing
/// about it that is not decoration. The convention this set keeps — see the module header — is
/// that a filled glyph is somewhere you can *be*; a group of bookmarks is not. Clicking one
/// folds it away, because there is nowhere to go, and the outline is what says so before the
/// click is tried.
///
/// It is also what keeps it clear of the rows underneath it. Those are real folders, and
/// Windows draws them itself — the shell's own bitmap, gradient and all — so a group drawn as a
/// folder would be a folder that does not open, in a list of ones that do. This is flat, hollow
/// and in the amber the bookmarks' own stars wear, none of which the shell produces.
///
/// The star is 5.4 units in a body 9 tall, which leaves 1.8 units — a pixel and a half at the
/// 14 the sidebar draws it at — between its points and the outline around them. That number is
/// the whole legibility argument at this size: [`flatten`], [`fit`] and [`columns`] all have
/// the same note, arrived at the same way, and half of it would read as a blot rather than as a
/// star in a box.
pub fn bookmark_group(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    // The tab, then the body, as one outline each — a single closed path round both would put a
    // seam where they meet, and at this weight a seam is a notch.
    path(
        p,
        vec![g.at(1.5, 5.0), g.at(1.5, 3.0), g.at(6.8, 3.0), g.at(7.8, 5.0)],
        stroke,
    );
    p.rect_stroke(
        g.rect(1.5, 4.6, 14.5, 13.4),
        g.radius(1.2),
        stroke,
        StrokeKind::Middle,
    );
    // 5.4 units of ink: `star_shape` fills four fifths of the box it is handed.
    star_shape(
        p,
        Rect::from_center_size(g.at(8.0, 8.8), Vec2::splat(6.75 * g.scale)),
        color,
        true,
    );
}

/// A plus. New group, new anything.
///
/// Two strokes 4.5 units either side of the middle. Shorter than the 2..14 most of this set
/// uses, because it is drawn inside a button rather than beside a row of text, and a plus that
/// reaches the button's edge reads as a crosshair.
///
/// **And a device pixel longer at the right and at the bottom, which is the whole of the
/// arithmetic here.** A bar drawn on a pixel boundary does not come out centred on it: at this
/// weight the rasteriser lands it half a pixel further down and right than it was put, so the
/// two arms of a mathematically symmetric plus render three pixels one side of the bar and two
/// the other — and a plus that is one pixel heavy up and left is the kind of fault that is
/// invisible in the source and the first thing you see on the button. Extending the far ends by
/// the same half-pixel-and-a-bit the bars moved puts the ink back either side of them.
///
/// In *device* pixels rather than grid units, because that is the unit the error is in: one
/// grid unit is 0.875 of a pixel at the 14 this is drawn at and two at 200% scaling, so a
/// correction written in units would over-shoot every screen but this one.
pub fn plus(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    let pixel = 1.0 / p.pixels_per_point().max(0.5);
    let center = g.at(8.0, 8.0);
    let arm = 4.5 * g.scale;
    p.line_segment(
        [
            pos2(center.x, center.y - arm),
            pos2(center.x, center.y + arm + pixel),
        ],
        stroke,
    );
    p.line_segment(
        [
            pos2(center.x - arm, center.y),
            pos2(center.x + arm + pixel, center.y),
        ],
        stroke,
    );
}

/// A fixed disk: the drive body and its activity light.
pub fn drive(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.rect_stroke(
        g.rect(1.6, 4.6, 14.4, 11.4),
        g.radius(1.4),
        stroke,
        StrokeKind::Middle,
    );
    p.circle_filled(g.at(11.6, 8.0), g.scale * 1.1, color);
    p.line_segment([g.at(4.0, 8.0), g.at(8.4, 8.0)], g.hairline(color));
}

/// A USB stick.
pub fn drive_removable(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.rect_stroke(
        g.rect(5.0, 5.6, 11.0, 14.2),
        g.radius(1.0),
        stroke,
        StrokeKind::Middle,
    );
    p.rect_stroke(
        g.rect(6.4, 1.8, 9.6, 5.6),
        g.radius(0.5),
        g.hairline(color),
        StrokeKind::Middle,
    );
    p.line_segment([g.at(7.0, 9.0), g.at(9.0, 9.0)], g.hairline(color));
}

/// An optical disc.
pub fn drive_optical(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    p.circle_stroke(g.at(8.0, 8.0), g.scale * 6.2, g.stroke(color));
    p.circle_stroke(g.at(8.0, 8.0), g.scale * 1.9, g.hairline(color));
}

/// A network share: a box with a plug lead.
pub fn drive_network(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.rect_stroke(
        g.rect(1.8, 3.0, 14.2, 8.0),
        g.radius(1.0),
        stroke,
        StrokeKind::Middle,
    );
    p.circle_filled(g.at(11.8, 5.5), g.scale * 0.9, color);
    let lead = g.hairline(color);
    p.line_segment([g.at(8.0, 8.0), g.at(8.0, 11.0)], lead);
    p.line_segment([g.at(3.6, 11.0), g.at(12.4, 11.0)], lead);
    p.line_segment([g.at(3.6, 11.0), g.at(3.6, 13.6)], lead);
    p.line_segment([g.at(12.4, 11.0), g.at(12.4, 13.6)], lead);
}

/// The glyph for a volume kind.
pub fn for_drive(kind: crate::fs::drives::DriveKind) -> azur_egui_theme::icons::Icon<'static> {
    use crate::fs::drives::DriveKind;
    match kind {
        DriveKind::Removable => &drive_removable,
        DriveKind::Optical => &drive_optical,
        DriveKind::Network => &drive_network,
        _ => &drive,
    }
}

/// The glyph for a sidebar place.
pub fn for_place(icon: crate::fs::places::PlaceIcon) -> azur_egui_theme::icons::Icon<'static> {
    use crate::fs::places::PlaceIcon;
    match icon {
        PlaceIcon::ThisPc => &this_pc,
        PlaceIcon::Home => &home,
        PlaceIcon::Desktop => &desktop,
        PlaceIcon::Documents => &document,
        PlaceIcon::Downloads => &downloads,
        PlaceIcon::Music => &audio,
        PlaceIcon::Pictures => &image,
        PlaceIcon::Videos => &video,
        PlaceIcon::Trash => &trash,
    }
}

// ---------------------------------------------------------------------------
// Navigation and commands
// ---------------------------------------------------------------------------

/// An arrow with a shaft, pointing in `turns` quarter-turns clockwise from up.
fn arrow(p: &Painter, rect: Rect, color: Color32, turns: f32) {
    let g = Grid::new(rect);
    let center = g.at(8.0, 8.0);
    let (sin, cos) = (turns * std::f32::consts::FRAC_PI_2).sin_cos();
    let turn = |x: f32, y: f32| {
        let (dx, dy) = (x - 8.0, y - 8.0);
        pos2(
            center.x + (dx * cos - dy * sin) * g.scale,
            center.y + (dx * sin + dy * cos) * g.scale,
        )
    };
    let stroke = g.stroke(color);
    p.line_segment([turn(8.0, 13.0), turn(8.0, 3.4)], stroke);
    path(
        p,
        vec![turn(3.8, 7.6), turn(8.0, 3.4), turn(12.2, 7.6)],
        stroke,
    );
}

/// Run: a filled triangle.
///
/// Filled rather than stroked, and that is the point of the pair — Run and Stop share one slot in
/// the console's prompt strip and swap, so they have to read as two states of one control rather
/// than as two different icons. Both are solid, and both fill about the same optical area.
///
/// **Centred by its ink, not by its box.** A triangle's centroid is a third of the way from its
/// base, so one drawn symmetrically about the grid's middle looks pushed right; the leading edge is
/// therefore inset less than the trailing point sticks out.
pub fn play(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    p.add(Shape::convex_polygon(
        vec![g.at(5.2, 3.6), g.at(12.4, 8.0), g.at(5.2, 12.4)],
        color,
        Stroke::NONE,
    ));
}

/// Stop: a filled square with the corners taken off.
///
/// `radius-small`'s worth of rounding at this size, so it belongs to the same family as every other
/// filled shape in the window rather than being the one hard-edged rectangle on screen.
pub fn stop(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    p.rect_filled(g.rect(4.6, 4.6, 11.4, 11.4), g.radius(1.0), color);
}

/// Pause: two filled bars.
///
/// The third member of the family [`play`] and [`stop`] make, and it obeys the same rule they do —
/// filled, and about the same optical area — because the preview panel's strip swaps this and Play
/// in one slot. Two glyphs of visibly different weight in a slot that swaps reads as the button
/// moving rather than as the state changing.
///
/// The bars are `radius-small` like Stop's square, and 2.4 units wide with a 1.4 gap: any thinner
/// and the pair turns into a single grey block at fourteen pixels, which is the size this is drawn
/// at everywhere it appears.
pub fn pause(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    p.rect_filled(g.rect(4.9, 3.6, 7.3, 12.4), g.radius(0.6), color);
    p.rect_filled(g.rect(8.7, 3.6, 11.1, 12.4), g.radius(0.6), color);
}

/// Sound: a speaker cone and the two arcs coming off it.
///
/// Filled, like [`play`] — this is a control in the same strip, not a file glyph, so the outlined
/// convention at the top of this file has nothing to say about it. The cone is two shapes because
/// the silhouette is not convex and `convex_polygon` is what draws a clean edge: a box and the
/// trapezoid flaring out of it.
pub fn sound(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    cone(p, &g, color);
    // Two arcs off the cone's mouth, the near one shorter — which is what reads as sound rather
    // than as two brackets.
    for (radius, sweep) in [(2.6, 0.85), (4.5, 1.0)] {
        const STEPS: usize = 10;
        let center = g.at(8.6, 8.0);
        let points = (0..=STEPS)
            .map(|i| {
                let a = -sweep + 2.0 * sweep * i as f32 / STEPS as f32;
                center + Vec2::new(a.cos(), a.sin()) * (g.scale * radius)
            })
            .collect();
        p.add(Shape::line(points, g.hairline(color)));
    }
}

/// And sound turned off: the same cone with a cross where the arcs were.
///
/// A cross rather than one diagonal slash across the whole glyph. The slash is what a web player
/// draws, and at fourteen pixels it lands *on* the cone and turns the silhouette into a smudge; a
/// cross beside it leaves the cone legible and is the same "not this" mark either way.
pub fn sound_off(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    cone(p, &g, color);
    let stroke = g.hairline(color);
    p.line_segment([g.at(10.8, 5.8), g.at(14.2, 10.2)], stroke);
    p.line_segment([g.at(14.2, 5.8), g.at(10.8, 10.2)], stroke);
}

/// Fullscreen: four corners pointing out of the box.
///
/// Corners rather than a rectangle with arrows in it, which is what fits at fourteen pixels — and
/// they are drawn as two strokes each rather than as one bent line, because a `Shape::line` of three
/// points renders its join and this reads better as two clean segments meeting.
pub fn fullscreen(p: &Painter, rect: Rect, color: Color32) {
    corners(p, rect, color, true);
}

/// And leaving it: the same four corners turned to point in.
///
/// The pair swap in one slot the way [`play`] and [`pause`] do, so they are the same drawing with one
/// argument between them — which is the only way two glyphs in a slot that swaps stay the same
/// weight.
pub fn fullscreen_exit(p: &Painter, rect: Rect, color: Color32) {
    corners(p, rect, color, false);
}

/// The four corners both fullscreen glyphs are, `out` deciding which way they open.
fn corners(p: &Painter, rect: Rect, color: Color32, out: bool) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    // The near and far ends of one arm, in grid units from the corner it belongs to. Pointing out,
    // the corner sits at the edge and the arms run inwards; pointing in, it sits inside and they run
    // towards the edge. Same ink either way, which is the point.
    let (corner, arm) = if out { (2.4, 4.2) } else { (6.0, 2.4) };
    for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
        // The corner of this quadrant, mirrored out of the top-left one.
        let at = |x: f32, y: f32| {
            g.at(
                if sx > 0.0 { x } else { 16.0 - x },
                if sy > 0.0 { y } else { 16.0 - y },
            )
        };
        p.line_segment([at(corner, corner), at(arm, corner)], stroke);
        p.line_segment([at(corner, corner), at(corner, arm)], stroke);
    }
}

/// The speaker both sound glyphs are built on, so the two cannot drift apart.
fn cone(p: &Painter, g: &Grid, color: Color32) {
    p.rect_filled(g.rect(2.4, 6.4, 5.6, 9.6), g.radius(0.4), color);
    fill(
        p,
        vec![
            g.at(5.2, 6.9),
            g.at(9.0, 3.4),
            g.at(9.0, 12.6),
            g.at(5.2, 9.1),
        ],
        color,
    );
}

/// Back.
pub fn arrow_left(p: &Painter, rect: Rect, color: Color32) {
    arrow(p, rect, color, 3.0);
}

/// Forward.
pub fn arrow_right(p: &Painter, rect: Rect, color: Color32) {
    arrow(p, rect, color, 1.0);
}

/// Up one level.
pub fn arrow_up(p: &Painter, rect: Rect, color: Color32) {
    arrow(p, rect, color, 0.0);
}

/// Down. Only the status line's "behind by" uses it, and it is the same arrow turned round.
pub fn arrow_down(p: &Painter, rect: Rect, color: Color32) {
    arrow(p, rect, color, 2.0);
}

/// A shortcut: an arrow on the diagonal, which is what "stands for somewhere else" looks like
/// everywhere.
///
/// The mark on the drag sign for an Alt-drag — see [`crate::ui::drag_sign`], where it sits beside
/// the plus that means copy and the level arrow that means move.
///
/// **[`arrow`] at half a turn, and not a figure of its own.** The first attempt here was a bent
/// elbow with a head on the corner, drawn from scratch: a bracket with a tick on it next to its
/// neighbours, a fifth heavier in the stroke, and sitting low and right of the arrows either side of
/// it. Every one of those faults came from re-deriving what [`arrow`] already knows — the 1.5-unit
/// stroke, the shaft from 13 to 3.4, the head at ±4.2 either side of the tip, and the rotation about
/// the grid's centre that keeps all four of them on one optical axis. So this is that arrow turned
/// 45°, and it is aligned with the other three by construction rather than by eye.
///
/// The diagonal is what separates it from [`arrow_right`] and its move: up-and-right is
/// *elsewhere*, level is *over there*. It is also the direction the badge on [`folder_link`] points,
/// so a row that already *is* a shortcut and a gesture about to make one agree.
pub fn link(p: &Painter, rect: Rect, color: Color32) {
    arrow(p, rect, color, 0.5);
}

// ---------------------------------------------------------------------------
// Git
// ---------------------------------------------------------------------------

/// A branch: a trunk, a fork off it, and a node on each end.
///
/// The mark every git tool uses, so it needs no label — and it is the *only* thing in the status
/// line that says the rest of that line is about a repository.
///
/// Drawn as two nodes joined by a line rather than three: the shape reads at eleven pixels, which
/// is the size a caption-height status line gives it, and a third node makes it a smudge.
pub fn branch(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.hairline(color);
    let radius = g.scale * 2.0;

    // The trunk, down the left, and the fork that leaves it halfway and rises to the right.
    path(p, vec![g.at(4.5, 3.5), g.at(4.5, 12.5)], stroke);
    path(
        p,
        vec![g.at(4.5, 8.5), g.at(9.0, 8.5), g.at(11.5, 6.0)],
        stroke,
    );
    for at in [g.at(4.5, 3.0), g.at(4.5, 13.0), g.at(12.0, 5.0)] {
        p.circle_filled(at, radius, color);
    }
}

/// A git state as a badge over a row's icon: a disc in `fill`, and a shape in `ink` where the shape
/// says something the colour alone does not.
///
/// The overlay is the shell's own idea — TortoiseSVN's, Explorer's own sync overlays — and it is
/// bottom-left for the same reason theirs are: that corner of a file glyph is empty in every icon
/// set, so the badge covers nothing worth seeing.
///
/// **A disc first, shape second.** At ten pixels a glyph alone is four grey pixels over whatever the
/// icon underneath happens to be; a filled disc is a solid, unambiguous mark at any size, and it is
/// what makes the state readable at a glance down a column. The shape inside it is for the states
/// somebody has to tell apart *without* relying on colour — added from deleted, clean from changed —
/// which is a legibility requirement rather than a decoration.
///
/// **And it is punched out of the row rather than laid on top of it.** `under` is the fill the row is
/// wearing, drawn as a ring around the badge and used for the shape inside it — so the badge separates
/// from the icon it overlaps whatever that icon happens to be. Without it, the amber of a changed
/// folder sits on the amber of the folder glyph and reads as a bite out of the corner, which is what
/// the first version of this did.
pub fn git_badge(p: &Painter, rect: Rect, state: crate::git::State, disc: Color32, under: Color32) {
    use crate::git::State;
    let g = Grid::new(rect);
    let center = g.at(8.0, 8.0);
    let radius = g.scale * 6.2;
    let ink = under;

    // The gap between the badge and everything under it.
    p.circle_filled(center, radius + (g.scale * 1.6).max(1.5), under);

    // Untracked is the one hollow badge: nothing is *in* git, and a ring reads as the outline of
    // something that is not there. It also keeps the state a disk is fullest of — build output,
    // editor backups — the quietest thing on screen.
    if state == State::Untracked {
        // Thick enough to be a ring rather than a scratch: at six pixels across, a one-pixel stroke
        // antialiases into an uneven arc and reads as a broken circle.
        p.circle_stroke(
            center,
            radius - g.scale * 1.3,
            Stroke::new((g.scale * 2.6).max(1.6), disc),
        );
        return;
    }

    p.circle_filled(center, radius, disc);
    let stroke = Stroke::new((g.scale * 1.9).max(1.5), ink);
    match state {
        // A tick, and the same one the design system draws — a shorter left arm than right, which is
        // what stops it reading as a V.
        State::Clean => path(
            p,
            vec![g.at(4.6, 8.2), g.at(6.9, 10.6), g.at(11.6, 5.4)],
            stroke,
        ),
        // Added: a plus, because that is what staging a new file is.
        State::Staged => {
            path(p, vec![g.at(8.0, 4.4), g.at(8.0, 11.6)], stroke);
            path(p, vec![g.at(4.4, 8.0), g.at(11.6, 8.0)], stroke);
        }
        // Gone: a minus, the plus with the other half of the story taken out.
        State::Deleted => path(p, vec![g.at(4.4, 8.0), g.at(11.6, 8.0)], stroke),
        // Moved: an arrow, which is the only thing a rename is.
        State::Renamed => {
            path(p, vec![g.at(4.2, 8.0), g.at(10.4, 8.0)], stroke);
            fill(
                p,
                vec![g.at(8.8, 4.6), g.at(12.4, 8.0), g.at(8.8, 11.4)],
                ink,
            );
        }
        // A conflict: the bar and dot of an exclamation mark, which is legible at this size where a
        // triangle-and-bar is not.
        State::Conflicted => {
            path(p, vec![g.at(8.0, 4.0), g.at(8.0, 9.0)], stroke);
            p.circle_filled(g.at(8.0, 11.6), g.scale * 1.3, ink);
        }
        // Modified is the disc itself. It is the state a working tree is *usually* in, so it gets
        // the plainest mark there is — and the one with no interior detail to lose at small sizes.
        State::Modified | State::Untracked => {}
    }
}

/// A sync state, as the Status column shows it: Explorer's own marks, drawn on the grid.
///
/// - **Online** is a cloud's outline — nothing here but the name.
/// - **Local** is a ring with a tick in it, and **Pinned** the same tick knocked out of a solid
///   disc: Explorer's pair, where filling the mark in is what "always" adds.
/// - **Syncing** is [`refresh`]'s arrow, **Warning** and **Error** a disc with the mark knocked out
///   of it — the exclamation mark [`git_badge`] uses for a conflict, and a cross.
///
/// `under` is what the row is filled with, and is what the knocked-out shapes are drawn in, so the
/// marks read as holes in the disc on a selected row as well as on a plain one.
pub fn sync_status(p: &Painter, rect: Rect, state: crate::fs::dir::Sync, ink: Color32, under: Color32) {
    use crate::fs::dir::Sync;
    let g = Grid::new(rect);
    let center = g.at(8.0, 8.0);
    let radius = g.scale * 6.0;
    let mark = Stroke::new((g.scale * 1.6).max(1.2), under);
    let tick = [g.at(5.0, 8.2), g.at(7.1, 10.3), g.at(11.2, 5.9)];
    match state {
        Sync::Online => {
            // The cloud is filled in `ink` and filled again in `under`, one line-width in, which
            // is how an outline of three overlapping circles comes out without the seams a stroke
            // around each would leave inside it. Centred by its ink: 2.2–13.8 across, 4–12 down.
            let w = (g.scale * 1.5).max(1.0);
            let lobes = [(4.8, 9.4, 2.6), (8.2, 7.6, 3.6), (11.4, 9.6, 2.4)];
            let base = g.rect(4.8, 9.0, 11.4, 12.0);
            for (color, inset) in [(ink, 0.0), (under, w)] {
                for (x, y, r) in lobes {
                    p.circle_filled(g.at(x, y), r * g.scale - inset, color);
                }
                p.rect_filled(
                    Rect::from_min_max(base.min, pos2(base.max.x, base.max.y - inset)),
                    CornerRadius::ZERO,
                    color,
                );
            }
        }
        Sync::Local => {
            p.circle_stroke(center, radius - g.scale * 0.6, g.stroke(ink));
            path(p, tick.to_vec(), g.stroke(ink));
        }
        Sync::Pinned => {
            p.circle_filled(center, radius, ink);
            path(p, tick.to_vec(), mark);
        }
        Sync::Syncing => refresh(p, rect, ink),
        Sync::Warning => {
            p.circle_filled(center, radius, ink);
            path(p, vec![g.at(8.0, 4.4), g.at(8.0, 8.8)], mark);
            p.circle_filled(g.at(8.0, 11.2), g.scale * 1.1, under);
        }
        Sync::Error => {
            p.circle_filled(center, radius, ink);
            path(p, vec![g.at(5.6, 5.6), g.at(10.4, 10.4)], mark);
            path(p, vec![g.at(10.4, 5.6), g.at(5.6, 10.4)], mark);
        }
    }
}

/// Refresh: a nearly-closed circle with an arrowhead.
pub fn refresh(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    let center = g.at(8.0, 8.0);
    let radius = g.scale * 5.4;

    // Three-quarters of a turn, leaving the gap at the top right for the head.
    const STEPS: usize = 24;
    let start = -std::f32::consts::FRAC_PI_2 + 0.5;
    let sweep = std::f32::consts::TAU * 0.82;
    let points = (0..=STEPS)
        .map(|i| {
            let a = start + sweep * i as f32 / STEPS as f32;
            center + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect();
    p.add(Shape::line(points, stroke));

    let tip = center + Vec2::new(start.cos(), start.sin()) * radius;
    fill(
        p,
        vec![
            tip + Vec2::new(-g.scale * 0.4, -g.scale * 2.6),
            tip + Vec2::new(g.scale * 3.0, -g.scale * 0.9),
            tip + Vec2::new(-g.scale * 0.9, g.scale * 1.4),
        ],
        color,
    );
}

/// A pen lying at 45° — the path bar's "this can be typed into" hint.
///
/// Filled rather than outlined, which is the one place this set's own convention bends and it
/// bends for legibility: at 14 pixels an outlined pencil is three near-parallel hairlines a
/// pixel apart, and what reads at that size is the silhouette. It is not a file or a folder,
/// so the rule those two divide has nothing to say about it.
///
/// Two shapes, not one: the barrel is a quadrilateral and the nib is the triangle that finishes
/// it, and keeping them apart is what lets the nib be sharper than a fifth corner on a convex
/// polygon would be.
/// It fills the grid corner to corner, which is the other half of being legible at this size:
/// a pen inset the way a folder is would be drawing a 10-unit glyph in a 16-unit box.
///
/// **The ink is symmetric about the centre of the grid — `2..14` on both axes** — because this
/// is drawn into a row, and a glyph in a row is centred by what you can see rather than by the
/// box it was handed. The first version of it spanned `4..16` vertically: the box was centred on
/// the path bar to the pixel and the pen still sat two units low, which is the one kind of
/// mistake in a row that everything says is correct and nothing looks it.
pub fn pencil(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    // The barrel: a 3.4-unit-wide bar at 45°, from the nib end (bottom left) to the butt.
    fill(
        p,
        vec![
            g.at(3.1, 10.5),
            g.at(11.6, 2.0),
            g.at(14.0, 4.4),
            g.at(5.5, 12.9),
        ],
        color,
    );
    // The nib: the tip, and the two barrel corners it comes to a point from — so the two
    // polygons share an edge and the join cannot show.
    fill(
        p,
        vec![g.at(2.0, 14.0), g.at(3.1, 10.5), g.at(5.5, 12.9)],
        color,
    );
}

/// A funnel — the filter box's affordance.
pub fn filter(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    path(
        p,
        vec![
            g.at(2.2, 3.2),
            g.at(13.8, 3.2),
            g.at(9.4, 8.4),
            g.at(9.4, 13.6),
            g.at(6.6, 12.2),
            g.at(6.6, 8.4),
        ],
        stroke,
    );
    p.line_segment([g.at(2.2, 3.2), g.at(6.6, 8.4)], stroke);
}

/// Three rows stepping in — a hierarchy. The path bar's flatten toggle.
///
/// The art is the *tree*, not the flat list the button produces, for the same reason the
/// hidden-files toggle shows an eye rather than a revealed file: what a toggle names is the
/// thing it is about, and the two states are said by the fill and the colour
/// [`crate::ui::tool_button`] gives a latched button. A flat list drawn as three flush bars
/// would be indistinguishable from a generic "list" glyph in any state.
///
/// Three strokes and nothing else, because this is drawn at 14 pixels: an arrowhead small
/// enough to fit beside them would be under three pixels wide, which at this weight is a blot
/// rather than a shape. The rows are right-aligned and step in by 3.6 units, which is what makes
/// the shape read as indentation rather than as a list.
///
/// The ink spans 2..14 on both axes, symmetric about the centre of the grid, so centring the
/// box centres the drawing — see [`pencil`], where getting that wrong cost two units.
pub fn flatten(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    for (indent, y) in [(2.0, 3.4), (5.6, 8.0), (9.2, 12.6)] {
        p.line_segment([g.at(indent, y), g.at(14.0, y)], stroke);
    }
}

/// Three bars of different lengths off a common left edge — the status line's measure toggle.
///
/// **The art is the answer, not the work.** A stopwatch or a `Σ` would be about the counting, and
/// what the button is about is the shape the counting produces: a column of bars saying which
/// folder holds the space.
///
/// **Horizontal, and that is the whole of it**: this is a picture of what appears in the Size
/// column, one bar per row, each as long as its share. It was drawn vertically first — three bars
/// rising off a baseline, which is the universal glyph for "a chart" and says *statistics* rather
/// than *these proportions*. Turning them on their side costs the generic reading and buys the
/// specific one, which is the better trade for a toggle whose whole job is to put those bars on
/// screen. Longest at the top, because that is the order the column comes out in: Size sorts
/// biggest-first, which is what somebody presses this to see.
///
/// Filled rather than stroked, so it cannot be mistaken for [`flatten`] over on the path bar —
/// which is also three horizontal marks, and is three *hairlines* stepping in from the right. The
/// two differences are the weight and the edge they line up on, and both are legible at 14 pixels.
/// Three outlined boxes would not be: that is six edges in twelve units, and it reads as hatching.
/// [`columns`] has the same note from the other side.
///
/// **The air between the bars is wider than they are thick**, which is the one measurement here that
/// had to be redone. `TOOL_ICON` is 14 pixels, so a grid unit is 0.875 of one — and the first version
/// put 2.8-unit bars a 0.8-unit gap apart, which is two and a half pixels of ink separated by
/// *seven tenths of a pixel*. It survived on antialiasing alone and read as one solid block with two
/// scratches in it. At 2.4 and 1.4 the ratio is 1.75:1 the other way, and the three bars resolve.
///
/// The axis is a [`Grid::hairline`] and not a [`Grid::stroke`], so it reads as the zero the bars
/// are measured from rather than as a fourth bar standing on end.
///
/// The ink spans 2.4..13.6 across and 2.6..13.4 down, symmetric about the centre of the grid, so
/// centring the box centres the drawing — see [`pencil`], where getting that wrong cost two units.
pub fn sizes(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    // The zero every bar starts at, a little past them at each end so it reads as an axis.
    p.line_segment([g.at(2.4, 2.6), g.at(2.4, 13.4)], g.hairline(color));
    for (top, end) in [(3.0, 13.6), (6.8, 10.2), (10.6, 6.8)] {
        p.rect_filled(g.rect(2.4, top, end, top + 2.4), g.radius(0.4), color);
    }
}

/// An eye — the path bar's preview toggle.
///
/// A lens rather than a panel-with-content, and the choice is the same one [`flatten`] makes:
/// **what a toggle draws is the thing it is about, not the shape of what it produces.** The
/// preview panel can be down the side or along the bottom, so a glyph of a panel would be wrong
/// half the time; the eye says "look at what is in this" whichever way the panel opens, and the
/// two states are said by the fill and the colour [`crate::ui::tool_button`] gives a latched
/// button.
///
/// Two arcs and a pupil, at 14 pixels. The lid is a pair of quadratic curves rather than one
/// ellipse, because an ellipse's ends are round and an eye's are points — and at this size that
/// difference is what tells it from a circle. Eight segments a side is enough that the curve reads
/// as smooth after the pixel snap.
///
/// **The bulge is `LID` units, which is most of the drawing**, and it has been raised twice. The
/// arithmetic is why: the curve's deviation from the midline peaks at `lift × 4t(1−t)`, whose
/// maximum is `lift`, so the eye is `2 × LID` tall against 12.8 wide. The first version used 1.8 —
/// 3.6 units, a 3.6:1 slit rather than an eye. 3.0 gave 6 units and 2.13:1, which is the proportion
/// an eye has when it is half closed; 4.0 gives 8 and 1.6:1, which is one that is open.
///
/// The pupil follows the lid rather than staying put: 1.9 units of radius in a 6-unit eye is 63% of
/// its height, and left alone in an 8-unit one it would read as a small dot in a large socket. 2.2
/// keeps it at 55%, and it is the *clearance* that says this was the right way round — 1.8 units
/// from the pupil's edge up to the lid's own line, where the old pair had 1.1 and very nearly
/// touched once both strokes were on the pixel grid.
///
/// The ink spans 1.6..14.4 across and 4.0..12.0 down, symmetric about the centre of the grid, so
/// centring the box centres the drawing — see [`pencil`], where getting that wrong cost two units.
pub fn eye(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    // A quadratic through the two corners of the eye, bulging by `lift` at its middle.
    let arc = |lift: f32| -> Vec<Pos2> {
        const STEPS: usize = 8;
        (0..=STEPS)
            .map(|i| {
                let t = i as f32 / STEPS as f32;
                // The Bézier's own basis, written out: one control point, so this is two
                // multiplies rather than a curve library.
                let x = 1.6 + 12.8 * t;
                let y = 8.0 + lift * 4.0 * t * (1.0 - t);
                g.at(x, y)
            })
            .collect()
    };
    for lift in [-LID, LID] {
        p.add(egui::Shape::line(arc(lift), stroke));
    }
    p.circle_filled(g.at(8.0, 8.0), 2.2 * g.scale, color);
}

/// How far one lid of [`eye`] bulges from the midline, in grid units. The eye is twice this tall.
const LID: f32 = 4.0;

/// Four arrows leaving the middle for the corners — *fit this in the panel*.
///
/// It was `window_maximize`, which is a rectangle and says "a window" rather than "make this fill
/// that". The arrows are what turn it into a verb.
///
/// **The corner bracket is the arrowhead**, and that is the whole trick at this size. The first
/// version drew a box with four headed arrows inside it, and at 14 pixels it came out as a blot:
/// each head was two 2.2-unit segments, which is about two device pixels, and the box's edge was
/// one more pixel a unit away from them. [`flatten`] has the same note. Here every element is at
/// least three units and there is nothing for them to collide with, so the shape resolves — and a
/// bracket at the end of a diagonal reads as an arrow anyway, which is why every media player's
/// fullscreen button is drawn this way.
///
/// The ink spans 2.4..13.6 on both axes, symmetric about the centre.
pub fn fit(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let out = |along: f32, across: f32| g.at(8.0 + dx * along, 8.0 + dy * across);
        // The shaft, from a clear gap at the middle out to the corner.
        p.line_segment([out(1.8, 1.8), out(5.6, 5.6)], stroke);
        // And the bracket: one arm along each axis, so the pair points where the shaft does.
        p.line_segment([out(5.6, 5.6), out(2.2, 5.6)], stroke);
        p.line_segment([out(5.6, 5.6), out(5.6, 2.2)], stroke);
    }
}

/// Three panels side by side — *show both images and the difference*, rather than the difference
/// alone.
///
/// Latched while all three are on show, which is what says the two you are comparing are still
/// there. Drawn as a box divided twice rather than as three separate boxes: three boxes at 14
/// pixels is three outlines and two gaps in 12 units, which is one pixel each and reads as hatching.
pub fn columns(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    p.rect_stroke(
        g.rect(1.8, 2.8, 14.2, 13.2),
        g.radius(1.2),
        stroke,
        StrokeKind::Middle,
    );
    for x in [6.0, 10.0] {
        p.line_segment([g.at(x, 2.8), g.at(x, 13.2)], g.hairline(color));
    }
}

/// A gutter with three lines beside it — *number the lines*.
///
/// Not digits. A `1` and a `2` at 14 pixels are three pixels wide and would be two smudges; what
/// carries the meaning at this size is the *gutter* — a rule with short marks to its left and full
/// lines to its right, which is the shape of a numbered listing whether or not the numerals resolve.
pub fn line_numbers(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let rule = g.hairline(color);
    p.line_segment([g.at(5.6, 2.6), g.at(5.6, 13.4)], rule);
    for y in [4.2, 8.0, 11.8] {
        // The number, as a mark: short, and clear of the rule.
        p.line_segment([g.at(2.4, y), g.at(4.2, y)], g.stroke(color));
        p.line_segment([g.at(7.2, y), g.at(14.0, y)], g.stroke(color));
    }
}

/// Two angle brackets — *show me the markup and not the document*.
///
/// The one glyph in the set that is a pair of *characters* rather than a picture, and that is why it
/// works: `<` and `>` are what markup looks like, in every language that has any, and nothing else
/// says "this is the source of that" in fourteen pixels. A page-with-a-corner-folded would say
/// "document", which is the state this button leaves rather than the one it goes to.
///
/// Two chevrons and no slash between them. `</>` is the more familiar mark, but a third element in a
/// 12-unit span leaves each one three units wide with one unit of air, and at this size that is a
/// blot — the same arithmetic as [`flatten`] and [`fit`]. The gap in the middle is what the pair
/// needs to read as two brackets and not as a diamond.
///
/// The ink spans 2.5..13.5 across and 4.5..11.5 down, symmetric about the centre.
pub fn markup(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.stroke(color);
    // Each chevron opens away from the middle, so the pair brackets the space between them.
    for (near, far) in [(6.5, 2.5), (9.5, 13.5)] {
        path(p, vec![g.at(near, 4.5), g.at(far, 8.0), g.at(near, 11.5)], stroke);
    }
}

/// A diff: a plus over a minus, which is the whole of what one says.
///
/// Not the `±` glyph and not two chevrons: at fourteen pixels the two bars have to be separated by
/// more than a hairline, so they are drawn as two rows with the plus's stem in the upper one.
pub fn diff(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.hairline(color);
    // The plus, in the top half.
    path(p, vec![g.at(3.2, 5.2), g.at(12.8, 5.2)], stroke);
    path(p, vec![g.at(8.0, 1.6), g.at(8.0, 8.8)], stroke);
    // The minus, in the bottom.
    path(p, vec![g.at(3.2, 11.6), g.at(12.8, 11.6)], stroke);
}

/// Collapse the parts that have not changed: two rows of text with the middle folded away.
///
/// The dotted middle is the point — it is what the collapsed region looks like on the canvas, so the
/// button says what it does rather than naming it.
pub fn collapse(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    let stroke = g.hairline(color);
    for y in [2.4, 4.8, 11.2, 13.6] {
        path(p, vec![g.at(2.6, y), g.at(13.4, y)], stroke);
    }
    // The seam, dotted rather than drawn: three marks read as "something is missing here" where a
    // line reads as one more row of text.
    for x in [5.4, 8.0, 10.6] {
        p.circle_filled(g.at(x, 8.0), (g.scale * 0.85).max(0.9), color);
    }
}

/// A console: a screen with a prompt in it.
///
/// The frame is [`split_down`]'s, deliberately — the console *is* a band across the bottom of a
/// pane, so the two glyphs sharing an outline says they are about the same rectangle. What is inside
/// it is the difference: a prompt rather than a filled band.
///
/// The prompt is drawn large for the size it is read at. This glyph's one job is on an 18-point
/// switch in a 22-point status bar, where the whole chevron is under four pixels tall — so it takes
/// most of the height of the screen it is in rather than being a faithful little `>_`.
pub fn terminal(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    p.rect_stroke(
        g.rect(1.8, 3.0, 14.2, 13.0),
        g.radius(1.2),
        g.stroke(color),
        StrokeKind::Middle,
    );
    let hair = g.hairline(color);
    path(
        p,
        vec![g.at(4.4, 6.0), g.at(7.0, 8.2), g.at(4.4, 10.4)],
        hair,
    );
    path(p, vec![g.at(8.4, 10.4), g.at(11.6, 10.4)], hair);
}

/// Split the pane left/right. Also the drop hint for a side dock.
pub fn split_side(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    p.rect_stroke(
        g.rect(1.8, 3.0, 14.2, 13.0),
        g.radius(1.2),
        g.stroke(color),
        StrokeKind::Middle,
    );
    p.line_segment([g.at(8.0, 3.0), g.at(8.0, 13.0)], g.stroke(color));
    p.rect_filled(g.rect(8.8, 4.6, 13.0, 11.4), g.radius(0.6), color);
}

/// Split the pane top/bottom.
pub fn split_down(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    p.rect_stroke(
        g.rect(1.8, 3.0, 14.2, 13.0),
        g.radius(1.2),
        g.stroke(color),
        StrokeKind::Middle,
    );
    p.line_segment([g.at(1.8, 8.0), g.at(14.2, 8.0)], g.stroke(color));
    p.rect_filled(g.rect(3.2, 8.8, 12.8, 11.8), g.radius(0.6), color);
}

/// Four tiles — *show this folder as large icons*.
///
/// Filled rather than outlined, which is the one thing that makes it read at eleven points on the
/// status line: four 5-unit squares in outline would be four rings a pixel and a half wide with a
/// pixel of air inside each, and at that size a ring is a blot. The same arithmetic as [`flatten`]
/// and [`fit`], arrived at the same way.
///
/// The convention it appears to break is the set's own — "folders are filled, files are outlined" —
/// and it does not: these are not files. They are the *cells of a grid*, which is what the button is
/// about, and every icon-view button on every platform is drawn as four squares for exactly that
/// reason.
///
/// The ink spans 2..14 on both axes, symmetric about the centre of the grid, so centring the box
/// centres the drawing — see [`pencil`], where getting that wrong cost two units.
pub fn grid_view(p: &Painter, rect: Rect, color: Color32) {
    let g = Grid::new(rect);
    for (x, y) in [(2.0, 2.0), (9.0, 2.0), (2.0, 9.0), (9.0, 9.0)] {
        p.rect_filled(g.rect(x, y, x + 5.0, y + 5.0), g.radius(0.8), color);
    }
}
