//! The preview panel: what is *in* the file the keyboard is on.
//!
//! **Inside the pane**, not across the window, and that is the whole design. A preview belongs to
//! the folder you are looking at — two panes side by side each get their own, showing their own
//! selection, which is what makes comparing two builds of the same DLL a matter of looking left
//! and right rather than clicking back and forth. So [`Preview`] hangs off [`crate::pane::Tab`]:
//! each folder has one, shut by default.
//!
//! # Right, bottom, or whichever fits
//!
//! [`Where`] is the window's preference — one setting, not one per folder, because "where the
//! preview goes" is a habit and "is it showing for this folder" is not. [`Where::Auto`] picks from
//! the pane's own shape every frame: **a pane wider than it is tall gets a panel down the right,
//! and anything squarer or taller gets one along the bottom.**
//!
//! The threshold is 1.25 rather than 1.0, and it leans that way deliberately: width is the scarcer
//! thing in a listing. Four columns and a path bar need it, and taking 40% of a 500-point pane
//! away leaves a Name column with room for `translations_f…`. Height costs a listing nothing but
//! rows, and rows scroll.
//!
//! # Four views
//!
//! A picture, **two** pictures compared, some text, or — for a binary — [`crate::ui::deps`]'s
//! dependency tree. Which one is [`crate::preview::kind_of`]'s answer and the *selection*'s (two
//! pictures selected at once is the comparison), the reading is [`crate::preview::Previews`]'s, and
//! this module is where each of them is drawn. Anything else says so rather than showing an empty
//! box.
//!
//! The comparison has a second way in, and it is the same view: **one** picture git has an older
//! version of, against that version. A `.png` has no lines to put a diff's band behind, so what
//! `Layout::diff` means for a picture is the two of them and the difference — see
//! [`crate::preview::against_head`].
//!
//! # What goes on the bar, and what goes first when it will not fit
//!
//! Left to right: the name, then a comment, then the size, then the controls — and the controls are
//! the last thing to go. What gives way, in order, is **the comment, then the size, then the name**,
//! which is [`header`]'s one non-obvious rule: a long name never crops while a detail could have
//! been dropped instead, because the name is what identifies the file and the size is a nicety.
//!
//! # One file per view
//!
//! This module keeps what the four views share — where the panel goes, how big it is, and the
//! state that survives a selection moving — and each view is drawn in a file of its own, the same
//! way [`crate::preview`] reads them:
//!
//! | module | what it draws |
//! | --- | --- |
//! | [`header`] | the bar along the top, for every view |
//! | [`picture`] | one image, or three being compared |
//! | [`text`] | the body, its colouring, and the gutter |
//! | [`diff`] | a text file against `HEAD` |
//! | [`document`] | Markdown, as a document rather than as source |
//! | [`find`] | the find bar over a text body |

use std::ops::Range;
use std::path::Path;

