//! The console panel: a log of blocks, a prompt under it, and one keyboard between them.
//!
//! [`crate::console`] is the shell and the protocol; this is the half you look at. It is a panel
//! along the bottom of a pane, in the stack between the listing's rows and its status line — not a
//! floating overlay, and not a window.
//!
//! # The keyboard belongs to the panel, not to a field
//!
//! **There is no [`egui::TextEdit`] here.** That is the single decision the rest of this file falls
//! out of, and it is worth the four hundred lines it costs.
//!
//! A `TextEdit` owns the focus it is given. It takes `Enter` and `Tab` before anything drawn
//! around it can look at them, egui spends `Tab` on focus navigation *before* user code runs at
//! all, and clicking the log to select a line takes the focus off the field — so every one of
//! `Enter`, `Tab`, `Shift+Tab`, `Shift+Up` and a text selection is a separate fight with a widget
//! that is trying to be helpful. The fix is not to win those fights but not to have them: **one
//! focus id for the whole panel**, [`id`], and a key pass that reads `events` in order and removes
//! what it acts on.
//!
//! Two mechanisms make that hold up, both egui's own:
//!
//! - [`egui::Memory::set_focus_lock_filter`] with `tab` set is how a focused widget says it wants
//!   `Tab` for itself rather than as "move to the next widget". It is what `TextEdit` uses, and
//!   without it `Shift+Tab` never arrives — it has already been spent moving focus somewhere else.
//! - **Every handled event is taken out of the list.** `App::keyboard` runs after the panes are
//!   drawn and stands down whenever anything holds focus, so consuming is what stops one `Escape`
//!   from both clearing a selection here and dropping the focus there.
//!
//! Typing always goes to the prompt, whatever the pointer last touched. What the arrows and a copy
//! are *about* is one thing at a time — see [`Aim`].
//!
//! # The log is rows, and one row is one line
//!
//! Painted at arithmetic rects through `ScrollArea::show_rows`, the same discipline as the file
//! listing: a thousand lines cost the forty on screen. Two things follow from it and are the
//! reason the rows line up at all:
//!
//! - **`item_spacing.y` has to be zero.** `show_rows` reserves `row_height + item_spacing.y` per
//!   row, and the installed style's spacing is not nothing. Left alone, every row is placed a few
//!   points below where the one above it ended, compounding down the panel until the text drifts
//!   out of its own row.
//! - **The row height *is* the font's row height**, so text centred in its row and a chevron
//!   centred in the same row agree by construction rather than by two roundings that happen to
//!   land together.
//!
//! Wrapping is `Alt+Z` and on by default, and it changes what the index *is* rather than only how a
//! row is drawn. Unwrapped, a line is a row and `show_rows` can be given a count taken from the
//! blocks alone. Wrapped, a line is as many rows as it has slices, which nothing but a pass over
//! every line can tell you — so [`State::flat`] appears, and the drag of a resize grip is what makes
//! it appear again. See [`State::rebuild`], which is both models in one function.
//!
//! # Monospace makes the arithmetic exact
//!
//! Every column in the log is `advance` points wide, so the character under the pointer is a
//! division and the x of a character is a multiplication — no galley to lay out to hit-test a row,
//! and no cursor API to keep up with. It is exact for everything a shell prints and wrong for
//! double-width CJK, which is the same trade a terminal makes.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use azur_egui_theme::tokens::{control, radius, space};
use egui::{
    pos2, vec2, Align2, CornerRadius, EventFilter, Id, Key, Modifiers, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};

use crate::console::{Block, Kind, Session};
use crate::pane::PaneId;
use crate::theme::Theme;
use crate::ui::{control_fills, seam, SEAM};

/// How much of a pane the console takes when it is first opened.
pub const SHARE: f32 = 0.35;

/// The least a console can be and still be one: the prompt strip and a few rows over it.
const LEAST: f32 = 96.0;

/// What the listing keeps whatever the console asks for.
const MIN_LIST: f32 = 72.0;

/// The prompt strip along the bottom.
const STRIP: f32 = control::SMALL + space::S2 * 2.0;

/// The air inside the panel, left and right.
const PAD: f32 = space::S3;

/// The fold chevron's box, and the gutter it sits in.
const FOLD: f32 = 12.0;
const GUTTER: f32 = FOLD + space::S2;

/// What a copied block's command is prefixed with, so a pasted transcript reads as one.
///
/// **Nothing is prefixed on screen.** A header used to wear an accent `>` as well as its fold
/// chevron, in the gutter right beside it — two marks saying the same thing, and the chevron is the
/// one that also does something. What tells a command from its output on screen is the band behind it.
///
/// A copy has no band, so it keeps the marker.
const MARK: &str = "> ";

/// How many commands the history keeps.
const HISTORY: usize = 200;

/// The tab stop, in columns. Eight, which is what a shell's output is written against.
const TAB: usize = 8;

/// The command line's caret. Twenty points, centred — not the strip's full height, which reads as a
/// bar rather than as a caret.
const CARET: f32 = 20.0;

/// How far either side of the seam the resize grip reaches.
///
/// [`SEAM`] is one point, and a one-point handle is one nobody can hit: being two pixels low landed on
/// the log instead and dragged a text selection across it. Four either side is nine points of target,
/// which is what a splitter is elsewhere — and it costs a quarter of the first row, which is a row that
/// scrolls rather than one you have to click.
const GRAB: f32 = 4.0;

/// The panel's own focus id — the whole panel's, since it has only the one.
pub fn id(pane: PaneId) -> Id {
    Id::new(("console", pane))
}

/// Take the console's room off the bottom of a pane, leaving the listing the rest.
///
/// The returned rect **includes the seam above it**, so the boundary is one line neither side
/// draws — exactly as between two panes. `None` when the console is shut, or when the pane is too
/// short to give it room without squeezing the listing past being usable.
pub fn split(above: Rect, open: bool, share: f32) -> (Rect, Option<Rect>) {
    if !open {
        return (above, None);
    }
    let room = above.height() - MIN_LIST - SEAM;
    if room < LEAST {
        return (above, None);
    }
    let take = (above.height() * share).clamp(LEAST, room);
    let edge = above.bottom() - take;
    (
        Rect::from_min_max(above.min, pos2(above.right(), edge - SEAM)),
        Some(Rect::from_min_max(pos2(above.left(), edge - SEAM), above.max)),
    )
}

// ---------------------------------------------------------------------------
// What the panel wants done
// ---------------------------------------------------------------------------

/// What a frame of keys and clicks asked for.
///
/// The panel decides; the caller owns the session and the pane, and does. Which keeps the drawing
/// free of the two things that cannot be tested from here — spawning a shell, and the clipboard.
#[derive(Default, Debug, PartialEq)]
pub struct Outcome {
    /// A command to run.
    pub send: Option<String>,
    /// End whatever is running.
    pub stop: bool,
    /// Point the panel at another shell.
    pub swap: Option<Kind>,
    /// Text for the clipboard.
    pub copy: Option<String>,
    /// Throw the log away.
    pub clear: bool,
    /// A folder a command moved the shell to, for the pane to follow.
    pub cwd: Option<PathBuf>,
    /// The share of the pane the panel should keep, once the grip has been dragged.
    pub share: Option<f32>,
    /// A press landed in the panel, so the keyboard is here now.
    ///
    /// Reported rather than left to the pane's own click-to-focus, which the panel covers: it is
    /// registered over the pane's claim and wins the hit test, so without this a click in the
    /// console left the *pane* unfocused while the console had the keys.
    pub claimed: bool,
}

// ---------------------------------------------------------------------------
// The line editor
// ---------------------------------------------------------------------------

/// The command being typed, and the caret in it.
///
/// Byte offsets, kept on character boundaries by only ever moving through [`prev`] and [`next`].
/// `caret` is where typing happens and `anchor` is the far end of the selection, equal to it when
/// there is none — which is the same shape every text editor uses, and the reason `Shift+Left`
/// extends rather than jumps.
#[derive(Clone, Debug, Default)]
pub struct Editor {
    text: String,
    caret: usize,
    anchor: usize,
}

/// The character boundary at or before `at`.
fn prev(text: &str, at: usize) -> usize {
    text[..at].chars().next_back().map_or(0, |c| at - c.len_utf8())
}

/// The character boundary after `at`.
fn next(text: &str, at: usize) -> usize {
    text[at..].chars().next().map_or(at, |c| at + c.len_utf8())
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replace the whole line, caret at the end — what recalling a command does.
    fn set(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.caret = self.text.len();
        self.anchor = self.caret;
    }

    fn clear(&mut self) {
        self.set("");
    }

    fn span(&self) -> Option<std::ops::Range<usize>> {
        let (from, to) = (self.caret.min(self.anchor), self.caret.max(self.anchor));
        (from < to).then_some(from..to)
    }

    fn selected(&self) -> &str {
        self.span().map_or("", |at| &self.text[at])
    }

    /// Take the selection out, and say whether there was one.
    fn erase(&mut self) -> bool {
        let Some(at) = self.span() else { return false };
        self.text.replace_range(at.clone(), "");
        self.caret = at.start;
        self.anchor = self.caret;
        true
    }

    fn insert(&mut self, text: &str) {
        self.erase();
        self.text.insert_str(self.caret, text);
        self.caret += text.len();
        self.anchor = self.caret;
    }

    fn backspace(&mut self) {
        if self.erase() {
            return;
        }
        let from = prev(&self.text, self.caret);
        if from < self.caret {
            self.text.replace_range(from..self.caret, "");
            self.caret = from;
            self.anchor = from;
        }
    }

    fn delete(&mut self) {
        if self.erase() {
            return;
        }
        let to = next(&self.text, self.caret);
        if to > self.caret {
            self.text.replace_range(self.caret..to, "");
        }
    }

    /// `Ctrl+Backspace`: back over the run of spaces, then over the word before it.
    fn kill_word(&mut self) {
        if self.erase() {
            return;
        }
        let from = word_left(&self.text, self.caret);
        self.text.replace_range(from..self.caret, "");
        self.caret = from;
        self.anchor = from;
    }

    /// Put the caret somewhere. `keep` extends the selection instead of dropping it.
    fn go(&mut self, to: usize, keep: bool) {
        self.caret = to.min(self.text.len());
        if !keep {
            self.anchor = self.caret;
        }
    }

    fn left(&mut self, keep: bool, word: bool) {
        // Without shift, a caret with a selection collapses to its near end rather than stepping
        // one character back from it — which is what every editor does and what makes
        // select-then-arrow feel like an escape rather than an edit.
        let to = match (self.span(), word, keep) {
            (Some(at), false, false) => at.start,
            (_, true, _) => word_left(&self.text, self.caret),
            _ => prev(&self.text, self.caret),
        };
        self.go(to, keep);
    }

    fn right(&mut self, keep: bool, word: bool) {
        let to = match (self.span(), word, keep) {
            (Some(at), false, false) => at.end,
            (_, true, _) => word_right(&self.text, self.caret),
            _ => next(&self.text, self.caret),
        };
        self.go(to, keep);
    }

    fn home(&mut self, keep: bool) {
        self.go(0, keep);
    }

    fn end(&mut self, keep: bool) {
        self.go(self.text.len(), keep);
    }

    fn all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// Which column the caret is in, counting characters.
    fn col(&self) -> usize {
        self.text[..self.caret].chars().count()
    }

    /// Where a column falls in the line, as a byte offset.
    fn at_col(&self, col: usize) -> usize {
        self.text
            .char_indices()
            .nth(col)
            .map_or(self.text.len(), |(at, _)| at)
    }

    /// Put the caret at a column, dragging the selection with it if `keep`.
    fn seek(&mut self, col: usize, keep: bool) {
        let at = self.at_col(col);
        self.go(at, keep);
    }
}

/// Back over whitespace, then over the word before it.
fn word_left(text: &str, from: usize) -> usize {
    let mut at = from;
    while at > 0 {
        let back = prev(text, at);
        if !text[back..at].chars().all(char::is_whitespace) {
            break;
        }
        at = back;
    }
    while at > 0 {
        let back = prev(text, at);
        if text[back..at].chars().all(char::is_whitespace) {
            break;
        }
        at = back;
    }
    at
}

/// Forward over the word, then over the whitespace after it.
fn word_right(text: &str, from: usize) -> usize {
    let mut at = from;
    while at < text.len() {
        let ahead = next(text, at);
        if text[at..ahead].chars().all(char::is_whitespace) {
            break;
        }
        at = ahead;
    }
    while at < text.len() {
        let ahead = next(text, at);
        if !text[at..ahead].chars().all(char::is_whitespace) {
            break;
        }
        at = ahead;
    }
    at
}

// ---------------------------------------------------------------------------
// What the keyboard is about
// ---------------------------------------------------------------------------

/// The one thing the arrows steer and a copy is about.
///
/// Three selections could be on screen at once — a drag in the log, a block reached with
/// `Shift+Up`, a range in the prompt — and `Ctrl+C` has to mean exactly one of them. So only one
/// exists at a time, and this is it. Typing always goes to the prompt whatever this says; what it
/// decides is what `Ctrl+C`, `Del` and `Left`/`Right` are *for*.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum Aim {
    /// The command line. The caret and any selection in it live on [`Editor`].
    #[default]
    Prompt,
    /// A range dragged out in the log.
    Text(Span),
    /// A whole block, by [`Block::id`] — reached with `Shift+Up`, folded with `Left`/`Right`,
    /// copied whole, and thrown away with `Del`.
    Block(u64),
}

/// A place in the log: which row, and how many characters into it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Spot {
    row: usize,
    col: usize,
}

/// A range in the log, always the right way round.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Span {
    from: Spot,
    to: Spot,
}

