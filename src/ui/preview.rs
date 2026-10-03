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

/// The zoom field.
///
/// Enough for `400%` and a chevron. It is a combo box rather than a label because a zoom you can
/// only reach through `+` and `−` is a zoom you cannot ask for: 100% from 874% is eight clicks.
const ZOOM_W: f32 = 64.0;

/// What the bar's controls cost when they are all there — the zoom field, Fit, out, in, and close.
///
/// Only used to size [`MIN_PANEL_W`], but worth naming: it is the number that decides how narrow a
/// panel is allowed to be.
const ACTIONS: f32 = ZOOM_W + TOOL_SIZE * 4.0 + PAD * 4.0;

/// The strip above each image in a comparison, holding which file it is.
const CAPTION: f32 = 16.0;

/// The find bar's height, and the field inside it with a margin either side.
const FIND_H: f32 = TOOL_SIZE + PAD;

/// How wide the find field would like to be, and the least it will accept.
///
/// It is the only part of the bar that gives way when the panel is narrow. Everything after it —
/// the count, the two arrows, the close button — is a control you cannot work the bar without,
/// where a field of half the width is still a field you can type a word into.
const FIND_W: f32 = 184.0;
const FIND_MIN: f32 = 72.0;

/// One of the three toggles inside the field.
///
/// Smaller than a tool button, because three of them have to sit inside a field that is itself the
/// height of one — and because they are *in* the field rather than beside it: a 24-point box would
/// leave its fill touching the border.
const FLAG: f32 = 18.0;

/// The room kept for the count, whatever it currently says.
///
/// Fixed rather than measured, because the bar is anchored to the *right* and a count that grew
/// from `9 of 12` to `10 of 12` would shove the field along under the caret. Wide enough for the
/// longest thing it says, which is `1 of 4096+` — see [`preview::HITS`].
const COUNTER: f32 = 68.0;

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

/// One image on the canvas.
struct Frame {
    texture: egui::TextureHandle,
    /// Its own size in pixels, which need not be the group's — two files being compared can be
    /// different shapes.
    pixels: Vec2,
    /// What the caption above it says. Empty when there is only one frame and no caption.
    label: String,
    /// It is a difference mask rather than a picture, so it is drawn in the status hue over the
    /// board rather than as it is. See [`crate::preview::Diff::mask`].
    mask: bool,
}

/// One or three images, and how they are being looked at.
///
/// Three when two files are being compared: the two of them and the difference. The zoom and the
/// pan are **shared**, which is the whole point of a comparison — three views that scrolled
/// independently would be three views of nothing in particular.
struct Picture {
    frames: Vec<Frame>,
    /// The size all frames are placed against: the larger of the two, so the same pixel lands at
    /// the same offset in each view.
    pixels: Vec2,
    /// What the first file is on disk, for the bar.
    natural: [u32; 2],
    /// And the second's, when there is one and it differs.
    other: Option<[u32; 2]>,
    scaled: bool,
    vector: bool,
    /// The share of pixels that differ, for a comparison.
    differing: Option<f32>,
    /// Show both sources as well as the difference. Only meaningful with three frames.
    all: bool,
    /// `None` is *fit*, recomputed from the canvas every frame; `Some` is an absolute scale
    /// somebody chose. That distinction is the whole zoom model: resizing the pane while fitted
    /// re-fits, and resizing it while zoomed leaves the zoom alone.
    zoom: Option<f32>,
    /// The scale that fits the canvas, as of the last frame drawn.
    ///
    /// Derived rather than chosen, and stored only so the bar's buttons and field have a number to
    /// work from — they run before the canvas is measured. Until a frame has been drawn it is 1.0,
    /// which is a sane picture rather than a blank one.
    fit: f32,
    /// How far the images are dragged from where they would sit, in points.
    pan: Vec2,
    /// The zoom field's text, and which preset was last taken from its list.
    ///
    /// The text is the field's to own while it has focus — that is what makes typing `137` possible
    /// — and is rewritten from [`Picture::percent`] on every frame it does not.
    zoom_text: String,
    zoom_pick: Option<usize>,
}

struct Text {
    body: String,
    truncated: bool,
    /// Its columns mean something, so it is set in the monospace role.
    code: bool,
    /// What language it is in, kept because the diff view has to colour a *different* string.
    lang: syntax::Lang,
    /// `body`'s syntax colouring. Empty for prose, for an unknown language, and for a
    /// file over [`syntax::CAP`].
    spans: Vec<syntax::Span>,
    /// The document, for a file that has one. `Some` only for Markdown.
    ///
    /// Both this and `spans` are worked out **once, when the file arrives**, and not while
    /// drawing: parsing a README on every frame the pointer moves over the panel would be
    /// the same mistake the find bar's `done` exists to avoid.
    doc: Option<markdown::Doc>,
    /// What git says changed in it, or `None` when nothing did — see [`crate::git::Changes`].
    changes: Option<crate::git::Changes>,
    /// The diff view, when one is being shown: a body of its own, and what each of its lines is.
    view: Option<Diffed>,
    /// Which pair of toggles `view` was built for, so it is built again when they move and not on
    /// every frame. `None` means "no view", which is also what an unchanged file has.
    view_for: Option<bool>,
}

/// How many unchanged lines are kept either side of a change when regions are collapsed.
///
/// Three, which is git's own default for `--unified` and what every review tool shows: enough to see
/// what the changed line is *inside* — the function it is in, the block it closes — and few enough
/// that two changes twenty lines apart still collapse.
const CONTEXT: usize = 3;

/// The diff view of a text file: a body that is not quite the file, and the facts per line that go
/// with it.
///
/// **A separate string rather than annotations over the file**, because both halves of this feature
/// need one. A removed line is not in the file and has to be put back to be shown; a collapsed region
/// means most of the file's lines are not shown at all. Everything downstream then works unchanged —
/// the layout job, the find bar, the syntax colouring — because all any of them ever sees is *a*
/// string.
struct Diffed {
    body: String,
    /// One per line of `body`, in order, and therefore one per row of the galley that begins a line.
    lines: Vec<Line>,
    /// `body`'s colouring, which cannot be the file's: every offset has moved.
    ///
    /// A collapsed body is also a discontinuous one, so a string or a block comment spanning a
    /// hidden region is lexed from where the visible text resumes. That is a real limit of showing
    /// part of a file and not a bug to fix here.
    spans: Vec<syntax::Span>,
}

/// What one line of a [`Diffed`] body is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mark {
    /// In the file and in `HEAD`: shown plainly.
    Same,
    /// In the file and not in `HEAD`.
    Added,
    /// In `HEAD` and not in the file. Put back for the view, and numbered by where it was in `HEAD`
    /// rather than by where it sits — in the danger ink, so a gutter that suddenly counts backwards
    /// cannot be read as one of the file's own lines.
    Removed,
    /// A run of unchanged lines that were left out, and how many.
    Skipped(u32),
}

/// A line of the diff view: what it is, and its number in the file if it has one.
#[derive(Clone, Copy, Debug)]
struct Line {
    mark: Mark,
    number: Option<u32>,
}

impl Diffed {
    /// Build the view of `body` that `changes` describes.
    fn build(body: &str, changes: &crate::git::Changes, collapse: bool, lang: syntax::Lang) -> Self {
        // Split on `\n` and not `lines()`: a body ending in a newline has one more row in the galley
        // than it has lines of text, and this vector is indexed by *row*. One entry out of step and
        // every number below the mistake is wrong.
        let file: Vec<&str> = body.split('\n').collect();

        // Which of the file's lines the working tree gained, and where the lines it lost belong.
        let mut added = vec![false; file.len()];
        for hunk in &changes.hunks {
            for line in hunk.added..hunk.added.saturating_add(hunk.added_count) {
                if let Some(flag) = added.get_mut(line as usize - 1) {
                    *flag = true;
                }
            }
        }

        // Which lines are worth showing at all. Everything, unless regions are being collapsed — and
        // then a change and [`CONTEXT`] lines either side of it, counting the *gap* a removal leaves
        // as a place a change happened.
        let mut keep = vec![!collapse; file.len()];
        if collapse {
            let lines = file.len();
            // Both windows in one closure, because two closures cannot borrow `keep` at once. `to` is
            // exclusive, and both bounds are indices — one less than the line number.
            let mut window = |from: usize, to: usize| {
                for flag in keep.iter_mut().take(to.min(lines)).skip(from) {
                    *flag = true;
                }
            };
            for hunk in &changes.hunks {
                // A changed line, and [`CONTEXT`] lines either side of *it*.
                for line in hunk.added..hunk.added.saturating_add(hunk.added_count) {
                    let at = line as usize;
                    window(at.saturating_sub(CONTEXT + 1), at + CONTEXT);
                }
                // A removal leaves no line to be the change: the gap sits *between* `after` and the
                // line after it, so the window is three either side of the gap and not of a line —
                // which is one line narrower at the top than treating `after` as changed, and getting
                // that wrong shows four lines of context above a deletion and three below it.
                if !hunk.removed.is_empty() {
                    let after = hunk.after as usize;
                    window(after.saturating_sub(CONTEXT), after + CONTEXT);
                }
            }
        }

        let mut out = Self {
            body: String::with_capacity(body.len() / if collapse { 4 } else { 1 }),
            lines: Vec::new(),
            spans: Vec::new(),
        };
        let mut skipped = 0u32;
        for (at, line) in file.iter().enumerate() {
            let number = at as u32 + 1;
            // The lines `HEAD` had here, in front of whatever replaced them — or after the line they
            // followed, for a hunk that only took lines away.
            for hunk in &changes.hunks {
                if hunk.after + 1 != number || hunk.removed.is_empty() {
                    continue;
                }
                out.flush(&mut skipped);
                for (which, gone) in hunk.removed.iter().enumerate() {
                    out.push(gone, Mark::Removed, Some(hunk.removed_at + which as u32));
                }
            }
            if !keep[at] {
                skipped += 1;
                continue;
            }
            out.flush(&mut skipped);
            let mark = if added[at] { Mark::Added } else { Mark::Same };
            out.push(line, mark, Some(number));
        }
        // A hunk that removed the end of the file has nothing after it to sit in front of.
        for hunk in &changes.hunks {
            if hunk.after as usize >= file.len() && !hunk.removed.is_empty() {
                out.flush(&mut skipped);
                for (which, gone) in hunk.removed.iter().enumerate() {
                    out.push(gone, Mark::Removed, Some(hunk.removed_at + which as u32));
                }
            }
        }
        out.flush(&mut skipped);

        out.spans = syntax::spans(&out.body, lang);
        out
    }

    fn push(&mut self, text: &str, mark: Mark, number: Option<u32>) {
        if !self.lines.is_empty() {
            self.body.push('\n');
        }
        self.body.push_str(text);
        self.lines.push(Line { mark, number });
    }

    /// Close a run of hidden lines with the one row that says how many there were.
    ///
    /// An **empty** line in the body, with the count painted over it: a row that said
    /// `12 unchanged lines` in the text itself would be text — selected by a drag, copied by
    /// `Ctrl+C`, and coloured by the lexer as whatever it happened to look like.
    fn flush(&mut self, skipped: &mut u32) {
        if *skipped > 0 {
            let count = std::mem::take(skipped);
            self.push("", Mark::Skipped(count), None);
        }
    }
}

impl Text {
    /// The string on screen, which is the document's own where one is being shown.
    ///
    /// **Everything about the find bar goes through here**, so a search is over what can
    /// be read rather than over the markup behind it: looking for `bold` in `**bold**`
    /// finds it, and looking for `**` finds nothing, because there are no asterisks on
    /// the screen. See [`crate::markdown`].
    fn shown(&self, markup: bool) -> &str {
        match (&self.doc, &self.view) {
            (Some(doc), _) if !markup => &doc.text,
            (_, Some(view)) => &view.body,
            _ => &self.body,
        }
    }

    /// Whether there is a document to show, and so a markup toggle to offer.
    fn renderable(&self) -> bool {
        self.doc.is_some()
    }

    /// Whether there is a diff to show, and so a diff toggle to offer.
    fn diffable(&self) -> bool {
        self.changes.is_some()
    }

    /// Build, keep or drop the diff view for the toggles as they now are.
    ///
    /// Called once a frame and costs a comparison unless something moved. The build is one pass over
    /// the file plus a lex of what comes out of it — the same order of work as arriving does, and for
    /// the same reason it is not done while drawing.
    fn follow(&mut self, on: bool, collapse: bool) -> bool {
        let want = (on && self.diffable()).then_some(collapse);
        if self.view_for == want {
            return false;
        }
        self.view_for = want;
        self.view = match (want, &self.changes) {
            (Some(collapse), Some(changes)) => {
                Some(Diffed::build(&self.body, changes, collapse, self.lang))
            }
            _ => None,
        };
        true
    }
}

/// The find bar over a text preview: what is being looked for, and what was found.
///
/// **It hangs off the [`Preview`] rather than off the [`Text`]**, so the query outlives the file: you
/// are usually looking for the same thing in the next one, and arrowing down a folder with the bar
/// open is the shape of "where else does this appear". What does not outlive the file is `hits`,
/// which is a set of offsets into one body — `done` is cleared whenever the content changes and the
/// search is run again on the next frame.
#[derive(Default)]
struct Find {
    open: bool,
    search: preview::Search,
    /// Byte ranges into the body on show, in order.
    hits: Vec<Range<usize>>,
    /// Which hit is current, as an index into `hits`. Meaningless while it is empty.
    at: usize,
    capped: bool,
    bad: bool,
    /// The search `hits` was computed from, or `None` for "nothing has been run against what is on
    /// the canvas now". The one thing that keeps a megabyte from being scanned on every frame the
    /// pointer moves.
    done: Option<preview::Search>,
    /// Bring the current hit into view on the next frame that draws the body.
    reveal: bool,
    /// Take the keyboard on the next frame — the one the bar opens on.
    grab: bool,
}

impl Find {
    /// Run the search again, if what it would answer has changed.
    ///
    /// Keeps your place across a change of query: the hit you were on has a byte offset, and the
    /// hit chosen from the new set is the first one at or after it. Without that, every keystroke of
    /// `foo` would send you back to the top of the file — which is the difference between typing a
    /// word and typing a word while reading.
    fn against(&mut self, body: &str) {
        if self.done.as_ref() == Some(&self.search) {
            return;
        }
        let was = self.hits.get(self.at).map_or(0, |hit| hit.start);
        let found = preview::hits(body, &self.search);
        self.at = found.at.iter().position(|hit| hit.end > was).unwrap_or(0);
        self.hits = found.at;
        self.capped = found.capped;
        self.bad = found.bad;
        self.done = Some(self.search.clone());
        self.reveal = !self.hits.is_empty();
    }