use azur_egui_theme::components::{
    field_frame, field_hovered, galley_on_baseline, ink_baseline, FieldLook,
};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::{radius, space};
use egui::{pos2, vec2, Color32, CornerRadius, Id, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::app::Action;
use crate::markdown;
use crate::pane::PaneId;
use crate::preview::{self, Ask, Payload};
use crate::syntax;
use crate::theme::Theme;
use crate::ui::{
    control_fills, deps, icon_rect, seam, text_center, tool_button, truncated, SEAM, TOOL_SIZE,
};

mod diff;
mod document;
mod find;
mod header;
mod picture;
mod text;
#[cfg(test)]
mod tests;

// The view modules reach each other through here: the panel is one thing on screen and the
// pieces of it are not independent — the find bar's marks land on the text canvas, the diff
// view hands that canvas a body, and the header draws a control for whichever view is up.
use diff::{diff_fill, Diffed, Line, Mark};
use find::{find_bar, Find, FIND_H};
use document::{CHIP, GUTTER_GAP};
use header::{header, CAPTION, ZOOM_MAX, ZOOM_MIN, ZOOM_STEP, ZOOM_W};
use picture::{pictures, Frame, Picture};
use text::{ink, overlay, reveal, text_canvas, Faces, Marks, Text};

/// The panel's own bar: the file's name, what it turned out to be, and the controls.
///
/// Shorter than a pane's path bar. This is furniture *inside* a pane, and a second 32-point bar
/// under the first one made the pane look like two panes.
pub const HEADER: f32 = 28.0;

/// How much of the pane the panel takes to begin with, along whichever axis it is split on.
pub const SHARE: f32 = 0.42;

/// What the listing keeps, whatever the panel is dragged to.
const MIN_LIST: f32 = 180.0;

/// What a panel down the side keeps: enough for its bar's controls and a little canvas.
///
/// Wider than it looks like it needs to be, and the reason is the bar: the zoom field and the four
/// buttons are a fixed cost before the name gets anything, and a panel too narrow to show its own
/// controls is a panel with no way back to 100%. See [`ACTIONS`].
const MIN_PANEL_W: f32 = 240.0;

/// And what a panel along the bottom keeps: its bar and a little canvas. Height only — its width is
/// the pane's, which nothing here gets to choose.
const MIN_PANEL_H: f32 = HEADER + 64.0;

/// A pane has to be wider than this multiple of its height before [`Where::Auto`] puts the panel
/// down the side. See the module header for why it is not 1.0.
const AUTO_RATIO: f32 = 1.25;

/// The air inside the panel's bar and around its canvas.
const PAD: f32 = space::S2;

/// The glyph in the bar, saying which of the views this is.
const GLYPH: f32 = 14.0;

/// The mark beside a count that must not itself be coloured.
const MARK: f32 = 12.0;

/// What the bar's controls cost when they are all there — the zoom field, Fit, out, in, and close.
///
/// Only used to size [`MIN_PANEL_W`], but worth naming: it is the number that decides how narrow a
/// panel is allowed to be.
const ACTIONS: f32 = ZOOM_W + TOOL_SIZE * 4.0 + PAD * 4.0;

/// How long the selection has to sit still before the panel follows it.
///
/// The same quarter second the filter waits, for the same reason and with more at stake: holding
/// the down arrow through a folder of photographs would otherwise decode thirty of them,
/// twenty-nine of which nobody is going to look at. See [`crate::pane::FILTER_DELAY`].
pub const FOLLOW_DELAY: f64 = 0.25;

/// Where the panel goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Where {
    #[default]
    Right,
    Bottom,
    /// Whichever suits the pane's shape. See the module header.
    Auto,
}

/// Which side it ends up on, once [`Where::Auto`] has been resolved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Right,
    Bottom,
}

impl Where {
    pub fn label(self) -> &'static str {
        match self {
            Self::Right => "Right",
            Self::Bottom => "Bottom",
            Self::Auto => "Auto",
        }
    }

    /// The three, in the order the menu lists them.
    pub const ALL: [Self; 3] = [Self::Right, Self::Bottom, Self::Auto];

    /// Which side the panel goes on in a pane of this shape.
    pub fn side(self, pane: Rect) -> Side {
        match self {
            Self::Right => Side::Right,
            Self::Bottom => Side::Bottom,
            Self::Auto => {
                if pane.width() >= pane.height() * AUTO_RATIO {
                    Side::Right
                } else {
                    Side::Bottom
                }
            }
        }
    }

    /// For the settings file, which is a `key=value` text file people are meant to be able to fix
    /// by hand — so the values are words rather than numbers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Bottom => "bottom",
            Self::Auto => "auto",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|at| at.as_str().eq_ignore_ascii_case(text))
    }
}

/// Where the panel goes, how much room it takes, and how its text view reads: the window's
/// preferences, one of each.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub at: Where,
    /// How much of the pane the panel takes along the split axis, before the clamps in
    /// [`split`].
    pub share: f32,
    /// Number the lines in the text view.
    ///
    /// A preference and not per-file, because it is a way of reading rather than a fact about a
    /// document: somebody who wants line numbers wants them in the next file too.
    pub numbers: bool,
    /// Show a document's markup instead of the document — the source of a `.md`.
    ///
    /// A preference for the same reason `numbers` is, and it only has any effect on a file that
    /// *has* a rendered form, which today means Markdown.
    pub markup: bool,
    /// Show what git says has changed in the file: the lines it gained on a green band, the ones it
    /// lost put back on a red one.
    ///
    /// **On by default**, because a file being previewed inside a repository is nearly always a file
    /// somebody is working on, and no other view answers "what did I change here" without leaving the
    /// window. It has no effect on a file with nothing changed in it — which is most files — so the
    /// default costs nothing to anybody who does not want it.
    pub diff: bool,
    /// Leave out the stretches of the file that have not changed, keeping [`CONTEXT`] lines either
    /// side of every change.
    ///
    /// **Off by default**, and the opposite argument to `diff`: this one hides most of the file, which
    /// is right when reviewing a change and wrong when reading a file that happens to have one.
    pub collapse: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            at: Where::default(),
            share: SHARE,
            numbers: false,
            markup: false,
            diff: true,
            collapse: false,
        }
    }
}