impl Span {
    fn new(a: Spot, b: Spot) -> Self {
        let (from, to) = if a <= b { (a, b) } else { (b, a) };
        Self { from, to }
    }

    fn empty(&self) -> bool {
        self.from == self.to
    }

    /// The columns of `row` this covers, given how long the row is.
    fn cut(&self, row: usize, len: usize) -> Option<std::ops::Range<usize>> {
        if row < self.from.row || row > self.to.row {
            return None;
        }
        let start = if row == self.from.row { self.from.col } else { 0 };
        let end = if row == self.to.row { self.to.col } else { len };
        (start < end.min(len)).then(|| start..end.min(len))
    }
}

/// What a drag in the log is selecting by.
///
/// A double click puts it in [`Grab::Word`] until the button comes back up, which is what "select
/// text like an editor" means everywhere else: the two ends of the selection snap out to whole words
/// as the pointer moves. A single press selects by character, exactly.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum Grab {
    #[default]
    Char,
    Word,
}

/// The three kinds of run a double click can land in.
///
/// Punctuation is its own class rather than part of a word, which is what makes `cpp` selectable on
/// its own in `source.cpp`: the dot is a run of one and the name either side of it is a word. Lumping
/// them together would select the whole of `source.cpp` and nothing shorter.
#[derive(PartialEq, Eq, Copy, Clone)]
enum Class {
    Word,
    Space,
    Mark,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Mark
    }
}

/// The run of like characters `col` falls in.
fn word_at(chars: &[char], col: usize) -> std::ops::Range<usize> {
    if chars.is_empty() {
        return 0..0;
    }
    let at = col.min(chars.len() - 1);
    let kind = class(chars[at]);
    let mut from = at;
    while from > 0 && class(chars[from - 1]) == kind {
        from -= 1;
    }
    let mut to = at + 1;
    while to < chars.len() && class(chars[to]) == kind {
        to += 1;
    }
    from..to
}

/// What one row of the log is.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    /// The blank row a block starts with — air between one command's output and the next one's
    /// header, so a run of commands reads as a run of blocks and not as one wall of text.
    ///
    /// The block it belongs to is the one *below* it, and the first block has none: a log that opens
    /// with an empty row looks like a log with something missing off the top of it.
    Gap(usize),
    /// A block's header: the command, and how it ended. The second number is which slice of a wrapped
    /// command this row is, and zero whenever wrapping is off.
    Head(usize, usize),
    /// A line the command printed, and which slice of it this row is.
    Text(usize, usize, usize),
    /// The note a capped block wears, saying what it threw away.
    Cut(usize),
}

impl Row {
    fn block(self) -> usize {
        match self {
            Row::Gap(at) | Row::Head(at, _) | Row::Text(at, _, _) | Row::Cut(at) => at,
        }
    }

    /// Which slice of a wrapped line this is.
    fn slice(self) -> usize {
        match self {
            Row::Head(_, at) | Row::Text(_, _, at) => at,
            Row::Gap(_) | Row::Cut(_) => 0,
        }
    }

    /// Whether two rows are slices of the same logical line — which is what stops a copy putting a
    /// newline into the middle of a path that only *looks* like two lines because it wrapped.
    fn same_line(self, other: Self) -> bool {
        match (self, other) {
            (Row::Head(a, _), Row::Head(b, _)) => a == b,
            (Row::Text(a, x, _), Row::Text(b, y, _)) => a == b && x == y,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// The panel's memory
// ---------------------------------------------------------------------------

/// Everything the panel remembers between frames.
///
/// Kept while the panel is shut as well, so closing it does not throw away what was typed or where
/// the history had got to.
#[derive(Debug)]
pub struct State {
    /// The command line.
    pub line: Editor,
    /// Which shell the panel is pointed at. The session follows it, not the other way round.
    kind: Kind,
    /// Commands that have been run, oldest first.
    history: Vec<String>,
    /// What was being typed before `Up` walked off into the history, so `Down` can come back to it.
    draft: String,
    /// How far back the history has been walked. `None` while the line is the one being typed.
    recall: Option<usize>,
    /// Whether the shell dropdown is up.
    picking: bool,
    /// Whether the log is stuck to the bottom. Set by sending a command, cleared by scrolling up —
    /// because reading a build log means scrolling back into it and having it yanked away is worse
    /// than missing the last line.
    tail: bool,
    /// What the arrows steer and a copy is about.
    aim: Aim,
    /// Bring this row into view on the next frame.
    reveal: Option<usize>,
    /// Whether the drag in progress is selecting by character or by word.
    grab: Grab,
    /// Whether the drag in progress is the resize border, latched for as long as the button is down.
    ///
    /// Decided when the press lands and then *remembered*, because dragging the border moves the
    /// border: by the second frame the panel has grown and the band the press started in is somewhere
    /// else, so a test against the band's current position says the press was in the log and starts
    /// selecting text. Which is what resizing a console with output in it did.
    resizing: bool,
    /// When the command line was last touched, which is what the caret's blink is measured from.
    ///
    /// A caret that carries on blinking through what you are typing is the tell of one that is on a
    /// timer rather than on the text — so every keystroke and every click in the field puts it back to
    /// solid, exactly as a text field does.
    edited: f64,
    /// When and where the last press in the log landed, so the *next* one can tell it is a double.
    ///
    /// Worked out here rather than read off `Response::double_clicked`, which is reported on the
    /// **release** — a frame too late to put the drag that has already started into word mode, and
    /// too late for a bare double click to select anything at all. egui's own delay is used, so this
    /// agrees with what every other double click in the window means.
    last_press: Option<(f64, Pos2)>,
    /// **Whether the panel considers the keyboard its own**, re-asserted on every frame.
    ///
    /// egui's focus is not somewhere a claim can be left lying: a widget's focus is revoked whenever
    /// a click completes and that widget is not the hovered one — and the panel never is, because the
    /// row or the field the click landed on is registered over the top of it. So a press claimed the
    /// keyboard and the matching *release*, one frame later, took it straight back off. Which is
    /// exactly what "a single click in the console gives the focus back to the file list" was.
    ///
    /// So the panel keeps the answer itself and tells egui again whenever egui has stopped agreeing —
    /// only then, because saying it while it is already true resets the focus filter along with it.
    /// A press inside makes it the panel's; a press anywhere else gives it up.
    owns: bool,
    /// Whether long lines wrap instead of running off to the right. `Alt+Z`, and on by default —
    /// a line that has run off the right edge of a panel this shape is a line nobody read, and
    /// reaching for the horizontal scrollbar to find the end of a compiler error is worse than
    /// losing the column `ls` and `git status` line their meaning up in. Off is a keystroke away
    /// for when that column is the point.
    wrap: bool,
    /// How many characters fit across the log, measured by the drawing and read by everything else —
    /// the index, a copy, the hit-testing. One number, so they cannot disagree about where a line
    /// breaks.
    width: usize,
    /// Whether the horizontal offset needs putting back to nothing, because wrapping has just been
    /// turned on and there is nowhere sideways to be any more.
    pan: bool,
    /// The row each block's header sits on, and one past the end: `starts[i]..starts[i + 1]` is
    /// block `i`'s rows. Rebuilt every frame, which costs one pass over the *blocks* — never over
    /// their lines.
    starts: Vec<usize>,
    /// Every visual row, when wrapping is on and a line is no longer a row. `None` otherwise, which is
    /// the whole point: unwrapped, this index does not exist and cannot cost anything.
    flat: Option<Vec<Row>>,
    /// What [`State::flat`] was built from, so it is only built again when something moved.
    shape: u64,
    /// Where the log was scrolled to last frame, which is what a nudge into view is measured
    /// against — `ScrollArea` takes an absolute offset.
    offset: f32,
    /// Whether a command was running last frame, so a `cd` can be noticed the moment one finishes.
    was_running: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            line: Editor::default(),
            kind: Kind::default(),
            history: Vec::new(),
            draft: String::new(),
            recall: None,
            picking: false,
            tail: true,
            aim: Aim::default(),
            reveal: None,
            grab: Grab::default(),
            resizing: false,
            edited: 0.0,
            last_press: None,
            owns: false,
            wrap: true,
            width: 0,
            pan: false,
            starts: Vec::new(),
            flat: None,
            shape: 0,
            offset: 0.0,
            was_running: false,
        }
    }
}

impl State {
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Point the panel at a shell before it has started one — how the remembered shell gets in.
    ///
    /// Ignored once there is a session, because changing the kind is what makes the session stale and
    /// replaces it; that is a thing to ask for, not something a settings file should do underneath a
    /// running shell.
    pub fn set_kind(&mut self, kind: Kind) {
        self.kind = kind;
    }

    /// The keyboard is the panel's now — opening it is one of the two ways that happens.
    pub fn take_keys(&mut self) {
        self.owns = true;
    }

    /// And it is not, which closing it is.
    pub fn drop_keys(&mut self) {
        self.owns = false;
    }

    /// **The one answer to "are the keys the console's".** Read by the panel before it looks at an
    /// event and by the listing before it draws a selection, from here rather than twice, so the two
    /// cannot end up disagreeing about who the arrow keys belong to.
    ///
    /// Both halves have to hold: the panel claimed the keyboard, *and* nothing else has taken it —
    /// which is what keeps two consoles in two panes from both reading one keystroke.
    ///
    /// `focused().is_none()` counts as the panel's, and that is not a loophole but the frame egui
    /// takes the keyboard off it for no reason of ours: a click *completing* anywhere over the panel
    /// revokes the focus of a widget that is not the hovered one, and the panel never is — the row or
    /// the field under the pointer is registered over the top of it. Without this, the frame a click
    /// finished in was a frame with no keyboard in it: a key pressed in it went nowhere, and the file
    /// listing lit up as though it had the keys back.
    pub fn keeps_keys(&self, ctx: &egui::Context, pane: PaneId) -> bool {
        self.owns && ctx.memory(|m| m.has_focus(id(pane)) || m.focused().is_none())
    }

    /// How many rows the log has.
    fn rows(&self) -> usize {
        match &self.flat {
            Some(flat) => flat.len(),
            None => self.starts.last().copied().unwrap_or(0),
        }
    }

    /// Work out where every block starts, and — when wrapping — what is on every row.
    ///
    /// **Unwrapped, this is one pass over the blocks**: a line is a row, so a folded block costs the
    /// same as an empty one and a five-thousand-line block costs the same as either.
    ///
    /// **Wrapped, a line is as many rows as it has slices**, which no amount of arithmetic over the
    /// blocks can tell you — so the index becomes one entry per visual row. It is rebuilt only when
    /// something it depends on moves, which is what [`State::shape`] is for: the blocks, their lengths,
    /// their folds, and the column count. Resizing the panel changes the last of those, which is why
    /// wrapping cannot be free the way the flat model is.
    ///
    /// A frame in which nothing moved does nothing at all, and that is worth the early return now that
    /// wrapping is what the panel opens in: both halves of the index are still about these blocks, and
    /// working `starts` out again means a pass over every line in the log — 200 blocks of 5,000 lines
    /// of it — for an answer already in hand.
    fn rebuild(&mut self, blocks: &[Block], cols: Option<usize>) {
        if let Some(cols) = cols {
            let shape = shape_of(blocks, cols);
            if self.shape == shape && self.flat.is_some() {
                return;
            }
            self.starts.clear();
            self.starts.reserve(blocks.len() + 1);
            let mut flat = self.flat.take().unwrap_or_default();
            flat.clear();
            for (which, block) in blocks.iter().enumerate() {
                self.starts.push(flat.len());
                if which > 0 {
                    flat.push(Row::Gap(which));
                }
                for seg in 0..slices(&block.command, cols) {
                    flat.push(Row::Head(which, seg));
                }
                if block.collapsed {
                    continue;
                }
                if block.dropped > 0 {
                    flat.push(Row::Cut(which));
                }
                for (at, line) in block.lines.iter().enumerate() {
                    for seg in 0..slices(&line.text, cols) {
                        flat.push(Row::Text(which, at, seg));
                    }
                }
            }
            self.starts.push(flat.len());
            self.shape = shape;
            self.flat = Some(flat);
            return;
        }

        self.starts.clear();
        self.starts.reserve(blocks.len() + 1);
        self.flat = None;
        let mut at = 0;
        for (which, block) in blocks.iter().enumerate() {
            // A block starts at its blank row — see [`Row::Gap`] — so revealing one brings its air
            // along with it and the header never lands jammed against the top edge.
            self.starts.push(at);
            if which > 0 {
                at += 1;
            }
            at += 1;
            if !block.collapsed {
                if block.dropped > 0 {
                    at += 1;
                }
                at += block.lines.len();
            }
        }
        self.starts.push(at);
    }

    /// What is on a row. A search over the blocks, then arithmetic inside the one it lands in — or a
    /// lookup, when wrapping has already worked it out.
    fn row(&self, blocks: &[Block], at: usize) -> Option<Row> {
        if let Some(flat) = &self.flat {
            return flat.get(at).copied();
        }
        let which = self.starts.partition_point(|&start| start <= at).checked_sub(1)?;
        let block = blocks.get(which)?;
        let base = self.starts[which];
        let mut line = at - base;
        // The blank row every block but the first starts with.
        if which > 0 {
            if line == 0 {
                return Some(Row::Gap(which));
            }
            line -= 1;
        }
        if line == 0 {
            return Some(Row::Head(which, 0));
        }
        line -= 1;
        if block.dropped > 0 {
            if line == 0 {
                return Some(Row::Cut(which));
            }
            line -= 1;
        }
        (line < block.lines.len()).then_some(Row::Text(which, line, 0))
    }

    // -- the keyboard --------------------------------------------------------

