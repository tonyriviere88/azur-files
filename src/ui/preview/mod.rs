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
    /// The view a button under `No preview for a .zip` asked for. See [`Preview::force`].
    forced: Option<Forced>,
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
            forced: None,
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
        self.ask_for(Ask::One(path, kind));
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
    match &mut preview.content {
        Content::Video(player) => video::theatre(ui, t, screen, pane, player, layout, out),
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

    // What the chooser needs, read before the content is borrowed: whether there is a single file for
    // it to be about — the empty canvas is also what a *folder* gets, and there is nothing to try on
    // one — and which view has already been picked, so its button can say so.
    let one = matches!(preview.of, Some(Ask::One(..)));
    let forced = preview.forced_kind();
    let mut picked = None;

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
            picked = nothing_to_show(ui, t, canvas, pane, &what, one, forced);
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
            picked = nothing_to_show(ui, t, canvas, pane, &why, one && forced.is_some(), forced);
        }
        Content::Picture(picture) => pictures(ui, t, canvas, pane, picture),
        // **A player that will not open is the same dead end**, and it gets the same way out for the
        // same reason — with one difference: a real video the machine has no codec for is news about
        // the machine, so the buttons only appear where something asked for this view. See
        // [`video::show`], which is what would otherwise draw the complaint.
        Content::Video(player) if forced == Some(preview::Kind::Video) && player.failed().is_some() => {
            let why = player.failed().unwrap_or_default().to_owned();
            picked = nothing_to_show(ui, t, canvas, pane, &why, one, forced);
        }
        Content::Video(player) => video::show(ui, t, canvas, pane, player, layout, out),
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

    // A view was asked for. After the canvas rather than inside it, because this replaces what the
    // canvas is drawing and the borrow of it ends here.
    if let Some(kind) = picked {
        preview.force(kind);
        // The read is queued rather than done, and this frame is the one the click arrived on: a
        // request nobody books a frame for is a request that lands the next time something unrelated
        // wants one. The same reasoning as the debounce's `request_repaint_after`.
        ui.ctx().request_repaint();
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
    pane: PaneId,
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
                Id::new(("preview-as", pane, label)),
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