/// What is on the canvas.
enum Content {
    /// Nothing has been asked for: no selection, or one with no preview.
    Nothing,
    /// A selection with nothing to show, and what it was — `"zip"`, `"mp4"`.
    Unsupported(String),
    /// Asked for, on its way.
    Reading,
    Picture(Box<Picture>),
    Text(Text),
    Binary(deps::View),
    /// Something went wrong, in a few words.
    Failed(String),
}

/// One folder's preview panel.
pub struct Preview {
    pub open: bool,
    /// What is on show, or being read.
    of: Option<Ask>,
    /// A selection that has not sat still long enough yet: what, and since when.
    pending: Option<(Ask, f64)>,
    /// The read this is waiting for, if it is waiting for one.
    awaiting: Option<u64>,
    content: Content,
    find: Find,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            open: false,
            of: None,
            pending: None,
            awaiting: None,
            content: Content::Nothing,
            find: Find::default(),
        }
    }
}

impl Preview {
    /// A duplicate for a new tab of the same folder: open the same way, showing nothing yet.
    ///
    /// The content is deliberately not copied. A decoded picture is up to 16 MB — three of them for
    /// a comparison — and a graph is a megabyte of names; handing a copy to every `Ctrl+T` would
    /// make duplicating a tab the most expensive thing in the window. The panel is following the
    /// selection anyway, so it fills itself in a quarter of a second.
    pub fn duplicate(&self) -> Self {
        Self {
            open: self.open,
            ..Self::default()
        }
    }

    /// Whether a read has been asked for, or is about to be, and has not landed yet.
    pub fn busy(&self) -> bool {
        self.awaiting.is_some() || self.pending.is_some()
    }

    /// Whether this panel is the one waiting for `token`.
    ///
    /// Asked before the payload is handed over rather than after, because a payload is a decoded
    /// picture or a walked graph and there is only one of it — every other panel would have to be
    /// given a copy in order to reject it.
    pub fn wants(&self, token: u64) -> bool {
        self.awaiting == Some(token)
    }

    /// What the panel is looking at. For the tests; the panel reads the field.
    #[cfg(test)]
    pub fn showing(&self) -> Option<&Path> {
        self.of.as_ref().map(Ask::first)
    }

    /// How many rows the dependency tree is showing, if that is what this is. For the tests.
    #[cfg(test)]
    pub fn dependency_rows(&self) -> Option<usize> {
        match &self.content {
            Content::Binary(view) => Some(view.shown()),
            _ => None,
        }
    }

    /// Which row of the dependency tree a click could unfold. See
    /// [`crate::ui::deps::View::first_foldable`], which is where the reason this is needed is
    /// written down.
    #[cfg(test)]
    pub fn dependency_first_foldable(&self) -> Option<usize> {
        match &self.content {
            Content::Binary(view) => view.first_foldable(),
            _ => None,
        }
    }

    /// How many images this is holding, and how many are on show. For the tests, which is where
    /// the difference — the comparison's toggle — can be seen from.
    #[cfg(test)]
    pub fn frames(&self) -> Option<(usize, usize)> {
        match &self.content {
            Content::Picture(picture) => Some((picture.frames.len(), picture.showing().len())),
            _ => None,
        }
    }

    /// Open the find bar on `text`, without anybody having typed it.
    ///
    /// For `--find=`, so a capture can photograph the bar doing its job — the same reason
    /// `--preview` and `--compare` exist, and for the same reason: a screenshot run has no keyboard.
    /// The tests use it too, for the parts that are not about whether a keystroke lands.
    pub fn look_for(&mut self, text: &str) {
        self.find.open = true;
        self.find.search.text = text.to_owned();
    }

    /// Turn the regex toggle on or off. For the tests; the bar's own button does it directly.
    #[cfg(test)]
    pub fn set_regex(&mut self, on: bool) {
        self.find.search.regex = on;
    }

    /// The find bar: whether it is open, which hit is current, how many there are, and what the
    /// counter says. For the tests.
    #[cfg(test)]
    pub fn finding(&self) -> (bool, usize, usize, String) {
        (
            self.find.open,
            self.find.at,
            self.find.hits.len(),
            self.find.counter(),
        )
    }