    /// Act on a frame's events, taking out the ones acted on.
    ///
    /// The whole keyboard is here rather than spread across the drawing, and it takes plain data —
    /// no [`Ui`], no context, nothing to spawn. Which is what lets the tests at the bottom of this
    /// file press keys and read the answer.
    fn keys(
        &mut self,
        events: &mut Vec<egui::Event>,
        held: Modifiers,
        blocks: &mut Vec<Block>,
        out: &mut Outcome,
    ) {
        let seen = std::mem::take(events);
        events.reserve(seen.len());
        for event in seen {
            if !self.event(&event, held, blocks, out) {
                events.push(event);
            }
        }
    }

    /// True when the event was ours.
    ///
    /// `held` is the frame's modifier state, and it is here for one thing: `Event::Text` does not carry
    /// modifiers, and a keystroke that means something else can still arrive as text. `Alt+Z` did —
    /// it typed a `z` into the command line instead of turning wrapping on.
    fn event(
        &mut self,
        event: &egui::Event,
        held: Modifiers,
        blocks: &mut Vec<Block>,
        out: &mut Outcome,
    ) -> bool {
        use egui::Event as E;
        match event {
            // **Not while `Alt` is down on its own**, which is a shortcut rather than a character.
            // `Alt` *with* `Ctrl` is `AltGr`, and on this keyboard that is how `@`, `#` and `€` are
            // typed — so those have to come through.
            E::Text(_) if held.alt && !held.ctrl => false,
            E::Text(text) => {
                self.aim = Aim::Prompt;
                self.line.insert(text);
                true
            }
            // A shell prompt is one line, and a pasted block of them is nearly always an accident
            // of copying with the newline. Folded into spaces rather than split into commands: one
            // paste that runs six things nobody read is the worse failure.
            E::Paste(text) => {
                self.aim = Aim::Prompt;
                // A run of line endings is one space, not one space each: a Windows clipboard
                // carries `\r\n` and a trailing newline, so replacing them one for one leaves a
                // double space in the middle of the command and another on the end.
                let flat = text
                    .split(['\r', '\n'])
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                self.line.insert(&flat);
                true
            }
            // egui-winit sends these *and* the key, and which of the two arrives is a platform
            // detail. Both are handled, and both are idempotent within a frame.
            E::Copy => {
                self.copy(blocks, out);
                true
            }
            E::Cut => {
                self.cut(out);
                true
            }
            E::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => self.press(*key, *modifiers, blocks, out),
            _ => false,
        }
    }

    fn press(
        &mut self,
        key: Key,
        m: Modifiers,
        blocks: &mut Vec<Block>,
        out: &mut Outcome,
    ) -> bool {
        // Alt belongs to the window — the menu bar, and whatever the platform wants of it — with one
        // exception, which is the shortcut every editor uses for this.
        if m.alt {
            if key == Key::Z && !m.command {
                self.wrap = !self.wrap;
                // The horizontal offset is meaningless once lines stop running off to the right, and a
                // log left scrolled sideways after a wrap looks empty.
                self.pan = self.wrap;
                return true;
            }
            return false;
        }
        match key {
            Key::Enter => {
                self.run(out);
                true
            }
            // Taken whether or not it does anything, because `Tab` is *why* the focus filter is
            // installed: letting a stray one through would move the keyboard out of the panel,
            // which is the one thing a console must never do to somebody mid-command.
            Key::Tab => {
                if m.shift {
                    self.kind = self.kind.next();
                    self.picking = false;
                    out.swap = Some(self.kind);
                }
                true
            }
            Key::ArrowUp | Key::ArrowDown => {
                let back = key == Key::ArrowUp;
                if m.shift {
                    self.walk(blocks, back);
                } else {
                    self.recall(back);
                }
                true
            }
            Key::ArrowLeft | Key::ArrowRight => {
                let open = key == Key::ArrowRight;
                // With a block picked, the arrows are about the block — and plain arrows as well
                // as shifted ones, because in that state there is no caret for them to be about.
                // Typing anything at all puts the aim back on the prompt.
                if let Aim::Block(id) = self.aim {
                    if let Some(block) = blocks.iter_mut().find(|block| block.id == id) {
                        block.collapsed = !open;
                    }
                } else {
                    self.aim = Aim::Prompt;
                    if open {
                        self.line.right(m.shift, m.command);
                    } else {
                        self.line.left(m.shift, m.command);
                    }
                }
                true
            }
            Key::Home | Key::End => {
                self.aim = Aim::Prompt;
                if key == Key::Home {
                    self.line.home(m.shift);
                } else {
                    self.line.end(m.shift);
                }
                true
            }
            Key::Backspace => {
                self.aim = Aim::Prompt;
                if m.command {
                    self.line.kill_word();
                } else {
                    self.line.backspace();
                }
                true
            }
            Key::Delete => {
                if let Aim::Block(id) = self.aim {
                    self.forget(blocks, id);
                } else {
                    self.aim = Aim::Prompt;
                    self.line.delete();
                }
                true
            }
            // One thing at a time, quietest first — and the panel is never dismissed by it. A
            // console you can lose a half-typed command out of with one keystroke is worse than
            // one that takes a second press of the key that opened it.
            Key::Escape => {
                if self.picking {
                    self.picking = false;
                } else if self.aim != Aim::Prompt {
                    self.aim = Aim::Prompt;
                } else if !self.line.is_empty() {
                    self.line.clear();
                } else {
                    return false;
                }
                true
            }
            Key::C if m.command => {
                self.copy(blocks, out);
                true
            }
            Key::X if m.command => {
                self.cut(out);
                true
            }
            Key::A if m.command => {
                self.aim = Aim::Prompt;
                self.line.all();
                true
            }
            Key::L if m.command => {
                self.aim = Aim::Prompt;
                out.clear = true;
                true
            }
            // The scroll area's, and the panel has no use for them.
            _ => false,
        }
    }

    /// Run what is on the line.
    fn run(&mut self, out: &mut Outcome) {
        let command = self.line.text().trim().to_owned();
        self.line.clear();
        self.recall = None;
        self.draft.clear();
        self.aim = Aim::Prompt;
        if command.is_empty() {
            return;
        }
        // **`clear` and `cls` are the panel's, not the shell's.** Handed to a shell they write the
        // escape sequences that move a terminal's cursor about, and this strips those — so the one
        // command everybody types to tidy up would have done nothing at all. Either spelling in
        // either shell, because which one is "right" is a thing about the shell you came from.
        //
        // Still remembered, so `Up` finds it, and still not sent — there is no block to make.
        if command.eq_ignore_ascii_case("clear") || command.eq_ignore_ascii_case("cls") {
            self.remember(command);
            self.tail = true;
            out.clear = true;
            return;
        }
        self.remember(command.clone());
        self.tail = true;
        out.send = Some(command);
    }

    /// Put a command in the history. The same one twice running is one entry, which is what makes
    /// `Up` useful after a handful of retries of the same build.
    fn remember(&mut self, command: String) {
        if self.history.last() != Some(&command) {
            self.history.push(command);
        }
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
    }

    /// Walk the history. Stops at the oldest; walking off the newest gives back the draft.
    fn recall(&mut self, back: bool) {
        self.aim = Aim::Prompt;
        if self.history.is_empty() {
            return;
        }
        match (self.recall, back) {
            (None, true) => {
                self.draft = self.line.text().to_owned();
                self.recall = Some(self.history.len() - 1);
            }
            (None, false) => return,
            (Some(0), true) => {}
            (Some(at), true) => self.recall = Some(at - 1),
            (Some(at), false) => {
                if at + 1 < self.history.len() {
                    self.recall = Some(at + 1);
                } else {
                    self.recall = None;
                    let draft = std::mem::take(&mut self.draft);
                    self.line.set(&draft);
                    return;
                }
            }
        }
        if let Some(at) = self.recall {
            let text = self.history[at].clone();
            self.line.set(&text);
        }
    }

    /// Walk the blocks. `Shift+Up` from the prompt lands on the newest; `Shift+Down` off the newest
    /// comes back to the prompt, which is the way out of block-selection without reaching for
    /// `Escape`.
    fn walk(&mut self, blocks: &[Block], back: bool) {
        if blocks.is_empty() {
            return;
        }
        let at = match self.aim {
            Aim::Block(id) => blocks.iter().position(|block| block.id == id),
            _ => None,
        };
        let to = match (at, back) {
            (None, true) => Some(blocks.len() - 1),
            (None, false) => None,
            (Some(0), true) => Some(0),
            (Some(at), true) => Some(at - 1),
            (Some(at), false) => (at + 1 < blocks.len()).then_some(at + 1),
        };
        match to {
            Some(at) => {
                self.aim = Aim::Block(blocks[at].id);
                self.reveal = self.starts.get(at).copied();
                self.tail = false;
            }
            None => {
                self.aim = Aim::Prompt;
                self.tail = true;
            }
        }
    }

    /// Throw a block away, and land the selection on whatever took its place.
    fn forget(&mut self, blocks: &mut Vec<Block>, id: u64) {
        let Some(at) = blocks.iter().position(|block| block.id == id) else {
            return;
        };
        // A running block's sentinel is still on its way, and the shell has no idea any of this
        // happened. Removing it would leave the output of a live command with nowhere to land.
        if blocks[at].running() {
            return;
        }
        blocks.remove(at);
        self.aim = match (blocks.get(at), at) {
            (Some(block), _) => Aim::Block(block.id),
            (None, 0) => Aim::Prompt,
            (None, at) => Aim::Block(blocks[at - 1].id),
        };
    }

    /// What `Ctrl+C` means here, in the order the panel means it.
    ///
    /// With nothing selected anywhere it is a stop, which is what somebody who has just typed
    /// `Ctrl+C` at a running command meant — the caller ignores it when nothing is running, so the
    /// worst it can do in the quiet case is nothing.
    fn copy(&mut self, blocks: &[Block], out: &mut Outcome) {
        if out.copy.is_some() {
            return;
        }
        let text = match self.aim {
            Aim::Block(id) => blocks
                .iter()
                .find(|block| block.id == id)
                .map(transcript)
                .filter(|text| !text.is_empty()),
            Aim::Text(span) if !span.empty() => Some(self.slice(blocks, span)),
            _ => {
                let picked = self.line.selected();
                (!picked.is_empty()).then(|| picked.to_owned())
            }
        };
        match text {
            Some(text) => out.copy = Some(text),
            None => out.stop = true,
        }
    }

    fn cut(&mut self, out: &mut Outcome) {
        if out.copy.is_some() {
            return;
        }
        let picked = self.line.selected().to_owned();
        if picked.is_empty() {
            return;
        }
        out.copy = Some(picked);
        self.line.erase();
    }

    /// The text a span covers, as it would be pasted.
    fn slice(&self, blocks: &[Block], span: Span) -> String {
        let mut text = String::new();
        let mut last: Option<Row> = None;
        for at in span.from.row..=span.to.row {
            let Some(row) = self.row(blocks, at) else { break };
            let whole = shown(blocks, row, self.cols());
            let chars: Vec<char> = whole.chars().collect();
            // A newline between rows — unless the two rows are slices of one wrapped line, which is
            // not two lines and must not paste as two. A wrapped path with a line ending dropped into
            // the middle of it is worse than no wrapping at all.
            if at > span.from.row && !last.is_some_and(|last| last.same_line(row)) {
                text.push_str("\r\n");
            }
            if let Some(cut) = span.cut(at, chars.len()) {
                text.extend(&chars[cut]);
            }
            last = Some(row);
        }
        text
    }

    /// The wrap width, or `None` when lines run off to the right instead.
    fn cols(&self) -> Option<usize> {
        self.wrap.then_some(self.width).filter(|cols| *cols > 0)
    }
}

/// A block as it would be pasted: the command, then everything it printed.
///
/// Through [`columns`] like everything else, so that a whole-block copy and a dragged one give the
/// same characters for the same line. What you see is what you get, and a panel where those two
/// disagree about a tab is a panel where one of them is wrong.
fn transcript(block: &Block) -> String {
    let mut text = format!("{MARK}{}", columns(&block.command));
    for line in &block.lines {
        text.push_str("\r\n");
        text.push_str(&columns(&line.text));
    }
    text
}

/// The text on a row, exactly as the log draws it — so what is copied is what was selected.
///
/// `cols` is the wrap width, and slicing here rather than at the paint is what keeps that true: the
/// row the pointer is over, the characters drawn on it and the characters a copy takes are one string.
fn shown(blocks: &[Block], row: Row, cols: Option<usize>) -> Cow<'_, str> {
    let whole = match row {
        // Nothing on it, and nothing is what a copy across it should take: a blank row between two
        // blocks pastes as the blank line it looks like.
        Row::Gap(_) => return Cow::Borrowed(""),
        Row::Head(at, _) => columns(&blocks[at].command),
        Row::Text(at, line, _) => columns(&blocks[at].lines[line].text),
        Row::Cut(at) => {
            return Cow::Owned(format!("… {} earlier lines dropped", blocks[at].dropped))
        }
    };
    match cols {
        None => whole,
        Some(cols) => Cow::Owned(
            whole
                .chars()
                .skip(row.slice() * cols.max(1))
                .take(cols.max(1))
                .collect(),
        ),
    }
}

/// How many rows a line takes at `cols` columns. At least one, so a blank line is still a line.
fn slices(text: &str, cols: usize) -> usize {
    let width = columns(text).chars().count();
    width.div_ceil(cols.max(1)).max(1)
}

