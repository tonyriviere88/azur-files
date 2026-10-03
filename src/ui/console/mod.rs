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

mod editor;
mod log;
mod prompt;
mod rows;
#[cfg(test)]
mod tests;

// One file per part of the panel, and the glob is what says they were one module: the split is an
// arrangement of files, not a narrowing of what the panel's own pieces may reach.
pub(crate) use editor::*;
pub(crate) use log::*;
pub(crate) use prompt::*;
pub(crate) use rows::*;

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

// ---------------------------------------------------------------------------
// What the keyboard is about
// ---------------------------------------------------------------------------

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