    /// Nothing has been run against what is on the canvas now.
    fn forget(&mut self) {
        self.hits.clear();
        self.done = None;
    }

    /// The hits to draw, which are none at all while the bar is shut.
    ///
    /// A bar that has been closed leaves its query in place — that is deliberate, so reopening it
    /// resumes — but the highlights go with the bar, because a file marked up by a search you can no
    /// longer see is a file with something wrong with it.
    fn showing(&self) -> &[Range<usize>] {
        if self.open {
            &self.hits
        } else {
            &[]
        }
    }

    /// The same three things, split so that a pass over many blocks can hold them at once.
    ///
    /// [`document`] draws one widget per block and each of them both reads the hits and may clear
    /// `reveal`, which is two borrows of one `Find`. Splitting it into disjoint fields here is what
    /// makes that a borrow of two things rather than a fight over one.
    fn marks(&mut self) -> Marks<'_> {
        let Self {
            hits,
            at,
            reveal,
            open,
            ..
        } = self;
        Marks {
            hits: if *open { &hits[..] } else { &[] },
            at: *at,
            reveal,
        }
    }

    /// Step to the next hit, or the previous one, wrapping at both ends.
    ///
    /// Wrapping because the arrows are how a file gets swept, and an arrow that stops working at
    /// the last hit is an arrow you have to think about.
    ///
    /// **`by == 0` does nothing at all**, which is not a triviality: the bar calls this once a frame
    /// with whatever its buttons and keys came to, and that is nought on almost all of them. Setting
    /// `reveal` anyway put a `scroll_to_rect` in every frame the panel drew — so the text could not
    /// be scrolled away from the current hit at all. It sprang back under the wheel.
    fn step(&mut self, by: isize) {
        if by == 0 || self.hits.is_empty() {
            return;
        }
        let n = self.hits.len() as isize;
        self.at = (self.at as isize + by).rem_euclid(n) as usize;
        self.reveal = true;
    }

    /// The count, in the words a find bar says it in.
    ///
    /// `4096+` rather than a number once the search has stopped counting, because a count that is
    /// really a floor has to look like one — see [`preview::HITS`]. Nothing at all until something
    /// has been typed: an empty field has not failed to find anything.
    fn counter(&self) -> String {
        if self.search.text.is_empty() {
            String::new()
        } else if self.bad {
            "Bad pattern".to_owned()
        } else if self.hits.is_empty() {
            "No results".to_owned()
        } else if self.capped {
            format!("{} of {}+", self.at + 1, self.hits.len())
        } else {
            format!("{} of {}", self.at + 1, self.hits.len())
        }
    }
}

/// The faces one block of a document is set in.
///
/// Held together because the interesting one is [`Faces::code`], which cannot be worked out from the
/// other two — it has to be *measured* against them. See [`Faces::of`].
struct Faces {
    /// The block's own face: body, a heading's size, or the monospace of a table row.
    base: egui::FontId,
    /// The semibold family, for `**bold**`.
    strong: egui::FontFamily,
    /// The monospace face for `` `inline code` ``, at the surrounding size.
    code: egui::FontId,
    /// And the line height that puts that face **on the surrounding text's baseline**.
    ///
    /// `None` where there is nothing to correct.
    code_line: Option<f32>,
}

impl Faces {
    /// Measure the three, for a block whose prose is set in `base`.
    ///
    /// # Why inline code needs a line height of its own
    ///
    /// epaint places a glyph in a row at `face_ascent + valign × (row_height − line_height)`. Two
    /// faces in one row therefore share a baseline only where their ascents *and* their line heights
    /// happen to agree, and these two do not: at 14 points the proportional face's ascent is 15.09
    /// against the monospace face's 10.41, and its line height is 18.59 against 16.41. Left alone,
    /// inline code came out **three pixels above** the prose around it — and the tinted fill behind
    /// it stayed put, which is what made it read as an underline.
    ///
    /// No value of `valign` fixes that. `TOP` aligns the two *ascents*, which differ; `BOTTOM`
    /// aligns the two line boxes' bottoms, which differ by the descents; `Center` splits the
    /// difference. The only per-section dial that moves a baseline by an arbitrary amount is
    /// `line_height`, which `BOTTOM` subtracts — so shortening the code section's line box by the
    /// gap between the two baselines lowers it by exactly that much.
    ///
    /// **The gap is read back rather than derived**, because deriving it needs the faces' ascents
    /// and epaint's public `Fonts` exposes only `row_height`. So both faces go into one probe job —
    /// two characters, a cached layout, one hash lookup — and the difference between where epaint
    /// actually put them is the correction. That it comes back already rounded to the pixel grid is
    /// what makes this land *on* the prose's baseline rather than near it: both positions are
    /// multiples of one physical pixel, so shifting by their difference commutes with the rounding.
    fn of(painter: &egui::Painter, base: egui::FontId, strong: egui::FontFamily) -> Self {
        let code = egui::FontId::new(base.size, egui::FontFamily::Monospace);
        let mut probe = egui::text::LayoutJob::default();
        for font in [base.clone(), code.clone()] {
            probe.append(
                "x",
                0.0,
                egui::TextFormat {
                    font_id: font,
                    color: Color32::PLACEHOLDER,
                    ..Default::default()
                },
            );
        }
        let probe = painter.layout_job(probe);
        let code_line = match probe.rows.first().map(|row| &row.glyphs[..]) {
            Some([prose, inline]) => Some(inline.line_height - (prose.pos.y - inline.pos.y)),
            // Either the probe laid out nothing, or the block is already monospace — a table row —
            // in which case the two are one face and the correction is nought anyway.
            _ => None,
        };
        Self {
            base,
            strong,
            code,
            code_line,
        }
    }
}

/// What the find bar contributes to one drawing pass: where the hits are, which of them is current,
/// and whether it still has to be brought into view.
struct Marks<'a> {
    hits: &'a [Range<usize>],
    at: usize,
    reveal: &'a mut bool,
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

/// The band behind a line of the diff view, or nothing for a line that is simply the file's.
///
/// **The `*_subtle` status roles**, which are the design system's translucent fills — the ones behind
/// a message bar. They compose over whatever surface they land on, which is what a band under text
/// has to do: the ink on top is the file's own colouring, syntax and search highlights included, and
/// a solid fill would have to be legible against all of it.
fn diff_fill(t: &Theme, mark: Mark) -> Option<Color32> {
    match mark {
        Mark::Same => None,
        Mark::Added => Some(t.status.success_subtle),
        Mark::Removed => Some(t.status.danger_subtle),
        // Not a change: the seam where a stretch of the file is not being shown. `layer_alt` is the
        // same grey the status line and the find bar are, so it reads as furniture rather than as a
        // third kind of change.
        Mark::Skipped(_) => Some(t.bg.layer_alt),
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

impl Picture {
    /// Fill the zoom field in from what the canvas is about to show.
    ///
    /// Called when the picture is built, so the field has a value on the frame it first appears
    /// rather than on every frame after it. `fit` is 1.0 until a frame has been drawn, so this is a
    /// plausible number that the first real frame corrects — which is a great deal better than a
    /// blank field, and the difference is visible if the window happens not to be asked for another
    /// frame straight away.
    fn labelled(mut self) -> Self {
        self.zoom_text = format!("{:.0}%", self.percent());
        self
    }

    /// The fields that are the same however many frames there are.
    fn fresh() -> Self {
        Self {
            frames: Vec::new(),
            pixels: Vec2::ZERO,
            natural: [0, 0],
            other: None,
            scaled: false,
            vector: false,
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
    fn scale(&self) -> f32 {
        self.zoom.unwrap_or(self.fit)
    }

    /// What the bar reports: the scale against the file's own pixels, not against the texture's —
    /// which are not the same thing for a picture scaled down to fit memory, or for vector art
    /// rasterised at a size of this program's choosing.
    fn percent(&self) -> f32 {
        self.scale() * self.pixels.x / self.natural[0].max(1) as f32 * 100.0
    }

    /// And the inverse, for a percentage somebody typed or picked.
    fn scale_for(&self, percent: f32) -> f32 {
        percent / 100.0 * self.natural[0].max(1) as f32 / self.pixels.x.max(1.0)
    }

    /// The frames on show: all of them, or the difference alone.
    fn showing(&self) -> &[Frame] {
        if self.frames.len() == 3 && !self.all {
            &self.frames[2..]
        } else {
            &self.frames
        }
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
            Some(doc) if !layout.markup => document(ui, t, canvas, pane, doc, find),
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

/// What is on show, what it turned out to be, and the controls.
///
/// Laid out **right to left**, because that is the order the priorities run in: the controls take
/// what they need first, then the two details in turn, and the name gets whatever is left. See the
/// module header for the rule.
#[allow(clippy::too_many_arguments)]
fn header(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    preview: &mut Preview,
    layout: &mut Layout,
    changed: bool,
    scratch: &mut String,
    out: &mut Vec<Action>,
) {
    use std::fmt::Write as _;

    // One baseline for the whole bar, taken from its principal font, so the title, the details and
    // the glyph beside them are on one line. `crate::ui::deps` says why at length.
    let baseline = ink_baseline(ui.painter(), &t.fonts.body, rect.top(), rect.height());
    let surface = t.bg.layer;
    let button = |right: f32| {
        Rect::from_center_size(
            pos2(right - TOOL_SIZE * 0.5, rect.center().y),
            vec2(TOOL_SIZE, TOOL_SIZE),
        )
    };

    // ---- The controls, from the right edge inwards ------------------------
    let close = button(rect.right() - PAD);
    if tool_button(
        ui,
        t,
        close,
        Id::new(("preview-close", pane)),
        &azur_icons::close,
        "Close the preview (Ctrl+P)",
        true,
        false,
        surface,
    )
    .clicked()
    {
        out.push(Action::ClosePreview(pane));
    }
    let mut right = close.left() - PAD;

    // The view's own toggle, immediately inside the close button: showing both sources of a
    // comparison, or numbering the lines of a text file. Never both — they belong to different
    // views — so they share the slot.
    match &mut preview.content {
        Content::Picture(picture) => {
            // **A picture git has moved on from gets the same toggle a text file does**, and it is
            // the same preference behind it: one answer to "show me what changed", whatever kind of
            // file is on screen. Offered only where there is something to compare with — and unlike
            // the text one, that cannot be read off the payload, because with the toggle off the
            // panel holds one picture and nothing that remembers there was another.
            if changed {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-diff", pane)),
                    &crate::icons::diff,
                    "Show what changed since the last commit",
                    true,
                    layout.diff,
                    surface,
                )
                .clicked()
                {
                    layout.diff = !layout.diff;
                    out.push(Action::RememberLayout);
                }
                right = at.left() - PAD;
            }
            // And inside it, the one that only means anything while there are three views to choose
            // between — exactly as the text view nests `collapse` inside `diff`.
            if picture.frames.len() == 3 {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-all", pane)),
                    &crate::icons::columns,
                    "Show both images as well as the difference",
                    true,
                    picture.all,
                    surface,
                )
                .clicked()
                {
                    picture.all = !picture.all;
                }
                right = at.left() - PAD;
            }
        }
        Content::Text(text) => {
            // Numbering a *rendered* document means nothing — its lines are not the file's
            // — so the gutter's toggle is only offered over something that has lines. That
            // is the one control in this bar that comes and goes with a preference rather
            // than with the file, and the alternative was a button that did nothing.
            if !text.renderable() || layout.markup {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-numbers", pane)),
                    &crate::icons::line_numbers,
                    "Number the lines",
                    true,
                    layout.numbers,
                    surface,
                )
                .clicked()
                {
                    layout.numbers = !layout.numbers;
                    out.push(Action::RememberLayout);
                }
                right = at.left() - PAD;
            }
            // **Only over a file git has something to say about**, which is the same argument the
            // numbers toggle makes: a control that does nothing is worse than no control. It appearing
            // is itself the news that this file has changed.
            if text.diffable() {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-diff", pane)),
                    &crate::icons::diff,
                    "Show what changed since the last commit",
                    true,
                    layout.diff,
                    surface,
                )
                .clicked()
                {
                    layout.diff = !layout.diff;
                    out.push(Action::RememberLayout);
                }
                right = at.left() - PAD;

                // And inside it, the one that only means anything while the diff is on: a file with
                // its unchanged stretches left out.
                if layout.diff {
                    let at = button(right);
                    if tool_button(
                        ui,
                        t,
                        at,
                        Id::new(("preview-collapse", pane)),
                        &crate::icons::collapse,
                        "Leave out the parts that have not changed",
                        true,
                        layout.collapse,
                        surface,
                    )
                    .clicked()
                    {
                        layout.collapse = !layout.collapse;
                        out.push(Action::RememberLayout);
                    }
                    right = at.left() - PAD;
                }
            }
            if text.renderable() {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-markup", pane)),
                    &crate::icons::markup,
                    "Show the Markdown itself",
                    true,
                    layout.markup,
                    surface,
                )
                .clicked()
                {
                    layout.markup = !layout.markup;
                    // The two views are two different strings, so every hit the find bar
                    // is holding is an offset into the wrong one. Forgotten rather than
                    // recomputed here: the search runs again on the next frame, against
                    // whichever of them is now on screen.
                    preview.find.forget();
                    out.push(Action::RememberLayout);
                }
                right = at.left() - PAD;
            }
        }
        _ => {}
    }

    // Find, immediately inside the line numbers, so the group reads `[find] [numbers] [close]`.
    // Outside the match above rather than in its `Text` arm: that one borrows the content, and this
    // one has to reach the find state beside it.
    //
    // A button and not a shortcut. `Ctrl+F` is the pane's filter box and has been since long before
    // this panel existed — a preview that took it would be taking the keyboard away from the window
    // it lives in, for a bar that only exists while one kind of file is selected.
    if matches!(preview.content, Content::Text(_)) {
        let at = button(right);
        if tool_button(
            ui,
            t,
            at,
            Id::new(("preview-find", pane)),
            &azur_icons::search,
            "Find in this file",
            true,
            preview.find.open,
            surface,
        )
        .clicked()
        {
            preview.find.open = !preview.find.open;
            // Opening it puts the caret in it: a find bar you have to click into after asking for it
            // is a find bar that wanted two clicks.
            preview.find.grab = preview.find.open;
        }
        right = at.left() - PAD;
    }

    // The zoom group: in, out, Fit, and the field — laid out right to left, so on screen it reads
    // `[field] [fit] [−] [+]`, which is the order asked for.
    //
    // Dropped whole if the bar is narrower than they are. That is the one case the priority rule
    // does not cover, and the alternative is drawing them on top of each other: a panel down the
    // side is never this narrow — `MIN_PANEL_W` is sized from `ACTIONS` for exactly this reason —
    // but a panel along the bottom is as wide as its pane, and a pane can be squeezed.
    if let Content::Picture(picture) = &mut preview.content {
        if right - rect.left() > ACTIONS {
            for (glyph, tip, step) in [
                (
                    &azur_icons::plus as azur_egui_theme::icons::Icon<'_>,
                    "Zoom in",
                    Some(ZOOM_STEP),
                ),
                (&azur_icons::minus, "Zoom out", Some(1.0 / ZOOM_STEP)),
                (&crate::icons::fit, "Fit the panel", None),
            ] {
                let at = button(right);
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-zoom", pane, tip)),
                    glyph,
                    tip,
                    true,
                    false,
                    surface,
                )
                .clicked()
                {
                    match step {
                        // Zoomed about the middle of the canvas, which is where the eye is when a
                        // button rather than the wheel was used.
                        Some(by) => {
                            picture.zoom = Some((picture.scale() * by).clamp(ZOOM_MIN, ZOOM_MAX));
                        }
                        None => {
                            picture.zoom = None;
                            picture.pan = Vec2::ZERO;
                        }
                    }
                }
                right = at.left() - PAD;
            }
            right = zoom_field(ui, rect, right, pane, picture) - PAD;
        }
    }

    // ---- The two details, in the order they give way ----------------------
    //
    // The comment first, then the size, and only once both are gone does the name start to crop.
    let mut comment = String::new();
    scratch.clear();
    let size = scratch;
    let mut mark = None;
    match &preview.content {
        Content::Picture(picture) => {
            // The dimensions, and both of them when two files are being compared and disagree —
            // because that *is* a difference.
            let mut dimensions = format!("{} × {}", picture.natural[0], picture.natural[1]);
            if let Some(other) = picture.other {
                let _ = write!(dimensions, " / {} × {}", other[0], other[1]);
            }
            match picture.differing {
                // **For a comparison the headline is the share that differs**, so it takes the
                // slot that survives and the dimensions take the one that goes first. The two
                // slots are priorities and not captions: "0.04% differs" is the answer somebody
                // opened the comparison for, and `300 × 200` is the nicety.
                Some(differing) => {
                    let _ = write!(size, "{:.2}% differs", differing * 100.0);
                    if differing > 0.0 {
                        mark = Some(t.status.danger);
                    }
                    comment.push_str(&dimensions);
                }
                None => {
                    size.push_str(&dimensions);
                    if picture.vector {
                        comment.push_str("vector, no text");
                    } else if picture.scaled {
                        comment.push_str("scaled to fit memory");
                    }
                }
            }
        }
        Content::Text(text) => {
            crate::fs::fmt::size(text.body.len() as u64, size);
            if text.truncated {
                comment.push_str("first part only");
            }
        }
        Content::Binary(view) => {
            let (files, api_sets, missing) = view.graph().tally();
            if missing > 0 {
                let _ = write!(size, "{missing} missing");
                // The count in `text-primary` with the status hue on a **glyph** beside it, rather
                // than in `status.danger` as text: measured on this surface the danger role is
                // under the floor for 12-point text and over the one for a shape, so the mark
                // carries the colour and the number stays legible. `crate::ui::deps` has the table.
                mark = Some(t.status.danger);
            } else {
                let _ = write!(
                    size,
                    "{files} file{}, {api_sets} API set{}",
                    if files == 1 { "" } else { "s" },
                    if api_sets == 1 { "" } else { "s" },
                );
            }
            if view.graph().truncated {
                comment.push_str("stopped early");
            }
        }
        _ => {}
    }

    // ---- The title, and what the details have left it --------------------
    let mut x = rect.left() + PAD;
    let (glyph, ink): (azur_egui_theme::icons::Icon<'_>, Color32) = match &preview.content {
        Content::Picture(_) => (&crate::icons::image, t.image),
        Content::Text(_) => (&crate::icons::document, t.document),
        Content::Binary(_) => (&crate::icons::executable, t.executable),
        Content::Failed(_) => (&azur_icons::error, t.status.danger),
        _ => (&crate::icons::file, t.text.secondary),
    };
    let box_rect = icon_rect(rect, x, GLYPH);
    glyph(ui.painter(), box_rect, ink);
    x = box_rect.right() + PAD;

    let name = preview
        .of
        .as_ref()
        .map(Ask::title)
        .unwrap_or_else(|| "Preview".to_owned());
    // How wide the name would like to be, which is what decides whether a detail has to go: the
    // name is never cropped while a detail could have been dropped instead.
    let wanted = ui
        .painter()
        .layout_no_wrap(name.clone(), t.fonts.body.clone(), t.text.primary)
        .size()
        .x;
    let costs = |text: &str| {
        if text.is_empty() {
            0.0
        } else {
            ui.painter()
                .layout_no_wrap(text.to_owned(), t.fonts.caption.clone(), t.text.secondary)
                .size()
                .x
                + PAD * 2.0
        }
    };
    let mark_w = if mark.is_some() { MARK + PAD } else { 0.0 };
    let (show_comment, show_size) = what_fits(
        wanted,
        costs(&comment),
        costs(size) + mark_w,
        (right - x).max(0.0),
    );

    {
        let mut detail = |ui: &Ui, text: &str, with_mark: bool| {
            if text.is_empty() {
                return;
            }
            let galley = truncated(
                ui.painter(),
                text,
                t.fonts.caption.clone(),
                // The one carrying the mark is the headline, so it is the one in the readable ink.
                if with_mark {
                    t.text.primary
                } else {
                    t.text.secondary
                },
                (right - x).max(0.0),
            );
            let w = galley.size().x;
            galley_on_baseline(ui.painter(), right - w, baseline, galley);
            right -= w + PAD;
            if with_mark {
                if let Some(ink) = mark {
                    azur_icons::error(ui.painter(), icon_rect(rect, right - MARK, MARK), ink);
                    right -= MARK + PAD;
                }
            }
        };
        if show_size {
            detail(ui, size, mark.is_some());
        }
        if show_comment {
            detail(ui, &comment, false);
        }
    }

    let galley = truncated(
        ui.painter(),
        &name,
        t.fonts.body.clone(),
        t.text.primary,
        (right - PAD - x).max(0.0),
    );
    galley_on_baseline(ui.painter(), x, baseline, galley);

    // A binary's title carries the whole answer: what the walk found, how long it took, and
    // where every name was looked for — which is the one thing a location column cannot say for
    // itself, and does not fit on a bar this narrow.
    if let Content::Binary(view) = &preview.content {
        let response = ui.interact(
            Rect::from_min_max(pos2(x, rect.top()), pos2(right, rect.bottom())),
            Id::new(("preview-title", pane)),
            Sense::hover(),
        );
        if response.hovered() {
            azur_egui_theme::components::tooltip(response, &deps::about(view.graph()));
        }
    }
}

