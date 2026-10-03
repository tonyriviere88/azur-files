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
//! # And a fifth thing, for a file that got none of them
//!
//! A file this program has no decoder for and Windows has no visualizer for used to be the end of the
//! panel: one sentence, `No preview for a .zip`, and nothing to do about it. Under that sentence there
//! is now **a button per view** — see [`nothing_to_show`] — because the classifier works from the
//! *name* and a name is a guess: a `.dat` that is a JPEG, a `.bin` that is a PE image, a log with an
//! extension nobody has heard of. The guess being overridable is worth more than the guess being
//! better, and the choice is remembered for that one file only. [`Preview::force`] is the whole of it.
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
    control_fills, deps, icon_rect, seam, snap, text_center, tool_button, truncated, SEAM,
    TOOL_SIZE,
};

mod diff;
mod document;
mod find;
mod header;
mod picture;
mod text;
#[cfg(test)]
mod tests;
mod video;

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

/// The air between the sentence in the middle of an empty canvas and the buttons under it.
///
/// Wider than the air inside them, so the two read as a sentence *and* its controls rather than as
/// four things in a stack. See [`nothing_to_show`].
const PICK_GAP: f32 = space::S4;

/// What a chooser button keeps at each end, inside its fill.
///
/// Twice [`PAD`], which is what makes it look like a button rather than like a word with a rectangle
/// round it: the bar's icon buttons are square and get their air from being square, and this one has
/// to earn the same air from a padding.
const PICK_PAD: f32 = space::S3;

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
///
/// [`Auto`](Self::Auto) is the default, and it is the only one of the three that is right about a
/// window it has never seen: `Right` is wrong in a pane too narrow to give 40% away, `Bottom` is
/// wrong in a wide one, and which of those a first run is depends on the monitor and on whether the
/// window opened split. Somebody who wants one side always gets it from the eye's context menu, and
/// that choice is kept — see [`crate::config`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Where {
    Right,
    Bottom,
    /// Whichever suits the pane's shape. See the module header.
    #[default]
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
    ///
    /// **On by default**, the same kind of default as `diff` below and for the same kind of reason: a
    /// file in this panel is nearly always a file somebody is working on, and the number is how a line
    /// gets named — to an editor, to a compiler's output, to somebody else. Against that, what it costs
    /// is a gutter measured to the file's own last line, so four points on a twelve-line file, and one
    /// click on the bar takes it away for good.
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
    /// Play video with the sound off.
    ///
    /// A preference and not a per-file setting, for the reason `numbers` is one: somebody who does
    /// not want a folder of clips making noise does not want the next one making noise either, and a
    /// mute that had to be pressed again on every file would be a mute nobody used.
    ///
    /// **Off by default** — a video plays with its sound. Muted-by-default is the web's convention
    /// and it is a convention about *pages that start playing at you*; this one plays because the
    /// keyboard was moved onto it and held still.
    pub muted: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            at: Where::default(),
            share: SHARE,
            numbers: true,
            markup: false,
            diff: true,
            collapse: false,
            muted: false,
        }
    }
}

/// What is on the canvas.
#[derive(Default)]
enum Content {
    /// Nothing has been asked for: no selection, or one with no preview.
    #[default]
    Nothing,
    /// A selection with nothing to show, and what it was — `"zip"`, `"mp4"`.
    Unsupported(String),
    /// Asked for, on its way.
    Reading,
    Picture(Box<Picture>),
    Text(Text),
    Binary(deps::View),
    /// A video, playing.
    ///
    /// **The only variant that is a live thing rather than an answer**, and the only one whose drop
    /// matters: it holds a decoder, a clock and the audio device, and a player let go of without
    /// being shut down goes on being audible. Every path that stops showing one therefore has to
    /// come through [`Preview::forget`] — which it does, because that is where the content is
    /// replaced, and replacing it is what drops this.
    Video(preview::Player),
    /// Something went wrong, in a few words.
    Failed(String),
}

/// A previewer this panel was **told** to use, for the one file it was told about.
///
/// What makes it a struct rather than a `Kind` on its own is the path: the classifier is asked about
/// the selection every frame, so a choice with no file attached to it would silently become a choice
/// about the next file the keyboard landed on.
struct Forced {
    /// The file it is about. A choice made about a `.dat` is not a choice about the `.dat` beside it.
    path: std::path::PathBuf,
    /// What to show that file as.
    kind: preview::Kind,
    /// And what its *name* asked for, which is what pressing the button again goes back to. Kept
    /// because [`Preview::follow`] is the only place the file's own kind is ever known, and by the
    /// time the button is pressed a second time that answer has been overridden for several frames.
    own: preview::Kind,
}

/// Which tile a widget belongs to: the pane it is in, and the tile's place in the panel.
///
/// **Everything below the header is keyed on this rather than on the pane alone.** egui keys
/// interaction state by id, so four canvases sharing one id are one canvas painted four times: they
/// would pan together, scroll together, and hand every video's play button to the same player. The
/// header is the exception and still takes a bare [`PaneId`], because there is one of it and it is
/// about the focused tile — see [`Preview::focus`].
/// **`pub` for the click tests and for no other reason.** They address a tile's widgets the way the
/// panel does — by id — so they have to be able to name one. Nothing outside this module builds one
/// to *draw* with.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Spot {
    pane: PaneId,
    at: usize,
}

impl Spot {
    /// The `at`-th tile of `pane`, counting the way [`tiles`] lays them out. The panel builds its
    /// own inline, so this exists for the click tests and is compiled only for them.
    #[cfg(test)]
    pub fn tile(pane: PaneId, at: usize) -> Self {
        Self { pane, at }
    }