    /// Flip a comparison between all three views and the difference alone. For the tests; the bar's
    /// button does it directly.
    #[cfg(test)]
    pub fn toggle_all(&mut self) {
        if let Content::Picture(picture) = &mut self.content {
            picture.all = !picture.all;
        }
    }

    /// Shut it, and let go of everything it was holding.
    ///
    /// Up to three decoded pictures, or a dependency graph. There is nothing to be gained by
    /// keeping any of it for a panel nobody is looking at, and re-reading is a quarter of a second
    /// — which is also what somebody who has just rebuilt the file means by opening the panel
    /// again.
    pub fn close(&mut self) {
        self.open = false;
        self.forget();
    }

    fn forget(&mut self) {
        self.of = None;
        self.pending = None;
        self.awaiting = None;
        self.content = Content::Nothing;
        // The hits belong to a body that is going. The *query* does not — see [`Find`].
        self.find.forget();
    }

    /// What the keyboard is on, offered every frame while the panel is open.
    ///
    /// `None` means the selection is not something with a preview — a folder, an archive, nothing
    /// at all — and then the panel says so rather than keeping the last answer. **Clearing is the
    /// right answer precisely because the panel is inside the pane**: it sits beside the row it is
    /// about, so a picture left next to a different selection would be read as being that
    /// selection's. A panel across the whole window could keep the last thing it was shown; this
    /// one cannot.
    pub fn follow(&mut self, what: Option<Ask>, now: f64) {
        let Some(ask) = what else {
            // The extension is the useful half of "nothing to show here".
            let ext = self
                .of
                .as_ref()
                .map(|ask| preview::extension_of(ask.first()));
            if self.of.is_some() || self.pending.is_some() {
                self.forget();
                self.content = Content::Unsupported(ext.unwrap_or_default());
            }
            return;
        };
        // Already the one on show, or the one being read: nothing to do, and in particular
        // nothing to ask for again. Without this the answer arriving would immediately queue
        // another read of the same file, for ever.
        if self.of.as_ref() == Some(&ask) {
            self.pending = None;
            return;
        }
        // Restarted on every change, so arrowing down a folder of images decodes the one you
        // stop on rather than each one on the way past.
        if self.pending.as_ref().map(|(pending, _)| pending) != Some(&ask) {
            self.pending = Some((ask, now));
        }
    }

    /// Ask for something at once, with no waiting — the keyboard asked for the panel itself.
    pub fn ask_for(&mut self, ask: Ask) {
        self.open = true;
        self.pending = Some((ask, f64::NEG_INFINITY));
    }

    /// Has the selection sat still long enough? What to read, or how long is left to wait.
    ///
    /// Both, because the caller has to book the frame that would notice: this program is idle
    /// between events, so a wait that nothing asks to be woken from is a wait that never ends.
    pub fn settle(&mut self, now: f64) -> (Option<Ask>, Option<f64>) {
        let Some((_, at)) = &self.pending else {
            return (None, None);
        };
        let left = FOLLOW_DELAY - (now - at);
        if left > 0.0 {
            return (None, Some(left));
        }
        let (ask, _) = self.pending.take().expect("just matched");
        (Some(ask), None)
    }

    /// A read has been started for what [`Self::settle`] handed back.
    pub fn asked(&mut self, ask: Ask, token: u64) {
        self.of = Some(ask);
        self.awaiting = Some(token);
        self.content = Content::Reading;
    }