/// Which of the two details survive: the comment, and the size.
///
/// **The rule the bar is built around**, and the only surprising thing about it: the name is never
/// cropped while a detail could have been dropped instead. So the comment goes first, then the
/// size, and the name starts losing characters only once there is nothing else left to give.
///
/// The consequence worth knowing is that a *long name* takes the details away even in a wide panel.
/// That is the right way round — the name is what identifies the file and `300 × 200` is a nicety —
/// but it does mean the details come and go as you arrow down a folder of mixed names, which is a
/// thing somebody reading this will otherwise take for a bug.
fn what_fits(name: f32, comment: f32, size: f32, room: f32) -> (bool, bool) {
    if name + comment + size <= room {
        (true, true)
    } else if name + size <= room {
        (false, true)
    } else {
        (false, false)
    }
}

/// The zoom field: a subtle, editable combo box. Returns its left edge.
///
/// **Editable and not a menu**, because a zoom you can only reach through `+` and `−` is a zoom you
/// cannot ask for — 100% from 874% is eight clicks. The presets are what people actually want and
/// the field is there for the time they want 137%.
///
/// `Variant::Subtle` gives up the fill and the border until the pointer is over it, which is the
/// right treatment on a bar this crowded: a field's border round every control is more lines than
/// there is information, and the affordance arrives when the pointer does.
fn zoom_field(ui: &mut Ui, bar: Rect, right: f32, pane: PaneId, picture: &mut Picture) -> f32 {
    use azur_egui_theme::components::{ComboBox, Size, Variant};

    /// What the list offers. `Fit` first, because it is where the panel starts and what a
    /// double click puts it back to.
    const PRESETS: [&str; 10] = [
        "Fit", "25%", "33%", "50%", "75%", "100%", "150%", "200%", "300%", "400%",
    ];

    let field = Rect::from_center_size(
        pos2(right - ZOOM_W * 0.5, bar.center().y),
        vec2(ZOOM_W, TOOL_SIZE),
    );
    let was = picture.zoom_pick;
    // A child `Ui` with an id of its own, rather than `Ui::put`: the widget takes its id from the
    // `Ui`'s auto-id counter, so two panes drawing their bars in the same frame would otherwise
    // depend on the order they were drawn in for their fields to stay apart.
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("preview-zoom-field", pane))
            .max_rect(field)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let response = child.add(
        ComboBox::new(&mut picture.zoom_text, &mut picture.zoom_pick)
            .options(PRESETS)
            // **Not a filtering combo box.** The field's text is the current zoom, so filtering the
            // list by it narrows ten presets down to the one you are already on — and a value
            // nobody offered, `137%`, narrows it to nothing and the list stops opening at all.
            // See `azur::ComboBox`, where the distinction is written down.
            .filter(false)
            // **And nothing can go in the leading column, so there is no leading column.** Ten
            // percentages cannot carry an icon, and a tick beside the current one would say what
            // the field above it already says — in a place you have to open the list to read.
            // Without this every entry is indented 24 points for something that never appears.
            .icons(false)
            .variant(Variant::Subtle)
            .size(Size::Small)
            .width(ZOOM_W),
    );

    let typing = response.has_focus();
    let (entered, escaped) = child.input(|i| {
        (
            typing && i.key_pressed(egui::Key::Enter),
            typing && i.key_pressed(egui::Key::Escape),
        )
    });
    if picture.zoom_pick != was {
        // Taken from the list.
        if let Some(picked) = picture.zoom_pick.and_then(|at| PRESETS.get(at)) {
            apply_zoom(picture, picked);
        }
        picture.zoom_pick = None;
    } else if entered || response.lost_focus() {
        let text = picture.zoom_text.clone();
        apply_zoom(picture, &text);
    } else if !typing {
        // Not being edited: the field reports what the canvas is actually showing, which the wheel
        // and the buttons and a resize all change without going through here.
        let now = format!("{:.0}%", picture.percent());
        if picture.zoom_text != now {
            picture.zoom_text = now;
        }
    }

    // **And then it lets the keyboard go.**
    //
    // A text field that keeps focus after you have finished with it takes every shortcut in the
    // window with it — `Ctrl+P`, `F5`, the arrow keys, the type-ahead — because `App::keyboard`
    // stands down whenever anything has focus, which is the right rule and the reason this matters.
    // egui hands focus back when a click lands on another *focusable* widget, and nothing else in
    // this panel is one: the canvas and the listing are bare `interact` rects.
    //
    // So: `Enter` and `Escape` are done with it, and so is a press anywhere but the field itself. A
    // press on the list is one of those — the pick has already been taken by the time this runs, so
    // letting go is right there too.
    if typing {
        let elsewhere = child.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten()
                .is_some_and(|at| !field.contains(at))
        });
        if entered || escaped || elsewhere {
            response.surrender_focus();
        }
    }
    field.left()
}

/// Read a percentage — or the word `Fit` — and go there.
///
/// Anything unparseable is ignored rather than reset to something: the field is rewritten from the
/// canvas on the next frame it does not have focus, so a typo puts the old value back by itself.
fn apply_zoom(picture: &mut Picture, text: &str) {
    let text = text.trim();
    if text.eq_ignore_ascii_case("fit") {
        picture.zoom = None;
        picture.pan = Vec2::ZERO;
        return;
    }
    let number: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if let Ok(percent) = number.parse::<f32>() {
        if percent > 0.0 {
            let scale = picture.scale_for(percent);
            picture.zoom = Some(scale.clamp(ZOOM_MIN, ZOOM_MAX));
        }
    }
}

/// What a zoom step multiplies by.
///
/// A quarter, which takes four steps to double. Photo viewers use anything from 1.1 to 2; a
/// quarter is fine enough to land near what you wanted and coarse enough that getting from fit to
/// 4:1 is not a dozen clicks.
const ZOOM_STEP: f32 = 1.25;

/// How far a picture may be zoomed, as a scale on the texture rather than a percentage of the file.
const ZOOM_MIN: f32 = 0.01;
const ZOOM_MAX: f32 = 32.0;