    /// The pane, for an [`Action`] that has to name one. A tile is not a thing the rest of the
    /// program has heard of, so nothing outside this module ever needs the other half.
    fn pane(self) -> PaneId {
        self.pane
    }
}

/// How many files the panel will show at once.
///
/// Four, because [`tiles`] runs out of shapes that are worth looking at: a fifth tile in a panel
/// that is 40% of a pane is a thumbnail, and a preview too small to read is not a preview. It is
/// also the point where the *selection* stops being a deliberate act — picking four files is
/// something somebody means, and picking forty is a select-all.
pub const MOST: usize = 4;

/// One file in the panel: what it is, what it is showing, and what is being looked for in it.
///
/// **Everything here is per-file**, which is what makes the panel able to hold up to [`MOST`] of
/// them. It was all fields of [`Preview`] when the panel showed one file, and the split is the whole
/// of that change: a second file needs its own decode, its own find bar and its own forced view, and
/// sharing any of those would mean two tiles disagreeing about which file they were about.
///
/// What is *not* here is the panel's chrome and its preferences — [`Layout`] is one per window, and
/// the header acts on whichever slot has focus. See [`Preview::focus`].
#[derive(Default)]
struct Slot {
    /// What is on show, or being read.
    of: Option<Ask>,
    /// A selection that has not sat still long enough yet: what, and since when.
    pending: Option<(Ask, f64)>,
    /// The read this is waiting for, if it is waiting for one.
    awaiting: Option<u64>,
    content: Content,
    find: Find,
    /// The view a button under `No preview for a .zip` asked for. See [`Slot::force`].
    forced: Option<Forced>,
}

/// One folder's preview panel: up to [`MOST`] files, tiled.
pub struct Preview {
    pub open: bool,
    /// The files on show, in the order [`tiles`] lays them out — which is the order they appear in
    /// the listing, so the panel reads the way the rows above it do.
    ///
    /// **Never empty.** One slot showing nothing is what a panel with no selection is, and a `Vec`
    /// that could be empty would make every accessor here answer two different kinds of "nothing".
    slots: Vec<Slot>,
    /// Which slot the header, the find bar and the keyboard act on, and which one has the sound.
    ///
    /// Always a valid index into `slots` — [`Preview::fit`] is the only thing that resizes them and
    /// it clamps this on the way out. Followed by a click on a tile, and by the selection shrinking
    /// under it.
    focus: usize,
    /// Show two selected pictures blended instead of side by side.
    ///
    /// **Off by default, because tiling is what selecting two files now means.** The blended
    /// comparison is the older answer and still the better one for two frames of the same thing, so
    /// it is what the header's diff button asks for while exactly two pictures are selected — see
    /// [`crate::app::App::selected_previews`]. Not a [`Layout`] field: it is about the files in front
    /// of you rather than a way of reading, so it dies with the selection.
    compare: bool,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            open: false,
            slots: vec![Slot::default()],
            focus: 0,
            compare: false,
        }
    }
}