    /// A read has come back. Ignored unless it is the one being waited for.
    pub fn arrived(&mut self, token: u64, payload: Payload, ctx: &egui::Context) {
        if self.awaiting != Some(token) {
            return;
        }
        self.awaiting = None;
        // A new body, so whatever was found in the last one is not an answer about this one. The
        // ranges are the sharp end of that: they become sections of the layout job that draws the
        // text, where an offset past the end of it is a panic and not a stray highlight.
        self.find.forget();
        // The names the captions use, for a comparison. Two files being compared are called by their
        // own names; a file against the version in the last commit is one file, so the captions have
        // to say which *version* each view is — the file's name twice would say nothing at all.
        let (first, second) = match &self.of {
            Some(Ask::Pair(a, b)) => (leaf(a), leaf(b)),
            Some(Ask::AgainstHead(_)) => ("last commit".to_owned(), "working copy".to_owned()),
            _ => (String::new(), String::new()),
        };
        self.content = match payload {
            // The one thing that has to happen on the UI thread: a texture is the graphics
            // device's, and the worker has no idea one exists.
            Payload::Picture(picture) => {
                let only = frame(ctx, picture.pixels, String::new(), false);
                Content::Picture(Box::new(Picture {
                    pixels: only.pixels,
                    frames: vec![only],
                    natural: picture.natural,
                    scaled: picture.scaled,
                    vector: picture.vector,
                    ..Picture::fresh()
                }
                .labelled()))
            }
            Payload::Diff(diff) => {
                let a = frame(ctx, diff.a.pixels, first, false);
                let b = frame(ctx, diff.b.pixels, second, false);
                let mask = frame(ctx, diff.mask.pixels, "differences".to_owned(), true);
                Content::Picture(Box::new(Picture {
                    // The mask is the larger of the two by construction, so it is the space all
                    // three are placed against — which is what lines corresponding pixels up
                    // across the views.
                    pixels: mask.pixels,
                    frames: vec![a, b, mask],
                    natural: diff.a.natural,
                    other: (diff.b.natural != diff.a.natural).then_some(diff.b.natural),
                    scaled: diff.a.scaled || diff.b.scaled,
                    vector: diff.a.vector || diff.b.vector,
                    differing: Some(diff.differing),
                    ..Picture::fresh()
                }
                .labelled()))
            }
            Payload::Text(text) => Content::Text(Text {
                spans: syntax::spans(&text.body, text.lang),
                doc: (text.lang == syntax::Lang::Markdown)
                    .then(|| markdown::parse(&text.body))
                    // A `.md` holding nothing a parser can see is not a document: the
                    // plain view is a better answer than one empty canvas.
                    .filter(|doc| !doc.is_empty()),
                body: text.body,
                truncated: text.truncated,
                code: text.code,
                lang: text.lang,
                changes: text.changes,
                // Built on the first frame this is drawn, from the toggles as they are then — the
                // panel's preference rather than anything this read decided.
                view: None,
                view_for: None,
            }),
            Payload::Binary(graph) => Content::Binary(deps::View::new(graph)),
            Payload::Failed(why) => Content::Failed(why),
        };
    }
}