/// One picture, or three, on a checkerboard.
///
/// The interactions are the ones every image viewer has: **the wheel zooms about the pointer**,
/// **dragging pans**, and **a double click goes back to fit**. Zooming about the pointer rather
/// than the middle is the one that matters — it is what makes it possible to get to a corner of a
/// large image without a dozen alternating zooms and drags.
///
/// With three frames all of that is **shared**: one zoom, one pan, one gesture over the whole
/// canvas. Three views that scrolled independently would be three views of nothing in particular.
fn pictures(ui: &mut Ui, t: &Theme, canvas: Rect, pane: PaneId, picture: &mut Picture) {
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

/// How tall a row of the text view should be for the ink to sit in the middle of it, or `None` to
/// leave the face's own line box alone.
///
/// # Why a row is not a line box
///
/// **Every band in this view is drawn on the row**, and one of them is not this program's to move:
/// epaint paints a text selection as vertices in the row it belongs to, spanning `0..row.height`. So
/// does a search hit's background. The diff's bands could be offset by hand — and were — but that only
/// moved one of the three, and a selection band and a diff band two points apart is worse than either
/// being off on its own.
///
/// A line box is not symmetric about its ink and has no reason to be. Measured on the two faces this
/// view uses, at 14 points, `pixels_per_point` 1:
///
/// | | monospace | proportional |
/// | --- | --- | --- |
/// | line box | 16.00 | 19.00 |
/// | baseline (the face's ascent) | 10.00 | 16.00 |
/// | ink, from the top of the box | 1.00 … 13.00 | 6.00 … 19.00 |
/// | **the ink's own band, centred** | **14.00** | 25.00 |
///
/// The last row is what this returns: `top + bottom`, which is the height whose middle is the ink's
/// middle — because the baseline stays where the face puts it whatever the line height says, so
/// changing the height moves the *box* around the ink rather than the ink inside the box.
///
/// **And it only ever tightens.** Monospace comes down from 16 to 14, which centres it. Proportional
/// would have to go the other way — its ink already reaches the bottom of its box — and 25 points for
/// a 14-point paragraph is a line and a half of leading, so prose keeps its own line box and its bands
/// stay a little low. One rule, applied where it makes a row fit its text and declined where it would
/// make a paragraph fall apart.
fn row_box(painter: &egui::Painter, font: &egui::FontId) -> Option<f32> {
    // A capital and a descender, never drawn. egui caches the layout, so this is a hash lookup after
    // the first call.
    let probe = painter.layout_no_wrap("Ay".to_owned(), font.clone(), Color32::PLACEHOLDER);
    let row = probe.rows.first()?;
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for glyph in row.glyphs.iter().filter(|g| !g.uv_rect.is_nothing()) {
        top = top.min(glyph.pos.y + glyph.uv_rect.offset.y);
        bottom = bottom.max(glyph.pos.y + glyph.uv_rect.offset.y + glyph.uv_rect.size.y);
    }
    let height = top + bottom;
    (height.is_finite() && height < row.size.y).then_some(height)
}

/// The air either side of a line number.
///
/// Wider than [`PAD`], and the reason is that this gap is between two runs of *text* rather than
/// between a control and its edge: at four points the number and the first character of the line
/// read as one word.
const GUTTER_GAP: f32 = space::S3;

/// One level of list nesting, in a rendered document.
///
/// Wider than the marker box, so a nested list's bullet is clear of the text of the item above it —
/// which is the only thing that makes the nesting visible at all in a panel this narrow.
const NEST: f32 = 16.0;

/// The bar beside a block quote, and the air between it and the text.
const QUOTE_BAR: f32 = 2.0;
const QUOTE_GAP: f32 = space::S3;

/// How far the fill behind inline code reaches past the glyphs, in points. See `ink`.
const CHIP: f32 = 2.0;

/// The room a list marker gets before its text, before [`GUTTER_GAP`] is added.
///
/// Enough for `99.`, measured against rather than assumed: a wider marker takes the room it needs,
/// and only the ones narrower than this are padded, so every item in a list starts at one x.
const MARKER: f32 = 14.0;

/// One square of the checkerboard, in points.
const CHECKER: f32 = 8.0;

/// The two-by-two texture the checkerboard is tiled from.
///
/// Built once per theme and kept in the context's own cache: it is one 2×2 image, it never
/// changes, and a texture uploaded per frame would be a texture uploaded per frame.
///
/// `Repeat` is the whole trick — it is what lets one quad with a `uv` of many tiles stand in for
/// a grid of rectangles.
fn checkerboard(ctx: &egui::Context, t: &Theme) -> egui::TextureHandle {
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

/// Text, wrapped and scrolled, optionally numbered.
///
/// **Monospace only where the columns mean something** — see [`crate::preview::is_code`], which
/// asks a question with an answer rather than a matter of taste: does moving a character sideways
/// change what the file means? In a log, a table, a diff or any source file it does. A `.md` or a
/// `.txt` is paragraphs, and paragraphs are what the proportional face is for.
///
/// Wrapped rather than scrolled sideways, which is what was asked for and also the only thing that
/// works in a panel this narrow. Which makes the **line numbers** the interesting part: a wrapped
/// paragraph is several visual rows of one logical line, so the gutter cannot simply count rows.
/// It walks the galley and numbers the rows that *begin* a line, which is a fact only the finished
/// layout knows — and the reason the galley is laid out here by hand and then handed to a `Label`
/// rather than left to the widget.
fn text_canvas(
    ui: &mut Ui,
    t: &Theme,
    canvas: Rect,
    pane: PaneId,
    text: &Text,
    numbers: bool,
    find: &mut Find,
) {
    let font = if text.code {
        t.fonts.mono.clone()
    } else {
        t.fonts.body.clone()
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(canvas.shrink2(vec2(PAD, 0.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(canvas.intersect(ui.clip_rect()));
    let output = egui::ScrollArea::vertical()
        .id_salt(("preview-text", pane))
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            // **What is actually on screen**: the file, or the diff's own body when one is being
            // shown. Everything below reads these two rather than the file, so the diff view costs
            // this line and no branching further down.
            let body = match &text.view {
                Some(view) => &view.body,
                None => &text.body,
            };
            let spans = match &text.view {
                Some(view) => &view.spans,
                None => &text.spans,
            };

            // The gutter, wide enough for the last line's number. Measured from the count rather
            // than guessed, so a 12,000-line file does not have its numbers clipped and a 12-line
            // one does not carry a gutter for four digits it will never use. From the *file's* count
            // either way: a collapsed diff shows fewer lines but the same numbers.
            let lines = text.body.lines().count().max(1);
            let gutter = if numbers {
                ui.painter()
                    .layout_no_wrap(
                        "0".repeat(lines.to_string().len()),
                        font.clone(),
                        t.text.secondary,
                    )
                    .size()
                    .x
                    + GUTTER_GAP * 2.0
            } else {
                0.0
            };

            // One galley for the whole thing, laid out once and cached by egui on the job. The
            // body is capped at `preview::TEXT_CAP` for exactly this reason: egui lays out every
            // line whether or not it is on screen.
            let plain = egui::TextFormat {
                font_id: font.clone(),
                color: t.text.primary,
                ..Default::default()
            };
            let hits = find.showing();
            let mut job = if spans.is_empty() && hits.is_empty() {
                // The common case, and worth keeping: a plain body is one section rather than a
                // vector of them.
                egui::text::LayoutJob::single_section(body.clone(), plain)
            } else {
                overlay(
                    body,
                    &(0..body.len()),
                    &coloured(body.len(), spans, &plain, t),
                    hits,
                    find.at,
                    t,
                )
            };
            job.wrap = egui::text::TextWrapping {
                max_width: (ui.available_width() - gutter).max(16.0),
                ..Default::default()
            };
            // **Every row of this view is as tall as the ink it holds.** See [`row_box`] — this is
            // the one line that decides where a band lands on a line of text, because *every* band
            // here is drawn on the row: the diff's, a search hit's, and the one egui paints behind a
            // text selection, which is not this program's to move.
            if let Some(height) = row_box(ui.painter(), &font) {
                for section in &mut job.sections {
                    section.format.line_height = Some(height);
                }
            }
            let galley = ui.painter().layout_job(job);
            // A slot in the paint list, taken *before* the text goes in it, so the diff's bands end up
            // underneath: a band drawn after the label would be a band over the line it is about.
            // `Shape::Noop` costs nothing when there is no diff to fill it with.
            let bands = ui.painter().add(egui::Shape::Noop);
            let shown = ui
                .horizontal(|ui| {
                    ui.add_space(gutter);
                    ui.add(egui::Label::new(galley.clone()).selectable(true))
                })
                .inner;

            // The current hit, brought into view.
            //
            // **Nudged rather than centred**, which is the same rule the listing follows for the
            // keyboard cursor and for the same reason: a view that recentres on every press of the
            // next-match arrow is a view you cannot read while you step through it. A hit already on
            // screen does not move the text at all.
            if find.reveal {
                if let Some(hit) = find.hits.get(find.at) {
                    // A `CCursor` counts characters and a hit is a range of bytes, because a layout
                    // section is a range of bytes. Counted here rather than carried: it is one pass
                    // over the front of the file when the current hit changes, against a second
                    // vector the length of the hits on every keystroke.
                    let chars = text.body[..hit.start].chars().count();
                    let at = galley.pos_from_cursor(egui::text::CCursor::new(chars));
                    let mut want = at.translate(shown.rect.min.to_vec2());
                    // **Grown upwards by the height of the bar**, because the bar is floating over
                    // the top of this view: without it, "bring the hit into the viewport" is
                    // satisfied by a hit sitting exactly where the bar is, and stepping through the
                    // matches at the top of a file scrolls to each one and hides it.
                    want.min.y -= FIND_H + PAD * 2.0;
                    ui.scroll_to_rect(want, None);
                }
                find.reveal = false;
            }

            // **One walk over the galley's rows**, which is the only place the layout's own shape is
            // known: a wrapped paragraph is several rows of one logical line, so neither the numbers
            // nor the diff's bands can count rows.
            //
            // Everything here is limited to the rows on screen. A 15,000-line file is 15,000 numbers
            // and 15,000 candidate bands, and laying out or filling the ones nobody can see would
            // undo the whole reason the body is one galley.
            if numbers || text.view.is_some() {
                let origin = shown.rect.min;
                let clip = ui.clip_rect();
                let full = Rect::from_min_max(
                    pos2(canvas.left(), clip.top()),
                    pos2(canvas.right(), clip.bottom()),
                );
                // **The band is the row, and the row is the ink** — see [`row_box`], which is where
                // that is arranged. It used to be offset from the row by hand to get it onto the ink,
                // and the trouble with that was the two bands this code does *not* draw: a text
                // selection's and a search hit's are epaint's, and they are the row exactly.
                let mut painted: Vec<egui::Shape> = Vec::new();
                let mut line = 0usize;
                let mut starts = true;
                // The logical line the row being looked at belongs to, which is not the same thing as
                // the row: a wrapped line is several rows, and all of them are that line's. Carried
                // across the continuation rows so a change that wraps is banded to its last row —
                // banding only the row that starts it left the rest of an added paragraph bare.
                let mut fact: Option<Line> = None;
                for row in &galley.rows {
                    // Whether this row begins a line of the file, and what the *next* row will be.
                    // Both settled before anything is drawn, so no branch below can leave the walk out
                    // of step with the document — which is what a `continue` past the update did.
                    let first = starts;
                    starts = row.ends_with_newline;

                    let y = origin.y + row.pos.y;
                    let on_screen = y + row.row.size.y >= clip.top() && y <= clip.bottom();
                    if first {
                        // What this line *is*, which for a diff is the view's own answer and otherwise
                        // simply the next line of the file.
                        fact = match &text.view {
                            Some(view) => view.lines.get(line).copied(),
                            None => Some(Line {
                                mark: Mark::Same,
                                number: Some(line as u32 + 1),
                            }),
                        };
                        line += 1;
                    }
                    let Some(fact) = fact.filter(|_| on_screen) else {
                        continue;
                    };
                    // The band, across the whole panel rather than the text's own width: a diff line
                    // is the line, and a fill that stopped at the last glyph would leave the
                    // indentation of an indented line uncoloured — which is exactly the part that says
                    // how deep the change is.
                    let band = Rect::from_min_max(
                        pos2(full.left(), y),
                        pos2(full.right(), y + row.row.size.y),
                    );
                    if let Some(fill) = diff_fill(t, fact.mark) {
                        painted.push(egui::Shape::rect_filled(band, CornerRadius::ZERO, fill));
                    }
                    // The rest belongs to the *line* rather than to each of its rows, so a wrapped
                    // one is numbered once, at the top, the way an editor's gutter numbers it.
                    if !first {
                        continue;
                    }
                    // A collapsed region says how much it is holding back, painted over the blank line
                    // that stands for it — see [`Diffed::flush`].
                    if let Mark::Skipped(count) = fact.mark {
                        let label = ui.painter().layout_no_wrap(
                            // `·` and not `⋯`: the interpuncts are in the installed faces — the
                            // status line separates with one — and the three-dot leader is not, so
                            // it came out as a hollow box.
                            format!(
                                "· · ·   {count} unchanged line{}",
                                if count == 1 { "" } else { "s" }
                            ),
                            t.fonts.caption.clone(),
                            t.text.tertiary,
                        );
                        // On the band's own ink baseline, which is what puts a caption-sized label in
                        // the middle of a band sized for the body face. Centring its *box* in the row
                        // is what the first version did, and a smaller font's box is a different shape
                        // from the one the band was measured against.
                        galley_on_baseline(
                            ui.painter(),
                            origin.x,
                            ink_baseline(ui.painter(), &t.fonts.caption, band.top(), band.height()),
                            label,
                        );
                    }
                    if numbers {
                        if let Some(number) = fact.number {
                            let ink = if fact.mark == Mark::Removed {
                                t.status.danger
                            } else {
                                t.text.secondary
                            };
                            let galley = ui.painter().layout_no_wrap(
                                number.to_string(),
                                // The **same font as the body**, which is what guarantees the two
                                // share a baseline: a caption-sized number beside a body-sized line
                                // would sit a point above it, and a gutter that does not line up is
                                // worse than no gutter. It is placed at the row's own `y` for the
                                // same reason — the *line box*, not the shifted band, because it is
                                // text coming level with text.
                                font.clone(),
                                ink,
                            );
                            ui.painter().galley(
                                pos2(origin.x - GUTTER_GAP - galley.size().x, y),
                                galley,
                                Color32::PLACEHOLDER,
                            );
                        }
                    }
                    starts = row.ends_with_newline;
                }
                if !painted.is_empty() {
                    ui.painter().set(bands, egui::Shape::Vec(painted));
                }
            }

            if text.truncated {
                ui.add_space(space::S2);
                let mut how_much = String::new();
                crate::fs::fmt::size(preview::TEXT_CAP as u64, &mut how_much);
                ui.label(
                    egui::RichText::new(format!("— the first {how_much} of a longer file —"))
                        .font(t.fonts.caption.clone())
                        .color(t.text.secondary),
                );
            }
            ui.add_space(space::S3);
        });
    // A file longer than the panel says so at the edge, the same way the listing and the console do.
    // Measured off the `ScrollArea`, since the content here is all there is — see
    // `azur_egui_theme::components::scroll_fades` for the rule.
    //
    // Painted on `child` rather than inside the closure: in there the `Ui` is translated by the scroll
    // offset, and the fade belongs to the panel's edge and not to the document.
    azur_egui_theme::components::scroll_fades_of(
        &child.painter_at(output.inner_rect),
        &output,
        t.bg.layer,
    );
}

/// A rendered document: one block at a time, down the canvas.
///
/// **Nothing is one galley here**, which is the difference from [`text_canvas`] and the reason the
/// two are separate functions. A code block has a fill behind it and a quote has a bar beside it; a
/// list item hangs its text off a marker; a heading is a different size from the paragraph under it
/// and wants air above it. None of that is expressible as a format over one run of text, so each
/// block is laid out and drawn on its own.
///
/// What that costs is line numbers — a rendered document has no line numbers, and the bar's toggle
/// hides itself accordingly — and what it buys is that the *find bar keeps working unchanged*: the
/// hits are offsets into [`markdown::Doc::text`], each block owns a slice of it, and [`overlay`]
/// takes the slice. See [`crate::markdown`] for why the document is one string.
fn document(
    ui: &mut Ui,
    t: &Theme,
    canvas: Rect,
    pane: PaneId,
    doc: &markdown::Doc,
    find: &mut Find,
) {
    use markdown::Kind;

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(canvas.shrink2(vec2(PAD * 2.0, 0.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(canvas.intersect(ui.clip_rect()));
    let output = egui::ScrollArea::vertical()
        .id_salt(("preview-doc", pane))
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let marks = &mut find.marks();
            // The semibold family, taken from the role that already has it rather than named here:
            // a heading is `body-strong` at a heading's size, and the family behind that role is
            // the design system's to choose.
            let strong = t.fonts.body_strong.family.clone();
            let mut last: Option<&Kind> = None;
            for block in &doc.blocks {
                let air = air_before(last, &block.kind);
                if air > 0.0 {
                    ui.add_space(air);
                }
                last = Some(&block.kind);

                let base = match &block.kind {
                    Kind::Heading(level) => egui::FontId::new(heading(*level), strong.clone()),
                    Kind::Code(_) | Kind::Row { .. } => t.fonts.mono.clone(),
                    _ => t.fonts.body.clone(),
                };
                // Per block, because a heading's inline code has to come into line with a heading.
                let faces = Faces::of(ui.painter(), base, strong.clone());
                let step = QUOTE_BAR + QUOTE_GAP;
                let left = block.quote as f32 * step + block.indent as f32 * NEST;

                let row = ui
                    .horizontal_top(|ui| {
                        ui.add_space(left);
                        match &block.kind {
                            Kind::Rule => rule_across(ui, t),
                            Kind::Code(_) => code_block(ui, t, doc, block, &faces.base, marks),
                            Kind::Item(marker) => {
                                bullet(ui, t, marker.as_ref(), block.indent, &faces.base);
                                paragraph(ui, t, doc, block, &faces, marks);
                            }
                            _ => paragraph(ui, t, doc, block, &faces, marks),
                        }
                    })
                    .response
                    .rect;

                // The quote bars, one per level, in the gutter the indent left for them.
                for level in 0..block.quote {
                    let x = row.left() + level as f32 * step;
                    ui.painter().rect_filled(
                        Rect::from_min_size(
                            pos2(x.round(), row.top()),
                            vec2(QUOTE_BAR, row.height()),
                        ),
                        CornerRadius::ZERO,
                        t.stroke.strong,
                    );
                }
                // A rule under the two headings that carry one, and under a table's header row.
                // Both are the same gesture — *what follows belongs to this* — and both are the
                // hairline every other divider in this window uses.
                let ruled = matches!(
                    block.kind,
                    Kind::Heading(1 | 2) | Kind::Row { header: true }
                );
                if ruled {
                    ui.add_space(space::S1);
                    rule_across(ui, t);
                }
            }
            // Cleared here rather than in each branch: a hit that is somehow in no block at all must
            // not leave the next frame trying to scroll to it again.
            *marks.reveal = false;
            ui.add_space(space::S3);
        });
    // As [`text_canvas`], and for the same reason: a document that carries on past the edge of the
    // panel has to look like it does.
    azur_egui_theme::components::scroll_fades_of(
        &child.painter_at(output.inner_rect),
        &output,
        t.bg.layer,
    );
}

/// One block of prose — a heading, a paragraph, a table row, or a list item's text.
fn paragraph(
    ui: &mut Ui,
    t: &Theme,
    doc: &markdown::Doc,
    block: &markdown::Block,
    faces: &Faces,
    marks: &mut Marks<'_>,
) {
    let quoted = block.quote > 0;
    let mut parts: Vec<(Range<usize>, egui::TextFormat)> = Vec::with_capacity(block.runs.len());
    let mut cut = block.at.start;
    for run in &block.runs {
        // A gap between runs cannot happen — `inline` covers the block end to end — but a covering
        // is what `overlay` needs and the cost of insisting on it here is one comparison.
        if run.at.start > cut {
            parts.push((cut..run.at.start, ink(t, faces, Default::default(), quoted)));
        }
        parts.push((run.at.clone(), ink(t, faces, run.style, quoted)));
        cut = run.at.end;
    }
    if cut < block.at.end {
        parts.push((cut..block.at.end, ink(t, faces, Default::default(), quoted)));
    }
    let mut job = overlay(&doc.text, &block.at, &parts, marks.hits, marks.at, t);
    job.wrap = egui::text::TextWrapping {
        max_width: ui.available_width().max(16.0),
        ..Default::default()
    };
    let galley = ui.painter().layout_job(job);
    let shown = ui.add(egui::Label::new(galley.clone()).selectable(true));
    reveal(ui, marks, &block.at, &galley, shown.rect.min, &doc.text);
}

/// A code block: the monospace face on a recessed fill, coloured by [`crate::syntax`].
fn code_block(
    ui: &mut Ui,
    t: &Theme,
    doc: &markdown::Doc,
    block: &markdown::Block,
    base: &egui::FontId,
    marks: &mut Marks<'_>,
) {
    let plain = egui::TextFormat {
        font_id: base.clone(),
        color: t.text.primary,
        ..Default::default()
    };
    // The fence's own language, already worked out — see `markdown::Doc::spans`. Sliced rather than
    // recomputed, and by the same walk `overlay` does over the hits.
    let mut parts = Vec::new();
    let mut cut = block.at.start;
    for span in &doc.spans {
        if span.at.end <= block.at.start {
            continue;
        }
        if span.at.start >= block.at.end {
            break;
        }
        if span.at.start > cut {
            parts.push((cut..span.at.start, plain.clone()));
        }
        parts.push((
            span.at.clone(),
            egui::TextFormat {
                color: t.tok(span.tok),
                ..plain.clone()
            },
        ));
        cut = span.at.end;
    }
    if cut < block.at.end {
        parts.push((cut..block.at.end, plain.clone()));
    }

    egui::Frame::new()
        .fill(t.syntax.fill)
        .corner_radius(CornerRadius::same(radius::SMALL))
        .inner_margin(egui::Margin::symmetric(space::S3 as i8, space::S2 as i8))
        .show(ui, |ui| {
            // Full width whatever the code is: a band that stops at the longest line makes a
            // three-line block look like three separate ones.
            ui.set_min_width(ui.available_width());
            let mut job = overlay(&doc.text, &block.at, &parts, marks.hits, marks.at, t);
            job.wrap = egui::text::TextWrapping {
                max_width: ui.available_width().max(16.0),
                ..Default::default()
            };
            let galley = ui.painter().layout_job(job);
            let shown = ui.add(egui::Label::new(galley.clone()).selectable(true));
            reveal(ui, marks, &block.at, &galley, shown.rect.min, &doc.text);
        });
}

/// A list item's marker, in the hanging indent before its text.
///
/// Three bullets down the levels, which is what every document that nests a list does — a filled
/// disc, then a ring, then a square — because "one level in" has to be visible without counting the
/// indent. A number gets its own text and a full stop.
fn bullet(ui: &mut Ui, t: &Theme, marker: Option<&u64>, depth: u8, base: &egui::FontId) {
    let text = match marker {
        Some(n) => format!("{n}."),
        None => match depth % 3 {
            0 => "\u{2022}".to_owned(),
            1 => "\u{25E6}".to_owned(),
            _ => "\u{25AA}".to_owned(),
        },
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text, base.clone(), t.text.secondary);
    // Right-aligned in a fixed box, so `9.` and `10.` put their text at the same place. The width is
    // the box or the marker, whichever is wider: a hundredth item must not push into its own text.
    let width = galley.size().x.max(MARKER) + GUTTER_GAP;
    let (rect, _) = ui.allocate_exact_size(vec2(width, galley.size().y), Sense::hover());
    ui.painter().galley(
        pos2(rect.right() - GUTTER_GAP - galley.size().x, rect.top()),
        galley,
        Color32::PLACEHOLDER,
    );
}

/// A hairline across whatever room is left.
fn rule_across(ui: &mut Ui, t: &Theme) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, t.stroke.default);
}

/// The air above a block, given what came before it.
///
/// A table of pairs rather than a margin per kind, because vertical rhythm is a fact about a
/// *boundary*: two list items want nothing between them and the same item under a paragraph wants
/// air, and neither of those is a property of an item.
fn air_before(last: Option<&markdown::Kind>, next: &markdown::Kind) -> f32 {
    use markdown::Kind::*;
    let Some(last) = last else {
        // The top of the document. A little, so the first line is not welded to the bar.
        return space::S2;
    };
    match (last, next) {
        // Rows and items pack: a table with air between its rows is not a table.
        (Row { .. }, Row { .. }) => 0.0,
        (Item(_), Item(_)) => space::S1,
        // A heading is about what follows it, so it sits closer to that than to what it left.
        (Heading(_), _) => space::S2,
        (_, Heading(1 | 2)) => space::S5,
        (_, Heading(_)) => space::S4,
        (_, Rule) | (Rule, _) => space::S3,
        _ => space::S2,
    }
}

/// Bring the current hit into view, if it is in this block.
///
/// The same rule as [`text_canvas`]'s — **nudged rather than centred**, and grown upwards by the
/// height of the find bar so that a hit at the top of the document does not end up underneath it.
fn reveal(
    ui: &Ui,
    marks: &Marks<'_>,
    block: &Range<usize>,
    galley: &egui::Galley,
    origin: egui::Pos2,
    text: &str,
) {
    if !*marks.reveal {
        return;
    }
    let Some(hit) = marks.hits.get(marks.at) else {
        return;
    };
    if hit.start < block.start || hit.start >= block.end {
        return;
    }
    // A `CCursor` counts characters, and it counts them from the start of *this galley* — which is
    // why the block's own slice is what is measured and not the document up to here.
    let chars = text[block.start..hit.start].chars().count();
    let mut want = galley
        .pos_from_cursor(egui::text::CCursor::new(chars))
        .translate(origin.to_vec2());
    want.min.y -= FIND_H + PAD * 2.0;
    ui.scroll_to_rect(want, None);
}

/// How a run of a document is set: the face, the colour, and the three decorations.
fn ink(t: &Theme, faces: &Faces, style: markdown::Style, quoted: bool) -> egui::TextFormat {
    let font = if style.code {
        // The monospace face at the surrounding size, not at the body size: inline code inside a
        // heading has to be the heading's size or the line grows a step where the code is.
        faces.code.clone()
    } else if style.bold {
        egui::FontId::new(faces.base.size, faces.strong.clone())
    } else {
        faces.base.clone()
    };
    // A link is the one run whose colour is about what it *does*; a quote's is about where it is.
    let color = if style.link {
        t.text.link
    } else if quoted {
        t.text.secondary
    } else {
        t.text.primary
    };
    egui::TextFormat {
        font_id: font,
        color,
        italics: style.italic,
        // The one thing here that is not a colour or a decoration, and [`Faces::of`] is the note:
        // it is what keeps inline code on the same baseline as the words either side of it.
        line_height: style.code.then_some(faces.code_line).flatten(),
        background: if style.code {
            t.syntax.fill
        } else {
            Color32::TRANSPARENT
        },
        // **Two points rather than epaint's one**, and the reason is the line height above: the
        // fill behind a section is its *logical* box, so shortening that box to fix the baseline
        // took the same amount off the bottom of it and left a `g`'s tail hanging below the chip.
        // Two points puts most of it back, and what it buys either side is a chip with a little air
        // in it rather than a rectangle clamped to the glyphs — which is what an inline code span
        // looks like everywhere else it appears.
        expand_bg: if style.code { CHIP } else { 1.0 },
        underline: if style.link {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        strikethrough: if style.strike {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    }
}

/// A heading's size, by level.
///
/// The design system's ramp as far as it goes — `title`, `title-small`, and `body-strong` for
/// anything past the third level, on the grounds that a `#####` in a README is a label rather than a
/// heading. The one number that is not a role is the third: there is no 16pt semibold in the ramp
/// (`subtitle` is 16 regular), and a document with three heading levels needs three sizes.
fn heading(level: u8) -> f32 {
    match level {
        1 => 20.0,
        2 => 17.5,
        3 => 16.0,
        _ => 14.0,
    }
}

/// A slice of `text` as a layout job: the formats it is set in, with the find's hits laid over the
/// top of them.
///
/// **Two layers, and the order is the point.** The lower one is what the text *is* — the body face,
/// or a syntax colour, or a markdown run's emphasis — and it covers `span` end to end. The upper one
/// is what is being *looked for*, and it wins wherever the two meet, because a highlight that a
/// keyword's colour showed through would be a highlight you could not see.
///
/// Two roles doing exactly what they are described as doing: `accent.subtle` is "an accent-tinted
/// fill quiet enough to put text on", which is every other hit, and `accent.default` with
/// `text.on_accent` is the one you are standing on. `every_ink_in_the_find_bar_can_be_read` measures
/// both, in both themes — a highlight you cannot read the text through is worse than no highlight,
/// because it hides the thing it is pointing at.
///
/// The sections have to be **contiguous and in order**, which epaint asserts rather than tolerates,
/// so the empty ones are dropped instead of pushed: `base` can hand this a zero-width part, and a
/// hit can start exactly where the one before it ended — `\b` and a pattern together do that, where
/// a literal search cannot.
///
/// `span` is where in `text` this job starts, which is how one document's blocks each get their own
/// galley while the hits stay offsets into the whole of it.
fn overlay(
    text: &str,
    span: &Range<usize>,
    base: &[(Range<usize>, egui::TextFormat)],
    hits: &[Range<usize>],
    current: usize,
    t: &Theme,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        text: text[span.clone()].to_owned(),
        ..Default::default()
    };
    let mut push = |range: Range<usize>, format: egui::TextFormat| {
        if range.start >= range.end {
            return;
        }
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: egui::text::ByteIndex(range.start - span.start)
                ..egui::text::ByteIndex(range.end - span.start),
            format,
        });
    };
    // A cursor into the hits that only ever moves forward. Both lists are sorted, so the two are
    // walked together rather than the hits being scanned once per part — which over a coloured
    // megabyte would be thirty thousand parts against four thousand hits.
    let mut next = 0;
    for (part, format) in base {
        while next < hits.len() && hits[next].end <= part.start {
            next += 1;
        }
        let mut cut = part.start;
        for (i, hit) in hits.iter().enumerate().skip(next) {
            if hit.start >= part.end {
                break;
            }
            let (from, to) = (hit.start.max(part.start), hit.end.min(part.end));
            push(cut..from, format.clone());
            let (fill, ink) = if i == current {
                (t.accent.default, t.text.on_accent)
            } else {
                (t.accent.subtle, t.text.primary)
            };
            push(
                from..to,
                egui::TextFormat {
                    background: fill,
                    color: ink,
                    ..format.clone()
                },
            );
            cut = to;
        }
        push(cut..part.end, format.clone());
    }
    job
}

/// The body's own formats: `plain` everywhere, and a syntax colour where there is a span.
///
/// The gaps are filled rather than left out, because [`overlay`] needs a covering: a section that is
/// not there is not text set in the default face, it is text egui will refuse to lay out.
fn coloured(
    len: usize,
    spans: &[syntax::Span],
    plain: &egui::TextFormat,
    t: &Theme,
) -> Vec<(Range<usize>, egui::TextFormat)> {
    let mut parts = Vec::with_capacity(spans.len() * 2 + 1);
    let mut cut = 0;
    for span in spans {
        if span.at.start > cut {
            parts.push((cut..span.at.start, plain.clone()));
        }
        parts.push((
            span.at.clone(),
            egui::TextFormat {
                color: t.tok(span.tok),
                ..plain.clone()
            },
        ));
        cut = span.at.end;
    }
    if cut < len {
        parts.push((cut..len, plain.clone()));
    }
    parts
}

/// The find bar, floating over the top right of the text — and inside the scroll bar rather than on
/// top of it, which is the one place it differs from where an editor puts one. See `gutter` below.
///
/// The order is the one every find bar uses, and it is worth reading as a sentence: what to look for,
/// how to look for it, how many there are, and the two ways to move through them. The three *hows*
/// are inside the field because they belong to the query rather than to the results.
fn find_bar(ui: &mut Ui, t: &Theme, canvas: Rect, pane: PaneId, find: &mut Find) {
    // Everything but the field is fixed, so the field is what the arithmetic solves for. A panel
    // narrow enough to squeeze it past `FIND_MIN` gets a bar wider than the canvas, clipped at the
    // left — which loses the start of what you typed and keeps every control. The other way round
    // loses the ability to close it.
    // **Clear of the scroll bar, not over it.** The text under the bar still scrolls, and a close
    // button sitting on the thumb is a thumb you have to scroll the panel to reach. Read from the
    // installed style rather than named here: the width is the design system's
    // (`StyleOptions::scroll_bar_width`) and this has to be whatever that is, plus the margin egui
    // keeps inside the scroll area for it. Twenty-two points as the two are set now.
    let gutter = {
        let scroll = ui.style().spacing.scroll;
        scroll.bar_width + scroll.bar_inner_margin + scroll.bar_outer_margin
    };
    let fixed = COUNTER + TOOL_SIZE * 3.0 + PAD * 3.0;
    let field_w = (canvas.width() - PAD * 2.0 - gutter - fixed).clamp(FIND_MIN, FIND_W);
    let size = vec2(fixed + field_w, FIND_H);
    let bar = Rect::from_min_size(
        pos2(
            (canvas.right() - PAD - gutter - size.x).round(),
            (canvas.top() + PAD).round(),
        ),
        size,
    );

    // **Painted into the panel's own layer, after the text, rather than into an `Area` of its own.**
    // An `Area` was the obvious way to float something and it does not work here: with the bar on
    // `Order::Foreground` — above the pane by every rule egui has — the body text still came out over
    // the top of it, faintly, at about a tenth of its own opacity. Measured off the framebuffer with
    // the bar's fill temporarily set to red, so it is not a trick of the eye: the glyphs are there,
    // lighter than the fill, in both themes. Whatever egui is doing with the shapes of a selectable
    // label inside a `ScrollArea`, it survives being put under a higher layer. Painting into the same
    // painter after the text cannot lose that race, because within one layer the order is the order
    // things were added.
    //
    // What the `Area` was buying was input: the body is a selectable label, so a press on the bar
    // that reached the text would start a selection under it. `claim` buys the same thing — a widget
    // over the whole bar, added after the label, and egui gives the pointer to the last widget added
    // over a point.
    let claim = ui.interact(
        bar,
        Id::new(("preview-find-bar", pane)),
        Sense::click_and_drag(),
    );
    if claim.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    {
        {
            // **The edge is what says this is a separate thing, not the fill.** `bg.layer_alt` is the
            // surface every popover in this program floats on and it stays that here — but a popover
            // appears over whatever happens to be underneath, while this one appears over the panel
            // that `layer_alt` is a step from: measured against `bg.layer` it is **2.2 ΔL\* in the
            // dark theme** and 8.5 in the light one, which is invisible in one theme and fine in the
            // other, and that is two rules rather than one. So the fill's job here is only to be
            // opaque, and `stroke.default` draws the outline. `the_find_bar_has_an_edge_you_can_see`
            // measures it.
            let corner = CornerRadius::same(radius::MEDIUM);
            ui.painter()
                .add(azur_egui_theme::tokens::shadow::S16.as_shape(bar, corner));
            ui.painter().rect(
                bar,
                corner,
                t.bg.layer_alt,
                Stroke::new(1.0, t.stroke.default),
                egui::StrokeKind::Inside,
            );

            let inner = bar.shrink(PAD * 0.5);
            let field = Rect::from_min_size(inner.min, vec2(field_w, inner.height()));

            // ---- The field, and the three toggles inside it -------------------
            //
            // Painted after the input, into a slot reserved before it: the border depends on focus,
            // and focus is only known once the input has run. `field_frame` is the design system's,
            // because a composite field still wears the same fill, radius and four-state border as
            // every other field in the window — including the danger border, which is how a pattern
            // that will not compile says so.
            let slot = ui.painter().add(egui::Shape::Noop);
            let glyph = icon_rect(field, field.left() + PAD * 0.5, 12.0);
            azur_icons::search(ui.painter(), glyph, t.text.tertiary);

            let mut x = field.right() - PAD * 0.5;
            for (label, tip, under, on) in [
                (
                    ".*",
                    "Use a regular expression",
                    false,
                    &mut find.search.regex,
                ),
                ("ab", "Whole word only", true, &mut find.search.word),
                ("Aa", "Match case", false, &mut find.search.case),
            ] {
                let at = Rect::from_center_size(
                    pos2(x - FLAG * 0.5, field.center().y),
                    vec2(FLAG, FLAG),
                );
                if flag_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-flag", pane, label)),
                    label,
                    tip,
                    under,
                    *on,
                )
                .clicked()
                {
                    *on = !*on;
                }
                x = at.left();
            }

            let typing = Rect::from_min_max(
                pos2(glyph.right() + PAD * 0.5, field.top()),
                pos2(x - PAD * 0.5, field.bottom()),
            );
            // The mark is taken before the field paints, so the caret can be found again below.
            let first_shape = crate::ui::shape_mark(ui);
            let edit = ui.put(
                typing,
                egui::TextEdit::singleline(&mut find.search.text)
                    // Explicit, not from the auto-id sequence. Two reasons, and the second one bit
                    // hard elsewhere in this panel: focus is a thing other code has to be able to ask
                    // about by name, and an id derived from how many widgets came before it in the
                    // frame moves when anything upstream changes — see `ui::deps`, whose rows stopped
                    // answering clicks for exactly that reason.
                    .id(Id::new(("preview-find-field", pane)))
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .desired_width(typing.width())
                    .font(egui::FontSelection::FontId(t.fonts.body.clone()))
                    .text_color(t.text.primary),
            );
            // The same correction the filter's caret gets, for the same reason — see
            // [`crate::ui::nudge_caret`]. Applied here rather than after `field_frame` below, so the
            // range it searches holds this field's shapes and not the frame's as well.
            crate::ui::nudge_caret(
                ui,
                first_shape,
                crate::ui::CARET_SHORTER,
                crate::ui::CARET_LOWER,
            );
            field_frame(
                ui,
                slot,
                field,
                t,
                FieldLook {
                    enabled: true,
                    focused: edit.has_focus(),
                    hovered: field_hovered(ui, field, true),
                    error: find.bad,
                },
            );
            if std::mem::take(&mut find.grab) {
                edit.request_focus();
            }

            // ---- The count, and the three buttons after it --------------------
            let count = find.counter();
            let after = Rect::from_min_max(pos2(field.right() + PAD, inner.top()), inner.max);
            let galley = truncated(
                ui.painter(),
                &count,
                t.fonts.caption.clone(),
                t.text.secondary,
                COUNTER,
            );
            galley_on_baseline(
                ui.painter(),
                after.left(),
                ink_baseline(ui.painter(), &t.fonts.caption, after.top(), after.height()),
                galley,
            );

            let mut step = 0isize;
            let mut shut = false;
            let mut right = inner.right();
            let any = !find.hits.is_empty();
            for (glyph, tip, id, enabled) in [
                (
                    &azur_icons::close as azur_egui_theme::icons::Icon<'_>,
                    "Close (Esc)",
                    "shut",
                    true,
                ),
                (&azur_icons::chevron_down, "Next match (Enter)", "next", any),
                (
                    &azur_icons::chevron_up,
                    "Previous match (Shift+Enter)",
                    "prev",
                    any,
                ),
            ] {
                let at = Rect::from_center_size(
                    pos2(right - TOOL_SIZE * 0.5, inner.center().y),
                    vec2(TOOL_SIZE, TOOL_SIZE),
                );
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-find", pane, id)),
                    glyph,
                    tip,
                    enabled,
                    false,
                    t.bg.layer_alt,
                )
                .clicked()
                {
                    match id {
                        "shut" => shut = true,
                        "next" => step = 1,
                        _ => step = -1,
                    }
                }
                right = at.left() - PAD * 0.25;
            }

            // ---- The keyboard, while the caret is in the field ----------------
            //
            // `Enter` and `Shift+Enter` are the arrows, which is what makes the bar usable without
            // the pointer ever going near it. egui's single-line field gives up focus on `Enter`, so
            // it is asked back for — otherwise the second `Enter` would go to the window.
            // **`lost_focus` and not only `has_focus`.** egui's single-line field takes both of these
            // keys as "the caret is finished here" and surrenders focus during its own run — so by
            // the time there is a response to ask, the focus this would have keyed off has already
            // gone, on the very keystroke being handled. Asking only `has_focus()` is a find bar
            // whose `Enter` does nothing at all, which is how this was first written.
            if edit.has_focus() || edit.lost_focus() {
                // `Enter` and `F3` both, because both are what people press: `Enter` is the field's
                // own idea of "again", and `F3` is what every Windows program has meant by "find
                // next" for thirty years. `Shift` reverses either.
                //
                // `F3` is the pane's filter shortcut when nothing is being typed into — see
                // `crate::ui::breadcrumb`, which gives it up while another field has the keyboard.
                // This is that other field.
                let (again, back, escape) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::F3),
                        i.modifiers.shift,
                        i.key_pressed(egui::Key::Escape),
                    )
                });
                if again {
                    step = if back { -1 } else { 1 };
                    // And the caret is asked back, or the second `Enter` would go to the window.
                    edit.request_focus();
                }
                shut |= escape;
            }
            find.step(step);
            if shut {
                find.open = false;
            }
        }
    }
}