impl Slot {
    /// Whether a read has been asked for, or is about to be, and has not landed yet.
    ///
    /// **A video that has not shown its first frame counts**, even though nothing was asked of
    /// [`preview::Previews`] for it: this is what `--shot --preview` waits on, and without it a
    /// capture of a video panel is a photograph of the word `Opening…`. It stops counting the moment
    /// there is a frame *or* a complaint, so a file Media Foundation cannot play does not hold a
    /// capture open until its patience runs out.
    pub fn busy(&self) -> bool {
        if let Content::Video(player) = &self.content {
            if player.opening() {
                return true;
            }
        }
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
    pub fn dependency_first_foldable(&self, rows: usize) -> Option<usize> {
        match &self.content {
            Content::Binary(view) => view.first_foldable(rows),
            _ => None,
        }
    }

    /// Whether this is the video view, and what the player has to say for itself: the complaint if
    /// it has one, and how long the file runs if it got that far. For the tests.
    #[cfg(test)]
    pub fn video(&self) -> Option<(Option<String>, Option<f64>)> {
        match &self.content {
            Content::Video(player) => Some((
                player.failed().map(str::to_owned),
                player.duration(),
            )),
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

    fn forget(&mut self) {
        self.of = None;
        self.pending = None;
        self.awaiting = None;
        self.content = Content::Nothing;
        // The hits belong to a body that is going. The *query* does not — see [`Find`].
        self.find.forget();
        // And nor does the view somebody picked for the file that is going. See [`Self::force`]: it
        // is a choice about one file, and this is the panel letting go of that file.
        self.forced = None;
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
        // A view somebody asked for beats the one the file's name implies — for that file, and for as
        // long as the keyboard stays on it. See [`Self::force`].
        let what = what.map(|ask| self.forced_over(ask));
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

    /// Substitute the view this file was told to use for the one its name implies.
    ///
    /// The override is dropped the moment the question is about anything else, which is what keeps
    /// [`Forced`] from leaking onto the next file: this is called with the classifier's own answer
    /// every frame, so "anything else" includes the keyboard moving one row down.
    fn forced_over(&mut self, ask: Ask) -> Ask {
        let Some(forced) = &self.forced else {
            return ask;
        };
        match ask {
            Ask::One(path, _) if path == forced.path => Ask::One(path, forced.kind),
            other => {
                self.forced = None;
                other
            }
        }
    }

    /// Show this file as `kind` instead, because a button under `No preview for a .zip` was pressed.
    ///
    /// **The classifier works from the name, and a name is a guess.** A `.dat` that is a JPEG, a
    /// `.bin` that is a PE image, a log called `.trace` — every one of those is a file whose contents
    /// this panel can show perfectly well and whose extension says nothing. So the guess is
    /// overridable, and [`Self::forced_over`] is where the override wins.
    ///
    /// Only for a single file: a comparison is two pictures and already has its view. Nothing is
    /// persisted, and the choice dies with the selection — the next `.dat` is a fresh question,
    /// because a folder of them is exactly as likely to be a folder of something else.
    fn force(&mut self, kind: preview::Kind) {
        let Some(Ask::One(path, showing)) = self.of.clone() else {
            return;
        };
        // What this file's name asks for. The kind on show, unless a choice has already displaced it
        // — then it is the one that choice pushed aside, which [`Forced`] kept for exactly this.
        let own = match &self.forced {
            Some(forced) if forced.path == path => forced.own,
            _ => showing,
        };
        // **Pressing the view that is already on takes it off again**, which is what every latched
        // button in this window does, and what leaves a way back: the shell's answer for a `.docx`
        // is a page of the document, and somebody who read its bytes as text has to be able to get
        // the page back without arrowing off the file and onto it again.
        let held = self
            .forced
            .as_ref()
            .is_some_and(|forced| forced.path == path && forced.kind == kind);
        let kind = if held { own } else { kind };
        self.forced = (kind != own).then(|| Forced {
            path: path.clone(),
            kind,
            own,
        });
        self.ask_now(Ask::One(path, kind));
    }

    /// The view this file was told to use, if it was told one. For the button that says so.
    ///
    /// Answered `None` for a choice about some *other* file, which the panel can be holding for the
    /// one frame between the keyboard moving and [`Self::follow`] noticing.
    fn forced_kind(&self) -> Option<preview::Kind> {
        let forced = self.forced.as_ref()?;
        (self.of.as_ref().map(Ask::first) == Some(forced.path.as_path())).then_some(forced.kind)
    }

    /// Ask for something at once, with no waiting — the keyboard asked for the panel itself.
    fn ask_now(&mut self, ask: Ask) {
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

    /// A player has been opened for it instead — see [`preview::Kind::Video`].
    ///
    /// The same bookkeeping as [`Self::asked`] minus the token, because there is no answer coming:
    /// the panel is holding the thing itself. Assigning over `content` is what shuts whatever player
    /// was there before, which is the whole of the teardown for arrowing from one clip to the next.
    pub fn plays(&mut self, ask: Ask, player: preview::Player) {
        self.of = Some(ask);
        self.awaiting = None;
        self.content = Content::Video(player);
    }

    /// And it could not be opened at all: no engine on this machine, or a path it would not take.
    ///
    /// Distinct from a [`Payload::Failed`] arriving only in where it comes from — there is no read to
    /// come back — so the panel shows the same thing either way: the sentence, in the middle, where
    /// the canvas would be.
    pub fn refused(&mut self, ask: Ask, why: String) {
        self.of = Some(ask);
        self.awaiting = None;
        self.content = Content::Failed(why);
    }

    /// The player the keyboard belongs to, if it is this panel's.
    ///
    /// `force` is for a video filling the screen, which has the keys by construction: there is nothing
    /// else on screen to have clicked, so requiring a click first would mean a fullscreen video that
    /// ignored the space bar until you had pressed it once for no reason.
    pub fn keyed_player(&mut self, force: bool) -> Option<&mut preview::Player> {
        match &mut self.content {
            Content::Video(player) if force || player.has_keys() => Some(player),
            _ => None,
        }
    }

    /// This panel's tab has stopped being the one on show.
    ///
    /// **Only a video needs telling.** Every other view is a still thing that costs nothing to be
    /// holding out of sight, and this one is a video going on playing in a tab nobody can see. It is
    /// paused rather than dropped: coming back to the tab should find the clip where it was left,
    /// which is what switching tabs means everywhere else in this window.
    pub fn out_of_sight(&mut self) {
        if let Content::Video(player) = &mut self.content {
            player.hidden();
        }
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
                    shell: picture.shell,
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
            // The shell had no visualizer for it either, so the panel says what it has always said
            // about a file with no preview. The extension comes from the file this panel is holding
            // rather than travelling with the answer: there is one right name for it and this is where
            // it is known.
            Payload::Unsupported => Content::Unsupported(
                self.of
                    .as_ref()
                    .map(|ask| preview::extension_of(ask.first()))
                    .unwrap_or_default(),
            ),
        };
    }
}

impl Preview {
    /// A duplicate for a new tab of the same folder: open the same way, showing nothing yet.
    ///
    /// The content is deliberately not copied. A decoded picture is up to 16 MB — three of them for
    /// a comparison, and now up to [`MOST`] of those — and a graph is a megabyte of names; handing a
    /// copy to every `Ctrl+T` would make duplicating a tab the most expensive thing in the window.
    /// The panel is following the selection anyway, so it fills itself in a quarter of a second.
    pub fn duplicate(&self) -> Self {
        Self {
            open: self.open,
            ..Self::default()
        }
    }

    /// The slot the header, the find bar and the keyboard are about.
    ///
    /// Infallible by construction: `slots` is never empty and [`Self::fit`] clamps `focus` every time
    /// it resizes them, which is the only way either can change.
    ///
    /// Only the tests read a slot without writing to it — the panel itself draws through
    /// [`Self::focused_mut`] or walks `slots` directly.
    #[cfg(test)]
    fn focused(&self) -> &Slot {
        &self.slots[self.focus.min(self.slots.len() - 1)]
    }

    fn focused_mut(&mut self) -> &mut Slot {
        let at = self.focus.min(self.slots.len() - 1);
        &mut self.slots[at]
    }

    /// How many files are on show. What [`tiles`] is asked for.
    pub fn count(&self) -> usize {
        self.slots.len()
    }

    /// Whether two selected pictures are to be blended rather than tiled. See [`Self::compare`].
    pub fn comparing(&self) -> bool {
        self.compare
    }

    /// The blend is on offer, or it is not — and a latch that is no longer on offer is dropped.
    ///
    /// Called every frame with [`crate::app::App::can_compare`]'s answer, which is what stops a
    /// choice about two pictures from surviving the selection it was about and blending the next two.
    pub fn allow_compare(&mut self, can: bool) {
        if !can {
            self.compare = false;
        }
    }


    /// Whether **any** slot is still waiting, which is what `--shot --preview` holds a capture open
    /// for: a photograph of four tiles is only worth taking once all four have something in them.
    pub fn busy(&self) -> bool {
        self.slots.iter().any(Slot::busy)
    }

    /// Whether one of this panel's slots is waiting for `token`.
    ///
    /// Asked before the payload is handed over rather than after, because a payload is a decoded
    /// picture or a walked graph and there is only one of it — every other panel would have to be
    /// given a copy in order to reject it.
    pub fn wants(&self, token: u64) -> bool {
        self.slots.iter().any(|slot| slot.wants(token))
    }

    /// A read has come back. Handed to the one slot that asked for it, and dropped otherwise.
    pub fn arrived(&mut self, token: u64, payload: Payload, ctx: &egui::Context) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.wants(token)) {
            slot.arrived(token, payload, ctx);
        }
    }

    /// Shut it, and let go of everything every slot was holding.
    ///
    /// Up to three decoded pictures per slot, or a dependency graph. There is nothing to be gained
    /// by keeping any of it for a panel nobody is looking at, and re-reading is a quarter of a
    /// second — which is also what somebody who has just rebuilt the file means by opening the panel
    /// again. **Back to one slot**, so reopening starts from the panel's resting shape rather than
    /// from four empty tiles.
    pub fn close(&mut self) {
        self.open = false;
        self.slots = vec![Slot::default()];
        self.focus = 0;
        self.compare = false;
    }

    /// This panel's tab has stopped being the one on show. Every slot is told, because every slot
    /// could be holding a video — see [`Slot::out_of_sight`].
    pub fn out_of_sight(&mut self) {
        for slot in &mut self.slots {
            slot.out_of_sight();
        }
    }

    /// Keep the panel's shape and its slots pointed at what is selected.
    ///
    /// One [`Ask`] per tile, in listing order. An empty list is a selection with nothing to preview,
    /// which is one slot told so rather than no slots at all — see [`Preview::slots`].
    ///
    /// **Taken by value and moved into the slots.** This runs every frame the panel is open, and each
    /// [`Ask`] holds one or two `PathBuf`s; borrowing the list meant cloning all of them on every
    /// frame to hand `follow` something it owns, and in the steady state — the selection has not
    /// changed, which is nearly always — every one of those clones was dropped again unused.
    pub fn follow_all(&mut self, asks: Vec<Ask>, now: f64) {
        self.fit(asks.len());
        if asks.is_empty() {
            self.slots[0].follow(None, now);
            return;
        }
        for (slot, ask) in self.slots.iter_mut().zip(asks) {
            slot.follow(Some(ask), now);
        }
    }

    /// Grow or shrink to `want` tiles, clamped to `1..=MOST`.
    ///
    /// **Shrinking drops the slots off the end**, and dropping a slot is what shuts a video that was
    /// playing in it — see [`Content::Video`], whose whole contract is that replacing or dropping it
    /// is the teardown. Going from four selected files to two therefore silences the two that went.
    fn fit(&mut self, want: usize) {
        let want = want.clamp(1, MOST);
        self.slots.truncate(want);
        while self.slots.len() < want {
            self.slots.push(Slot::default());
        }
        // The selection shrinking under the focus pulls it back to the last tile there is.
        self.focus = self.focus.min(self.slots.len() - 1);
    }

    /// Which slots have waited long enough, and how long until the soonest of the rest is ready.
    ///
    /// One deadline out of all of them, because one frame serves every slot that is waiting — the
    /// same argument [`crate::app::App::collect_previews`] already makes across panels.
    pub fn settle_all(&mut self, now: f64) -> (Vec<(usize, Ask)>, Option<f64>) {
        let mut ready: Vec<(usize, Ask)> = Vec::new();
        let mut soonest: Option<f64> = None;
        for (at, slot) in self.slots.iter_mut().enumerate() {
            let (ask, left) = slot.settle(now);
            if let Some(ask) = ask {
                ready.push((at, ask));
            }
            if let Some(left) = left {
                soonest = Some(soonest.map_or(left, |had: f64| had.min(left)));
            }
        }
        (ready, soonest)
    }

    /// A read has been started for what [`Self::settle_all`] handed back, for the slot that wanted
    /// it. Out-of-range is dropped rather than clamped: the selection has changed under the request,
    /// and the answer is about a file no tile is showing any more.
    pub fn asked(&mut self, at: usize, ask: Ask, token: u64) {
        if let Some(slot) = self.slots.get_mut(at) {
            slot.asked(ask, token);
        }
    }

    /// A player has been opened for it instead — see [`preview::Kind::Video`].
    pub fn plays(&mut self, at: usize, ask: Ask, player: preview::Player) {
        if let Some(slot) = self.slots.get_mut(at) {
            slot.plays(ask, player);
        }
    }

    /// And it could not be opened at all.
    pub fn refused(&mut self, at: usize, ask: Ask, why: String) {
        if let Some(slot) = self.slots.get_mut(at) {
            slot.refused(ask, why);
        }
    }

    /// Ask for one file at once, with no waiting — the keyboard asked for the panel itself.
    ///
    /// **Back to one tile.** This is the shortcut that opens the panel on the file under the cursor,
    /// so it is a statement about one file; leaving three stale tiles beside it would be answering a
    /// different question. The next frame's [`Self::follow_all`] restores the tiling if the selection
    /// really is several files.
    pub fn ask_for(&mut self, ask: Ask) {
        self.open = true;
        self.fit(1);
        self.focus = 0;
        self.slots[0].ask_now(ask);
    }

    /// Open the find bar on `text` in the focused tile, without anybody having typed it.
    ///
    /// For `--find=`, so a capture can photograph the bar doing its job — the same reason
    /// `--preview` and `--compare` exist.
    pub fn look_for(&mut self, text: &str) {
        self.focused_mut().look_for(text);
    }

    /// The player the keyboard belongs to. **The focused tile's and no other**, which is the whole
    /// of "the keys go to one video": four clips can be playing and the space bar means the one you
    /// last clicked on.
    pub fn keyed_player(&mut self, force: bool) -> Option<&mut preview::Player> {
        self.focused_mut().keyed_player(force)
    }

    /// Ask for the blend without pressing the button.
    ///
    /// For `--compare` and for the tests, and for the one reason both exist: a capture run has no
    /// pointer, so the gesture has to be reachable without one. The latch still dies with the
    /// selection — see [`Self::allow_compare`], which runs every frame either way, so this cannot
    /// blend two files it was not asked about.
    pub fn set_compare(&mut self, on: bool) {
        self.compare = on;
    }

    /// Follow one file, and settle one file. For the tests about the *debounce*, which is per slot:
    /// the wait belongs to a tile and one tile is the shape those tests are written against. The
    /// panel itself always goes through [`Self::follow_all`] and [`Self::settle_all`].
    #[cfg(test)]
    pub fn follow(&mut self, what: Option<Ask>, now: f64) {
        self.follow_all(what.into_iter().collect(), now);
    }

    #[cfg(test)]
    pub fn settle(&mut self, now: f64) -> (Option<Ask>, Option<f64>) {
        let (ready, left) = self.settle_all(now);
        (ready.into_iter().next().map(|(_, ask)| ask), left)
    }

    /// What the focused tile is looking at. For the tests; the panel reads the slots.
    #[cfg(test)]
    pub fn showing(&self) -> Option<&Path> {
        self.focused().showing()
    }

    /// Every tile's file, in tile order. For the tests that are about the tiling itself.
    #[cfg(test)]
    pub fn showing_all(&self) -> Vec<&Path> {
        self.slots.iter().filter_map(Slot::showing).collect()
    }

    /// Which tile the header acts on, and which one has the sound.
    pub fn focused_at(&self) -> usize {
        self.focus.min(self.slots.len() - 1)
    }

    /// Move the focus, as a click on a tile does. For the tests, which have no pointer.
    #[cfg(test)]
    pub fn focus_on(&mut self, at: usize) {
        self.focus = at.min(self.slots.len() - 1);
    }

    #[cfg(test)]
    pub fn dependency_rows(&self) -> Option<usize> {
        self.focused().dependency_rows()
    }

    #[cfg(test)]
    pub fn dependency_first_foldable(&self, rows: usize) -> Option<usize> {
        self.focused().dependency_first_foldable(rows)
    }

    #[cfg(test)]
    pub fn video(&self) -> Option<(Option<String>, Option<f64>)> {
        self.focused().video()
    }

    /// Every tile that is a video, and whether it has the sound. For the test that is about exactly
    /// that: all of them play and one of them is audible.
    #[cfg(test)]
    pub fn videos_muted(&self) -> Vec<bool> {
        self.slots
            .iter()
            .filter_map(|slot| match &slot.content {
                Content::Video(player) => Some(player.muted()),
                _ => None,
            })
            .collect()
    }

    /// How many times each tile's mute has reached the engine.
    ///
    /// **The value being right is not the claim** — see [`preview::Player::mute_writes`]. A mute
    /// written twice a frame settles on the correct value and crackles the whole time.
    #[cfg(test)]
    pub fn video_mute_writes(&self) -> Vec<u32> {
        self.slots
            .iter()
            .filter_map(|slot| match &slot.content {
                Content::Video(player) => Some(player.mute_writes()),
                _ => None,
            })
            .collect()
    }

    #[cfg(test)]
    pub fn frames(&self) -> Option<(usize, usize)> {
        self.focused().frames()
    }

    #[cfg(test)]
    pub fn set_regex(&mut self, on: bool) {
        self.focused_mut().set_regex(on);
    }

    #[cfg(test)]
    pub fn finding(&self) -> (bool, usize, usize, String) {
        self.focused().finding()
    }

    #[cfg(test)]
    pub fn toggle_all(&mut self) {
        self.focused_mut().toggle_all();
    }
}

/// Draw this panel's video over the whole window instead of in its panel. See [`video::theatre`].
///
/// `false` when there is nothing to fill a screen with — no video in this panel any more, or one that
/// will not play — and then the caller takes the window back. It is asked as one question rather than
/// two so that "is there a player" and "draw the player" cannot disagree: this is the only thing on
/// screen while it is true, so a frame that decided wrongly is a frame with nothing in it at all.
pub fn theatre(
    ui: &mut Ui,
    t: &Theme,
    screen: Rect,
    pane: PaneId,
    preview: &mut Preview,
    layout: &mut Layout,
    out: &mut Vec<Action>,
) -> bool {
    // The focused tile's video and no other: filling the screen is a statement about one clip, and
    // with four playing the one it is about is the one holding the keys. See [`Preview::keyed_player`].
    //
    // Its own tile's spot, so the controls over a fullscreen video are the same widgets they were in
    // the panel — the play button keeps its state across the two, which is what makes the transition
    // look like the same player growing rather than a second one appearing.
    let spot = Spot {
        pane,
        at: preview.focused_at(),
    };
    match &mut preview.focused_mut().content {
        Content::Video(player) => video::theatre(ui, t, screen, spot, player, layout, out),
        _ => false,
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

/// Cut the canvas into one tile per file on show.
///
/// | files | shape |
/// | --- | --- |
/// | 1 | the whole canvas |
/// | 2 | side by side |
/// | 3 | two across the top, the third the full width beneath them |
/// | 4 | two by two |
///
/// **The seam is taken out of the tiles rather than drawn over them**, exactly as [`split`] does
/// between the panel and the listing: a boundary neither side paints is a boundary that cannot be
/// half a pixel off, and it means a tile's content is never under the line that separates it from its
/// neighbour.
///
/// The three-file shape puts the odd one along the bottom rather than down the side because the
/// bottom of a panel is the wider edge in the common case — a panel on the right is taller than it is
/// wide, so a full-width row reads better than a full-height column. It is the shape a contact sheet
/// has for the same reason.
///
/// Anything past [`MOST`] is not laid out; the caller does not ask for more, and a count of zero
/// gives one tile because a panel always has somewhere to say "nothing selected".
fn tiles(canvas: Rect, count: usize) -> Vec<Rect> {
    // Half of the gap between two tiles, which each side gives up.
    let half = SEAM / 2.0;
    let left = |r: Rect, x: f32| Rect::from_min_max(r.min, pos2(x - half, r.bottom()));
    let right = |r: Rect, x: f32| Rect::from_min_max(pos2(x + half, r.top()), r.max);
    let top = |r: Rect, y: f32| Rect::from_min_max(r.min, pos2(r.right(), y - half));
    let bottom = |r: Rect, y: f32| Rect::from_min_max(pos2(r.left(), y + half), r.max);

    let mid_x = canvas.center().x;
    let mid_y = canvas.center().y;
    match count {
        0 | 1 => vec![canvas],
        2 => vec![left(canvas, mid_x), right(canvas, mid_x)],
        3 => {
            let upper = top(canvas, mid_y);
            vec![
                left(upper, mid_x),
                right(upper, mid_x),
                bottom(canvas, mid_y),
            ]
        }
        _ => {
            let (upper, lower) = (top(canvas, mid_y), bottom(canvas, mid_y));
            vec![
                left(upper, mid_x),
                right(upper, mid_x),
                left(lower, mid_x),
                right(lower, mid_x),
            ]
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
    // Whether the selection is two pictures, which turns the header's diff button into the compare
    // button. See [`crate::app::App::can_compare`].
    can_compare: bool,
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
    header(ui, t, bar, pane, preview, layout, changed, can_compare, scratch, out);

    if canvas.height() < 24.0 || canvas.width() < 48.0 {
        return;
    }

    // One tile per file on show, and the focus read before the slots are borrowed.
    let count = preview.count();
    let focus = preview.focused_at();
    let rects = tiles(canvas, count);
    // Which tile a view was asked for in, and which one the pointer claimed. Both are acted on after
    // the loop, because both write to the panel and the loop is holding its slots.
    let mut picked: Option<(usize, preview::Kind)> = None;
    let mut clicked: Option<usize> = None;

    for (at, slot) in preview.slots.iter_mut().enumerate() {
        let Some(&whole) = rects.get(at) else { continue };
        let spot = Spot { pane, at };
        // **The name strip, and only when there is more than one tile.** A single preview is named by
        // the header above it, and a strip repeating that name would be a second answer to a question
        // already answered. With two or more it is the only thing that says which file you are
        // looking at. See [`strip_for`].
        let tile = if count > 1 {
            strip_for(ui, t, whole, slot, at == focus)
        } else {
            whole
        };
        if tile.height() < 16.0 || tile.width() < 32.0 {
            continue;
        }
        // Salted per tile, so the pointer landing in one canvas is not also landing in the others.
        if ui
            .interact(whole, Id::new(("preview-tile", spot)), Sense::click())
            .clicked()
        {
            clicked = Some(at);
        }
        // **The focused tile is the one allowed the sound**, and it is told here rather than fixed up
        // afterwards: a second pass re-asserting it is a second owner of the player's mute, and two
        // owners writing it every frame is what made four clips crackle.
        if let Some(kind) = tile_content(ui, t, tile, spot, slot, layout, at == focus, out) {
            picked = Some((at, kind));
        }
    }

    // The pointer chose which tile the header, the find bar and the sound belong to.
    if let Some(at) = clicked {
        preview.focus = at;
    }
    // A view was asked for, in the tile it was asked in. After the loop rather than inside it,
    // because this replaces what that tile is drawing and the borrow of the slots ends here.
    if let Some((at, kind)) = picked {
        if let Some(slot) = preview.slots.get_mut(at) {
            slot.force(kind);
        }
        // The read is queued rather than done, and this frame is the one the click arrived on: a
        // request nobody books a frame for is a request that lands the next time something unrelated
        // wants one. The same reasoning as the debounce's `request_repaint_after`.
        ui.ctx().request_repaint();
    }
}

/// The thin strip along the top of a tile that says which file it holds, and returns what is left for
/// the content.
///
/// **A name and nothing else.** Every control belongs to the header, which acts on the focused tile —
/// four copies of the view buttons would not fit across a 2×2 anyway, and a tile is chosen by
/// clicking it rather than by operating it. The focused one is named in the primary ink and underlined
/// along the bottom of its strip; the others are secondary. That is the same one-step ink ladder the
/// sidebar uses for a found machine, and it needs no badge.
fn strip_for(ui: &Ui, t: &Theme, whole: Rect, slot: &Slot, focused: bool) -> Rect {
    let strip = Rect::from_min_size(whole.min, vec2(whole.width(), header::CAPTION));
    let name = slot
        .of
        .as_ref()
        .map(Ask::title)
        .unwrap_or_else(|| "Nothing selected".to_owned());
    let ink = if focused {
        t.text.primary
    } else {
        t.text.secondary
    };
    // On its baseline rather than centred in the strip's box — see [`ink_baseline`], which is what
    // puts the name and the rule under it on one grid.
    let baseline = ink_baseline(ui.painter(), &t.fonts.caption, strip.top(), strip.height());
    let galley = truncated(
        ui.painter(),
        &name,
        t.fonts.caption.clone(),
        ink,
        (strip.width() - PAD * 2.0).max(0.0),
    );
    galley_on_baseline(ui.painter(), strip.left() + PAD, baseline, galley);
    if focused {
        // The line under the focused name, which is what says the header above is about this tile.
        ui.painter().hline(
            strip.x_range(),
            strip.bottom() - 1.0,
            Stroke::new(1.0, t.text.primary),
        );
    }
    Rect::from_min_max(strip.left_bottom(), whole.max)
}

/// Draw one tile's content. Returns the view a button under `No preview for a .zip` asked for.
#[allow(clippy::too_many_arguments)]
fn tile_content(
    ui: &mut Ui,
    t: &Theme,
    canvas: Rect,
    spot: Spot,
    slot: &mut Slot,
    layout: &mut Layout,
    // Whether this is the focused tile, which for a video is whether it is the one heard. See
    // [`video::show`], which owns that decision along with [`Layout::muted`].
    audible: bool,
    out: &mut Vec<Action>,
) -> Option<preview::Kind> {
    // What the chooser needs, read before the content is borrowed: whether there is a single file for
    // it to be about — the empty canvas is also what a *folder* gets, and there is nothing to try on
    // one — and which view has already been picked, so its button can say so.
    let one = matches!(slot.of, Some(Ask::One(..)));
    let forced = slot.forced_kind();
    let mut picked = None;

    // The two halves of the slot that are looked at together, and the only place they are: the search
    // is *about* the content, and the content is what the canvas draws.
    let Slot { content, find, .. } = slot;
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
            picked = nothing_to_show(ui, t, canvas, spot, &what, one, forced);
        }
        // Nothing is drawn but the word. A read takes tens of milliseconds and a spinner that
        // appears and vanishes inside three frames is worse than nothing.
        Content::Reading => note(ui, t, canvas, "Reading…"),
        Content::Failed(why) => {
            let why = why.clone();
            // **The chooser stays up while a view somebody picked is the thing that failed**, so a
            // wrong guess is not a dead end: reading a `.zip` as a picture is a reasonable thing to
            // try and `Format error` is a reasonable answer, and the next thing to try has to be one
            // click away rather than a trip off the file and back. Not offered for a decoder failing
            // on a file it was the *right* choice for — a corrupt `.png` has nothing else to be.
            picked = nothing_to_show(ui, t, canvas, spot, &why, one && forced.is_some(), forced);
        }
        Content::Picture(picture) => pictures(ui, t, canvas, spot, picture),
        // **A player that will not open is the same dead end**, and it gets the same way out for the
        // same reason — with one difference: a real video the machine has no codec for is news about
        // the machine, so the buttons only appear where something asked for this view. See
        // [`video::show`], which is what would otherwise draw the complaint.
        Content::Video(player) if forced == Some(preview::Kind::Video) && player.failed().is_some() => {
            let why = player.failed().unwrap_or_default().to_owned();
            picked = nothing_to_show(ui, t, canvas, spot, &why, one, forced);
        }
        Content::Video(player) => video::show(ui, t, canvas, spot, player, layout, out, audible),
        Content::Text(text) => match &text.doc {
            Some(doc) if !layout.markup => document::draw(ui, t, canvas, spot, doc, find),
            _ => text_canvas(ui, t, canvas, spot, text, layout.numbers, find),
        },
        Content::Binary(view) => deps::show(ui, t, canvas, view),
    }

    // And the find bar over the top of it, last, because it floats.
    if find.open && matches!(&*content, Content::Text(_)) {
        find_bar(ui, t, canvas, spot, find);
    }
    picked
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

/// The canvas with nothing on it: what happened, and — for a file that could be shown some other way
/// — a button for each of the ways. Returns the one that was pressed.
///
/// **The sentence and the buttons are centred as one block.** The alternative is what the panel did
/// when the sentence was all there was: put it on the middle of the canvas and hang the buttons under
/// it, which reads as a line of text with an unrelated piece of furniture below it rather than as a
/// question and its answers.
///
/// `offer` is what decides whether the buttons appear at all — see the call sites, which is where the
/// three states that reach here differ. Off, this is [`note`] with one extra galley.
fn nothing_to_show(
    ui: &mut Ui,
    t: &Theme,
    canvas: Rect,
    spot: Spot,
    saying: &str,
    offer: bool,
    forced: Option<preview::Kind>,
) -> Option<preview::Kind> {
    // Every view this program has a decoder of its own for, in the order it offers them: text first,
    // because a file with a strange extension is far more often a log or a dump than anything else.
    //
    // **`Kind::Shell` is deliberately not here.** It is what a file gets by default and what already
    // came back with nothing, so a button for it would be a button that changes nothing — the same
    // argument the bar's toggles make about controls that do nothing. What plays that part is the
    // pressed button un-pressing: see [`Preview::force`].
    let offered: [(preview::Kind, &str, &str, azur_egui_theme::icons::Icon<'_>); 4] = [
        (
            preview::Kind::Text,
            "Text",
            "Read it as text",
            &crate::icons::document,
        ),
        (
            preview::Kind::Picture,
            "Picture",
            "Decode it as an image",
            &crate::icons::image,
        ),
        (
            preview::Kind::Video,
            "Video",
            "Play it",
            &crate::icons::video,
        ),
        (
            preview::Kind::Binary,
            "Binary",
            "Walk what it imports",
            &crate::icons::executable,
        ),
    ];

    let words = ui
        .painter()
        .layout_no_wrap(saying.to_owned(), t.fonts.body.clone(), t.text.secondary);
    let said = words.size();

    // What each button costs, measured before any of them is placed: the row is centred and a narrow
    // panel wraps it, so where the first one goes depends on all of them.
    let mut buttons = Vec::new();
    if offer {
        for (kind, label, tip, glyph) in offered {
            // Nothing to play off Windows — there is no engine there, and `kind_of` never answers
            // `Video` either. See [`crate::preview::Kind::Video`].
            if kind == preview::Kind::Video && !cfg!(windows) {
                continue;
            }
            buttons.push((kind, label, tip, glyph, chip_width(ui.painter(), t, label)));
        }
    }

    // Packed into as many lines as it takes. A panel dragged down to `MIN_PANEL_W` is narrower than
    // the four of them in a row, and the alternative to wrapping is a button that is off the edge of
    // the panel — or the whole row shrinking away, which is the one thing that must not happen: the
    // buttons are the only way out of this state.
    let room = canvas.width() - PAD * 2.0;
    let mut lines: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
    for (at, button) in buttons.iter().enumerate() {
        match lines.last_mut() {
            Some((line, width)) if *width + PAD + button.4 <= room => {
                line.end = at + 1;
                *width += PAD + button.4;
            }
            _ => lines.push((at..at + 1, button.4)),
        }
    }

    // And then the block, which is what the middle of the canvas is measured against.
    let stack = lines.len() as f32 * TOOL_SIZE + (lines.len() as f32 - 1.0).max(0.0) * PAD;
    let mut block = said.y;
    if !lines.is_empty() && block + PICK_GAP + stack <= canvas.height() {
        block += PICK_GAP + stack;
    } else {
        // A canvas too short for both keeps the sentence: it is the half that says what happened, and
        // the buttons drawn over the panel's own edge would be worse than the buttons being missing.
        // Reachable — a panel along the bottom is only `MIN_PANEL_H` tall at its smallest.
        lines.clear();
    }
    let top = canvas.center().y - block * 0.5;
    ui.painter().galley(
        snap(ui.painter(), pos2(canvas.center().x - said.x * 0.5, top)),
        words,
        Color32::PLACEHOLDER,
    );

    let mut picked = None;
    let mut y = top + said.y + PICK_GAP;
    for (line, width) in lines {
        let mut x = canvas.center().x - width * 0.5;
        for (kind, label, tip, glyph, wide) in &buttons[line] {
            let at = Rect::from_min_size(pos2(x.round(), y.round()), vec2(*wide, TOOL_SIZE));
            if chip(
                ui,
                t,
                at,
                // By label rather than by index, so wrapping the row does not renumber the buttons
                // under a pointer that is resting on one.
                Id::new(("preview-as", spot, label)),
                *glyph,
                label,
                tip,
                forced == Some(*kind),
            )
            .clicked()
            {
                picked = Some(*kind);
            }
            x += wide + PAD;
        }
        y += TOOL_SIZE + PAD;
    }
    picked
}

/// How wide one of [`nothing_to_show`]'s buttons is: its glyph, its word, and the air around them.
fn chip_width(painter: &egui::Painter, t: &Theme, label: &str) -> f32 {
    let words = painter.layout_no_wrap(label.to_owned(), t.fonts.caption.clone(), t.text.primary);
    (PICK_PAD * 2.0 + GLYPH + PAD + words.size().x).ceil()
}

/// One view to try, as a button with its glyph and its name in it.
///
/// [`tool_button`] with a word beside the glyph and one difference in the states: **a fill at rest**.
/// Those buttons sit in a row of chrome where being in the row is what says they are buttons, and this
/// one stands on its own in the middle of an empty canvas — where an icon and a word with nothing
/// behind them are an icon and a word. `bg.control` is the design system's answer for a button on
/// `bg.layer`, which is the surface the panel painted.
///
/// A separate function rather than an argument to that one because its whole shape is a square whose
/// side is its icon's: the two would share the four lines of fill arithmetic below and nothing else.
#[allow(clippy::too_many_arguments)]
fn chip(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    id: Id,
    glyph: azur_egui_theme::icons::Icon<'_>,
    label: &str,
    tooltip: &str,
    active: bool,
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let corner = CornerRadius::same(radius::SMALL);
    let (hover, pressed) = control_fills(t, t.bg.layer);
    let latched = azur_egui_theme::desktop::latched(t, response.hovered());
    let fill = if active {
        latched.0
    } else if response.is_pointer_button_down_on() {
        pressed
    } else if response.hovered() {
        hover
    } else {
        t.bg.control
    };
    ui.painter().rect_filled(rect, corner, fill);

    // The glyph and the word on one line — the glyph centred on its own ink by `azur::icons`, and the
    // word put on the baseline that matches it. `crate::ui::deps` has the reasoning at length; the
    // short version is that a galley centred in the same box sits a point and a half lower.
    let ink = if active { latched.1 } else { t.text.primary };
    let box_rect = icon_rect(rect, rect.left() + PICK_PAD, GLYPH);
    glyph(ui.painter(), box_rect, ink);
    let baseline = ink_baseline(ui.painter(), &t.fonts.caption, rect.top(), rect.height());
    let words = ui
        .painter()
        .layout_no_wrap(label.to_owned(), t.fonts.caption.clone(), ink);
    galley_on_baseline(ui.painter(), box_rect.right() + PAD, baseline, words);

    if response.has_focus() {
        azur_icons::focus_ring_inset(ui.painter(), rect, corner, t.stroke.focus);
    }
    if !tooltip.is_empty() {
        azur_egui_theme::components::tooltip(response.clone(), tooltip);
    }
    response
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