/// A file's own name.
fn leaf(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Upload one image and describe it.
fn frame(ctx: &egui::Context, pixels: egui::ColorImage, label: String, mask: bool) -> Frame {
    let size = pixels.size;
    Frame {
        texture: ctx.load_texture(
            "preview",
            pixels,
            // Linear, because a preview is nearly always shown at some scale other than 1:1 and
            // nearest would make every one of them a staircase.
            egui::TextureOptions::LINEAR,
        ),
        pixels: vec2(size[0] as f32, size[1] as f32),
        label,
        mask,
    }
}

/// Take the panel's room off the pane's body, leaving the listing what is left.
///
/// The returned panel rect **includes the seam before it**, which is what makes the boundary one
/// line neither side draws — exactly as between two panes. `None` when the panel is shut, or when
/// the pane is too small to give it room without squeezing the listing past being usable.
pub fn split(body: Rect, open: bool, layout: Layout) -> (Rect, Option<Rect>) {
    if !open {
        return (body, None);
    }
    let side = layout.at.side(body);
    let (span, least) = match side {
        Side::Right => (body.width(), MIN_PANEL_W),
        Side::Bottom => (body.height(), MIN_PANEL_H),
    };
    let room = span - MIN_LIST - SEAM;
    if room < least {
        return (body, None);
    }
    let take = (span * layout.share).clamp(least, room);
    match side {
        Side::Right => {
            let edge = body.right() - take;
            (
                Rect::from_min_max(body.min, pos2(edge - SEAM, body.bottom())),
                Some(Rect::from_min_max(pos2(edge - SEAM, body.top()), body.max)),
            )
        }
        Side::Bottom => {
            let edge = body.bottom() - take;
            (
                Rect::from_min_max(body.min, pos2(body.right(), edge - SEAM)),
                Some(Rect::from_min_max(pos2(body.left(), edge - SEAM), body.max)),
            )
        }
    }
}

/// Draw the panel, and answer the pointer.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    preview: &mut Preview,
    layout: &mut Layout,
    body: Rect,
    // Whether git has a version of this file that is not the one on disk. Only a *picture* needs
    // telling — see [`header`].
    changed: bool,
    scratch: &mut String,
    out: &mut Vec<Action>,
) {
    let side = layout.at.side(body);

    // The seam before the panel, and the panel's own surface after it.
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::ZERO, seam(t));
    let inside = match side {
        Side::Right => Rect::from_min_max(pos2(rect.left() + SEAM, rect.top()), rect.max),
        Side::Bottom => Rect::from_min_max(pos2(rect.left(), rect.top() + SEAM), rect.max),
    };
    ui.painter()
        .rect_filled(inside, egui::CornerRadius::ZERO, t.bg.layer);

    grip(ui, rect, pane, side, layout, body);

    let bar = Rect::from_min_size(inside.min, vec2(inside.width(), HEADER));
    let canvas = Rect::from_min_max(bar.left_bottom(), inside.max);
    header(ui, t, bar, pane, preview, layout, changed, scratch, out);

    if canvas.height() < 24.0 || canvas.width() < 48.0 {
        return;
    }

    // The two halves of the panel state that are looked at together, and the only place they are:
    // the search is *about* the content, and the content is what the canvas draws.
    let Preview { content, find, .. } = preview;
    // The diff view, before anything reads the body: it *is* the body while it is on, so a search
    // that ran first would be holding offsets into the other string.
    if let Content::Text(text) = &mut *content {
        if text.follow(layout.diff, layout.collapse) {
            find.forget();
        }
    }
    // Run the search, if the query or the body has changed since it last ran. Before the canvas,
    // because the canvas is what highlights the hits and scrolls to the current one.
    if find.open {
        if let Content::Text(text) = &*content {
            find.against(text.shown(layout.markup));
        }
    }

    match content {
        Content::Nothing => note(ui, t, canvas, "Nothing selected"),
        Content::Unsupported(ext) => {
            let what = if ext.is_empty() {
                "No preview for this".to_owned()
            } else {
                format!("No preview for a .{ext}")
            };
            note(ui, t, canvas, &what);
        }
        // Nothing is drawn but the word. A read takes tens of milliseconds and a spinner that
        // appears and vanishes inside three frames is worse than nothing.
        Content::Reading => note(ui, t, canvas, "Reading…"),
        Content::Failed(why) => {
            let why = why.clone();
            note(ui, t, canvas, &why)
        }
        Content::Picture(picture) => pictures(ui, t, canvas, pane, picture),
        Content::Text(text) => match &text.doc {
            Some(doc) if !layout.markup => document::draw(ui, t, canvas, pane, doc, find),
            _ => text_canvas(ui, t, canvas, pane, text, layout.numbers, find),
        },
        Content::Binary(view) => deps::show(ui, t, canvas, view),
    }

    // And the find bar over the top of it, last, because it floats.
    if find.open && matches!(&*content, Content::Text(_)) {
        find_bar(ui, t, canvas, pane, find);
    }
}

/// A line of secondary text in the middle of the canvas, for the states that have nothing to draw.
fn note(ui: &Ui, t: &Theme, rect: Rect, text: &str) {
    text_center(
        ui.painter(),
        rect,
        t.fonts.body.clone(),
        t.text.secondary,
        text,
    );
}

/// The panel's edge, which is also how much room it has.
///
/// The same gesture as the sidebar's splitter and the column edges in the listing: drag to size,
/// double click to put it back. Reaching a few points either side of the one-point seam, because
/// a one-point grab target is a one-point grab target.
fn grip(ui: &mut Ui, rect: Rect, pane: PaneId, side: Side, layout: &mut Layout, body: Rect) {
    let band = match side {
        Side::Right => Rect::from_min_max(
            pos2(rect.left() - 3.0, rect.top()),
            pos2(rect.left() + SEAM + 3.0, rect.bottom()),
        ),
        Side::Bottom => Rect::from_min_max(
            pos2(rect.left(), rect.top() - 3.0),
            pos2(rect.right(), rect.top() + SEAM + 3.0),
        ),
    };
    let response = ui.interact(
        band,
        Id::new(("preview-grip", pane)),
        Sense::click_and_drag(),
    );
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(match side {
            Side::Right => egui::CursorIcon::ResizeHorizontal,
            Side::Bottom => egui::CursorIcon::ResizeVertical,
        });
    }
    if response.dragged() {
        // As a share rather than as points, so the panel keeps its proportion when the window is
        // resized — which is what makes `Auto` bearable: the panel does not have to be re-dragged
        // every time a split changes its pane's shape.
        let (delta, span) = match side {
            Side::Right => (-response.drag_delta().x, body.width()),
            Side::Bottom => (-response.drag_delta().y, body.height()),
        };
        layout.share = (layout.share + delta / span.max(1.0)).clamp(0.1, 0.9);
    }
    if response.double_clicked() {
        layout.share = SHARE;
    }
}