/// One of the three toggles inside the find field.
///
/// `Aa`, `ab` and `.*` are what every find bar draws there, and they are **letterforms rather than
/// glyphs** — so they are drawn as text, on the ink baseline of the box they sit in, like everything
/// else on a row in this program. `ab` carries a rule under it, which is the only thing that makes
/// two letters read as "the whole word and not the start of one".
///
/// Latched the way a tool button latches, from the same `desktop::latched` pair, because that is the
/// one rule in the window for "this control is on".
#[allow(clippy::too_many_arguments)]
fn flag_button(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    id: Id,
    label: &str,
    tip: &str,
    under: bool,
    on: bool,
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let corner = CornerRadius::same(radius::SMALL);
    let (hover, pressed) = control_fills(t, t.bg.layer_alt);
    let latched = azur_egui_theme::desktop::latched(t, response.hovered());
    let fill = if on {
        Some(latched.0)
    } else if response.is_pointer_button_down_on() {
        Some(pressed)
    } else if response.hovered() {
        Some(hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, corner, fill);
    }
    let ink = if on {
        latched.1
    } else if response.hovered() {
        t.text.primary
    } else {
        t.text.secondary
    };

    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), t.fonts.caption.clone(), ink);
    let width = galley.size().x;
    let baseline = ink_baseline(ui.painter(), &t.fonts.caption, rect.top(), rect.height());
    let left = (rect.center().x - width * 0.5).round();
    galley_on_baseline(ui.painter(), left, baseline, galley);
    if under {
        // On the baseline itself rather than under the descenders: `ab` has none, and a rule two
        // points lower would read as a border on the button.
        let y = (baseline + 1.0).round() - 0.5;
        ui.painter().line_segment(
            [pos2(left, y), pos2(left + width, y)],
            Stroke::new(1.0, ink),
        );
    }
    if !tip.is_empty() {
        azur_egui_theme::components::tooltip(response.clone(), tip);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The three hunks of the diff in [`crate::git`]'s own tests, against the file they describe.
    ///
    /// `a b c d e f g h` was committed; the working tree now has `a B c NEW1 NEW2 d e h`.
    fn changed() -> (String, crate::git::Changes) {
        let body = "a\nB\nc\nNEW1\nNEW2\nd\ne\nh\n".to_owned();
        let changes = crate::git::Changes {
            hunks: vec![
                crate::git::Hunk {
                    added: 2,
                    added_count: 1,
                    after: 1,
                    removed: vec!["b".to_owned()],
                    removed_at: 2,
                },
                crate::git::Hunk {
                    added: 4,
                    added_count: 2,
                    after: 3,
                    removed: Vec::new(),
                    removed_at: 3,
                },
                crate::git::Hunk {
                    added: 0,
                    added_count: 0,
                    after: 7,
                    removed: vec!["f".to_owned(), "g".to_owned()],
                    removed_at: 6,
                },
            ],
        };
        (body, changes)
    }

    /// **The whole of the diff view's arithmetic**: every row, what it is, and which line it is.
    ///
    /// A removed line goes back in front of whatever replaced it and keeps its number in `HEAD`; an
    /// added line keeps the file's; and the file's own lines are untouched and in order. One row out
    /// of step here is a gutter that is wrong for the rest of the file.
    #[test]
    fn a_diff_view_puts_the_removed_lines_back_where_they_were() {
        let (body, changes) = changed();
        let view = Diffed::build(&body, &changes, false, syntax::Lang::None);
        let rows: Vec<(&str, Mark, Option<u32>)> = view
            .body
            .split('\n')
            .zip(&view.lines)
            .map(|(text, line)| (text, line.mark, line.number))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("a", Mark::Same, Some(1)),
                ("b", Mark::Removed, Some(2)),
                ("B", Mark::Added, Some(2)),
                ("c", Mark::Same, Some(3)),
                ("NEW1", Mark::Added, Some(4)),
                ("NEW2", Mark::Added, Some(5)),
                ("d", Mark::Same, Some(6)),
                ("e", Mark::Same, Some(7)),
                ("f", Mark::Removed, Some(6)),
                ("g", Mark::Removed, Some(7)),
                ("h", Mark::Same, Some(8)),
                // The row a body ending in a newline has, which is why this is built by splitting on
                // `\n` rather than by `lines()`: the galley has it too, and the two have to agree.
                ("", Mark::Same, Some(9)),
            ]
        );
    }

    /// Collapsed, the same file keeps every change and [`CONTEXT`] lines around it, and says how much
    /// it left out.
    #[test]
    fn collapsing_keeps_the_changes_and_counts_what_it_hides() {
        // Twenty lines, with one changed in the middle: `line 10` was replaced.
        let mut body = String::new();
        for at in 1..=20 {
            body.push_str(&format!("line {at}\n"));
        }
        let changes = crate::git::Changes {
            hunks: vec![crate::git::Hunk {
                added: 10,
                added_count: 1,
                after: 9,
                removed: vec!["old 10".to_owned()],
                removed_at: 10,
            }],
        };
        let view = Diffed::build(&body, &changes, true, syntax::Lang::None);
        let shown: Vec<(Mark, Option<u32>)> = view
            .lines
            .iter()
            .map(|line| (line.mark, line.number))
            .collect();
        assert_eq!(
            shown,
            vec![
                (Mark::Skipped(6), None),
                (Mark::Same, Some(7)),
                (Mark::Same, Some(8)),
                (Mark::Same, Some(9)),
                (Mark::Removed, Some(10)),
                (Mark::Added, Some(10)),
                (Mark::Same, Some(11)),
                (Mark::Same, Some(12)),
                (Mark::Same, Some(13)),
                (Mark::Skipped(8), None),
            ]
        );
        // The count is what is *not* shown, and the two of them plus the rows account for the file.
        let hidden: u32 = view
            .lines
            .iter()
            .filter_map(|line| match line.mark {
                Mark::Skipped(count) => Some(count),
                _ => None,
            })
            .sum();
        let kept = view.lines.len() as u32 - 2 - 1; // the two seams, and the removed line
        assert_eq!(hidden + kept, 21, "twenty lines and the empty last row");
    }

    /// A file with nothing changed in it has no diff view at all, whatever the toggles say — which is
    /// what makes the default costless for most files.
    #[test]
    fn nothing_changed_means_nothing_to_show() {
        let mut text = Text {
            body: "a\nb\n".to_owned(),
            truncated: false,
            code: true,
            lang: syntax::Lang::None,
            spans: Vec::new(),
            doc: None,
            changes: None,
            view: None,
            view_for: None,
        };
        assert!(!text.follow(true, false), "nothing to build");
        assert!(text.view.is_none());
        assert_eq!(text.shown(false), "a\nb\n");

        // And with changes, the toggles decide — each move rebuilding once and once only.
        text.changes = Some(changed().1);
        assert!(text.follow(true, false));
        assert!(!text.follow(true, false), "already built for these");
        assert!(text.view.is_some());
        assert!(text.follow(true, true), "collapsing is a different body");
        assert!(text.follow(false, true), "and off is no body");
        assert!(text.view.is_none());
        assert_eq!(text.shown(false), "a\nb\n");
    }

    /// A probe: the metrics of the two faces the preview lays text out in.
    #[test]
    #[ignore = "a probe"]
    fn probe_font_metrics() {
        let ctx = egui::Context::default();
        azur_egui_theme::fonts::install(&ctx);
        let t = Theme::dark();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let p = ui.painter();
            for (name, font) in [
                ("mono", t.fonts.mono.clone()),
                ("body", t.fonts.body.clone()),
                ("caption", t.fonts.caption.clone()),
            ] {
                let probe = p.layout_no_wrap("Ay".to_owned(), font.clone(), Color32::WHITE);
                let rows = p.layout_no_wrap("Ay\nBg".to_owned(), font.clone(), Color32::WHITE);
                let baseline = azur_egui_theme::components::galley_baseline(&probe);
                let lift = azur_egui_theme::components::ink_lift(p, &font);
                let top = probe.rows[0]
                    .glyphs
                    .iter()
                    .filter(|g| !g.uv_rect.is_nothing())
                    .map(|g| g.pos.y + g.uv_rect.offset.y)
                    .fold(f32::INFINITY, f32::min);
                let bottom = probe.rows[0]
                    .glyphs
                    .iter()
                    .filter(|g| !g.uv_rect.is_nothing())
                    .map(|g| g.pos.y + g.uv_rect.offset.y + g.uv_rect.size.y)
                    .fold(f32::NEG_INFINITY, f32::max);
                println!(
                    "{name:8} size {:.1}  row {:.2}  pitch {:.2}  baseline(ascent) {:.2}  ink {:.2}..{:.2}  ink_ascent {:.2}  lift {:+.2}",
                    font.size,
                    probe.rows[0].size.y,
                    rows.rows[1].pos.y - rows.rows[0].pos.y,
                    baseline,
                    top,
                    bottom,
                    baseline - top,
                    lift,
                );
            }
        });
    }

    /// A pane wider than it is tall gets the panel down the side; anything squarer gets it along
    /// the bottom. Which is the whole of `Auto`, and it is the reason it exists: two panes side by
    /// side are each half as wide, and a preview taking 40% of *that* leaves no listing at all.
    #[test]
    fn auto_puts_the_panel_where_the_pane_has_room_for_it() {
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(820.0, 540.0));
        let narrow = Rect::from_min_size(pos2(0.0, 0.0), vec2(410.0, 540.0));
        assert_eq!(Where::Auto.side(wide), Side::Right, "one pane in a window");
        assert_eq!(Where::Auto.side(narrow), Side::Bottom, "two side by side");
        // A square pane goes to the bottom, because width is the scarcer thing in a listing.
        let square = Rect::from_min_size(pos2(0.0, 0.0), vec2(500.0, 500.0));
        assert_eq!(Where::Auto.side(square), Side::Bottom);
        // And the two fixed ones do not care what shape anything is.
        for pane in [wide, narrow, square] {
            assert_eq!(Where::Right.side(pane), Side::Right);
            assert_eq!(Where::Bottom.side(pane), Side::Bottom);
        }
    }

    #[test]
    fn a_position_survives_a_trip_through_the_settings_file() {
        for at in Where::ALL {
            assert_eq!(Where::parse(at.as_str()), Some(at));
            assert_eq!(Where::parse(&at.as_str().to_uppercase()), Some(at));
        }
        assert_eq!(Where::parse("sideways"), None);
        assert_eq!(Where::parse(""), None);
    }

    /// A shut panel takes nothing, an open one leaves the listing usable, and the seam between
    /// them belongs to neither.
    #[test]
    fn the_panel_leaves_the_listing_usable_on_both_sides() {
        let body = Rect::from_min_size(pos2(0.0, 32.0), vec2(900.0, 620.0));
        let shut = Layout::default();
        assert_eq!(split(body, false, shut), (body, None));

        type Span = fn(Rect) -> f32;
        for (at, span, least) in [
            (Where::Right, (|r: Rect| r.width()) as Span, MIN_PANEL_W),
            (Where::Bottom, |r: Rect| r.height(), MIN_PANEL_H),
        ] {
            let layout = Layout {
                at,
                ..Default::default()
            };
            let (list, panel) = split(body, true, layout);
            let panel = panel.expect("an open panel is somewhere");
            assert!(
                span(list) >= MIN_LIST,
                "{at:?}: the listing got {} of {}",
                span(list),
                span(body)
            );
            assert!(
                span(panel) >= least + SEAM,
                "{at:?}: the panel is too small"
            );
            // They abut, with the seam inside the panel's first point.
            match at.side(body) {
                Side::Right => assert_eq!(list.right(), panel.left()),
                Side::Bottom => assert_eq!(list.bottom(), panel.top()),
            }
            assert_eq!(
                span(list) + span(panel),
                span(body),
                "{at:?}: room went missing"
            );

            // Dragged past either stop, both sides keep their minimum.
            for share in [0.001, 0.999] {
                let (list, panel) = split(
                    body,
                    true,
                    Layout {
                        at,
                        share,
                        ..Default::default()
                    },
                );
                let panel = panel.expect("still open");
                assert!(span(list) >= MIN_LIST, "{at:?} at {share}: the listing");
                assert!(span(panel) >= least, "{at:?} at {share}: the panel");
            }

            // And a pane with no room for it does not get one, rather than getting one over the
            // top of the listing it is about.
            let cramped = match at.side(body) {
                Side::Right => Rect::from_min_size(body.min, vec2(MIN_LIST + 8.0, 400.0)),
                Side::Bottom => Rect::from_min_size(body.min, vec2(400.0, MIN_LIST + 8.0)),
            };
            assert_eq!(split(cramped, true, layout), (cramped, None), "{at:?}");
        }
    }

    /// **The comment goes first, then the size, and only then does the name crop.**
    ///
    /// The bar's one non-obvious rule, stated as a table. What it is really guarding is the third
    /// case: a name long enough to need cropping has already cost both details, so nothing is ever
    /// abbreviated while something droppable is still on the bar.
    #[test]
    fn the_details_give_way_before_the_name_does() {
        // Everything fits.
        assert_eq!(what_fits(100.0, 80.0, 60.0, 300.0), (true, true));
        // Exactly enough is enough.
        assert_eq!(what_fits(100.0, 80.0, 60.0, 240.0), (true, true));
        // One point short: the comment goes, and the name is untouched.
        assert_eq!(what_fits(100.0, 80.0, 60.0, 239.0), (false, true));
        // Shorter still: the size goes too.
        assert_eq!(what_fits(100.0, 80.0, 60.0, 159.0), (false, false));
        // And a name that cannot fit even alone still keeps the controls — it crops instead, which
        // is what the `(false, false)` answer leaves the caller to do.
        assert_eq!(what_fits(400.0, 80.0, 60.0, 120.0), (false, false));
        // A view with no details of its own: whichever way the flags fall there is nothing to
        // draw, and a name that does fit is reported as fitting.
        assert_eq!(what_fits(40.0, 0.0, 0.0, 50.0), (true, true));
    }

    /// **A panel down the side is never too narrow for its own controls.**
    ///
    /// The bar's zoom field and four buttons are a fixed cost, and a panel that cannot show them is
    /// a panel whose only way back to 100% has gone. `MIN_PANEL_W` is what guarantees it, so this
    /// is the assertion that ties the two constants together — change either and it says so.
    #[test]
    fn the_narrowest_panel_still_has_room_for_its_controls() {
        // A compile-time assertion, because both sides are constants: this is a statement about the
        // source rather than about a run, and `const` is where clippy rightly insists it goes.
        const _: () = assert!(MIN_PANEL_W >= ACTIONS + GLYPH + PAD * 3.0);
        // And there is something left over for a name, or the bar is controls and nothing else.
        const _: () = assert!(MIN_PANEL_W - ACTIONS - GLYPH - PAD * 3.0 >= 24.0);
    }

    /// The panel follows the keyboard, but only once it stops moving — and it *does* let go when
    /// the keyboard moves onto something with no preview, which is the half of [`Preview::follow`]
    /// that being inside the pane makes necessary.
    #[test]
    fn the_panel_waits_for_the_selection_to_stop_moving() {
        let mut it = Preview {
            open: true,
            ..Default::default()
        };
        let one = Ask::One(PathBuf::from(r"C:\pics\one.png"), preview::Kind::Picture);
        let two = Ask::One(PathBuf::from(r"C:\pics\two.png"), preview::Kind::Picture);

        it.follow(Some(one.clone()), 10.0);
        let (ready, left) = it.settle(10.0 + FOLLOW_DELAY * 0.6);
        assert!(ready.is_none(), "it read before the wait was up");
        assert!((left.expect("waiting") - FOLLOW_DELAY * 0.4).abs() < 1e-6);

        // Moved on before the wait was up: the clock restarts, and the first one is never read
        // at all — which is the point of the wait rather than a side effect of it.
        it.follow(Some(two.clone()), 10.0 + FOLLOW_DELAY * 0.6);
        assert!(
            it.settle(10.0 + FOLLOW_DELAY * 1.2).0.is_none(),
            "the wait did not restart"
        );
        let (ready, left) = it.settle(10.0 + FOLLOW_DELAY * 2.0);
        assert_eq!(ready.as_ref(), Some(&two));
        assert_eq!(left, None);

        // The file already on show is never asked for again, or the answer arriving would queue
        // another read of it for ever.
        it.asked(two.clone(), 1);
        it.follow(Some(two.clone()), 40.0);
        assert_eq!(it.settle(41.0), (None, None));
        assert_eq!(it.showing(), Some(two.first()));

        // Selecting a *second* picture is a different question, so it is asked afresh.
        let pair = Ask::Pair(
            PathBuf::from(r"C:\pics\one.png"),
            PathBuf::from(r"C:\pics\two.png"),
        );
        it.follow(Some(pair.clone()), 50.0);
        assert_eq!(it.settle(50.0 + FOLLOW_DELAY * 2.0).0.as_ref(), Some(&pair));

        // And the keyboard moving onto something with no preview clears it: this panel is beside
        // the row it is about, so a stale picture next to a different selection would be a lie.
        it.asked(pair, 2);
        it.follow(None, 60.0);
        assert_eq!(it.showing(), None);
        assert!(matches!(it.content, Content::Unsupported(_)));

        // The keyboard asking for the panel itself does not wait at all.
        it.ask_for(one.clone());
        assert_eq!(it.settle(70.0).0, Some(one));
    }

    /// Closing lets go of what the panel was holding, and a duplicated tab does not inherit it.
    #[test]
    fn a_shut_panel_holds_nothing() {
        let mut it = Preview {
            open: true,
            ..Default::default()
        };
        it.asked(
            Ask::One(PathBuf::from(r"C:\a.png"), preview::Kind::Picture),
            3,
        );
        assert!(it.busy());
        // A duplicate opens the same way and reads for itself: a copy of three 16 MB textures per
        // `Ctrl+T` would make duplicating a tab the most expensive thing in the window.
        let copy = it.duplicate();
        assert!(copy.open && !copy.busy() && copy.showing().is_none());

        it.close();
        assert!(!it.open && !it.busy() && it.showing().is_none());
        assert!(matches!(it.content, Content::Nothing));
    }

    /// The bar's title says what is on show, and a comparison says both names.
    #[test]
    fn a_comparison_is_titled_with_both_names() {
        let one = Ask::One(PathBuf::from(r"C:\pics\a.png"), preview::Kind::Picture);
        assert_eq!(one.title(), "a.png");
        let pair = Ask::Pair(
            PathBuf::from(r"C:\pics\a.png"),
            PathBuf::from(r"D:\other\b.png"),
        );
        assert_eq!(pair.title(), "a.png ↔ b.png");
        // The first is what anything needing one path uses — the folder for the tooltip, the
        // staleness test in `follow`.
        assert_eq!(pair.first(), Path::new(r"C:\pics\a.png"));
    }

    /// The checkerboard reads as a checkerboard: its two squares are far enough apart to see and
    /// close enough together not to compete with the picture on top of them.
    ///
    /// Both halves matter. A board whose squares are the same colour says nothing about an alpha
    /// channel, and one in black and white would be the loudest thing in the window — the point
    /// of it is to be recognisably *absence*.
    #[test]
    fn the_checkerboard_reads_as_a_checkerboard() {
        use azur_egui_theme::contrast::apart;

        for t in [Theme::dark(), Theme::light()] {
            let name = if t.dark { "dark" } else { "light" };
            let got = apart(t.bg.layer, t.bg.control_active);
            assert!(
                (8.0..=18.0).contains(&got),
                "{name}: the checkerboard's squares are {got:.1} ΔL* apart"
            );
        }
        // And the two themes have to agree about it, which is what ruled out the first pair this
        // used: a board that is plain in one theme and invisible in the other is not one rule, it
        // is two.
        let dark = Theme::dark();
        let light = Theme::light();
        let gap = apart(dark.bg.layer, dark.bg.control_active)
            - apart(light.bg.layer, light.bg.control_active);
        assert!(
            gap.abs() < 5.0,
            "the board is {gap:.1} ΔL* stronger in one theme than the other"
        );
    }

    // -----------------------------------------------------------------------
    // The find bar
    // -----------------------------------------------------------------------

    fn searching(text: &str, case: bool, word: bool, regex: bool) -> preview::Search {
        preview::Search {
            text: text.to_owned(),
            case,
            word,
            regex,
        }
    }

    /// **A highlight you cannot read the text through hides the thing it is pointing at**, which is
    /// worse than no highlight at all. Both fills, in both themes, against the ink each one is paired
    /// with — and the pairing is the point: the strong fill comes with `on_accent` and the quiet one
    /// with `primary`, and swapping either is what the measurement catches.
    #[test]
    fn every_ink_in_the_find_bar_can_be_read() {
        use azur_egui_theme::contrast::{ratio, TEXT};

        for t in [Theme::dark(), Theme::light()] {
            let name = if t.dark { "dark" } else { "light" };
            for (what, ink, fill) in [
                ("the current hit", t.text.on_accent, t.accent.default),
                ("the other hits", t.text.primary, t.accent.subtle),
            ] {
                let got = ratio(ink, fill);
                assert!(
                    got >= TEXT,
                    "{name}: {what} is {got:.2}:1, under the {TEXT}:1 floor for text"
                );
            }
        }
    }

    /// **The bar floats over the surface its own fill is a step from**, so the step is the thing to
    /// measure — and measuring it is what turned this bar from a hole in the text into a bar.
    ///
    /// `SAME` and not WCAG's `SHAPE`, deliberately. A one-point line looks like ink, but what it is
    /// doing here is dividing a surface from the surface behind it, and that is a *seam* — the case
    /// the design system's own ruler exists for, because WCAG's floors are about ink on a fill and
    /// have nothing to say about two greys meeting at an edge. Held to `SHAPE` instead, nothing in
    /// the dark neutral ramp would pass: `stroke.default` on `bg.layer` is **1.51:1**, since a ratio
    /// between two near-blacks is dominated by the 0.05 in its own denominator.
    #[test]
    fn the_find_bar_has_an_edge_you_can_see() {
        use azur_egui_theme::contrast::{apart, SAME};

        for t in [Theme::dark(), Theme::light()] {
            let name = if t.dark { "dark" } else { "light" };
            let got = apart(t.stroke.default, t.bg.layer);
            assert!(
                got >= SAME,
                "{name}: the bar's outline is {got:.1} ΔL* from the panel it floats on"
            );
        }
        // And the reason the outline is doing the work rather than the fill: the fill on its own
        // reads in one theme and not the other, which is two rules and not one. The same thing that
        // was wrong with the first checkerboard — see the test above.
        let dark = apart(Theme::dark().bg.layer, Theme::dark().bg.layer_alt);
        let light = apart(Theme::light().bg.layer, Theme::light().bg.layer_alt);
        assert!(
            dark < SAME && light > SAME,
            "the fill has stopped being the asymmetric one ({dark:.1} dark, {light:.1} light); if it \
             now reads on both sides, the outline can go back to `subtle`"
        );
    }

    /// epaint asserts that a job's sections are ordered and *contiguous* rather than tolerating a
    /// gap, so this is the shape of the one thing that would turn a highlight into a panic.
    ///
    /// Run over **two bases**, and the second is the one syntax colouring added: when the body is
    /// already cut into coloured parts, a hit can begin in one and end in the next, and the two
    /// layers have to be intersected rather than concatenated. What is asserted about the fills is
    /// therefore the *ground they cover* and not how many there are — one hit across two parts is
    /// two sections, and that is correct.
    #[test]
    fn a_highlighted_body_is_one_contiguous_run_of_sections() {
        let t = Theme::dark();
        let plain = egui::TextFormat {
            font_id: egui::FontId::monospace(12.0),
            color: t.text.primary,
            ..Default::default()
        };
        // A hit at the very start, one in the middle, and one at the very end: the three places an
        // empty section would be pushed if they were not dropped.
        let body = "foo bar foo baz foo";
        let hits = preview::hits(body, &searching("foo", false, false, false)).at;
        assert_eq!(hits, vec![0..3, 8..11, 16..19]);

        /// Touching ranges joined, so a hit split across two parts counts once.
        fn merged(ranges: &[Range<usize>]) -> Vec<Range<usize>> {
            let mut out: Vec<Range<usize>> = Vec::new();
            for range in ranges {
                match out.last_mut() {
                    Some(last) if last.end == range.start => last.end = range.end,
                    _ => out.push(range.clone()),
                }
            }
            out
        }
        let fills = |job: &egui::text::LayoutJob, want: Color32| -> Vec<Range<usize>> {
            merged(
                &job.sections
                    .iter()
                    .filter(|s| {
                        if want == Color32::TRANSPARENT {
                            s.format.background != want
                        } else {
                            s.format.background == want
                        }
                    })
                    .map(|s| s.byte_range.start.0..s.byte_range.end.0)
                    .collect::<Vec<_>>(),
            )
        };

        // The second base's parts are picked to straddle: `9..14` starts inside the middle hit and
        // ends outside it.
        let straddling = coloured(
            body.len(),
            &[
                syntax::Span {
                    at: 0..5,
                    tok: syntax::Tok::Keyword,
                },
                syntax::Span {
                    at: 9..14,
                    tok: syntax::Tok::Str,
                },
            ],
            &plain,
            &t,
        );
        for (what, base) in [
            ("a plain body", vec![(0..body.len(), plain.clone())]),
            ("a coloured one", straddling),
        ] {
            for at in 0..hits.len() {
                let job = overlay(body, &(0..body.len()), &base, &hits, at, &t);
                assert_eq!(job.text, body);
                let first = job.sections.first().expect("sections");
                assert_eq!(
                    first.byte_range.start.0, 0,
                    "{what}: does not start at the beginning"
                );
                let last = job.sections.last().expect("sections");
                assert_eq!(
                    last.byte_range.end.0,
                    body.len(),
                    "{what}: does not reach the end"
                );
                for pair in job.sections.windows(2) {
                    assert_eq!(
                        pair[0].byte_range.end.0, pair[1].byte_range.start.0,
                        "{what}: a gap between sections"
                    );
                }
                assert_eq!(
                    fills(&job, Color32::TRANSPARENT),
                    hits,
                    "{what}: the fills do not cover exactly the hits"
                );
                assert_eq!(
                    fills(&job, t.accent.default),
                    vec![hits[at].clone()],
                    "{what}: the strong fill is not exactly the current hit"
                );
            }
        }
    }

    /// A slice of a body comes out as a job whose own offsets start at nought.
    ///
    /// Which is what makes a rendered document possible: the hits are offsets into the whole of
    /// [`markdown::Doc::text`] and each block is laid out as its own galley, so every section has to
    /// be shifted back by where the block began. Getting that wrong is not a wrong colour, it is a
    /// panic inside epaint or a highlight in the wrong paragraph.
    #[test]
    fn a_block_of_a_document_is_offset_back_to_its_own_start() {
        let t = Theme::dark();
        let plain = egui::TextFormat {
            font_id: egui::FontId::monospace(12.0),
            color: t.text.primary,
            ..Default::default()
        };
        let text = "first block\nsecond has foo in it";
        let block = 12..text.len();
        let hits = preview::hits(text, &searching("foo", false, false, false)).at;
        assert_eq!(hits, vec![23..26], "the fixture moved");

        let job = overlay(
            text,
            &block,
            &[(block.clone(), plain.clone())],
            &hits,
            0,
            &t,
        );
        assert_eq!(job.text, "second has foo in it");
        assert_eq!(
            job.sections.first().expect("sections").byte_range.start.0,
            0
        );
        assert_eq!(
            job.sections.last().expect("sections").byte_range.end.0,
            job.text.len()
        );
        // And the fill lands on the word rather than eleven characters along from it.
        let filled = job
            .sections
            .iter()
            .find(|s| s.format.background != Color32::TRANSPARENT)
            .expect("a fill");
        assert_eq!(
            &job.text[filled.byte_range.start.0..filled.byte_range.end.0],
            "foo"
        );

        // A hit in a *different* block contributes nothing to this one, rather than being clamped
        // to its edge — which would put a highlight on a character nobody searched for.
        let other = overlay(text, &(0..11), &[(0..11, plain)], &hits, 0, &t);
        assert!(
            other
                .sections
                .iter()
                .all(|s| s.format.background == Color32::TRANSPARENT),
            "a hit outside the block was drawn inside it"
        );
    }

    #[test]
    fn the_counter_says_which_of_how_many() {
        let mut find = Find::default();
        assert_eq!(find.counter(), "", "an empty field has not failed to find");

        find.search = searching("foo", false, false, false);
        find.against("foo bar foo");
        assert_eq!(find.counter(), "1 of 2");
        find.step(1);
        assert_eq!(find.counter(), "2 of 2");

        find.search = searching("nowhere", false, false, false);
        find.against("foo bar foo");
        assert_eq!(find.counter(), "No results");
        // A search that found nothing has nowhere to be scrolled to either.
        assert!(!find.reveal);

        find.search = searching("(unclosed", false, false, true);
        find.against("foo");
        assert_eq!(find.counter(), "Bad pattern");

        // A count that is really a floor has to look like one.
        find.search = searching("a", false, false, false);
        find.against(&"a".repeat(preview::HITS + 10));
        assert_eq!(find.counter(), format!("1 of {}+", preview::HITS));
    }

    #[test]
    fn stepping_through_the_hits_wraps_at_both_ends() {
        let mut find = Find {
            search: searching("a", false, false, false),
            ..Find::default()
        };
        find.against("a a a");
        assert_eq!(find.at, 0);
        find.step(-1);
        assert_eq!(find.at, 2, "back from the first goes round to the last");
        find.step(1);
        assert_eq!(find.at, 0, "and on from the last comes round again");

        // **Standing still asks for nothing.** The bar calls `step` once a frame with whatever its
        // buttons came to, which is nought on almost every frame, and a `reveal` set then is a
        // `scroll_to_rect` in every frame the panel draws — text that springs back to the current hit
        // under the wheel and cannot be read around.
        find.reveal = false;
        find.step(0);
        assert!(!find.reveal, "a step of nothing asked to be scrolled to");
        find.step(1);
        assert!(find.reveal, "a step did not ask to be scrolled to");

        // And it is safe with nothing found, which is the state the arrows are disabled in — but
        // `Enter` reaches this too.
        find.search = searching("z", false, false, false);
        find.against("a a a");
        find.step(1);
        assert_eq!(find.at, 0);
    }

    /// Typing `foo` is three searches, and the first two are `f` and `fo`. Landing back at the top of
    /// the file for each of them is the difference between typing a word and typing a word while
    /// reading — so the hit chosen is the first one at or after where you already were.
    #[test]
    fn a_longer_query_keeps_your_place_in_the_file() {
        let body = "fizz\nfoo\nfizz\nfoo\nfizz\nfoo";
        let mut find = Find {
            search: searching("f", false, false, false),
            ..Find::default()
        };
        find.against(body);
        // Down to the `f` of the third `foo`, which is the eighth hit.
        for _ in 0..7 {
            find.step(1);
        }
        let was = find.hits[find.at].start;
        assert_eq!(&body[was..was + 3], "foo");

        find.search = searching("foo", false, false, false);
        find.against(body);
        assert_eq!(
            find.hits[find.at].start, was,
            "the longer query went back to the top instead of staying put"
        );

        // And a query with nothing at or after where you were comes back to the first hit rather
        // than off the end of the vector.
        find.search = searching("fizz", false, false, false);
        find.against("fizz\nnothing after");
        assert_eq!(find.at, 0);
    }

    /// The hits are offsets into one body, so a different body has to invalidate them: they end up as
    /// layout sections over the *new* text, where an offset past the end is a panic.
    #[test]
    fn a_new_file_forgets_what_was_found_in_the_last_one() {
        let mut find = Find {
            search: searching("aaaa", false, false, false),
            ..Find::default()
        };
        find.against("aaaa aaaa aaaa");
        assert_eq!(find.hits.len(), 3);

        find.forget();
        assert!(find.hits.is_empty());
        assert!(find.done.is_none(), "the next frame would not re-run it");
        // And the query itself survives, which is the half that should.
        assert_eq!(find.search.text, "aaaa");
        find.against("x");
        assert!(find.hits.is_empty());
    }
}