/// What the wrapped index depends on: the blocks, their lengths, their folds, and the width.
///
/// Hashed rather than compared, because comparing means keeping a copy of all of it. One pass over the
/// *blocks* — never over their lines — so asking "has anything moved" stays cheap even when answering
/// "what is on every row" is not.
fn shape_of(blocks: &[Block], cols: usize) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cols.hash(&mut hasher);
    blocks.len().hash(&mut hasher);
    for block in blocks {
        block.id.hash(&mut hasher);
        block.lines.len().hash(&mut hasher);
        block.dropped.hash(&mut hasher);
        block.collapsed.hash(&mut hasher);
        // The last line is the one that grows in place while a progress bar redraws itself, and its
        // length is what decides how many rows it takes.
        block.lines.last().map(|line| line.text.len()).hash(&mut hasher);
    }
    hasher.finish()
}

/// One character, one column — tabs expanded and anything else invisible turned into a space.
///
/// **The whole panel is columns**: the character under the pointer is `(x - left) / advance`, the x of
/// a character is the multiplication back, and a copy slices the row by those same numbers. All of
/// which is only true while every character is exactly one column wide, and a tab is not: epaint
/// advances it to the next stop, so a single leading tab put every character after it four columns
/// along while this counted one.
///
/// Nothing looked wrong, which is what made it worth a bug report rather than a glance. The highlight
/// is drawn at the column the pointer was over, so it lands on the glyphs you dragged across; the copy
/// takes the characters at that column *index*, three of which were the tab's. Drag across
/// `LgsxDetailsWidget.h` on a line starting with a tab and the clipboard says `xDetailsWidget.h` — the
/// same number of characters, three along.
///
/// So the tab is expanded once, on the way to being shown, and the panel never sees one. Control
/// characters go the same way for the same reason: they have no glyph, so whatever a font does with
/// them is not one column either.
///
/// What is left is scripts wider than one column, which is a trade a terminal makes too — see the
/// module header.
fn columns(text: &str) -> Cow<'_, str> {
    if !text.contains(|c: char| c.is_control()) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + TAB);
    let mut col = 0;
    for c in text.chars() {
        match c {
            '\t' => {
                let stop = (col / TAB + 1) * TAB;
                for _ in col..stop {
                    out.push(' ');
                }
                col = stop;
            }
            c if c.is_control() => {
                out.push(' ');
                col += 1;
            }
            c => {
                out.push(c);
                col += 1;
            }
        }
    }
    Cow::Owned(out)
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Draw the panel and answer the keyboard.
///
/// `dir` is the folder the pane is showing, which is what a finished command's own folder is
/// compared against — see [`Outcome::cwd`]. `body` is the pane's body, for the resize grip.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    state: &mut State,
    session: Option<&mut Session>,
    dir: Option<&Path>,
    body: Rect,
    share: f32,
) -> Outcome {
    let mut out = Outcome::default();
    let me = id(pane);

    // The seam this panel hangs from, then its own surface. `bg.canvas` — the window's own colour
    // showing through the pane that is sitting on it, which is what makes a monospace log read as
    // recessed rather than as a second panel. It is the same argument, and the same colour, as the
    // fill behind a code block in the preview.
    ui.painter().rect_filled(rect, CornerRadius::ZERO, seam(t));
    let inside = Rect::from_min_max(pos2(rect.left(), rect.top() + SEAM), rect.max);
    ui.painter()
        .rect_filled(inside, CornerRadius::ZERO, t.bg.canvas);

    // The band the resize grip owns. Wider than the seam, or a one-point line is something nobody can
    // grab — which means it overlaps the log's first row, and the log has to know so that a drag of
    // the border is not also a drag across the text. See [`grip`], which is drawn last.
    let band = Rect::from_min_max(
        pos2(rect.left(), rect.top() - GRAB),
        pos2(rect.right(), rect.top() + SEAM + GRAB),
    );

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inside)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(inside.intersect(ui.clip_rect()));

    // **The panel is registered as a widget every frame, whatever the pointer is doing.** Not
    // decoration: egui drops focus at the end of any frame in which the focused id did not put a
    // rect on the record — a dead-man's-switch for widgets that have gone away — so a panel that
    // only ever called `request_focus` held the keyboard for exactly one frame and every key after
    // that went to the listing. Which is precisely how this looked when it was broken: the console
    // on screen, the caret in it, and `e` jumping to a folder in the rows above.
    let _ = child.interact(inside, me, Sense::click());

    let hot = state.keeps_keys(child.ctx(), pane);
    if hot {
        // What this panel wants for itself, and every one of them is a key egui would otherwise
        // spend on moving the focus somewhere else.
        child.memory_mut(|m| {
            m.set_focus_lock_filter(
                me,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
    }

    // Scalars first, so nothing is borrowed off the session when its blocks are.
    let (running, gone) = session
        .as_ref()
        .map_or((false, false), |s| (s.running(), s.gone()));
    let cwd = session
        .as_ref()
        .and_then(|s| s.cwd().map(Path::to_path_buf));
    let mut nothing = Vec::new();
    let blocks: &mut Vec<Block> = match session {
        Some(session) => &mut session.blocks,
        None => &mut nothing,
    };

    let strip = Rect::from_min_max(pos2(inside.left(), inside.bottom() - STRIP), inside.max);
    let rule = strip.top().round() - 0.5;
    let log_rect = Rect::from_min_max(inside.min, pos2(inside.right(), strip.top() - 1.0));

    // **How many characters fit across the log**, worked out here because the row index needs it and
    // the index is built before the keys are read. One number for the index, the drawing, the
    // hit-testing and a copy — see [`State::width`].
    let bar = child.spacing().scroll.allocated_width().max(1.0);
    let across = (log_rect.width() - bar - PAD * 2.0 - GUTTER) / metrics(&child, &t.fonts.mono).advance;
    state.width = across.floor().max(1.0) as usize;

    // Before the keys, because `Shift+Up` scrolls to a row and has to know which one.
    let cols = state.cols();
    state.rebuild(blocks, cols);

    if hot {
        let mut events = child.input(|i| i.events.clone());
        let before = events.len();
        let held = child.input(|i| i.modifiers);
        state.keys(&mut events, held, blocks, &mut out);
        // Anything at all having been ours counts as touching the line: the caret goes back to solid,
        // which is what makes it readable while somebody is typing at it.
        if events.len() != before {
            state.edited = child.input(|i| i.time);
        }
        child.input_mut(|i| i.events = events);
    }

    // **A command that finished somewhere else moved the shell, and the pane follows it.** Only on
    // the frame one finishes: the pane's own folder is sent as a `cd` before every command, so any
    // difference at that moment is something the command itself did. Comparing at any other time
    // would drag the pane back the moment somebody navigated it by hand.
    if state.was_running && !running {
        if let (Some(shell), Some(here)) = (cwd.as_deref(), dir) {
            if shell != here {
                out.cwd = Some(shell.to_path_buf());
            }
        }
    }
    state.was_running = running;

    // **Which gesture this drag is, decided when it starts.** The border moves while it is being
    // dragged, so this cannot be re-asked each frame against a band that has gone somewhere else.
    if !child.input(|i| i.pointer.primary_down()) {
        state.resizing = false;
    } else if child.input(|i| {
        i.pointer
            .any_pressed()
            .then(|| i.pointer.interact_pos())
            .flatten()
    })
    .is_some_and(|at| band.contains(at))
    {
        state.resizing = true;
    }

    if log_rect.height() >= 1.0 {
        log(&mut child, t, log_rect, pane, state, blocks, gone, band);
    }

    // The separator: one `stroke-subtle` hairline, which is the quietest line the palette has and
    // the whole of what the spec asks for here — the two halves are one surface with a crease in
    // it, not two panels.
    child.painter().line_segment(
        [pos2(inside.left(), rule), pos2(inside.right(), rule)],
        Stroke::new(1.0, t.stroke.subtle),
    );

    prompt(&mut child, t, strip, pane, state, hot, running, &mut out);

    // **Last, so the border wins the pointer.** Registered after the log's rows, it is the topmost
    // thing in the band they share — so it takes the hover and the drag, and its cursor is the one
    // the frame ends with. Drawn first, the rows were on top of it: the pointer showed a text caret
    // over the border and dragging it selected the output instead of resizing the panel.
    grip(ui, t, rect, band, pane, body, share, &mut out);

    // **Who owns the keyboard, decided from the pointer and then asserted — every frame, at the end.**
    //
    // From the pointer rather than from a response, because the panel is several widgets and whichever
    // one the click landed on takes the click; the outer response never sees it. And re-asserted
    // rather than claimed once, because egui revokes a widget's focus the moment a click *completes*
    // while that widget is not the hovered one — which the panel never is, since the row or the field
    // under the pointer is registered over the top of it. A press therefore claimed the keyboard and
    // the matching release, a frame later, took it back: one click in the console, and the keys went
    // to the file listing again.
    //
    // At the end, so that nothing drawn inside this panel can un-focus it afterwards.
    //
    // This is also why the first version of it passed its own test and failed in the user's hands.
    // The test clicked the empty space under the last row — the one place in the panel where there is
    // no inner widget to take the click and no row to be hovered instead.
    if let Some(at) = child.input(|i| {
        i.pointer
            .any_pressed()
            .then(|| i.pointer.interact_pos())
            .flatten()
    }) {
        // `band` as well as `inside`: the resize grip reaches a couple of points *above* the panel, and
        // grabbing your own border is not leaving. Without it, dragging the top edge handed the
        // keyboard back to the listing — so the `Ctrl+C` after a resize reported "Nothing selected"
        // from the file listing instead of doing anything in the console.
        state.owns = inside.contains(at) || band.contains(at);
        out.claimed = state.owns;
    }
    // **Asserted only when it is not already ours**, and that condition is the whole of the fix for a
    // one-frame flash of the file listing every time `Up` walked the history.
    //
    // `Memory::request_focus` builds the focus record from scratch, and a fresh record carries the
    // *default* [`EventFilter`] — every field false. So re-stating a claim already held threw away the
    // lock installed at the top of this function, every frame, before egui had ever read a key through
    // it. `Focus::begin_pass` then saw a bare `Up` as "move the focus up", `Focus::end_pass` handed the
    // keyboard to whichever rect it found in that direction — the resize grips, the sidebar's, the
    // listing's own hit area — and the panel took it back the frame after. One frame with the keyboard
    // somewhere else is one frame of the selected file drawn as though the listing had it back, which
    // is exactly what was on screen. `Tab`, `Left`, `Right` and `Escape` were all doing the same thing.
    if state.owns {
        if !child.memory(|m| m.has_focus(me)) {
            child.memory_mut(|m| m.request_focus(me));
        }
    } else if child.memory(|m| m.has_focus(me)) {
        child.memory_mut(|m| m.surrender_focus(me));
    }

    out
}

/// What a font measures, taken off one laid-out glyph.
#[derive(Clone, Copy)]
struct Metrics {
    /// A row's height, rounded up to a whole **device** pixel.
    ///
    /// The rounding is not cosmetic. Rows are placed at `top + n * row`, so a fractional pitch puts
    /// every row on a different subpixel phase — and epaint rounds a galley's baseline to a whole
    /// pixel relative to the galley's own origin, so each row's text gets rounded a different way.
    /// The result is a column of sharp text that visibly does not sit on one line, which is what
    /// this panel looked like until it was measured. A whole-pixel pitch plus [`crate::ui::snap`] at
    /// each origin gives every row the same phase and therefore the same rounding.
    row: f32,
    /// One column's width, taken off the glyph rather than off the galley.
    ///
    /// **`Galley::size()` is rounded to whole pixels** — `round_output_to_gui` does it so that a
    /// widget measured from text lands on the grid — and a rounded advance is wrong by a fraction of
    /// a pixel *per column*. Consolas advances 8.4 and the galley reports 8, so by column twenty the
    /// arithmetic is a whole character out and by column forty it is two: the highlight stops
    /// agreeing with the glyphs, the tail of a long line cannot be reached at all, and dragging
    /// slowly moves the selection in jumps. `Glyph::advance_width` is the unrounded number.
    advance: f32,
    /// How far **above** the middle of a row the ink of text in it sits.
    lift: f32,
}

/// Measure a font.
///
/// `lift` is the one worth explaining, and it is the whole of a complaint that has been made about
/// this panel twice. A row box is ascent *plus descent*, and a digit's ink stops at the baseline —
/// so the ink of a line of text sits above the middle of the box holding it, by about half the
/// descent. A chevron centred in the same box is therefore centred on nothing the eye is looking at,
/// and reads low.
///
/// Measured rather than nudged by a constant: a laid-out glyph carries its baseline in `pos` and its
/// ink's own offset from that baseline in `uv_rect`, so this is the font's answer and not a taste.
fn metrics(ui: &Ui, font: &egui::FontId) -> Metrics {
    let probe = ui
        .painter()
        .layout_no_wrap("0".to_owned(), font.clone(), egui::Color32::PLACEHOLDER);
    let ppp = ui.ctx().pixels_per_point();
    let row = (probe.size().y.max(1.0) * ppp).ceil() / ppp;
    let glyph = probe
        .rows
        .first()
        .and_then(|placed| placed.row.glyphs.first().map(|glyph| (placed.pos.y, glyph)));
    let ink = glyph.map_or(row * 0.5, |(top, glyph)| {
        top + glyph.pos.y + glyph.uv_rect.offset.y + glyph.uv_rect.size.y * 0.5
    });
    Metrics {
        row,
        advance: glyph.map_or(row * 0.5, |(_, glyph)| glyph.advance_width).max(1.0),
        lift: row * 0.5 - ink,
    }
}

/// Where to put a line of text — with `Align2::LEFT_TOP` — so that its **ink** ends up in the middle
/// of `rect` rather than its box.
///
/// A galley's box is ascent plus descent, and the ink of a line of text stops at the baseline: laid
/// flush in a row, all of the slack ends up underneath it and the text reads as stuck to the top of
/// its row. Measured here, that was ten pixels of ink at the top of a sixteen-point row with six
/// empty ones below — which is exactly what it looked like.
///
/// This is [`Metrics::lift`]'s only job, and doing it once here is what lets everything *beside* the
/// text — a chevron, a note — simply centre in the row through [`crate::ui::icon_rect`] and land on
/// it. Centre the ink, and the box centre becomes the right answer for everything else.
fn ink_top(rect: Rect, m: &Metrics) -> f32 {
    rect.center().y - m.row * 0.5 + m.lift
}

/// How wide a string is in a font, for laying a strip out before drawing it.
fn width(ui: &Ui, font: &egui::FontId, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
        .size()
        .x
}

/// The drag handle on the seam above the panel.
#[allow(clippy::too_many_arguments)]
fn grip(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    band: Rect,
    pane: PaneId,
    body: Rect,
    share: f32,
    out: &mut Outcome,
) {
    let response = ui.interact(band, Id::new(("console-grip", pane)), Sense::drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeRow);
    }
    if response.dragged() {
        let by = response.drag_delta().y;
        if by != 0.0 && body.height() > 1.0 {
            let taken = (body.height() * share - by) / body.height();
            out.share = Some(taken.clamp(0.05, 0.95));
        }
    }
    if response.hovered() || response.dragged() {
        ui.painter().rect_filled(
            Rect::from_min_max(
                pos2(rect.left(), rect.top()),
                pos2(rect.right(), rect.top() + SEAM),
            ),
            CornerRadius::ZERO,
            t.accent.default,
        );
    }
}

/// The scrollable log.
#[allow(clippy::too_many_arguments)]
fn log(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    state: &mut State,
    blocks: &mut [Block],
    gone: bool,
    // The resize grip's band, which overlaps the first row. A press that started there is the border
    // being dragged, not text being selected — and it stays excluded for the whole drag, because the
    // press origin does not move even after the pointer has travelled down into the log.
    band: Rect,
) {
    let font = t.fonts.mono.clone();
    // All three off the *same* galley on purpose: the height a row is placed at and the height the
    // text inside it is centred in have to be one number, and asking two questions is how they come
    // to differ by a rounding. `0` rather than a space, because a space is the one glyph a font is
    // allowed to give a different advance to.
    let m = metrics(ui, &font);
    let (row_h, advance) = (m.row, m.advance);
    let count = state.rows();
    // The wrap width, decided in `show` before the index was built. `None` means lines run off to the
    // right instead.
    let cols = state.cols();
    // What a scrollbar takes: the design system's width plus its margins, asked of the style rather
    // than named here so the gutter is the one egui is actually going to reserve.
    let bar = ui.spacing().scroll.allocated_width().max(1.0);

    if count == 0 {
        let (text, ink) = if gone {
            ("the shell has gone — Shift+Tab to start another", t.status.danger)
        } else {
            ("type a command", t.text.tertiary)
        };
        crate::ui::text_center(ui.painter(), rect, font, ink, text);
        return;
    }

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    // See the module header: `show_rows` reserves `row_height + item_spacing.y` per row, and rows
    // painted at exact rects need the spacing to be nothing or they drift out of their own boxes.
    child.spacing_mut().item_spacing = Vec2::ZERO;

    let mut scroll = egui::ScrollArea::both()
        .id_salt(("console-log", pane))
        .auto_shrink([false, false]);
    // Wrapping has just been turned on, and a log left scrolled sideways when there is no longer
    // anywhere sideways to be looks empty.
    if std::mem::take(&mut state.pan) {
        scroll = scroll.horizontal_scroll_offset(0.0);
    }
    // **What the foot of the log is held back for, which while lines wrap is nothing.**
    //
    // Unwrapped, a horizontal scrollbar can arrive at any moment — the moment a long line does — so a
    // bar's height is kept clear at the bottom whether or not one is showing, and the newest line
    // stays above where the bar goes rather than under it. Wrapped, there is no such moment: the
    // content is exactly as wide as the page and a horizontal bar can never appear. So the log reaches
    // the panel's bottom edge, and so does the fade that says there is more below it.
    let foot = if cols.is_some() { 0.0 } else { bar };
    let view = rect.height() - foot;
    let reach = (count as f32 * row_h - view).max(0.0);
    if let Some(row) = state.reveal.take() {
        // Nudged into view rather than centred, and against last frame's offset because
        // `ScrollArea` takes an absolute one — the same discipline as the listing's cursor.
        let top = row as f32 * row_h;
        let mut offset = state.offset;
        if top < offset {
            offset = top;
        } else if top + row_h > offset + view {
            offset = top + row_h - view;
        }
        scroll = scroll.vertical_scroll_offset(offset.clamp(0.0, reach));
    } else if state.tail {
        scroll = scroll.vertical_scroll_offset(reach);
    }

    // **The rows that exist, and one more of nothing while a bar can still arrive.** The slack is what
    // lets the last line of output be scrolled clear of a horizontal scrollbar — without it the bar
    // appears with the long line that caused it and sits on top of the very line you scrolled down to
    // read. Wrapped, no bar can appear, so there is nothing to be clear of and the extra row would only
    // be a strip of nothing under the last line. The listing keeps slack for its own reason; see its
    // `TAIL_ROWS`.
    let slack = usize::from(cols.is_none());
    let output = scroll.show_rows(&mut child, row_h, count + slack, |ui, range| {
        let range = range.start.min(count)..range.end.min(count);
        let first = range.start;
        // The panned origin: `show_rows` hands out a `Ui` already translated by the scroll offset,
        // so this is where row `first` column `0` actually lands.
        let origin = ui.min_rect().min;
        let text_x = origin.x + PAD + GUTTER;

        // The visible rows, and their text, taken once: the same strings are measured, hit-tested
        // and painted, and the borrow of `blocks` has to end before a click can fold one.
        let seen: Vec<(Row, String)> = range
            .clone()
            .filter_map(|at| {
                state
                    .row(blocks, at)
                    .map(|row| (row, shown(blocks, row, cols).into_owned()))
            })
            .collect();
        let widest = seen
            .iter()
            .map(|(_, text)| text.chars().count())
            .max()
            .unwrap_or(0);
        // What the horizontal scrollbar reaches: the widest line *on screen*, which grows as one
        // comes into view and is therefore always enough to read what is showing. Wrapped, there is
        // nowhere sideways to go and the content is exactly the page.
        let content = match cols {
            Some(cols) => cols,
            None => widest,
        };
        ui.set_min_width(PAD * 2.0 + GUTTER + content as f32 * advance);

        let visible = Rect::from_min_max(
            pos2(rect.left(), origin.y),
            pos2(rect.right(), origin.y + seen.len() as f32 * row_h),
        );
        // **The page: what is actually readable, scrollbars excluded.**
        //
        // `ui.clip_rect()` inside `show_rows` is the scroll area's *viewport* — the rect left after
        // egui reserved a gutter for whichever bars it is showing. Painting and hit-testing against
        // the panel's own rect instead is what made the bars behave like text: the pointer over one
        // showed a caret, and dragging one dragged a selection along with the scroll.
        //
        // And a gutter is reserved on whichever axis egui did *not* reserve one, so the width of the
        // text does not change when a bar appears — a bar arriving is a long line arriving, which is
        // the worst moment to reflow everything. Sideways, always: a vertical bar comes and goes with
        // the length of the log. Downwards, only while lines run off to the right — see [`foot`], which
        // is nothing while they wrap, and then the page is the panel down to its bottom edge.
        let page = {
            let seen = ui.clip_rect().intersect(rect);
            Rect::from_min_max(
                seen.min,
                pos2(
                    if seen.right() > rect.right() - bar * 0.5 {
                        seen.right() - bar
                    } else {
                        seen.right()
                    },
                    if seen.bottom() > rect.bottom() - foot * 0.5 {
                        seen.bottom() - foot
                    } else {
                        seen.bottom()
                    },
                ),
            )
        };
        // **Clipped to the page, because `visible` is not.** `show_rows` places the first laid-out row
        // wherever the scroll offset puts it, which is up to one row *above* the panel — so the rows'
        // rect reaches into the listing overhead, and a press there was taken for a press on a row.
        // That is how dragging the resize border came to select text again after the border had moved
        // out from under the pointer: the origin was no longer in the grip's band and still in this.
        let hit = visible.intersect(page);
        // One interaction for the whole visible block, and the row worked out from the pointer —
        // the listing's argument, and the same saving: no id, hit-test or hover slot per row. The
        // response itself is not read — every gesture here comes off the raw pointer, because the
        // ones this panel needs are reported a frame later than it needs them.
        let _rows = ui.interact(hit, Id::new(("console-rows", pane)), Sense::click_and_drag());
        if !state.resizing
            && ui
                .input(|i| i.pointer.latest_pos())
                .is_some_and(|at| hit.contains(at) && !band.contains(at))
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }

        let spot = |at: Pos2| -> Spot {
            let slot = (((at.y - origin.y) / row_h).floor().max(0.0) as usize)
                .min(seen.len().saturating_sub(1));
            let col = ((at.x - text_x) / advance).round().max(0.0) as usize;
            let len = seen.get(slot).map_or(0, |(_, text)| text.chars().count());
            Spot {
                row: first + slot,
                col: col.min(len),
            }
        };
        // The run of like characters a spot falls in, for a word-mode drag.
        let word = |spot: Spot| -> std::ops::Range<usize> {
            let slot = spot.row.saturating_sub(first);
            let chars: Vec<char> = seen
                .get(slot)
                .map(|(_, text)| text.chars().collect())
                .unwrap_or_default();
            word_at(&chars, spot.col)
        };

        // **Where the button went down, for as long as it is down.** `press_origin` rather than
        // `drag_started`, which is the bug this replaces: egui only calls a press a drag once it
        // has *travelled*, and by then the pointer is a row or two along — so the selection
        // anchored on the wrong row, or never anchored at all on a short drag.
        let held = ui.input(|i| {
            i.pointer
                .primary_down()
                .then(|| i.pointer.press_origin())
                .flatten()
        });
        let held = held.filter(|_| !state.resizing);
        if let Some(from) = held.filter(|at| hit.contains(*at) && !band.contains(*at)) {
            if from.x < origin.x + PAD + FOLD + space::S1 {
                // The gutter: a fold, on the press rather than on the release, and only for a
                // header that has something to hide.
                if ui.input(|i| i.pointer.primary_pressed()) {
                    let at = spot(from).row;
                    if let Some(Row::Head(which, _)) = state.row(blocks, at) {
                        let block = &mut blocks[which];
                        if !block.lines.is_empty() || block.dropped > 0 {
                            block.collapsed = !block.collapsed;
                        }
                        state.aim = Aim::Block(block.id);
                    }
                }
            } else {
                // A double click puts the drag in word mode for as long as the button is held, and a
                // double click on its own therefore selects exactly one word. Decided on the press:
                // `Response::double_clicked` is reported on the release, which is a frame after the
                // drag it would have to govern has already started.
                if ui.input(|i| i.pointer.primary_pressed()) {
                    let now = ui.input(|i| i.time);
                    let delay = ui
                        .ctx()
                        .options(|o| o.input_options.max_double_click_delay);
                    let again = state
                        .last_press
                        .is_some_and(|(when, at)| now - when <= delay && at.distance(from) < 8.0);
                    state.grab = if again { Grab::Word } else { Grab::Char };
                    state.last_press = Some((now, from));
                }
                let to = ui.input(|i| i.pointer.latest_pos()).map_or(from, |at| at);
                let (mut a, mut b) = (spot(from), spot(to));
                if state.grab == Grab::Word {
                    // Both ends out to their own word's edges, in whichever direction the drag is
                    // going — which is what makes a double click alone select exactly one word.
                    let (near, far) = if a <= b { (&mut a, &mut b) } else { (&mut b, &mut a) };
                    near.col = word(*near).start;
                    far.col = word(*far).end;
                }
                state.aim = Aim::Text(Span::new(a, b));
            }
        }

        let picked = match state.aim {
            Aim::Block(id) => Some(id),
            _ => None,
        };
        let span = match state.aim {
            Aim::Text(span) if !span.empty() => Some(span),
            _ => None,
        };
        let fill = t.bg.layer;
        let wash = crate::ui::row_fill(t, true, false);
        let ink_selected = ui.visuals().selection.bg_fill;
        // Clipped to the page, so a long line stops where the vertical scrollbar starts rather than
        // running the width of the viewport, which is a whole bar wider.
        let paint = ui.painter_at(page);

        for (slot, (row, text)) in seen.iter().enumerate() {
            let at = first + slot;
            let box_ = Rect::from_min_size(
                pos2(page.left(), origin.y + slot as f32 * row_h),
                vec2(page.width(), row_h),
            );
            // **The blank row above a block is not part of it to look at.** It carries the block's
            // index so the index arithmetic has one owner for every row, but a picked block's fill
            // starts at its header — a selection that begins one row early reads as a gap in the
            // selection rather than as air between blocks.
            let air = matches!(row, Row::Gap(_));
            let mine = !air && picked.is_some_and(|id| blocks[row.block()].id == id);

            // A header reads as a band, which is what gives the log its blocks without a rule or a
            // colour per command; a picked block wears the listing's own selected fill instead.
            match (matches!(row, Row::Head(..)), mine) {
                (_, true) => {
                    if let Some(wash) = wash {
                        paint.rect_filled(box_, CornerRadius::ZERO, wash);
                    }
                }
                (true, false) => {
                    paint.rect_filled(box_, CornerRadius::ZERO, fill);
                }
                _ => {}
            }
            if mine {
                crate::ui::selection_bar(&paint, box_, t);
            }

            let chars = text.chars().count();
            if let Some(cut) = span.and_then(|span| span.cut(at, chars)) {
                let from = text_x + cut.start as f32 * advance;
                let to = text_x + cut.end as f32 * advance;
                paint.rect_filled(
                    Rect::from_min_max(pos2(from, box_.top()), pos2(to, box_.bottom())),
                    CornerRadius::ZERO,
                    ink_selected,
                );
            }

            if let Row::Head(which, _) = row {
                let block = &blocks[*which];
                // **Always on a header, and now it is the only mark there is.** With the accent `>`
                // gone it is what says "this row is a command", so a block with nothing to fold keeps
                // it too — drawn in `text-disabled`, which is the same thing every tree says about a
                // twisty with no children.
                let folds = !block.lines.is_empty() || block.dropped > 0;
                // Centred in the row, plainly: the text in it has its own ink centred there, so this
                // is centred on the text as well.
                let chevron = crate::ui::icon_rect(box_, origin.x + PAD, FOLD);
                let glyph = if block.collapsed || !folds {
                    azur_egui_theme::icons::chevron_right
                } else {
                    azur_egui_theme::icons::chevron_down
                };
                let quiet = match (folds, mine) {
                    (false, _) => t.text.disabled,
                    (true, true) => t.text.primary,
                    (true, false) => t.text.tertiary,
                };
                glyph(&paint, chevron, quiet);
            }

            // Only two things are marked at all — a command that is still going and one that
            // failed — so the two worth noticing are the only marks on the panel.
            let ink = match row {
                Row::Head(which, _) if blocks[*which].failed() => t.status.danger,
                Row::Cut(_) => t.text.tertiary,
                Row::Text(which, line, _) if blocks[*which].lines[*line].err => t.status.danger,
                // **Output is `text-primary`, the same as the command above it.** It is the thing
                // being read; greying it to tell it apart from its header is the wrong way round,
                // and the header has a band, a mark and a chevron already.
                Row::Gap(_) | Row::Head(..) | Row::Text(..) => t.text.primary,
            };
            // **Snapped, and placed by its ink.** Two separate corrections in one line, both of
            // which this panel has been wrong about:
            //
            // - `Align2::LEFT_CENTER` at the row's middle put each origin half a fractional row
            //   height along, and epaint rounds a baseline to a whole pixel relative to the galley's
            //   own origin — so every row got rounded a different way and the column of text
            //   visibly did not sit on one line. A whole-pixel pitch and a snapped origin give every
            //   row the same phase and therefore the same rounding.
            // - Flush at the row's top left all of the slack under the text. See [`ink_top`].
            let at = crate::ui::snap(&paint, pos2(text_x, ink_top(box_, &m)));
            paint.text(at, Align2::LEFT_TOP, text, font.clone(), ink);

            if let Row::Head(which, _) = row {
                let block = &blocks[*which];
                let note = if block.running() {
                    Some(("running".to_owned(), t.accent.default))
                } else {
                    block
                        .code
                        .filter(|code| *code != 0)
                        .map(|code| (format!("exit {code}"), t.status.danger))
                };
                if let Some((note, ink)) = note {
                    let caption = t.fonts.caption.clone();
                    let cm = metrics(ui, &caption);
                    paint.text(
                        pos2(page.right() - PAD, ink_top(box_, &cm)),
                        Align2::RIGHT_TOP,
                        note,
                        caption,
                        ink,
                    );
                }
            }
        }
        // Handed back out, because the fades are drawn after the scroll area has closed and they have
        // to sit inside the same page the rows did.
        page
    });

    state.offset = output.state.offset.y;
    // Stuck to the bottom, and only the scroll position says so: a command sends it back down, and a
    // drag of the scrollbar takes it off again, with no flag to get out of step. Against `reach`
    // rather than against the content, because unwrapped the content carries a row of slack the log
    // never scrolls into.
    state.tail = output.state.offset.y >= reach - row_h * 0.5;

    // **There is more above, and there is more below** — the design system's rule for the edge of any
    // scrolling collection, and the log is one. See `azur_egui_theme::components::scroll_fades`, which
    // is where the fade itself, its depth and the argument for it live now.
    //
    // Its own two figures rather than the `ScrollArea`'s: unwrapped, this log reserves a row of trailing
    // slack past its last line, and measuring the bottom off the content would fade for slack nobody
    // put anything in. `output.inner` is the *page* — the viewport with the scrollbars taken off and,
    // while lines run off to the right, a bar's height held back at the foot for one that has not
    // arrived yet — worked out inside the closure, and not `inner_rect`, which knows about neither.
    let above = output.state.offset.y;
    azur_egui_theme::components::scroll_fades(
        &ui.painter_at(output.inner),
        output.inner,
        t.bg.canvas,
        above,
        (reach - above).max(0.0),
    );
}

