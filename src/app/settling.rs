//! Keeping frames coming while the window changes shape, so the compositor never stretches one.
//!
//! Lived in `capture.rs` beside the developer flags until those left the release build; this half
//! never was one of them — every window runs it.

/// What the window looks like to the platform, as far as rendering crisply is concerned.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(super) struct Shape {
    /// In *physical pixels*, which is what the framebuffer is sized in.
    pub(super) pixels: (u32, u32),
    /// Rounded, because a scale factor arrives as a float and comparing floats for
    /// equality every frame is a way to request repaints for ever.
    pub(super) scale: u32,
    /// What the *platform* says the scale is, against `scale`, which is what egui rasterised
    /// the glyph atlas for.
    ///
    /// Tracked separately because the two disagreeing is the one thing that would make text
    /// soft without anything else moving: an atlas built for one scale, drawn at another.
    /// Zero when the platform has not said.
    pub(super) native_scale: u32,
    pub(super) focused: bool,
    pub(super) minimized: bool,
}

impl Shape {
    pub(super) fn of(ctx: &egui::Context) -> Self {
        let scale = ctx.pixels_per_point();
        ctx.input(|i| {
            let size = i.viewport_rect().size() * scale;
            Self {
                pixels: (size.x.round() as u32, size.y.round() as u32),
                scale: (scale * 1000.0).round() as u32,
                native_scale: i
                    .viewport()
                    .native_pixels_per_point
                    .map_or(0, |ppp| (ppp * 1000.0).round() as u32),
                focused: i.viewport().focused.unwrap_or(true),
                minimized: i.viewport().minimized.unwrap_or(false),
            }
        })
    }
}

/// Asks for frames for a moment after the window's shape changes.
///
/// This program paints on demand, which is the right default — a file listing that repaints
/// sixty times a second to show the same rows is a laptop fan. But it means that between
/// events the screen holds whatever was painted last, and a *stale* frame is only as good
/// as the assumption that the window still looks the way it did: restore it from the
/// taskbar, drag it to a monitor at another scale, or let a maximised window resize when
/// the taskbar hides, and the compositor has a surface of the wrong size to show. It
/// stretches it, which arrives as text going soft for as long as nothing asks for a new
/// frame — and then coming back sharp the moment something does.
///
/// So: notice the shape changing, and keep asking for frames until it has been still for
/// [`Self::QUIET`]. Costs a handful of frames per window gesture and nothing at rest.
#[derive(Default)]
pub(super) struct Settling {
    pub(super) was: Shape,
    /// When the shape last changed, in `InputState::time`.
    pub(super) changed_at: Option<f64>,
}

impl Settling {
    /// How long after a change to keep painting. Long enough to cover a restore animation
    /// and a scale change, short enough that nobody notices the frames.
    pub(super) const QUIET: f64 = 0.75;

    /// Whether a frame should be asked for.
    pub(super) fn observe(&mut self, now: Shape, time: f64) -> bool {
        if now != self.was {
            self.was = now;
            self.changed_at = Some(time);
        }
        match self.changed_at {
            Some(at) if time - at < Self::QUIET => true,
            Some(_) => {
                self.changed_at = None;
                false
            }
            None => false,
        }
    }
}