/// The prompt strip: the shell button, the line, and Run.
#[allow(clippy::too_many_arguments)]
fn prompt(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    state: &mut State,
    hot: bool,
    running: bool,
    out: &mut Outcome,
) {
    let font = t.fonts.mono.clone();
    let line_metrics = metrics(ui, &font);
    let advance = line_metrics.advance;

    // Sized for the widest label, so the line does not shift when the shell changes.
    let label = t.fonts.caption.clone();
    let label_metrics = metrics(ui, &label);
    let widest = Kind::ALL
        .iter()
        .map(|kind| width(ui, &label, kind.label()))
        .fold(0.0_f32, f32::max);
    // Padding, the label, a gap, the chevron, padding — spelled out, because the version that
    // guessed one number for the whole lot printed `bash^` with the caret against the `h`.
    let shell = Rect::from_min_size(
        pos2(rect.left() + PAD, rect.center().y - control::SMALL * 0.5),
        vec2(
            space::S3 + widest + space::S2 + FOLD + space::S3,
            control::SMALL,
        ),
    );
    let run = Rect::from_min_size(
        pos2(
            rect.right() - PAD - control::SMALL,
            rect.center().y - control::SMALL * 0.5,
        ),
        Vec2::splat(control::SMALL),
    );
    let field = Rect::from_min_max(
        pos2(shell.right() + space::S3, rect.top()),
        pos2(run.left() - space::S3, rect.bottom()),
    );

    // -- the shell button --------------------------------------------------
    let picker = ui.interact(shell, Id::new(("console-shell", pane)), Sense::click());
    if picker.clicked() {
        state.picking = !state.picking;
    }
    if picker.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    // The shortcut in brackets comes out in `text-secondary` on its own: the design system's tooltip
    // recognises a trailing `(…)` and sets it as an aside, which is the same two colours every other
    // tooltip in this window says it in.
    azur_egui_theme::components::tooltip(picker.clone(), "Change shell (Shift+Tab)");
    // **Revealed on hover, like Run beside it.** No resting fill: the strip is one surface with two
    // controls sitting on it, and a filled box around the shell name made it the loudest thing in a
    // panel whose whole job is the text above it. The name alone says which shell; the box only has
    // to appear when the pointer is looking for something to press.
    let (hover, pressed) = control_fills(t, t.bg.canvas);
    let fill = if state.picking || picker.is_pointer_button_down_on() {
        Some(pressed)
    } else if picker.hovered() {
        Some(hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter()
            .rect_filled(shell, CornerRadius::same(radius::SMALL), fill);
    }
    ui.painter().text(
        pos2(shell.left() + space::S3, ink_top(shell, &label_metrics)),
        Align2::LEFT_TOP,
        state.kind.label(),
        label.clone(),
        t.text.primary,
    );
    azur_egui_theme::icons::chevron_up(
        ui.painter(),
        crate::ui::icon_rect(shell, shell.right() - space::S3 - FOLD, FOLD),
        t.text.tertiary,
    );

    // -- Run, which is Stop while something is going -----------------------
    let button = ui.interact(run, Id::new(("console-run", pane)), Sense::click());
    if button.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    if button.clicked() {
        if running {
            out.stop = true;
        } else {
            state.run(out);
        }
    }
    if button.hovered() || button.is_pointer_button_down_on() {
        let fill = if button.is_pointer_button_down_on() {
            pressed
        } else {
            hover
        };
        ui.painter()
            .rect_filled(run, CornerRadius::same(radius::SMALL), fill);
    }
    azur_egui_theme::components::tooltip(
        button.clone(),
        if running {
            "Stop the command (Ctrl+C)"
        } else {
            "Launch command (Enter)"
        },
    );
    let glyph = if running {
        crate::icons::stop
    } else {
        crate::icons::play
    };
    // **The glyph has to lift when the fill arrives under it.** An empty line greys it to
    // `text-disabled`, and `text-disabled` on `background-control-hover` is two rungs of the same
    // neutral ramp — so hovering Run made Run disappear, which is the opposite of what a hover is for.
    let ink = if running {
        t.status.danger
    } else if state.line.is_empty() {
        if button.hovered() {
            t.text.secondary
        } else {
            t.text.disabled
        }
    } else {
        t.accent.default
    };
    glyph(
        ui.painter(),
        crate::ui::icon_rect(run, run.left(), control::SMALL),
        ink,
    );

    // -- the line ----------------------------------------------------------
    //
    // Not a text field: the same surface as the log above it, and a border only under the pointer.
    // A box drawn around it would make the panel two things stacked up rather than one.
    let strip = ui.interact(field, Id::new(("console-field", pane)), Sense::click_and_drag());
    // A text cursor and nothing else. The border that used to appear here made the line look like a
    // field that had grown out of the panel; the caret and the cursor already say it is somewhere you
    // can type, and the strip is one surface with the log above it.
    if strip.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    let text_x = field.left() + space::S2;
    if let Some(from) = ui.input(|i| {
        i.pointer
            .primary_down()
            .then(|| i.pointer.press_origin())
            .flatten()
    }) {
        if field.contains(from) {
            let col = |at: Pos2| ((at.x - text_x) / advance).round().max(0.0) as usize;
            let to = ui.input(|i| i.pointer.latest_pos()).unwrap_or(from);
            state.aim = Aim::Prompt;
            state.edited = ui.input(|i| i.time);
            if ui.input(|i| i.pointer.primary_pressed()) {
                state.line.seek(col(from), false);
            } else {
                state.line.seek(col(to), true);
            }
        }
    }

    let caret_x = text_x + state.line.col() as f32 * advance;
    if let Some(span) = state.line.span() {
        let from = state.line.text()[..span.start].chars().count() as f32;
        let to = state.line.text()[..span.end].chars().count() as f32;
        ui.painter().rect_filled(
            Rect::from_min_max(
                pos2(text_x + from * advance, field.top() + space::S1),
                pos2(text_x + to * advance, field.bottom() - space::S1),
            ),
            CornerRadius::ZERO,
            ui.visuals().selection.bg_fill,
        );
    }
    ui.painter().text(
        crate::ui::snap(ui.painter(), pos2(text_x, ink_top(field, &line_metrics))),
        Align2::LEFT_TOP,
        state.line.text(),
        font,
        t.text.primary,
    );
    if hot {
        // **egui's own caret**, colour, width, blink and all — `visuals.text_cursor` is where a text
        // field's caret comes from, and this window has one caret whatever is drawing the text under
        // it. It also schedules the repaint the blink needs, so the panel is still asleep between
        // them rather than running at the refresh rate.
        //
        // The phase is measured from the last edit, not from the epoch: a caret that carries on
        // blinking through what you are typing is the tell of one that is on a timer rather than on
        // the text.
        let since = ui.input(|i| i.time) - state.edited;
        egui::text_selection::visuals::paint_text_cursor(
            ui,
            ui.painter(),
            Rect::from_center_size(
                pos2(caret_x.round(), field.center().y),
                vec2(0.0, CARET),
            ),
            since,
        );
    }

    if state.picking {
        shells(ui, t, shell, pane, state, out);
    }
}

/// The shell dropdown, opening upwards out of its button.
fn shells(ui: &mut Ui, t: &Theme, anchor: Rect, pane: PaneId, state: &mut State, out: &mut Outcome) {
    let row = control::SMALL;
    let size = vec2(
        anchor.width().max(96.0),
        row * Kind::ALL.len() as f32 + space::S2 * 2.0,
    );
    let at = pos2(anchor.left(), anchor.top() - size.y - space::S1);
    let area = egui::Area::new(Id::new(("console-shells", pane)))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ui.ctx(), |ui| {
            let rect = Rect::from_min_size(at, size);
            // `bg.layer_alt`, not `bg.overlay`. The overlay role is a *scrim* — a translucent wash
            // over the window while something modal is up — so a menu wearing it had the log showing
            // through its own rows. This is the surface the design system's tooltip and menu sit on.
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius::MEDIUM), t.bg.layer_alt);
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(radius::MEDIUM),
                Stroke::new(1.0, t.stroke.subtle),
                StrokeKind::Inside,
            );
            for (slot, kind) in Kind::ALL.iter().enumerate() {
                let item = Rect::from_min_size(
                    pos2(rect.left(), rect.top() + space::S2 + slot as f32 * row),
                    vec2(rect.width(), row),
                );
                let response =
                    ui.interact(item, Id::new(("console-shell-item", pane, slot)), Sense::click());
                if response.hovered() {
                    ui.painter()
                        .rect_filled(item, CornerRadius::ZERO, crate::ui::hover_fill(t));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                }
                if *kind == state.kind {
                    crate::ui::selection_bar(ui.painter(), item, t);
                }
                let caption = t.fonts.caption.clone();
                let cm = metrics(ui, &caption);
                ui.painter().text(
                    pos2(item.left() + space::S3, ink_top(item, &cm)),
                    Align2::LEFT_TOP,
                    kind.label(),
                    caption,
                    t.text.primary,
                );
                if response.clicked() {
                    state.picking = false;
                    if *kind != state.kind {
                        state.kind = *kind;
                        out.swap = Some(*kind);
                    }
                }
            }
            rect
        });

    // Dismissed by a press anywhere else — including on the button, which is what makes a second
    // click on it close the menu rather than reopen it. The press that *opened* it is inside the
    // button, so it is excluded here and the menu survives its own opening click.
    if let Some(at) = ui.input(|i| {
        i.pointer
            .any_pressed()
            .then(|| i.pointer.interact_pos())
            .flatten()
    }) {
        if !area.inner.contains(at) && !anchor.contains(at) {
            state.picking = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block with an id and some output, without a shell.
    fn block(id: u64, command: &str, lines: &[&str]) -> Block {
        Block {
            command: command.to_owned(),
            lines: lines
                .iter()
                .map(|text| crate::console::Line {
                    text: (*text).to_owned(),
                    err: false,
                })
                .collect(),
            code: Some(0),
            dropped: 0,
            collapsed: false,
            id,
        }
    }

    fn press(key: Key, m: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: m,
        }
    }

    /// Push a frame's worth of events through, and say what came out.
    ///
    /// The modifiers are taken from the last key in the batch, which is what a real frame's state
    /// amounts to for a single keystroke.
    fn feed(state: &mut State, blocks: &mut Vec<Block>, events: Vec<egui::Event>) -> Outcome {
        let mut out = Outcome::default();
        let mut events = events;
        state.rebuild(blocks, state.cols());
        let held = events
            .iter()
            .rev()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or(Modifiers::NONE);
        state.keys(&mut events, held, blocks, &mut out);
        out
    }

    const SHIFT: Modifiers = Modifiers::SHIFT;
    const CTRL: Modifiers = Modifiers::COMMAND;

    // -- the line ----------------------------------------------------------

    #[test]
    fn typing_goes_in_and_enter_sends_it() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        let out = feed(
            &mut state,
            &mut blocks,
            vec![
                egui::Event::Text("git ".to_owned()),
                egui::Event::Text("status".to_owned()),
                press(Key::Enter, Modifiers::NONE),
            ],
        );
        assert_eq!(out.send.as_deref(), Some("git status"));
        // And the line is empty again, ready for the next one.
        assert_eq!(state.line.text(), "");
    }

    #[test]
    fn an_empty_line_is_not_a_command() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        let out = feed(
            &mut state,
            &mut blocks,
            vec![
                egui::Event::Text("   ".to_owned()),
                press(Key::Enter, Modifiers::NONE),
            ],
        );
        assert_eq!(out.send, None);
        assert!(state.history.is_empty());
    }

    #[test]
    fn the_caret_walks_words_and_characters() {
        let mut editor = Editor::default();
        editor.insert("git commit --amend");
        editor.left(false, true);
        assert_eq!(editor.caret, "git commit ".len(), "one word back");
        editor.left(false, true);
        assert_eq!(editor.caret, "git ".len(), "and another");
        editor.right(false, false);
        assert_eq!(editor.caret, "gitc".len() + 1, "one character forward");
    }

    #[test]
    fn a_selection_is_replaced_by_what_is_typed() {
        let mut editor = Editor::default();
        editor.insert("cargo build");
        editor.home(false);
        editor.right(true, true);
        assert_eq!(editor.selected(), "cargo ");
        editor.insert("rustc ");
        assert_eq!(editor.text(), "rustc build");
    }

    #[test]
    fn a_caret_at_a_selection_collapses_to_its_edge() {
        let mut editor = Editor::default();
        editor.insert("abcdef");
        editor.all();
        editor.left(false, false);
        assert_eq!(editor.caret, 0, "to the near end, not one back from it");
        editor.all();
        editor.right(false, false);
        assert_eq!(editor.caret, 6, "and the far one going the other way");
    }

    /// Byte offsets and character columns are not the same number, and a caret that confuses them
    /// panics on the next edit rather than merely looking wrong.
    #[test]
    fn the_line_survives_characters_wider_than_a_byte() {
        let mut editor = Editor::default();
        editor.insert("écho café");
        assert_eq!(editor.col(), 9, "nine characters, eleven bytes");
        editor.backspace();
        assert_eq!(editor.text(), "écho caf");
        editor.home(false);
        editor.right(false, false);
        assert_eq!(editor.caret, 2, "past a two-byte character");
        editor.delete();
        assert_eq!(editor.text(), "ého caf");
    }

    // -- the history -------------------------------------------------------

    #[test]
    fn up_and_down_walk_the_history_and_come_back() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        for command in ["one", "two"] {
            feed(
                &mut state,
                &mut blocks,
                vec![
                    egui::Event::Text(command.to_owned()),
                    press(Key::Enter, Modifiers::NONE),
                ],
            );
        }
        feed(&mut state, &mut blocks, vec![egui::Event::Text("half".to_owned())]);
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, Modifiers::NONE)]);
        assert_eq!(state.line.text(), "two");
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, Modifiers::NONE)]);
        assert_eq!(state.line.text(), "one");
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, Modifiers::NONE)]);
        assert_eq!(state.line.text(), "one", "and it stops at the oldest");
        feed(&mut state, &mut blocks, vec![press(Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(state.line.text(), "two");
        feed(&mut state, &mut blocks, vec![press(Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(state.line.text(), "half", "the half-typed line comes back");
    }

    #[test]
    fn the_same_command_twice_is_one_entry() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        for _ in 0..3 {
            feed(
                &mut state,
                &mut blocks,
                vec![
                    egui::Event::Text("make".to_owned()),
                    press(Key::Enter, Modifiers::NONE),
                ],
            );
        }
        assert_eq!(state.history, vec!["make".to_owned()]);
    }

    // -- the shell ---------------------------------------------------------

    /// The one that has been reported broken three times. It is a plain `Shift+Tab` press with no
    /// exact-modifier match and no `consume_key` anywhere near it.
    #[test]
    fn shift_tab_walks_the_shells_round() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        assert_eq!(state.kind(), Kind::Bash);
        for expected in [Kind::PowerShell, Kind::Cmd, Kind::Bash] {
            let out = feed(&mut state, &mut blocks, vec![press(Key::Tab, SHIFT)]);
            assert_eq!(state.kind(), expected);
            assert_eq!(out.swap, Some(expected), "and the caller is told to swap");
        }
    }

    /// Both halves of the reason the focus filter exists: a `Tab` of either kind is *ours*, so
    /// neither can reach egui's focus navigation and take the keyboard out of the panel.
    #[test]
    fn no_tab_of_any_kind_escapes_the_panel() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        let mut events = vec![press(Key::Tab, Modifiers::NONE), press(Key::Tab, SHIFT)];
        let mut out = Outcome::default();
        state.keys(&mut events, Modifiers::NONE, &mut blocks, &mut out);
        assert!(events.is_empty(), "both were consumed");
    }

    // -- the blocks --------------------------------------------------------

    #[test]
    fn shift_up_picks_blocks_from_the_newest_back() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "one", &["a"]), block(2, "two", &["b"])];
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
        assert_eq!(state.aim, Aim::Block(2), "the newest first");
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
        assert_eq!(state.aim, Aim::Block(1));
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
        assert_eq!(state.aim, Aim::Block(1), "and stops at the oldest");
        feed(&mut state, &mut blocks, vec![press(Key::ArrowDown, SHIFT)]);
        assert_eq!(state.aim, Aim::Block(2));
        feed(&mut state, &mut blocks, vec![press(Key::ArrowDown, SHIFT)]);
        assert_eq!(state.aim, Aim::Prompt, "walking off the end is the way out");
    }

    #[test]
    fn a_picked_block_folds_with_left_and_right() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "ls", &["a", "b"])];
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
        feed(&mut state, &mut blocks, vec![press(Key::ArrowLeft, SHIFT)]);
        assert!(blocks[0].collapsed, "shift+left folds it");
        feed(&mut state, &mut blocks, vec![press(Key::ArrowRight, SHIFT)]);
        assert!(!blocks[0].collapsed, "shift+right opens it");
        // And the plain arrows too, because with a block picked there is no caret to move.
        feed(&mut state, &mut blocks, vec![press(Key::ArrowLeft, Modifiers::NONE)]);
        assert!(blocks[0].collapsed);
    }

    /// The arrows have to be about the caret again the moment the prompt is.
    #[test]
    fn the_arrows_go_back_to_the_caret_when_typing_resumes() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "ls", &["a"])];
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
        assert_eq!(state.aim, Aim::Block(1));
        feed(&mut state, &mut blocks, vec![egui::Event::Text("x".to_owned())]);
        assert_eq!(state.aim, Aim::Prompt, "typing takes the aim back");
        feed(&mut state, &mut blocks, vec![press(Key::ArrowLeft, Modifiers::NONE)]);
        assert!(!blocks[0].collapsed, "and the arrow moved the caret");
        assert_eq!(state.line.caret, 0);
    }

    #[test]
    fn del_forgets_a_picked_block_but_never_a_running_one() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "one", &["a"]), block(2, "two", &["b"])];
        blocks[1].code = None; // still going
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
        assert_eq!(state.aim, Aim::Block(2));
        feed(&mut state, &mut blocks, vec![press(Key::Delete, Modifiers::NONE)]);
        assert_eq!(blocks.len(), 2, "a running block's output has to land somewhere");

        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
        assert_eq!(state.aim, Aim::Block(1));
        feed(&mut state, &mut blocks, vec![press(Key::Delete, Modifiers::NONE)]);
        assert_eq!(blocks.len(), 1);
        assert_eq!(state.aim, Aim::Block(2), "and the selection lands on the next");
    }

    // -- copying -----------------------------------------------------------

    #[test]
    fn ctrl_c_copies_the_picked_block_whole() {
        let mut state = State::default();
        let mut blocks = vec![block(7, "ls -l", &["one", "two"])];
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);
        let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
        assert_eq!(out.copy.as_deref(), Some("> ls -l\r\none\r\ntwo"));
    }

    #[test]
    fn ctrl_c_copies_a_dragged_span_across_rows() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "ls", &["alpha", "beta"])];
        state.rebuild(&blocks, None);
        // Row 0 is the header, 1 and 2 the output. From "ph" in alpha to "be" in beta.
        state.aim = Aim::Text(Span::new(
            Spot { row: 1, col: 2 },
            Spot { row: 2, col: 2 },
        ));
        let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
        assert_eq!(out.copy.as_deref(), Some("pha\r\nbe"));
    }

    /// The blank row between two blocks is a row, so a drag across it copies the blank line it looks
    /// like — what you see is what you get, air included.
    #[test]
    fn a_span_across_two_blocks_keeps_the_line_between_them() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "one", &["a"]), block(2, "two", &["b"])];
        state.rebuild(&blocks, None);
        // 0 header, 1 output, 2 the blank row, 3 the next header, 4 its output.
        state.aim = Aim::Text(Span::new(
            Spot { row: 1, col: 0 },
            Spot { row: 4, col: 1 },
        ));
        let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
        assert_eq!(out.copy.as_deref(), Some("a\r\n\r\ntwo\r\nb"));
    }

    #[test]
    fn ctrl_c_copies_the_prompts_own_selection() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        feed(
            &mut state,
            &mut blocks,
            vec![egui::Event::Text("cargo build".to_owned())],
        );
        feed(&mut state, &mut blocks, vec![press(Key::A, CTRL)]);
        let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
        assert_eq!(out.copy.as_deref(), Some("cargo build"));
    }

    /// With nothing selected anywhere it is the Ctrl+C somebody meant.
    #[test]
    fn ctrl_c_with_nothing_selected_is_a_stop() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
        assert!(out.stop);
        assert_eq!(out.copy, None);
    }

    /// egui-winit may send `Event::Copy` as well as the keystroke, and a cut that ran twice would
    /// take the text out and then take out whatever was next to it.
    #[test]
    fn a_copy_that_arrives_twice_only_happens_once() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        feed(&mut state, &mut blocks, vec![egui::Event::Text("abcdef".to_owned())]);
        feed(&mut state, &mut blocks, vec![press(Key::A, CTRL)]);
        let out = feed(
            &mut state,
            &mut blocks,
            vec![egui::Event::Cut, press(Key::X, CTRL)],
        );
        assert_eq!(out.copy.as_deref(), Some("abcdef"));
        assert_eq!(state.line.text(), "", "cut once, not twice");
    }

    // -- selecting by word -------------------------------------------------

    /// The reported case: in `source.cpp`, `cpp` has to be selectable on its own.
    #[test]
    fn a_dot_is_its_own_run_so_either_side_of_it_is_a_word() {
        let chars: Vec<char> = "source.cpp".chars().collect();
        assert_eq!(word_at(&chars, 0), 0..6, "source");
        assert_eq!(word_at(&chars, 5), 0..6);
        assert_eq!(word_at(&chars, 6), 6..7, "the dot alone");
        assert_eq!(word_at(&chars, 7), 7..10, "cpp");
        assert_eq!(word_at(&chars, 9), 7..10);
    }

    #[test]
    fn runs_of_space_and_punctuation_are_words_too() {
        let chars: Vec<char> = " M  src/ui/console.rs".chars().collect();
        assert_eq!(word_at(&chars, 0), 0..1, "the leading space");
        assert_eq!(word_at(&chars, 1), 1..2, "M");
        assert_eq!(word_at(&chars, 2), 2..4, "both spaces, as one run");
        assert_eq!(word_at(&chars, 4), 4..7, "src");
        assert_eq!(word_at(&chars, 7), 7..8, "the slash");
    }

    #[test]
    fn a_word_at_the_end_of_a_line_still_ends_at_the_line() {
        let chars: Vec<char> = "abc".chars().collect();
        assert_eq!(word_at(&chars, 3), 0..3, "a column past the end clamps back in");
        assert_eq!(word_at(&chars, 99), 0..3);
        assert_eq!(word_at(&[], 0), 0..0, "and an empty line has no word");
    }

    // -- one character, one column -----------------------------------------

    #[test]
    fn a_tab_becomes_spaces_up_to_the_next_stop() {
        assert_eq!(columns("\tnew file:"), "        new file:");
        assert_eq!(columns("ab\tc"), "ab      c", "from column two to column eight");
        assert_eq!(
            columns("12345678\tx"),
            "12345678        x",
            "a tab on a stop still moves a whole one"
        );
        assert_eq!(columns("plain text"), "plain text", "and nothing else is touched");
    }

    /// The reported case, to the character.
    ///
    /// A `git status` line begins with a tab, so every character after it sat four columns along
    /// while the arithmetic counted one — and a selection that *looked* right came out three
    /// characters along, being the tab's other three columns.
    #[test]
    fn a_selection_on_a_tabbed_line_copies_what_was_highlighted() {
        let line = "\tnew file:   ../Plugin/Src/View/LgsxDetailsWidget.h";
        let mut state = State::default();
        let mut blocks = vec![block(1, "git status", &[line])];
        state.rebuild(&blocks, None);

        // Row 1 is the output. Find `LgsxDetailsWidget.h` by the column it is *drawn* at, which is
        // what the pointer would have picked.
        let shown = super::shown(&blocks, Row::Text(0, 0, 0), None).into_owned();
        let at = shown.find("LgsxDetailsWidget.h").expect("the name is on the row");
        let from = shown[..at].chars().count();
        let to = from + "LgsxDetailsWidget.h".chars().count();
        state.aim = Aim::Text(Span::new(
            Spot { row: 1, col: from },
            Spot { row: 1, col: to },
        ));

        let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
        assert_eq!(out.copy.as_deref(), Some("LgsxDetailsWidget.h"));
    }

    // -- clearing ----------------------------------------------------------

    #[test]
    fn clear_and_cls_are_the_panels_own_and_never_reach_a_shell() {
        for word in ["clear", "cls", "CLS"] {
            let mut state = State::default();
            let mut blocks = vec![block(1, "ls", &["a"])];
            let out = feed(
                &mut state,
                &mut blocks,
                vec![
                    egui::Event::Text(word.to_owned()),
                    press(Key::Enter, Modifiers::NONE),
                ],
            );
            assert!(out.clear, "{word} clears the log");
            assert_eq!(out.send, None, "{word} is not handed to the shell");
            // And it is still in the history, so `Up` finds it like anything else typed.
            assert_eq!(state.history, vec![word.to_owned()]);
        }
    }

    // -- wrapping ----------------------------------------------------------

    #[test]
    fn alt_z_turns_wrapping_on_and_off() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        state.width = 10;
        assert_eq!(state.cols(), Some(10), "on to begin with");
        feed(&mut state, &mut blocks, vec![press(Key::Z, Modifiers::ALT)]);
        assert_eq!(state.cols(), None);
        feed(&mut state, &mut blocks, vec![press(Key::Z, Modifiers::ALT)]);
        assert_eq!(state.cols(), Some(10));
    }

    #[test]
    fn a_wrapped_line_is_as_many_rows_as_it_has_slices() {
        assert_eq!(slices("", 10), 1, "a blank line is still a line");
        assert_eq!(slices("0123456789", 10), 1, "exactly one row");
        assert_eq!(slices("0123456789a", 10), 2);
        assert_eq!(slices("\tab", 10), 1, "measured after the tab is expanded");
        assert_eq!(slices("\tabc", 10), 2, "eight columns of tab and three of text");
    }

    #[test]
    fn wrapping_slices_a_line_across_rows_and_copies_it_back_whole() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "ls", &["abcdefghij"])];
        state.width = 4;
        state.wrap = true;
        state.rebuild(&blocks, state.cols());
        // The header is one row at four columns wide (`ls`), then the line in three.
        assert_eq!(state.rows(), 4);
        assert_eq!(state.row(&blocks, 0), Some(Row::Head(0, 0)));
        assert_eq!(state.row(&blocks, 1), Some(Row::Text(0, 0, 0)));
        assert_eq!(state.row(&blocks, 3), Some(Row::Text(0, 0, 2)));
        assert_eq!(shown(&blocks, Row::Text(0, 0, 1), state.cols()), "efgh");

        // **Copied back without the wrap in it.** Three rows on screen, one line on the clipboard —
        // a line ending dropped into the middle of a wrapped path is worse than not wrapping at all.
        state.aim = Aim::Text(Span::new(
            Spot { row: 1, col: 0 },
            Spot { row: 3, col: 2 },
        ));
        let out = feed(&mut state, &mut blocks, vec![press(Key::C, CTRL)]);
        assert_eq!(out.copy.as_deref(), Some("abcdefghij"));
    }

    /// Turning it on and off again has to give the same log back.
    #[test]
    fn the_index_agrees_with_itself_whichever_way_it_is_built() {
        let mut state = State::default();
        let blocks = vec![block(1, "one", &["a", "b"]), block(2, "two", &["c"])];
        state.rebuild(&blocks, None);
        let flat: Vec<_> = (0..state.rows())
            .map(|at| state.row(&blocks, at))
            .collect();
        // Wide enough that nothing wraps, so the wrapped index must come out the same.
        state.width = 80;
        state.wrap = true;
        state.rebuild(&blocks, state.cols());
        let wrapped: Vec<_> = (0..state.rows())
            .map(|at| state.row(&blocks, at))
            .collect();
        assert_eq!(flat, wrapped);
    }

    // -- the row index -----------------------------------------------------

    #[test]
    fn the_rows_are_a_header_and_its_lines() {
        let mut state = State::default();
        let blocks = vec![block(1, "one", &["a", "b"]), block(2, "two", &["c"])];
        state.rebuild(&blocks, None);
        assert_eq!(state.rows(), 6);
        assert_eq!(state.row(&blocks, 0), Some(Row::Head(0, 0)), "no air at the top");
        assert_eq!(state.row(&blocks, 1), Some(Row::Text(0, 0, 0)));
        assert_eq!(state.row(&blocks, 2), Some(Row::Text(0, 1, 0)));
        assert_eq!(state.row(&blocks, 3), Some(Row::Gap(1)), "a line before the next");
        assert_eq!(state.row(&blocks, 4), Some(Row::Head(1, 0)));
        assert_eq!(state.row(&blocks, 5), Some(Row::Text(1, 0, 0)));
        assert_eq!(state.row(&blocks, 6), None);
    }

    /// A block starts at its blank row, so `Shift+Up` brings the air along and the header it lands on
    /// is never jammed against the top edge.
    #[test]
    fn a_block_starts_at_the_blank_row_above_it() {
        let mut state = State::default();
        let blocks = vec![block(1, "one", &["a"]), block(2, "two", &["b"])];
        state.rebuild(&blocks, None);
        assert_eq!(state.starts, vec![0, 2, 5]);
    }

    #[test]
    fn a_folded_block_is_one_row() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "one", &["a", "b"]), block(2, "two", &["c"])];
        blocks[0].collapsed = true;
        state.rebuild(&blocks, None);
        assert_eq!(state.rows(), 4);
        assert_eq!(state.row(&blocks, 0), Some(Row::Head(0, 0)));
        assert_eq!(state.row(&blocks, 1), Some(Row::Gap(1)), "straight to the next");
        assert_eq!(state.row(&blocks, 2), Some(Row::Head(1, 0)));
        assert_eq!(state.row(&blocks, 3), Some(Row::Text(1, 0, 0)));
    }

    #[test]
    fn a_capped_block_says_so_on_a_row_of_its_own() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "find /", &["a"])];
        blocks[0].dropped = 4_000;
        state.rebuild(&blocks, None);
        assert_eq!(state.rows(), 3);
        assert_eq!(state.row(&blocks, 1), Some(Row::Cut(0)));
        assert_eq!(state.row(&blocks, 2), Some(Row::Text(0, 0, 0)));
    }

    /// The whole point of the index: the cost is the blocks, never their lines.
    #[test]
    fn the_index_does_not_grow_with_the_output() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "yes", &[])];
        blocks[0].lines = (0..50_000)
            .map(|n| crate::console::Line {
                text: n.to_string(),
                err: false,
            })
            .collect();
        state.rebuild(&blocks, None);
        assert_eq!(state.starts.len(), 2, "one entry per block, and the total");
        assert_eq!(state.rows(), 50_001);
        assert_eq!(state.row(&blocks, 50_000), Some(Row::Text(0, 49_999, 0)));
    }

    /// Wrapped, a frame in which nothing moved has to do nothing — the mode the panel now opens in
    /// cannot be paying a pass over every line of the log to be told what it already knows.
    ///
    /// Proved by leaving a mark in the index and finding it still there: had the second call rebuilt
    /// anything at all, the first thing it does is clear that away.
    #[test]
    fn a_wrapped_index_is_left_alone_until_something_moves() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "ls", &["abcdefghij"])];
        state.width = 4;
        state.rebuild(&blocks, state.cols());
        let rows = state.rows();

        state.starts.push(usize::MAX);
        state.rebuild(&blocks, state.cols());
        assert_eq!(state.starts.last().copied(), Some(usize::MAX), "untouched");
        assert_eq!(state.rows(), rows);

        // And a line arriving is something moving.
        blocks[0].lines.push(crate::console::Line {
            text: "k".to_owned(),
            err: false,
        });
        state.rebuild(&blocks, state.cols());
        assert_eq!(state.rows(), rows + 1, "built again");
        assert_eq!(state.starts.last().copied(), Some(state.rows()));
    }

    // -- who has the keyboard ----------------------------------------------

    /// The panel's own claim, egui's answer, and the frame in which egui has no answer at all.
    #[test]
    fn the_keys_are_the_panels_until_something_else_takes_them() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let pane: PaneId = 1;
        let mut checked = false;
        let _ = ctx.run_ui(Default::default(), |ui| {
            let ctx = ui.ctx();
            assert!(!state.keeps_keys(ctx, pane), "never claimed");

            state.take_keys();
            // Nothing focused: the frame a click *completes* in, where egui has revoked the panel's
            // focus and the panel has not asked for it back yet. Still the panel's — otherwise a key
            // pressed in that frame goes nowhere and the listing lights up for it.
            assert!(state.keeps_keys(ctx, pane), "claimed, and nothing else holds it");

            ctx.memory_mut(|m| m.request_focus(id(pane)));
            assert!(state.keeps_keys(ctx, pane), "claimed, and egui agrees");

            ctx.memory_mut(|m| m.request_focus(Id::new("a-field-somewhere")));
            assert!(
                !state.keeps_keys(ctx, pane),
                "something else holds it: a field, or the other pane's console"
            );

            state.drop_keys();
            ctx.memory_mut(|m| m.request_focus(id(pane)));
            assert!(
                !state.keeps_keys(ctx, pane),
                "given up, whatever egui is still saying"
            );
            checked = true;
        });
        assert!(checked, "the pass ran");
    }

    // -- the geometry ------------------------------------------------------

    #[test]
    fn the_console_takes_its_share_off_the_bottom() {
        let above = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        let (list, panel) = split(above, true, 0.35);
        let panel = panel.expect("there is room in six hundred points");
        assert_eq!(panel.bottom(), above.bottom(), "it is the bottom of the pane");
        assert_eq!(list.bottom(), panel.top(), "and the seam is the panel's");
        assert!((panel.height() - SEAM - 600.0 * 0.35).abs() < 0.5);
    }

    #[test]
    fn a_pane_too_short_keeps_its_listing_instead() {
        let above = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 120.0));
        let (list, panel) = split(above, true, 0.35);
        assert_eq!(panel, None);
        assert_eq!(list, above);
    }

    #[test]
    fn a_shut_console_takes_nothing() {
        let above = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        assert_eq!(split(above, false, 0.35), (above, None));
    }

    #[test]
    fn escape_lets_go_of_one_thing_at_a_time() {
        let mut state = State::default();
        let mut blocks = vec![block(1, "ls", &["a"])];
        feed(&mut state, &mut blocks, vec![egui::Event::Text("half".to_owned())]);
        feed(&mut state, &mut blocks, vec![press(Key::ArrowUp, SHIFT)]);

        feed(&mut state, &mut blocks, vec![press(Key::Escape, Modifiers::NONE)]);
        assert_eq!(state.aim, Aim::Prompt, "the block first");
        assert_eq!(state.line.text(), "half", "and the line is still there");

        feed(&mut state, &mut blocks, vec![press(Key::Escape, Modifiers::NONE)]);
        assert_eq!(state.line.text(), "", "then the line");

        // And with nothing left to let go of, it is the window's again.
        let mut events = vec![press(Key::Escape, Modifiers::NONE)];
        let mut out = Outcome::default();
        state.keys(&mut events, Modifiers::NONE, &mut blocks, &mut out);
        assert_eq!(events.len(), 1, "passed through");
    }

    #[test]
    fn a_pasted_block_of_lines_stays_one_command() {
        let mut state = State::default();
        let mut blocks = Vec::new();
        let out = feed(
            &mut state,
            &mut blocks,
            vec![
                egui::Event::Paste("git add .\r\ngit commit\n".to_owned()),
                press(Key::Enter, Modifiers::NONE),
            ],
        );
        assert_eq!(out.send.as_deref(), Some("git add . git commit"));
    }
}
