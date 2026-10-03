//! Drag and drop, both ways, through OLE.
//!
//! The same mechanism Explorer uses, which is what makes it interoperate: files
//! dragged out of this window land in Explorer, in an archiver, in an editor's tab
//! bar, in an upload field; files dragged in from any of those arrive here. Nothing
//! about it is specific to this program.
//!
//! # Dragging out
//!
//! [`drag_out`] builds the shell's own data object for the selection and hands it to
//! `DoDragDrop`, with a tiny `IDropSource` to answer the two questions OLE asks during
//! a drag — has the gesture been abandoned, and what cursor to show. `DoDragDrop` is
//! modal: it runs its own message loop and does not return until the drop happens or the
//! drag is cancelled, so whichever thread calls it does nothing else until then. It is not
//! the UI thread, for the reason set out on [`Drag`]: a window that cannot paint during a
//! drag cannot show what the drag is about to do.
//!
//! # Dropping in
//!
//! winit already registers a drop target on the window to produce its own
//! `HoveredFile` / `DroppedFile` events, and those events carry only paths — no
//! modifier state, so no way to tell a copy from a move, and no way to answer with an
//! effect so the cursor shows what will happen. That is most of what a drop *is*.
//!
//! So [`Zone::attach`] revokes winit's target and registers this one, which answers
//! `DragOver` with a real `DROPEFFECT` and reports the modifiers on `Drop`. The rule it
//! applies is Explorer's:
//!
//! | | |
//! | --- | --- |
//! | `Ctrl` held | copy |
//! | `Shift` held | move |
//! | neither, same volume | move |
//! | neither, different volume | copy |
//! | neither, source under `%TEMP%` | copy — see [`under_temp`] |
//!
//! Whatever comes out of that is then narrowed to what the source said it would allow.
//! `pdwEffect` arrives holding the effects the source passed to `DoDragDrop`, and answering
//! with one that is not among them is not a harmless liberty: `DROPEFFECT_MOVE` returned to a
//! source is an *instruction* to delete what it handed over.
//!
//! # Saying what the drop will do
//!
//! A cursor with a `+` on it says a copy is coming and not where it is going, which over a
//! listing full of folders is most of the question. So the drag says it in words — *Move one.txt
//! into docs*, *Copy 4 items into src*, *Pin src to Bookmarks* — carries a stack of the icons it
//! picked up, and lights up the place it would land in.
//!
//! **The sentence names both ends and picks them out in the accent**, which is why it travels as
//! pieces rather than as a string: see [`Told`] for the shape and [`Told::runs`] for which pieces
//! are the blue ones. `crate::ui::drag_saying` draws it just below the pointer, under the stack
//! `crate::ui::drag_ghost` draws just above it, and `crate::ui::drop_target` is the mark on the
//! destination. **The rows it came from are not marked** — a row's style does not change for being
//! in the air; see `crate::ui::is_cut`, where that is written down beside the one mark a listing
//! does put on a row it is about to lose.
//!
//! **All of it is drawn by this program, and that is a decision.** Windows has a mechanism of its
//! own — a `CFSTR_DROPDESCRIPTION` written onto the data object, laid out by the shell inside a
//! drag image the *source* has to have asked the drag-image manager for — and it was built here
//! and taken back out again. It works for a drag out of Explorer and not for a drag this window
//! starts: the manager will not draw text while OLE is drawing a cursor, and giving up the
//! standard copy-and-move cursors to get it is the wrong trade. What was left was one gesture
//! that looked two different ways depending on where the files came from, decided by a flag on
//! somebody else's object. Drawing it here is fewer moving parts and the same picture every time.
//!
//! The destination's *name* still cannot be worked out in the callbacks: they run with the
//! application out of reach, a group's name is only in the bookmark list, and a shell
//! display-name lookup is not a thing to do on every mouse move. So it is published with the
//! region it belongs to — see [`Region`].
//!
//! The callbacks arrive on the UI thread from inside winit's message pump, where the
//! application state is not reachable — so they read and write a small shared block
//! instead, which the frame loop publishes into and drains from. **And then ask for a frame**,
//! because for a drag from another program nothing else will: OLE has the pointer, so no mouse
//! event reaches winit and nothing wakes the window to look at what the callbacks just wrote. See
//! [`Shared::hovering`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::shell::clipboard::Effect;

/// What a zone does with whatever lands on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Onto {
    /// Copy or move the items into this folder.
    Folder(PathBuf),
    /// Pin them in the sidebar. Nothing is copied and nothing is moved, which is why it
    /// reports itself to the pointer as a link — the same answer Explorer gives when you
    /// drag a folder onto Quick Access.
    Bookmarks,
    /// Pin them into the group at this position in the bookmark list.
    ///
    /// A zone of its own over the group's row, published after [`Self::Bookmarks`] so that it
    /// wins — exactly the arrangement a listing's folder rows have over the listing they are in,
    /// and for the same reason: dropping *on* something has to mean into that something.
    BookmarkGroup(usize),
}

/// What a drop is about to do, as the pointer is told it.
///
/// Not [`Effect`], which is the pair of things the *filesystem* can be asked for: pinning a folder
/// in the sidebar copies nothing and moves nothing, and is still one of the three answers a drop
/// here can have.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Doing {
    Copy,
    Move,
    /// Making a shortcut to each item — the Alt-drag. See [`crate::shell::ops::Job::Link`].
    Link,
    /// Pinning in the sidebar, which is what Explorer calls the same gesture onto Quick access.
    Pin,
}

impl Doing {
    /// The two plain stretches of the sentence: the verb, and the word joining the ends.
    ///
    /// *Copy `src` into `docs`* — the verb, what is being carried, the joining word, where it is
    /// going. `into` for a folder, because that is what a copy or a move does to one; `to` for the
    /// sidebar, because nothing goes *into* a bookmark. Those are the two idioms and not one rule
    /// spelled two ways, which is why they sit beside the verb rather than being appended to it.
    ///
    /// `refused` negates the verb and leaves everything else alone — *Cannot move src into main* —
    /// so a refusal reads as the same sentence about the same gesture. Written out rather than
    /// built from the verb, because `Cannot ` plus a lowercased word is a rule that holds for
    /// exactly these three and not for the next one.
    ///
    /// The sidebar's own refusal is a sentence of its own and does not come through here — see
    /// [`Refused::AFile`] — so `Cannot pin ` is not reached today. It is written down anyway,
    /// because this table is about how each verb negates and not about which refusals exist.
    fn words(self, refused: bool) -> (&'static str, &'static str) {
        let (yes, no, joining) = match self {
            Self::Copy => ("Copy ", "Cannot copy ", " into "),
            Self::Move => ("Move ", "Cannot move ", " into "),
            // *Link to one.txt in docs* — the two halves read as one sentence about a shortcut
            // without the word appearing twice, and `in` rather than `into` because what goes
            // into the folder is the shortcut and not the file.
            Self::Link => ("Link to ", "Cannot link to ", " in "),
            Self::Pin => ("Pin ", "Cannot pin ", " to "),
        };
        (if refused { no } else { yes }, joining)
    }
}

/// The sentence under the pointer, in the pieces it is drawn in.
///
/// Kept apart rather than joined into a string because **the two ends are drawn in the accent** —
/// see [`Self::runs`]. A tooltip reading *Copy one.txt into docs* with the two names picked out is
/// the same information as three plain words and a pair of quotes, and it is read at a glance
/// instead of parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Told {
    /// Which of the three it is, which decides the words either side of the two names.
    pub doing: Doing,
    /// Why this is a drop the program will not make, when it is one — see [`refuses`].
    ///
    /// The sentence then says so of the same gesture rather than describing a different one:
    /// *Cannot move src into main, which is inside it*, with the mark that goes with it. Which is
    /// the whole reason a refusal is carried *beside* the verb rather than as a fourth [`Doing`]:
    /// what the pointer is refusing is exactly the thing it would otherwise have promised, and
    /// saying it any other way loses that.
    pub refused: Option<Refused>,
    /// What is being carried: one item's name, or `4 items` — see [`carrying`].
    ///
    /// **Except in a refusal, where it is the one item being refused** — see [`culprit`]. The
    /// sentence is then about that item and not about the load, because that item is the reason
    /// there is a sentence at all.
    ///
    /// `None` when the drag will not say what it holds until it lands. That is a real case and not
    /// a defence against one — a source is entitled to render nothing until the drop is real, which
    /// is what an archiver does, so the sentence has to read without it. See `Incoming`.
    pub source: Option<String>,
    /// Where it would land: a folder's name, a group's, or the Bookmarks section's.
    pub target: String,
}

impl Told {
    /// The sentence as a run of pieces, each with whether it is one of the **blue** ones.
    ///
    /// Between two and five of them, which is the whole reason this is a `Vec` and not an array:
    ///
    /// | | |
    /// | --- | --- |
    /// | *Copy one.txt into docs* | the ordinary one |
    /// | *Copy into docs* | a drag that has not said what it holds — see [`Self::source`] |
    /// | *Cannot move src into itself* | [`Refused::Itself`]: the far end is a word, not a name |
    /// | *Cannot move src into main, which is inside it* | [`Refused::Inside`]: with a reason |
    /// | *Cannot pin a file* | [`Refused::AFile`], which names nothing at all |
    pub fn runs(&self) -> Vec<(&str, bool)> {
        // The one refusal with nothing to name, so nothing else about the gesture is drawn: a
        // sentence naming the folder it was aimed at would read as though the folder were the
        // problem. See [`Refused::AFile`].
        if self.refused == Some(Refused::AFile) {
            return vec![("Cannot pin a file", false)];
        }
        let (verb, joining) = self.doing.words(self.refused.is_some());
        let mut runs = Vec::with_capacity(5);
        match &self.source {
            Some(source) => {
                runs.push((verb, false));
                runs.push((source.as_str(), true));
            }
            // Nothing to name at the near end, so the verb runs straight into the joining word:
            // `Copy ` + ` into ` would read with two spaces in it.
            None => runs.push((verb.trim_end(), false)),
        }
        runs.push((joining, false));
        if self.refused == Some(Refused::Itself) {
            // The destination *is* what is being dragged, so naming it twice would read as two
            // folders that happen to share a name. A word instead, and not a blue one — there is
            // one name in this sentence and it has already been said.
            runs.push(("itself", false));
        } else {
            runs.push((self.target.as_str(), true));
            if self.refused == Some(Refused::Inside) {
                // Which is the whole difference from a refusal onto the folder itself, and not
                // something the two names can say between them: `main` looks like an ordinary
                // destination until you are told where it is.
                runs.push((", which is inside it", false));
            }
        }
        runs
    }

    /// The whole sentence as one string, which is what a test asserts on: the pieces are a
    /// drawing decision, and what the reader ends up with is the words in order.
    #[cfg(test)]
    pub fn sentence(&self) -> String {
        self.runs().into_iter().map(|(text, _)| text).collect()
    }
}

/// A path with its case folded, for comparing two of them the way Windows compares them.
///
/// `Path`'s own comparisons are byte-wise: `C:\Src` and `c:\src` are one folder to the filesystem
/// and two different paths to `Path::starts_with` and to `==`. Every comparison below goes
/// through this — still *component*-wise afterwards, so `C:\src2` is not taken for something
/// inside `C:\src`. The same pair of decisions [`under_temp`] makes, for the same reason.
fn folded(path: &std::path::Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

/// Whether `into` is `item` itself, or somewhere inside it.
///
/// The drop a folder cannot take. Dragging `src` onto `src\main` asks for a folder to be put inside
/// itself, which is not a slow copy or a partial one — it is a copy with no end, and the shell
/// refuses it with a dialog rather than a cursor. Onto *itself* is the same question with nothing
/// to recurse through, and [`Refused`] keeps the two apart because they do not read the same way.
pub fn swallows(item: &std::path::Path, into: &std::path::Path) -> bool {
    folded(into).starts_with(folded(item))
}

/// Why a drop cannot happen.
///
/// Three of them, and each is a different sentence — which is the reason this is an enum and not a
/// flag. *Cannot move src into itself* and *Cannot move src into main, which is inside it* are two
/// different mistakes, and a reader told only that something is refused has to work out which one
/// they made. See [`Told::runs`], where each becomes its words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refused {
    /// The drag is carrying at least one file, and the sidebar is a list of *places*.
    ///
    /// The one refusal with nothing to name: which of the files is in the way does not matter, and
    /// naming one of several would read as though the rest were fine.
    AFile,
    /// The destination is the folder being dragged.
    Itself,
    /// The destination is somewhere inside the folder being dragged.
    Inside,
}

/// Why a drop of `items` onto `onto` cannot happen, if it cannot.
///
/// Both rules are about what the *destination* is rather than about which button or key the gesture
/// is holding — so both are known while the drag is still moving, which is what lets the pointer be
/// told before it lets go. See [`Told::refused`] for what the drag then says.
///
/// **Either rule refuses the whole selection over one item**, which is the same decision twice:
/// half a gesture is worse than none. A drop that pinned the folders and quietly skipped the files,
/// or moved four of five items and left the fifth where it was, is one the user has to go and check
/// afterwards — and nothing on screen would have said which half happened.
///
/// - **The sidebar takes folders**, so a selection with a file anywhere in it is refused.
///   `all_folders` is asked once when the drag arrives rather than here — `is_dir` is a syscall and
///   this runs on every mouse move.
/// - **A folder cannot take a drop of itself, or of anything it is inside** — see [`swallows`] — so
///   a selection with one such folder in it is refused over that destination. Which item it is is
///   [`culprit`], and it is the one the sentence names.
///
/// A drag that has not said what it holds refuses nothing. `items` is empty for a source that
/// renders on demand — see `Incoming` — and a refusal invented for files nobody has named yet would
/// be a no-entry sign over a drop that was going to work. `crate::app::App::droppable` is the guard
/// for that one: it drops the same items again where the answer had to be given without them.
pub fn refuses(onto: &Onto, items: &[PathBuf], all_folders: bool) -> Option<Refused> {
    if items.is_empty() {
        return None;
    }
    match onto {
        Onto::Bookmarks | Onto::BookmarkGroup(_) => (!all_folders).then_some(Refused::AFile),
        Onto::Folder(into) => {
            let culprit = culprit(items, into)?;
            // Which of the two it is, from the item [`culprit`] picked — which prefers the one that
            // *is* the destination, so that this and the sentence agree about the same item.
            let itself = folded(culprit) == folded(into);
            Some(if itself { Refused::Itself } else { Refused::Inside })
        }
    }
}

/// Which dragged item a folder's refusal is about: the one the destination is itself, or is inside.
///
/// **The sentence names this item and not the drag**, because this is the one that cannot go
/// where it is being taken. With one item in the air the two are the same string; with a folder and
/// a file selected together, *Cannot move src into itself* is the explanation and *Cannot move 2
/// items into itself* is a sentence about something nobody dragged.
///
/// The item that *is* the destination wins over one merely containing it, so that a selection
/// holding both a folder and its parent reads as [`Refused::Itself`] and says so — the nearer of
/// the two mistakes is the one being made.
pub fn culprit<'a>(items: &'a [PathBuf], into: &std::path::Path) -> Option<&'a std::path::Path> {
    let itself = items.iter().find(|item| folded(item) == folded(into));
    itself
        .or_else(|| items.iter().find(|item| swallows(item, into)))
        .map(PathBuf::as_path)
}

/// Whether `item` already lives in `into` — the drop that has nowhere to take it.
///
/// Case-folded and by the parent, not by [`swallows`]: `C:\work\src` is *in* `C:\work` and is not
/// inside itself. See [`does_nothing`], and `crate::app::App::droppable`, which acts on the same
/// question once the drop has landed.
pub fn already_in(item: &std::path::Path, into: &std::path::Path) -> bool {
    item.parent()
        .is_some_and(|parent| folded(parent) == folded(into))
}

/// Whether a drop of `items` onto `onto` would do nothing whatever.
///
/// **A move into the folder the items are already in.** There is no such thing: a move is a change
/// of which folder holds a name, and this drop asks for the one it has. Nothing happens on the way
/// past either — the drop is answered with no effect at all, so it is never delivered — and the
/// pointer says nothing about it, which is the whole reason this is asked *while the drag is
/// moving*. A promise of a move that will not happen is worse than silence, and a no-entry sign
/// over your own folder is worse still: it reads as though something were wrong. See
/// [`Shared::silent`].
///
/// **A copy there is a real gesture and is left alone**, which is why `moving` is a parameter
/// rather than an assumption. Ctrl held over the folder a file is already in is how Explorer is
/// asked for `one - Copy.txt`, and so is a right drag — which has not decided yet what it is, so
/// `asked` keeps its feedback too and the menu on drop says what the options are.
///
/// Every item, not any: a selection with one file from elsewhere in it has something to do, and
/// doing it is not "nothing" merely because the rest of the selection is already home.
pub fn does_nothing(onto: &Onto, items: &[PathBuf], moving: bool, asked: bool) -> bool {
    if items.is_empty() || !moving || asked {
        return false;
    }
    match onto {
        Onto::Folder(into) => items.iter().all(|item| already_in(item, into)),
        // Pinning is not a move and never was: it copies nothing, so there is nothing for it to do
        // nothing *of*. Whether a folder is already pinned is not knowable from here anyway — the
        // bookmark list is not published to the callbacks.
        Onto::Bookmarks | Onto::BookmarkGroup(_) => false,
    }
}

/// What to call what a drag is carrying: the one item's name, or how many there are.
///
/// A name while there is one to give, because *Move one.txt into docs* is a sentence about the file
/// in front of you; a count past that, because ten names under the pointer is not a label anybody
/// reads and there is nowhere to put them. `None` for a drag that has not said what it holds — see
/// [`Told::source`].
pub fn carrying(items: &[PathBuf]) -> Option<String> {
    match items {
        [] => None,
        [one] => Some(crate::fs::display_name(one)),
        many => Some(format!("{} items", many.len())),
    }
}

/// One droppable region: where it is, what a drop there means, and what to call the place it
/// would land in.
///
/// The **name** is here because the callbacks cannot go and ask for it. They run on the UI
/// thread from inside the message pump with the application out of reach — see [`Shared`] — and
/// what they have to answer is not only an effect but a sentence: *Copy one.txt into src*, *Pin
/// src to Work*. A group's name lives in the bookmark list and a folder's is a path away, so the
/// frame loop decides both and publishes them alongside the rectangle they apply to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    /// Where it is, in physical pixels: left, top, right, bottom.
    pub rect: (i32, i32, i32, i32),
    /// What a drop here is for.
    pub onto: Onto,
    /// What to call the destination in the drop description: the folder's own name, the
    /// group's, or the Bookmarks section's.
    pub name: String,
}

/// What a completed drop asks for.
#[derive(Clone, Debug)]
pub struct Dropped {
    pub items: Vec<PathBuf>,
    pub effect: Effect,
    /// Where it landed, in physical pixels.
    pub at: (i32, i32),
    /// What the zone under it was for.
    pub onto: Onto,
    /// Whether the *right* button carried the drag, which in Windows means "ask me what to do
    /// with it" rather than "do the obvious thing".
    ///
    /// Read from the last `DragOver` rather than from the drop, because by the time `Drop` is
    /// called the button has been released and its bit is gone.
    pub asked: bool,
}

/// Where a drop would go, published by the frame loop for the drop target to read.
///
/// The target callbacks cannot reach the application, and they have to answer
/// `DragOver` *immediately* with an effect — so the answer has to already be here.
#[derive(Clone, Default)]
pub struct Targets {
    /// Each droppable region, back to front.
    pub zones: Vec<Region>,
    /// The pane a drag *this window started* was picked up from, in the same pixels as the zones.
    ///
    /// `None` for a drag out of another program, and between drags. It is here because the pointer
    /// has one thing to say that depends on where the gesture *began* rather than on what is under
    /// it now — see [`Shared::silent`] — and the callbacks have no other way to know: they cannot
    /// reach the application, and a pane is a rectangle only the frame loop knows.
    pub from: Option<(i32, i32, i32, i32)>,
}

impl Targets {
    /// What is at a point, if anything.
    pub fn at(&self, at: (i32, i32)) -> Option<&Region> {
        self.zones.iter().rev().find(|region| holds(region.rect, at))
    }

    /// Whether a point is inside the pane the drag came out of — see [`Self::from`].
    pub fn started_in(&self, at: (i32, i32)) -> bool {
        self.from.is_some_and(|rect| holds(rect, at))
    }
}

/// Whether a rectangle in physical pixels holds a point.
///
/// Right and bottom exclusive, so two zones that share an edge do not both claim it.
fn holds((l, t, r, b): (i32, i32, i32, i32), (x, y): (i32, i32)) -> bool {
    x >= l && x < r && y >= t && y < b
}

/// The block the OLE callbacks and the frame loop share.
#[derive(Default)]
pub struct Shared {

    /// Published by the frame loop.
    pub targets: Targets,
    /// Whether the right button was down the last time the drag was seen moving.
    pub right_button: bool,
    /// Where a drag is hovering, for the frame loop to highlight.
    ///
    /// **Writing this is not enough on its own: the frame loop has to be woken to read it.** For a
    /// drag this window started that happens anyway — [`crate::app::App::pump_drag`] asks for a
    /// repaint on every frame for the length of it — but a drag from *another* program has nothing
    /// driving the window at all. OLE holds the pointer, so not one mouse event reaches winit;
    /// `DragOver` arrives instead, wrote this, and nothing ever came to look. The highlight
    /// therefore appeared for a drag between two panes and not for one out of an archive or out of
    /// Explorer, which is the same window and the same folder row answering the same question two
    /// different ways. So the callbacks request a repaint of their own — see `Target::wake`.
    pub hovering: Option<(i32, i32)>,
    /// What a drop where the drag is hovering would do, in words — and `None` when there is
    /// nothing under the pointer that would take it.
    ///
    /// Written by the same callback that answers the effect, so the sentence and the cursor cannot
    /// disagree, and read by the frame loop that draws it. See the module header, and [`Told`] for
    /// why it is pieces rather than a string.
    pub telling: Option<Told>,
    /// Whether the drop under the pointer is one to say **nothing at all** about: no sentence, no
    /// mark, no highlight, and not even a cursor.
    ///
    /// Two cases, and what they have in common is that nothing can land — the effect is
    /// `DROPEFFECT_NONE` either way — while *saying so would be the wrong thing to say*. What this
    /// suppresses is the telling of it, the no-entry cursor OLE would put on the pointer included.
    /// That cursor is why this lives here rather than in the frame loop: only the drag source can
    /// override it, and it reads this on the drag's own thread between one `DragOver` and the next.
    ///
    /// - **A folder over its own row, in the pane it was picked up from.** The beginning of every
    ///   drag of a folder is spent there, and a mistake is not what that is: it is where the folder
    ///   *is*. Across panes the same drop is a deliberate aim at a wrong answer and says so, which
    ///   is why this asks about [`Targets::from`] and not about the refusal alone.
    ///   [`Self::telling`] still carries the reason here, for the frame loop to stand the highlight
    ///   down by.
    /// - **A move into the folder the items are already in**, in any pane and out of any program —
    ///   see [`does_nothing`]. Nothing is refused as such; there is simply nothing to do, so there
    ///   is nothing to promise and no mistake to point at either. [`Self::telling`] is `None` for
    ///   this one, and the highlight stands down from this flag instead.
    pub silent: bool,
    /// Completed drops, waiting to be acted on.
    pub dropped: Vec<Dropped>,
}

/// The receiving side.
pub struct Zone {
    shared: Arc<Mutex<Shared>>,
    #[cfg(windows)]
    registered: bool,
    #[cfg(windows)]
    hwnd: isize,
}

impl Zone {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared::default())),
            #[cfg(windows)]
            registered: false,
            #[cfg(windows)]
            hwnd: 0,
        }
    }

    /// Tell the target where drops may land this frame.
    /// What a drop at this point would be for, as the OLE callbacks see it.
    ///
    /// For tests: the callbacks answer from the published zones on another stack entirely, and
    /// this is the only way to ask them the same question from here.
    #[cfg(test)]
    pub fn resolve(&self, at: (i32, i32)) -> Option<Region> {
        self.shared.lock().ok()?.targets.at(at).cloned()
    }

    pub fn publish(&self, targets: Targets) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.targets = targets;
        }
    }

    /// Put a drag over the window, or take it away, as `DragOver` and `DragLeave` do.
    ///
    /// For tests, and the only way to have one from here: what writes this runs on OLE's stack
    /// from inside the message pump, and nothing a test can do starts a drag from another
    /// program. Written into the shared block rather than into
    /// [`crate::app::App::drop_hover`], because that field is refreshed from here at the top of
    /// every frame — so a test that set it directly would have it cleared before the frame it
    /// was set for drew anything.
    #[cfg(test)]
    pub fn hover(&self, at: Option<(i32, i32)>) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.hovering = at;
        }
    }

    /// Where a drag is currently hovering, for the highlight.
    pub fn hovering(&self) -> Option<(i32, i32)> {
        self.shared.lock().ok().and_then(|shared| shared.hovering)
    }

    /// What the drag under the pointer would do, for the frame loop to draw.
    ///
    /// See [`Shared::telling`].
    pub fn telling(&self) -> Option<Told> {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.telling.clone())
    }

    /// Put a sentence under a drag, as `DragOver` does.
    ///
    /// For tests, and for the same reason [`Self::hover`] exists: what writes this runs on OLE's
    /// stack, and nothing a test can do starts a drag from another program.
    #[cfg(test)]
    pub fn tell(&self, told: Option<Told>) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.telling = told;
        }
    }

    /// Whether the drop under the pointer is one to say nothing about — see [`Shared::silent`].
    pub fn silent(&self) -> bool {
        self.shared
            .lock()
            .map(|shared| shared.silent)
            .unwrap_or(false)
    }

    /// Hold the pointer's tongue, as `DragOver` does over the row a drag started on.
    ///
    /// For tests, alongside [`Self::tell`]: the rule itself is the callbacks' — driven for real in
    /// `the_pane_a_drag_came_out_of_hears_nothing_about_it` — and this is how the *drawing* side of
    /// it is asked the question from here.
    #[cfg(test)]
    pub fn be_silent(&self, silent: bool) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.silent = silent;
        }
    }

    /// Whether a point is in the pane the drag in flight came out of — see [`Targets::from`].
    ///
    /// For tests: what the frame loop publishes is read on OLE's stack, and this is the only way to
    /// ask from here whether it published the right rectangle.
    #[cfg(test)]
    pub fn started_in(&self, at: (i32, i32)) -> bool {
        self.shared
            .lock()
            .map(|shared| shared.targets.started_in(at))
            .unwrap_or(false)
    }

    /// Take any completed drops.
    pub fn take_drops(&self) -> Vec<Dropped> {
        self.shared
            .lock()
            .map(|mut shared| std::mem::take(&mut shared.dropped))
            .unwrap_or_default()
    }

    /// Register on the window, replacing the one winit installed.
    ///
    /// Idempotent, and safe to call before the window exists — it does nothing until
    /// there is a handle.
    ///
    /// `ctx` is how the callbacks wake the frame loop, and without it a drag from another program
    /// is invisible: see [`Shared::hovering`].
    pub fn attach(&mut self, owner: super::Owner, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            if self.registered || owner.0 == 0 {
                return;
            }
            use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};

            let target: IDropTarget =
                win::Target::new(self.shared.clone(), owner.0, ctx.clone()).into();
            // SAFETY: winit registered its own target on this window; ours replaces it.
            // OLE takes a reference of its own, and the local one is deliberately leaked
            // so the target outlives this scope — `Drop` revokes it, which releases it.
            unsafe {
                let _ = RevokeDragDrop(owner.hwnd());
                if RegisterDragDrop(owner.hwnd(), &target).is_ok() {
                    self.registered = true;
                    self.hwnd = owner.0;
                    std::mem::forget(target);
                }
            }
        }
        #[cfg(not(windows))]
        let _ = (owner, ctx);
    }
}

impl Default for Zone {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl Drop for Zone {
    fn drop(&mut self) {
        if self.registered {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::Ole::RevokeDragDrop;
            // SAFETY: registered by `attach` on this handle, revoked once.
            let _ = unsafe { RevokeDragDrop(HWND(self.hwnd as *mut std::ffi::c_void)) };
        }
    }
}

/// A drag this program started, in flight on an apartment of its own.
///
/// `DoDragDrop` is modal — it runs its own message loop and does not return until the drop
/// lands or the gesture is abandoned — so the thread that calls it does nothing else for the
/// length of the drag. On the UI thread that means no frames: not one repaint reaches the
/// window while a drag started *inside* it is running, so the row a drop would land in could
/// not be highlighted and a file selected by the drag itself could not be seen to be selected.
/// Neither is cosmetic. They are the only feedback the gesture has.
///
/// So it runs here instead, and the UI thread keeps painting underneath it. The drop target is
/// registered in the UI thread's apartment, and OLE marshals the callbacks back into it — the
/// same machinery that lets Explorer call into this process at all — so the highlight is driven
/// by the same `DragOver` that answers the cursor.
pub struct Drag {
    done: std::sync::mpsc::Receiver<Option<Effect>>,
}

impl Drag {
    /// What the target did with the files, once the drag has ended.
    ///
    /// `None` while it is still in flight. `Some(None)` for a drag that was abandoned, or
    /// dropped somewhere that took nothing.
    pub fn finished(&self) -> Option<Option<Effect>> {
        match self.done.try_recv() {
            Ok(effect) => Some(effect),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            // The thread went away without answering. Still an ended drag, and leaving the
            // handle in place would wedge every later one.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(None),
        }
    }

    /// A drag with nothing behind it, and the end of the wire to finish it from.
    ///
    /// For the test that a drag in flight keeps the window painting — the property this whole
    /// arrangement exists for, and one no unit test could reach if the only way to have a drag
    /// were to hold a real mouse button down.
    #[cfg(test)]
    pub fn pretend() -> (Self, std::sync::mpsc::Sender<Option<Effect>>) {
        let (tx, done) = std::sync::mpsc::channel();
        (Self { done }, tx)
    }
}

/// Pick these files up and start dragging them.
///
/// Returns as soon as the drag is under way. Ask the handle for the outcome — a move has taken
/// the files out of the folder they were in, so the source needs re-reading.
///
/// `zone` is the window's own drop target, and the source is handed the block it writes: **the
/// cursor is the source's to set**, and what it should be is decided at the other end of the
/// gesture. See [`Shared::silent`], which is the one thing the standard cursors cannot say.
pub fn drag_out(items: Vec<PathBuf>, zone: &Zone) -> Option<Drag> {
    if items.is_empty() {
        return None;
    }
    #[cfg(windows)]
    {
        // The thread that owns the window and its input, for the attachment below.
        let ui_thread = unsafe {
            windows::Win32::System::Threading::GetCurrentThreadId()
        };
        let shared = zone.shared.clone();
        let (tx, done) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("drag-source".to_owned())
            .spawn(move || {
                // This thread's own apartment: the data object, the drop source and the modal
                // loop all belong to it.
                crate::shell::init();
                let _ = tx.send(win::drag_out(&items, ui_thread, shared));
            })
            .ok()?;
        Some(Drag { done })
    }
    #[cfg(not(windows))]
    {
        let _ = (items, zone);
        None
    }
}

/// Explorer's rule for what an unmodified drag means.
///
/// Within a volume a drag moves; across volumes it copies. Which is not arbitrary — a
/// move within a volume is a rename of a directory entry and effectively free, while
/// across volumes it is a copy followed by a delete, and defaulting to that would make
/// an accidental drag both slow and destructive.
pub fn default_effect(source: Option<&std::path::Path>, target: &std::path::Path) -> Effect {
    let volume = |path: &std::path::Path| -> Option<String> {
        let text = path.to_string_lossy();
        // A drive letter, or the `\\server\share` of a UNC path.
        if text.len() >= 2 && text.as_bytes()[1] == b':' {
            return Some(text[..2].to_lowercase());
        }
        if let Some(rest) = text.strip_prefix(r"\\") {
            let mut parts = rest.split(['\\', '/']);
            let server = parts.next()?;
            let share = parts.next()?;
            return Some(format!(r"\\{server}\{share}").to_lowercase());
        }
        None
    };
    match (source.and_then(volume), volume(target)) {
        (Some(from), Some(to)) if from == to => Effect::Move,
        _ => Effect::Copy,
    }
}

/// Whether a path is inside the user's temporary directory.
///
/// This is the signal that what a source is offering is a *materialisation* rather than the
/// user's own files: the contents of an archive, a mail attachment, anything a source had to
/// unpack somewhere before it had a path to put in a `CF_HDROP` at all. It deletes them again
/// once the drag is over, so the same-volume rule above — which would call a drop into any
/// folder on `C:` a move, `%TEMP%` being on `C:` — is answering a question the user cannot
/// have asked. Dragging the contents of a 7-Zip archive into a folder reported
/// `DROPEFFECT_MOVE` to 7-Zip, which took it as leave to delete its extraction, and it did so
/// while the copy was still reading out of it.
///
/// Only the *default* is decided here. `Shift` still asks for a move and still gets one, if
/// the source allows one.
pub fn under_temp(path: &std::path::Path) -> bool {
    Temp::current().is_some_and(|temp| temp.holds(path))
}

/// `%TEMP%` in both the forms a path can arrive in, worked out once.
///
/// Split out of [`under_temp`] so that [`claim`] can ask the question of *every* item it was
/// handed without paying for the resolution below once per item — the directory's own resolved
/// form is the same for all of them, and it is only the per-item `canonicalize` that is
/// unavoidable.
struct Temp {
    /// The directory itself, case-folded.
    folded: PathBuf,
    /// And with every link and 8.3 name resolved. `None` when it does not resolve, which makes
    /// [`Self::holds`] answer from the cheap test alone rather than answer `true`.
    resolved: Option<PathBuf>,
}

impl Temp {
    /// `None` when there is no temporary directory to compare against — which makes every
    /// answer `false` rather than every answer `true`.
    fn current() -> Option<Self> {
        let temp = temp_root();
        if temp.as_os_str().is_empty() {
            return None;
        }
        Some(Self {
            folded: folded(&temp),
            // `%TEMP%` is an 8.3 short path on some machines while `CF_HDROP` carries the long
            // form. They are the same directory and only resolving both shows it.
            resolved: std::fs::canonicalize(&temp).ok().map(|temp| folded(&temp)),
        })
    }

    /// Whether `path` is inside it.
    fn holds(&self, path: &std::path::Path) -> bool {
        // Case-folded and component-wise, through [`folded`], so `C:\Temporary` is not taken for
        // something inside `C:\Temp`.
        if folded(path).starts_with(&self.folded) {
            return true;
        }
        // Only now, and only for this one path: the syscall is worth it because a drag whose
        // `%TEMP%` is a short path fails the test above for every item, and calling that a drag
        // of the user's own files is the mistake this exists to stop.
        let Some(resolved) = self.resolved.as_ref() else {
            return false;
        };
        std::fs::canonicalize(path).is_ok_and(|path| folded(&path).starts_with(resolved))
    }
}

/// `%TEMP%`, or wherever a test has pointed it.
///
/// Every part of a claim is decided against this directory: whether a drop's items are a source's
/// materialisation ([`under_temp`]), where the staging directory is made ([`staging`]), and what
/// [`is_staging`] will authorise a `remove_dir_all` of. A test that could not move it would have to
/// build its fixture in the real `%TEMP%` to reach any of that, and the containment rule in
/// `crate::sandbox` does not allow it — which is why the claim went untested through two rounds of
/// the same bug.
fn temp_root() -> PathBuf {
    #[cfg(test)]
    if let Some(root) = TEMP_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return root;
    }
    std::env::temp_dir()
}

#[cfg(test)]
thread_local! {
    /// See [`temp_root`]. Thread-local rather than an environment variable, so that two tests
    /// running at once cannot move each other's `%TEMP%` — and neither can move the real one.
    static TEMP_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Treat `root` as `%TEMP%` for as long as the returned guard is alive.
#[cfg(test)]
fn temp_here(root: &std::path::Path) -> impl Drop {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            TEMP_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
        }
    }
    let root = root.to_path_buf();
    TEMP_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(root));
    Guard
}

/// What marks a staging directory as this program's own. See [`claim`].
const STAGING: &str = "yafe-drop-";

/// Take a source's temporary files, before it takes them back.
///
/// The problem this solves is one of timing and cannot be solved by being quick. A source that
/// materialises its data — an archiver — deletes it again as soon as `DoDragDrop` returns,
/// which is as soon as `IDropTarget::Drop` returns. So the whole of the copy would have to
/// happen inside `Drop`, on the UI thread, with the window unable to paint: the drop of a large
/// archive would freeze everything, and no amount of scoping helps, because a thread stuck
/// inside a callback cannot paint one tab and not another.
///
/// What it *can* do inside `Drop` is stop being the source's problem. The files are in `%TEMP%`
/// by definition — that is what [`under_temp`] established — so a staging directory made
/// alongside them is on the same volume, and moving them into it is a directory-entry rename:
/// microseconds, whatever the archive weighs. The source is then welcome to delete a folder
/// that is empty, and the copy to the real destination runs on the ops thread like every other
/// one, with the window live and every tab usable. When the rename is refused — which is a
/// case, not a theory, and is [`take`] — there is more to it than that.
///
/// It also makes the right-button menu safe, which it could not otherwise be: `Copy here` is
/// answered whenever the user gets round to it, long after any source has cleaned up.
///
/// Non-temporary items are returned untouched — the same rename applied to a drag from
/// Explorer would move the user's actual files into a scratch folder. That is the whole reason
/// the test is narrow.
///
/// **And the test is per item, not per drop.** It was the first item's answer applied to all of
/// them, on the grounds that no source offers files from two places at once. Explorer does: a
/// drag out of a search result, or out of Recent files, is one `CF_HDROP` spanning as many folders
/// as it matched. Searching `C:\` for a name that a program also has open — the autosave in
/// `%TEMP%` sorting above the real one in `Documents` — and dragging both here staged the pair,
/// which moved `Documents\report.docx` out of `Documents`, and then [`crate::shell::ops::Scratch`]
/// removed the staging directory when the copy was done. A copy the user asked for became a move,
/// and a copy that failed or was cancelled took the only remaining name for the file with it.
pub(crate) fn claim(items: Vec<PathBuf>) -> Vec<PathBuf> {
    let Some(temp) = Temp::current() else {
        return items;
    };
    // Asked once per item and kept, rather than asked again inside the map below: the answer can
    // cost a `canonicalize` and it cannot change under us in a way that would help.
    let materialised: Vec<bool> = items.iter().map(|item| temp.holds(item)).collect();
    if !materialised.iter().any(|&temporary| temporary) {
        return items;
    }
    let Some(staging) = staging() else {
        return items;
    };
    let claimed: Vec<PathBuf> = items
        .into_iter()
        .zip(materialised)
        .enumerate()
        .map(|(index, (item, temporary))| {
            if temporary {
                claim_one(&staging, index, item)
            } else {
                item
            }
        })
        .collect();
    // A claim that got nothing leaves an empty directory in `%TEMP%` that nothing will ever come
    // back for: the job only takes one with it when its items are *in* it — see
    // `crate::shell::ops::claimed` — and [`sweep`] leaves alone anything belonging to a process
    // that is still running. Which is how the failure that prompted all this was found in the
    // first place, sitting next to the archiver's own leftovers.
    if !claimed.iter().any(|item| item.starts_with(&staging)) {
        let _ = std::fs::remove_dir(&staging);
    }
    claimed
}

/// One item into the staging directory, under its own name.
///
/// The name has to survive: `IFileOperation` names what it copies after the source, so an item
/// staged under a different name would arrive at the destination with it.
fn claim_one(staging: &std::path::Path, index: usize, item: PathBuf) -> PathBuf {
    let Some(name) = item.file_name() else {
        return item;
    };
    let mut to = staging.join(name);
    // Two items with one name, which means they came from different folders. Resolved with a
    // folder rather than a suffix, for the reason above.
    if to.exists() {
        let nested = staging.join(index.to_string());
        if std::fs::create_dir_all(&nested).is_err() {
            return item;
        }
        to = nested.join(name);
    }
    if take(&item, &to) {
        to
    } else {
        // Nothing moved, so the original is still the right answer — and still a race.
        item
    }
}

/// Get whatever is at `from` over to `to`, by whatever means the filesystem will allow.
///
/// **A directory cannot be renamed while a single file anywhere beneath it is open.** Windows
/// answers `ERROR_ACCESS_DENIED` — 5, not the sharing violation the situation sounds like — and it
/// does so however that handle was opened: `FILE_SHARE_DELETE` and all, a plain reader is enough.
/// Renaming that same open file on its own succeeds. It is only the directory above it that
/// becomes unmovable.
///
/// That is the whole of why this bug came back after [`under_temp`] and [`claim`] had between them
/// already fixed it. Dragging *files* out of an archive claims them one rename at a time, so a held
/// file costs that file; dragging a *folder* out staked the entire tree on one rename, and anything
/// that had so much as looked at a freshly extracted file — a virus scanner is enough, and takes as
/// long as it likes over a folder full of them — refused it. The claim then gave up and returned
/// the archiver's own paths, the copy ran out of the archiver's temporary directory, and the
/// archiver deleted it part way through. Which is the original bug exactly: half the files arrive.
///
/// So a refusal is not the end of it. In order:
///
/// | | |
/// | --- | --- |
/// | rename | the whole item in one directory entry, and what nearly every claim still is |
/// | recurse, for a directory | the tree recreated and each child claimed in turn, so one held file costs one file |
/// | hard link | a second name for the same data: the source deleting *its* name leaves ours, and this is allowed where a rename is refused |
/// | copy | the bytes, when nothing cheaper is permitted — reading is the one thing a scanner's handle still allows |
///
/// Returns whether `to` is now where the item is to be found.
fn take(from: &std::path::Path, to: &std::path::Path) -> bool {
    // The fast path, and the one this is nearly always on: one directory entry rewritten,
    // microseconds, whatever the item weighs.
    if std::fs::rename(from, to).is_ok() {
        return true;
    }
    let Ok(what) = std::fs::symlink_metadata(from) else {
        return false;
    };
    // A junction or a symlink, whose rename has just been refused. A copy would follow it and
    // duplicate what it points at — which for a link to somewhere outside the extraction would be
    // copying the user's own files into a scratch folder. Left where it is instead.
    if what.file_type().is_symlink() {
        return false;
    }
    if what.is_dir() {
        return take_dir(from, to);
    }
    // A file whose rename was refused: a hard link is a second name for the same data, so the
    // source deleting its own name leaves the data reachable under ours. Failing that, the bytes.
    std::fs::hard_link(from, to).is_ok() || std::fs::copy(from, to).is_ok()
}

/// A directory whose rename was refused, one child at a time.
///
/// The children are what is claimed; the directories themselves are recreated. That loses the
/// timestamps the archive carried for the folder — the files keep theirs, since they are moved and
/// not remade — which is a real if small difference from what the copy would otherwise have
/// arrived with, and worth strictly less than the files this exists to save. Setting them would
/// mean a directory handle opened with `FILE_FLAG_BACKUP_SEMANTICS`, so it belongs in
/// `crate::windows` and not here, on a path taken only when the rename has already failed.
fn take_dir(from: &std::path::Path, to: &std::path::Path) -> bool {
    if std::fs::create_dir_all(to).is_err() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(from) else {
        return false;
    };
    let (mut arrived, mut refused) = (false, false);
    for entry in entries.flatten() {
        if take(&entry.path(), &to.join(entry.file_name())) {
            arrived = true;
        } else {
            refused = true;
        }
    }
    // Nothing came across at all and there was something to come: the source is still the better
    // answer of the two, and the empty shell made here is not one. A directory that was *already*
    // empty is a different thing, and is claimed — neither flag set.
    if !arrived && refused {
        let _ = std::fs::remove_dir(to);
        return false;
    }
    // Anything at all having moved settles it: reporting `from` now would send the copy to a
    // directory this had just emptied, which is a worse version of the bug being fixed.
    true
}

/// A directory of this program's own, directly inside `%TEMP%` — so on the same volume as
/// anything [`claim`] will put in it, which is what makes the claim a rename.
fn staging() -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);
    let temp = temp_root();
    // Bounded rather than `loop`: if something is answering `AlreadyExists` to every name this
    // can produce, the answer is to give up and let the drop go on unclaimed.
    for _ in 0..64 {
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = temp.join(format!("{STAGING}{}-{next}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Some(dir),
            Err(why) if why.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Whether a directory is a staging directory belonging to *this* process.
///
/// This is the test that authorises a recursive delete — `crate::shell::ops` removes the
/// directory a job's items were claimed into once the job is done — so it asks for both halves:
/// the name, and that the thing is sitting directly in the temporary directory. Neither on its
/// own would be enough to be trusted with `remove_dir_all`.
pub(crate) fn is_staging(dir: &std::path::Path) -> bool {
    if dir.parent() != Some(temp_root().as_path()) {
        return false;
    }
    let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(rest) = name.strip_prefix(STAGING) else {
        return false;
    };
    let Some((pid, _)) = rest.split_once('-') else {
        return false;
    };
    pid.parse::<u32>().is_ok_and(|pid| pid == std::process::id())
}

/// Remove staging directories left behind by a run that is over.
///
/// Called once at startup. Nothing should ever be left — the job that consumes a claim takes
/// the directory with it — but being killed between the claim and the copy would otherwise
/// leave an extracted archive in `%TEMP%` for good. A directory belonging to a process that is
/// still running is left alone, which is the safe way round: another window may be copying out
/// of it, and that is the exact bug all of this exists to fix.
pub fn sweep() {
    let temp = temp_root();
    let Ok(entries) = std::fs::read_dir(&temp) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(rest) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(STAGING))
        else {
            continue;
        };
        let Some(pid) = rest.split_once('-').and_then(|(pid, _)| pid.parse::<u32>().ok()) else {
            continue;
        };
        if pid == std::process::id() || running(pid) {
            continue;
        }
        // Deliberately *not* `crate::sandbox::remove`: this is the running program clearing its
        // own staging directories out of `%TEMP%`, not a test tidying a fixture. The sandbox rule
        // is about where tests are allowed to reach, and a guard here would both fail to compile
        // in a release build and panic on the one job this function exists to do.
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Whether a process id is still in use.
///
/// A handle that opens means yes, and a recycled id means yes as well — both answers keep the
/// directory, which is the harmless mistake to make. Only an id nothing answers to gets swept.
///
/// **Which is why a failure to open is not an answer of "no".** `OpenProcess` says
/// `ERROR_INVALID_PARAMETER` for an id that belongs to nothing, and that one alone means the
/// process is gone. `ERROR_ACCESS_DENIED` means the opposite: there *is* a process and it is out of
/// this one's reach — another instance running elevated, which is a thing a user does to get at a
/// protected folder. Reading that as dead let the plain instance's startup sweep `remove_dir_all`
/// the elevated one's staging directory, which is the exact failure the whole claim exists to
/// prevent: a copy running out of a folder somebody else deletes underneath it. So anything that is
/// not a positive "no such process" keeps the directory, and the worst that costs is a folder in
/// `%TEMP%` until the next launch.
#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: a query for a handle that is closed again immediately.
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(why) => why.code() != ERROR_INVALID_PARAMETER.to_hresult(),
        }
    }
}

/// Nothing is ever claimed off Windows, so nothing is ever swept.
#[cfg(not(windows))]
fn running(_pid: u32) -> bool {
    true
}

/// Private, with one exception in a test build: [`crate::shell::links`]'s naming test reaches
/// `win::link_drop_through_the_shell` — the shell's own answer to a link drop, and so the only
/// honest measuring stick for the names this program gives one.
#[cfg(all(windows, not(test)))]
#[path = "../windows/dnd.rs"]
mod win;
#[cfg(all(windows, test))]
#[path = "../windows/dnd.rs"]
pub(crate) mod win;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_drag_within_a_volume_moves_and_across_copies() {
        // The rule that makes an accidental drag cheap rather than expensive.
        assert_eq!(
            default_effect(Some(Path::new(r"C:\a\one.txt")), Path::new(r"C:\b")),
            Effect::Move
        );
        assert_eq!(
            default_effect(Some(Path::new(r"C:\a\one.txt")), Path::new(r"D:\b")),
            Effect::Copy
        );
        // Case is not a volume difference.
        assert_eq!(
            default_effect(Some(Path::new(r"c:\a\one.txt")), Path::new(r"C:\b")),
            Effect::Move
        );
    }

    #[test]
    fn a_network_share_is_its_own_volume() {
        assert_eq!(
            default_effect(
                Some(Path::new(r"\\server\share\one.txt")),
                Path::new(r"\\server\share\sub")
            ),
            Effect::Move
        );
        assert_eq!(
            default_effect(
                Some(Path::new(r"\\server\share\one.txt")),
                Path::new(r"\\server\other\sub")
            ),
            Effect::Copy,
            "two shares on one server are still two volumes"
        );
        assert_eq!(
            default_effect(
                Some(Path::new(r"\\server\share\one.txt")),
                Path::new(r"C:\b")
            ),
            Effect::Copy
        );
    }

    #[test]
    fn an_unknown_source_copies() {
        // A drag from somewhere with no volume — a virtual folder, a browser — cannot be
        // a move, and guessing otherwise would be the destructive guess.
        assert_eq!(default_effect(None, Path::new(r"C:\b")), Effect::Copy);
    }

    /// A sandbox directory standing in for `%TEMP%`, and the guard that makes it one.
    ///
    /// Everything a claim reads comes from [`temp_root`], so this is all it takes to hold the whole
    /// mechanism inside `target/sandbox` — the reason it can be tested at all now, where before
    /// checking any of it would have meant building fixtures in the real `%TEMP%`.
    fn sandboxed_temp(name: &str) -> (PathBuf, impl Drop) {
        let temp = crate::sandbox::fresh(name).join("temp");
        std::fs::create_dir_all(&temp).unwrap();
        let guard = temp_here(&temp);
        (temp, guard)
    }

    /// **A folder dragged out of an archive while a file inside it is open.**
    ///
    /// The gesture that has now lost half a decompression twice, and the second time is not a
    /// regression in this file: [`under_temp`] still calls the drop a copy, and [`claim`] still
    /// stages it. What failed is narrower and is in [`take`] — **Windows refuses to rename a
    /// directory that has any open file beneath it**, with `ERROR_ACCESS_DENIED`, whatever sharing
    /// that handle was opened with. A drag of *files* never noticed, because each one is claimed by
    /// a rename of its own; a drag of a *folder* staked the entire tree on one rename, and one
    /// handle anywhere under it — a scanner reading a freshly extracted file is enough — sent the
    /// copy back to reading out of the archiver's directory, which the archiver then deleted part
    /// way through.
    ///
    /// So the assertion is not "the claim succeeded". It is that the files are somewhere the
    /// archiver is not about to delete, under the name they have to keep.
    #[test]
    fn a_folder_is_claimed_even_when_a_file_inside_it_is_open() {
        let (temp, _temp) = sandboxed_temp("claim-a-held-folder");

        // What an archiver leaves for a folder drag: a directory of its own in `%TEMP%` with the
        // dragged folder inside it, and a tree under that.
        let master = temp.join("7zE436F3BDA").join("master");
        std::fs::create_dir_all(master.join("qml").join("QtQuick")).unwrap();
        std::fs::write(master.join("qml").join("QtQuick").join("one.qml"), b"one").unwrap();
        std::fs::write(master.join("two.txt"), b"two").unwrap();

        // The handle that does it. Nothing about it is exotic — a reader, sharing everything it is
        // able to share, which is what a scanner or a thumbnailer holds.
        let held = std::fs::File::open(master.join("qml").join("QtQuick").join("one.qml")).unwrap();
        // Stated rather than assumed: the test is worth nothing if the platform has stopped
        // refusing this, because the refusal is the thing being survived.
        assert!(
            std::fs::rename(&master, temp.join("moved")).is_err(),
            "the directory rename was allowed, so this is no longer a test of what it was written for"
        );

        let claimed = claim(vec![master.clone()]);
        drop(held);

        assert_eq!(claimed.len(), 1);
        let landed = &claimed[0];
        assert!(
            landed
                .parent()
                .is_some_and(|parent| is_staging(parent) || parent.parent().is_some_and(is_staging)),
            "the folder was left with the archiver, at {}",
            landed.display()
        );
        assert_eq!(
            landed.file_name(),
            master.file_name(),
            "the name has to survive, or the copy arrives at the destination under another one"
        );
        // The whole tree, the file that was open included.
        assert_eq!(
            std::fs::read_to_string(landed.join("qml").join("QtQuick").join("one.qml")).unwrap(),
            "one",
            "the file that was open did not come across"
        );
        assert_eq!(
            std::fs::read_to_string(landed.join("two.txt")).unwrap(),
            "two"
        );
        assert!(
            !master.join("two.txt").exists(),
            "a file was left where the archiver is about to delete it"
        );
    }

    /// The fast path, which is the one nearly every claim is still on: one rename, and the
    /// archiver no longer has it.
    #[test]
    fn a_file_is_claimed_by_moving_it_rather_than_copying_it() {
        let (temp, _temp) = sandboxed_temp("claim-a-file");

        let extracted = temp.join("7zE00E731CF");
        std::fs::create_dir_all(&extracted).unwrap();
        let one = extracted.join("one.txt");
        std::fs::write(&one, b"one").unwrap();

        let claimed = claim(vec![one.clone()]);
        assert_eq!(claimed.len(), 1);
        assert!(claimed[0].parent().is_some_and(is_staging));
        assert_eq!(std::fs::read_to_string(&claimed[0]).unwrap(), "one");
        assert!(
            !one.exists(),
            "the claim left a copy behind, so it cost the archive's weight rather than a rename"
        );
    }

    /// A file open with less sharing than that: its *own* rename is refused too, and the hard link
    /// is what gets it across. Which is the rung below the recursion and the reason there is one —
    /// a second name for the same data, so the archiver deleting its name leaves the data under
    /// ours, which is the half of it the second assertion is about.
    #[cfg(windows)]
    #[test]
    fn a_file_that_cannot_be_renamed_is_claimed_by_a_second_name_for_it() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let (temp, _temp) = sandboxed_temp("claim-an-unmovable-file");
        let master = temp.join("7zE8C57170C").join("master");
        std::fs::create_dir_all(&master).unwrap();
        let scanned = master.join("scanned.dll");
        std::fs::write(&scanned, b"bytes").unwrap();

        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(&scanned)
            .unwrap();
        assert!(
            std::fs::rename(&scanned, temp.join("moved")).is_err(),
            "the file's own rename was allowed, so this never reaches the hard link"
        );

        let claimed = claim(vec![master.clone()]);
        assert_eq!(claimed.len(), 1);
        let landed = &claimed[0];
        assert_eq!(
            std::fs::read_to_string(landed.join("scanned.dll")).unwrap(),
            "bytes",
            "the file nothing would move did not come across"
        );

        drop(held);
        crate::sandbox::remove_file(&scanned);
        assert_eq!(
            std::fs::read_to_string(landed.join("scanned.dll")).unwrap(),
            "bytes",
            "the archiver's own delete took the data with it"
        );
    }

    /// The narrow test that keeps a drag from Explorer out of all of this: a claim applied to the
    /// user's own files would move them into a scratch folder. And with nothing claimed there is
    /// nothing to stage, which is the litter this leaves in `%TEMP%` otherwise.
    #[test]
    fn files_that_are_not_a_source_s_temporary_are_left_where_they_are() {
        let (temp, _temp) = sandboxed_temp("claim-leaves-real-files");
        let theirs = temp.with_file_name("documents");
        std::fs::create_dir_all(&theirs).unwrap();

        let one = theirs.join("one.txt");
        std::fs::write(&one, b"one").unwrap();
        assert_eq!(claim(vec![one.clone()]), vec![one.clone()]);
        assert!(
            one.exists(),
            "a file that was not a materialisation was moved into a scratch folder anyway"
        );
        assert_eq!(
            std::fs::read_dir(&temp).unwrap().count(),
            0,
            "a staging directory was made for a drop that had nothing to claim"
        );
    }

    /// **One drop carrying a source's temporary *and* one of the user's own files.**
    ///
    /// The test above is the same claim with nothing in `%TEMP%`, and it passed throughout: the
    /// decision was the *first* item's, applied to all of them, so a drop that began with a
    /// materialisation staged everything behind it as well. That is not a hypothetical shape of
    /// drop — Explorer's search results are one `CF_HDROP` spanning every folder that matched, so
    /// searching `C:\` for a name a program also has open and dragging both here is exactly this
    /// list. The user's file was renamed out of its folder, the copy ran from the staging
    /// directory, and `crate::shell::ops::Scratch` then deleted that directory: a copy became a
    /// move, and a copy that failed took the file's only remaining name with it.
    #[test]
    fn a_drop_mixing_a_temporary_with_the_user_s_own_files_claims_only_the_temporary() {
        let (temp, _temp) = sandboxed_temp("claim-a-mixed-drop");

        let extracted = temp.join("7zE436F3BDA");
        std::fs::create_dir_all(&extracted).unwrap();
        let autosave = extracted.join("report.docx");
        std::fs::write(&autosave, b"autosave").unwrap();

        let documents = temp.with_file_name("documents");
        std::fs::create_dir_all(&documents).unwrap();
        let theirs = documents.join("report.docx");
        std::fs::write(&theirs, b"theirs").unwrap();

        // The materialisation first, which is the order that used to decide it for both.
        let claimed = claim(vec![autosave.clone(), theirs.clone()]);

        assert_eq!(claimed.len(), 2);
        assert!(
            claimed[0].parent().is_some_and(is_staging),
            "the source's own temporary was left with the source, at {}",
            claimed[0].display()
        );
        assert_eq!(
            claimed[1], theirs,
            "the user's file was staged, so the copy became a move"
        );
        assert!(
            theirs.exists() && std::fs::read_to_string(&theirs).unwrap() == "theirs",
            "the user's file was moved out of its own folder"
        );
    }

    /// **A drag from another program has to wake the window, or its drop highlight never appears.**
    ///
    /// The highlight is drawn from `App::drop_hover`, which is refreshed in
    /// [`crate::app::App::collect_drops`] — inside a frame. A drag this window started keeps frames
    /// coming, because [`crate::app::App::pump_drag`] asks for one every frame for the length of it.
    /// A drag out of 7-Zip, or out of Explorer, has no such thing: OLE holds the pointer, so winit
    /// sees no mouse move, and `DragOver` wrote where the drag was to a block nothing came to read.
    /// The row lit up for a drag between two panes and stayed dark for a drag out of an archive.
    ///
    /// Driven through the real `IDropTarget` rather than through a stand-in, because the whole
    /// question is what *that object* does when Windows calls it: an `hwnd` of zero is the one
    /// concession, and it is one the point conversion already makes room for.
    #[cfg(windows)]
    #[test]
    fn a_drag_from_another_program_wakes_the_window() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Ole::{IDropTarget, DROPEFFECT_COPY};
        use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;

        let shared = Arc::new(Mutex::new(Shared::default()));
        shared.lock().unwrap().targets = Targets {
            zones: vec![Region {
                rect: (0, 0, 100, 100),
                onto: Onto::Folder(PathBuf::from(r"C:\into")),
                name: "into".to_owned(),
            }],
            from: None,
        };
        let ctx = egui::Context::default();
        let target: IDropTarget = win::Target::new(shared.clone(), 0, ctx.clone()).into();

        // Frames first, so that what is asserted below is this drag's doing and not the repaint
        // every freshly built context wants for its first paint. Bounded rather than `while`, so a
        // context that went on wanting one fails the suite instead of hanging it — and stated,
        // because a context still asking would make the assertion at the end pass for the wrong
        // reason and say nothing at all.
        for _ in 0..8 {
            if !ctx.has_requested_repaint() {
                break;
            }
            let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        }
        assert!(
            !ctx.has_requested_repaint(),
            "the context never settled, so a wake could not be told from what was already pending"
        );

        let mut effect = DROPEFFECT_COPY;
        // SAFETY: an out-parameter this call owns for its duration, and no data object — `DragOver`
        // is the callback that carries none, which is why the highlight can be tested without one.
        unsafe {
            target
                .DragOver(MODIFIERKEYS_FLAGS(0), POINTL { x: 10, y: 20 }, &mut effect)
                .expect("DragOver refused");
        }

        assert_eq!(
            shared.lock().unwrap().hovering,
            Some((10, 20)),
            "the drag was not recorded, so there would be nothing to highlight"
        );
        assert!(
            ctx.has_requested_repaint(),
            "the window was not woken, so the frame that draws the highlight never runs"
        );
        assert_eq!(effect, DROPEFFECT_COPY, "a folder should take a copy");
    }

    /// **The pointer is told what the drop will do, at both ends of the gesture.**
    ///
    /// *Copy one.txt into into* — the verb, what is being carried, where it would land. Driven
    /// through the real `IDropTarget`, because that is where all three are decided and only one of
    /// them is a lookup: over a folder the verb follows the effect, and over the sidebar it is
    /// *Pin* whatever effect came out — pinning copies nothing, and a source that offers no
    /// `DROPEFFECT_LINK` has that answer degraded to a copy on the way past [`permitted`]. Reading
    /// the verb back off the effect would therefore promise a *copy* into Bookmarks for a gesture
    /// that copies nothing, which is the bug this shape exists to make impossible.
    #[cfg(windows)]
    #[test]
    fn the_pointer_is_told_what_the_drop_will_do() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Ole::{IDropTarget, DROPEFFECT_COPY};
        use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;

        let shared = Arc::new(Mutex::new(Shared::default()));
        shared.lock().unwrap().targets = Targets {
            zones: vec![
                Region {
                    rect: (0, 0, 100, 100),
                    onto: Onto::Folder(PathBuf::from(r"C:\parent\into")),
                    name: "into".to_owned(),
                },
                Region {
                    rect: (0, 100, 100, 200),
                    onto: Onto::Bookmarks,
                    name: "Bookmarks".to_owned(),
                },
            ],
            from: None,
        };
        let ctx = egui::Context::default();
        let target: IDropTarget = win::Target::new(shared.clone(), 0, ctx).into();

        let told = |at: POINTL| {
            let mut effect = DROPEFFECT_COPY;
            // SAFETY: an out-parameter this call owns for its duration, and no data object —
            // `DragOver` is the callback that carries none.
            unsafe {
                target
                    .DragOver(MODIFIERKEYS_FLAGS(0), at, &mut effect)
                    .expect("DragOver refused");
            }
            shared.lock().unwrap().telling.clone()
        };

        // With no data object there is nothing to have read, so the near end is unnamed — which is
        // the case a lazy source puts this in for real, and the sentence still has to read.
        assert_eq!(
            told(POINTL { x: 10, y: 10 }).map(|told| told.sentence()),
            Some("Copy into into".to_owned()),
            "over a folder the words follow the effect, and name the folder"
        );
        assert_eq!(
            told(POINTL { x: 10, y: 150 }).map(|told| told.sentence()),
            Some("Pin to Bookmarks".to_owned()),
            "over the sidebar the gesture is a pin, whatever effect the source allowed"
        );
        assert_eq!(
            told(POINTL { x: 400, y: 400 }),
            None,
            "nowhere that takes a drop has nothing to promise"
        );
    }

    /// **One item in the way refuses the whole selection, and the sentence names that item.**
    ///
    /// A folder and a file dragged together, driven through the real `IDropTarget` because the
    /// refusal and the words are decided in the same callback and the effect is the third thing
    /// that has to agree with them: nothing may land — see [`refuses`] — so `DROPEFFECT_NONE` comes
    /// out, which is what makes the drop a no-op wherever it is let go.
    ///
    /// The near end of the sentence is the **item**, not the load: *Cannot move src into itself*
    /// and not *Cannot move 2 items into itself*, which would be a sentence about something nobody
    /// dragged. See [`culprit`]. And the same selection over an ordinary folder is described by
    /// what it is carrying, as any allowed drop is.
    #[cfg(windows)]
    #[test]
    fn one_item_in_the_way_refuses_the_selection_and_is_the_one_named() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Ole::{
            IDropTarget, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE,
        };
        use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;

        let folder = PathBuf::from(r"C:\parent\src");
        let shared = Arc::new(Mutex::new(Shared::default()));
        shared.lock().unwrap().targets = Targets {
            zones: vec![
                Region {
                    rect: (0, 0, 100, 100),
                    onto: Onto::Folder(folder.clone()),
                    name: "src".to_owned(),
                },
                Region {
                    rect: (0, 100, 100, 200),
                    onto: Onto::Folder(folder.join("main")),
                    name: "main".to_owned(),
                },
                Region {
                    rect: (0, 200, 100, 300),
                    onto: Onto::Folder(PathBuf::from(r"C:\parent\docs")),
                    name: "docs".to_owned(),
                },
                Region {
                    rect: (0, 300, 100, 400),
                    onto: Onto::Bookmarks,
                    name: "Bookmarks".to_owned(),
                },
            ],
            from: None,
        };
        // A folder and a file in the air together: the selection the whole of this is about.
        let items = vec![folder.clone(), PathBuf::from(r"C:\parent\one.txt")];
        let ctx = egui::Context::default();
        let target: IDropTarget =
            win::Target::holding(shared.clone(), ctx, items, false).into();

        let told = |y: i32| {
            // Both effects offered, so nothing is degraded on the way out and the verb is the one
            // the same-volume rule picked — see [`default_effect`] and `permitted`.
            let mut effect = DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0);
            // SAFETY: an out-parameter this call owns for its duration, and no data object —
            // `DragOver` is the callback that carries none.
            unsafe {
                target
                    .DragOver(MODIFIERKEYS_FLAGS(0), POINTL { x: 10, y }, &mut effect)
                    .expect("DragOver refused");
            }
            let said = shared.lock().unwrap().telling.clone();
            (effect, said.map(|told| told.sentence()))
        };

        assert_eq!(
            told(10),
            (
                DROPEFFECT_NONE,
                Some("Cannot move src into itself".to_owned())
            ),
            "the folder in the selection cannot go into itself, so none of the selection goes"
        );
        assert_eq!(
            told(150),
            (
                DROPEFFECT_NONE,
                Some("Cannot move src into main, which is inside it".to_owned())
            ),
            "nor into what is inside it, and the reason is the tail of the sentence"
        );
        assert_eq!(
            told(350),
            (DROPEFFECT_NONE, Some("Cannot pin a file".to_owned())),
            "one file refuses a pin of the whole selection"
        );
        assert_eq!(
            told(250),
            (
                DROPEFFECT_MOVE,
                Some("Move 2 items into docs".to_owned())
            ),
            "and a folder beside them takes all of it, described by what is carried"
        );
    }

    /// **A move into the folder the items are already in is nothing, and a copy there is not.**
    ///
    /// See [`does_nothing`]. The distinction is the whole of it: a move asks for the folder a name
    /// already has, and Ctrl over that same folder asks for `one - Copy.txt`, which is a gesture
    /// people use on purpose. A right drag has not decided which it is, so it keeps its feedback
    /// and the menu on drop is where it says so.
    #[test]
    fn a_move_into_the_folder_the_items_are_in_does_nothing_and_a_copy_does_not() {
        let here = PathBuf::from(r"C:\work");
        let file = here.join("one.txt");
        let folder = here.join("src");
        let elsewhere = PathBuf::from(r"D:\other\two.txt");
        let into = Onto::Folder(here.clone());
        let mine = [file.clone()];

        // ---- Already there: a move is nothing, whatever it is carrying ----
        assert!(does_nothing(&into, &mine, true, false));
        let both = [file.clone(), folder.clone()];
        assert!(
            does_nothing(&into, &both, true, false),
            "a folder is in its parent the same way a file is"
        );
        assert!(
            does_nothing(
                &Onto::Folder(PathBuf::from(r"C:\WORK")),
                &mine,
                true,
                false
            ),
            "the same folder with a different shift key held"
        );

        // ---- And every way it is something after all ----
        assert!(
            !does_nothing(&into, &mine, false, false),
            "a copy into the folder a file is in is `one - Copy.txt`"
        );
        assert!(
            !does_nothing(&into, &mine, true, true),
            "a right drag has not said which it is yet, and its menu is where it will"
        );
        let mixed = [file.clone(), elsewhere.clone()];
        assert!(
            !does_nothing(&into, &mixed, true, false),
            "one item from elsewhere has somewhere to go, so the drop is not empty"
        );
        assert!(
            !does_nothing(&Onto::Folder(folder.clone()), &mine, true, false),
            "a subfolder is a different folder"
        );
        assert!(
            !does_nothing(&Onto::Bookmarks, &mine, true, false),
            "pinning is not a move and has nothing to do nothing of"
        );
        assert!(
            !does_nothing(&into, &[], true, false),
            "a source that has not named its files must not be refused for it"
        );

        // ---- The primitive underneath, which the drop acts on again ----
        assert!(already_in(&file, &here));
        assert!(already_in(&folder, &here));
        assert!(!already_in(&file, &here.join("src")));
        assert!(!already_in(&elsewhere, &here));
    }

    /// **The pane a drag came out of hears nothing about the drop it started on.**
    ///
    /// A folder is over its own row for the first inch of every drag of it, and that drop is
    /// refused — so a sign there would mark the *start of the gesture* as a mistake. It is not one:
    /// it is where the folder is. See [`Shared::silent`], and note what does **not** change: the
    /// effect is still `DROPEFFECT_NONE` and the reason is still published, so nothing lands and
    /// the highlight the frame loop draws still stands down. Only the saying of it goes — the
    /// words, their mark, and the no-entry cursor OLE would otherwise put up.
    ///
    /// Three zones, because the rule has to be narrow to be right. The same folder's row in
    /// *another* pane is a deliberate aim at a wrong answer and says so; a *different* refusal
    /// inside the pane the drag came from — the folder over something inside itself — is a
    /// deliberate aim too, and the pointer had no reason to pass over it on the way anywhere.
    #[cfg(windows)]
    #[test]
    fn the_pane_a_drag_came_out_of_hears_nothing_about_it() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Ole::{IDropTarget, DROPEFFECT_MOVE, DROPEFFECT_NONE};
        use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;

        let folder = PathBuf::from(r"C:\parent\src");
        let shared = Arc::new(Mutex::new(Shared::default()));
        shared.lock().unwrap().targets = Targets {
            zones: vec![
                // The row the drag came off, in the pane it came from.
                Region {
                    rect: (0, 0, 100, 100),
                    onto: Onto::Folder(folder.clone()),
                    name: "src".to_owned(),
                },
                // A folder inside it, in that same pane.
                Region {
                    rect: (0, 100, 100, 200),
                    onto: Onto::Folder(folder.join("main")),
                    name: "main".to_owned(),
                },
                // And the same row again, in the pane below.
                Region {
                    rect: (0, 200, 100, 300),
                    onto: Onto::Folder(folder.clone()),
                    name: "src".to_owned(),
                },
            ],
            // The top half of the window is the pane the drag was picked up in.
            from: Some((0, 0, 100, 200)),
        };
        let ctx = egui::Context::default();
        let target: IDropTarget =
            win::Target::holding(shared.clone(), ctx, vec![folder], true).into();

        let told = |y: i32| {
            let mut effect = DROPEFFECT_MOVE;
            // SAFETY: an out-parameter this call owns for its duration, and no data object —
            // `DragOver` is the callback that carries none.
            unsafe {
                target
                    .DragOver(MODIFIERKEYS_FLAGS(0), POINTL { x: 10, y }, &mut effect)
                    .expect("DragOver refused");
            }
            let shared = shared.lock().unwrap();
            (
                effect,
                shared.telling.as_ref().map(Told::sentence),
                shared.silent,
            )
        };

        let (effect, sentence, silent) = told(50);
        assert!(silent, "the drag is being signed where it started");
        assert_eq!(
            effect, DROPEFFECT_NONE,
            "the drop is still refused, whatever the pointer says about it"
        );
        assert_eq!(
            sentence.as_deref(),
            Some("Cannot move src into itself"),
            "and the reason is still published, for the highlight to stand down by"
        );

        let (effect, sentence, silent) = told(250);
        assert!(!silent, "in another pane the same refusal is worth saying");
        assert_eq!(effect, DROPEFFECT_NONE);
        assert_eq!(sentence.as_deref(), Some("Cannot move src into itself"));

        let (_, sentence, silent) = told(150);
        assert!(
            !silent,
            "aiming a folder at something inside it is a mistake in any pane"
        );
        assert_eq!(
            sentence.as_deref(),
            Some("Cannot move src into main, which is inside it")
        );
    }

    /// **A drop that would do nothing is offered nothing: no effect, no words, no cursor.**
    ///
    /// The pointer's half of [`does_nothing`], driven through the real `IDropTarget` because the
    /// two things that decide it arrive there and nowhere else: the effect the gesture is asking
    /// for, and the button carrying it.
    ///
    /// **In any pane and out of any program**, which is why no [`Targets::from`] is published here
    /// — two panes on the same folder, or Explorer dragging a file back into the folder it is
    /// showing, are the same nothing as a drag that never left home. And `telling` is `None` rather
    /// than a refusal, because there is no mistake to name: what stands the destination's highlight
    /// down is [`Shared::silent`] itself.
    #[cfg(windows)]
    #[test]
    fn a_drop_that_would_do_nothing_is_not_offered() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Ole::{
            IDropTarget, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE,
        };
        use windows::Win32::System::SystemServices::{MK_CONTROL, MK_RBUTTON, MODIFIERKEYS_FLAGS};

        let here = PathBuf::from(r"C:\work");
        let file = here.join("one.txt");
        let shared = Arc::new(Mutex::new(Shared::default()));
        shared.lock().unwrap().targets = Targets {
            zones: vec![
                // The folder the file is already in.
                Region {
                    rect: (0, 0, 100, 100),
                    onto: Onto::Folder(here.clone()),
                    name: "work".to_owned(),
                },
                // And a folder it could actually go into.
                Region {
                    rect: (0, 100, 100, 200),
                    onto: Onto::Folder(here.join("src")),
                    name: "src".to_owned(),
                },
            ],
            from: None,
        };
        let ctx = egui::Context::default();
        let target: IDropTarget =
            win::Target::holding(shared.clone(), ctx, vec![file], false).into();

        let told = |y: i32, keys: MODIFIERKEYS_FLAGS| {
            let mut effect = DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0);
            // SAFETY: an out-parameter this call owns for its duration, and no data object —
            // `DragOver` is the callback that carries none.
            unsafe {
                target
                    .DragOver(keys, POINTL { x: 10, y }, &mut effect)
                    .expect("DragOver refused");
            }
            let shared = shared.lock().unwrap();
            (
                effect,
                shared.telling.as_ref().map(Told::sentence),
                shared.silent,
            )
        };
        let plain = MODIFIERKEYS_FLAGS(0);

        // Over the folder it is in: a move with nowhere to go, so nothing at all.
        assert_eq!(
            told(50, plain),
            (DROPEFFECT_NONE, None, true),
            "a move into the folder the file is in was offered as something"
        );

        // Ctrl over the same folder is a copy, and that is `one - Copy.txt`.
        assert_eq!(
            told(50, MODIFIERKEYS_FLAGS(MK_CONTROL.0)),
            (
                DROPEFFECT_COPY,
                Some("Copy one.txt into work".to_owned()),
                false
            ),
            "a copy into the folder a file is in is a gesture people use on purpose"
        );

        // And the right button is a question rather than a move, so it keeps its feedback.
        let (effect, sentence, silent) = told(50, MODIFIERKEYS_FLAGS(MK_RBUTTON.0));
        assert!(!silent, "a right drag is asked about, not hushed");
        assert_eq!(effect, DROPEFFECT_MOVE);
        assert!(sentence.is_some());

        // A folder that would actually take it is untouched by any of this.
        assert_eq!(
            told(150, plain),
            (
                DROPEFFECT_MOVE,
                Some("Move one.txt into src".to_owned()),
                false
            )
        );
    }

    /// **The four modifiers, and Explorer's order of precedence between them.**
    ///
    /// | held | effect | said |
    /// | --- | --- | --- |
    /// | nothing | the volume rule — a move, here | *Move one.txt into src* |
    /// | Ctrl | copy | *Copy one.txt into src* |
    /// | Shift | move | *Move one.txt into src* |
    /// | Alt | link | *Link to one.txt in src* |
    /// | Ctrl+Shift | link, the same as Alt | *Link to one.txt in src* |
    ///
    /// The last row is the one with a trap in it, and it is the same trap `Ctrl+Shift+T` has in
    /// [`crate::app::App::keyboard`]: `Ctrl` alone is *also* true when Shift is down, so a pair
    /// tested after its halves never wins. Ctrl+Shift asked for a plain copy until it was tested
    /// first, and nothing about that reads as wrong in the source.
    ///
    /// Alt is passed here as `MK_ALT`, which is what an OLE drag reports it as when it reports it.
    /// The other half of that answer — the physical key, for a drag loop that does not — cannot be
    /// reached from a test, because it is a real key under a real pointer; see `win::alt_held`.
    #[test]
    fn the_modifiers_ask_for_copy_move_and_link() {
        use windows::Win32::Foundation::POINTL;
        use windows::Win32::System::Ole::{
            IDropTarget, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE,
        };
        use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};

        let here = PathBuf::from(r"C:\work");
        let file = here.join("one.txt");
        let shared = Arc::new(Mutex::new(Shared::default()));
        shared.lock().unwrap().targets = Targets {
            zones: vec![Region {
                rect: (0, 0, 100, 100),
                onto: Onto::Folder(here.join("src")),
                name: "src".to_owned(),
            }],
            from: None,
        };
        let ctx = egui::Context::default();
        let target: IDropTarget =
            win::Target::holding(shared.clone(), ctx, vec![file], false).into();

        let told = |keys: u32| {
            // All three offered, so nothing is degraded on the way out and what comes back is
            // what the gesture asked for. See `permitted`.
            let mut effect =
                DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0);
            // SAFETY: an out-parameter this call owns for its duration, and no data object —
            // `DragOver` is the callback that carries none.
            unsafe {
                target
                    .DragOver(
                        MODIFIERKEYS_FLAGS(keys),
                        POINTL { x: 10, y: 50 },
                        &mut effect,
                    )
                    .expect("DragOver refused");
            }
            let said = shared.lock().unwrap().telling.as_ref().map(Told::sentence);
            (effect, said)
        };

        const ALT: u32 = windows::Win32::System::Ole::MK_ALT;
        let moving = (DROPEFFECT_MOVE, Some("Move one.txt into src".to_owned()));
        let copying = (DROPEFFECT_COPY, Some("Copy one.txt into src".to_owned()));
        let linking = (DROPEFFECT_LINK, Some("Link to one.txt in src".to_owned()));

        assert_eq!(told(0), moving, "the same volume, so the plain drag moves");
        assert_eq!(told(MK_CONTROL.0), copying);
        assert_eq!(told(MK_SHIFT.0), moving);
        assert_eq!(told(ALT), linking, "Alt asks for a shortcut");
        assert_eq!(
            told(MK_CONTROL.0 | MK_SHIFT.0),
            linking,
            "Ctrl+Shift is the other way of asking, and must not come out as a plain copy"
        );
        // And Alt wins over either of them, which is what a hand resting on Ctrl needs it to do.
        assert_eq!(told(ALT | MK_CONTROL.0), linking);
        assert_eq!(told(ALT | MK_SHIFT.0), linking);
    }

    /// A link into the folder the items are already in is a **real gesture**, unlike a move there.
    ///
    /// `one.txt.lnk` appears beside `one.txt`, which is what Explorer does and is occasionally the
    /// point. So [`does_nothing`] must not hush it the way it hushes a move — the rule is keyed on
    /// `moving` for exactly this reason, and this is the third caller of it after the copy and the
    /// right drag.
    #[test]
    fn a_link_into_the_folder_the_items_are_in_is_something() {
        let here = PathBuf::from(r"C:\work");
        let into = Onto::Folder(here.clone());
        let mine = [here.join("one.txt")];

        assert!(
            does_nothing(&into, &mine, true, false),
            "a move there is still nothing"
        );
        assert!(
            !does_nothing(&into, &mine, false, false),
            "a link or a copy there makes a new name and is not nothing"
        );
    }

    /// **What a drag is carrying is a name while there is one, and a count past that.**
    ///
    /// The near end of the sentence — see [`carrying`]. One file is *one.txt*, because that is the
    /// file in front of you; four are *4 items*, because four names under the pointer is not a
    /// label anybody reads. And a drag whose source has rendered nothing yet has no near end at
    /// all, which is a case rather than a guard: an archiver is entitled to hold its files back
    /// until the drop is real.
    #[test]
    fn a_drag_is_described_by_its_one_name_or_by_how_many() {
        assert_eq!(carrying(&[]), None);
        assert_eq!(
            carrying(&[PathBuf::from(r"C:\docs\one.txt")]).as_deref(),
            Some("one.txt")
        );
        assert_eq!(
            carrying(&[
                PathBuf::from(r"C:\docs\one.txt"),
                PathBuf::from(r"C:\docs\two.txt"),
            ])
            .as_deref(),
            Some("2 items")
        );
        // A folder dragged by its own name, and a root by the only name it has.
        assert_eq!(
            carrying(&[PathBuf::from(r"C:\docs\src")]).as_deref(),
            Some("src")
        );
        assert_eq!(carrying(&[PathBuf::from(r"C:\")]).as_deref(), Some("C:"));
    }

    /// **A folder cannot be dropped into itself, or into anything inside it.**
    ///
    /// The test [`swallows`] exists for, and the two ways of getting it wrong are both here: a
    /// **case** difference, because `Path::starts_with` is case-sensitive and Windows paths are
    /// not, and a **prefix** that is not a parent — `C:\src2` shares five characters with `C:\src`
    /// and is nowhere near inside it, which a `to_string_lossy().starts_with()` would have called
    /// a descendant.
    #[test]
    fn a_folder_swallows_itself_and_everything_under_it() {
        let src = Path::new(r"C:\work\src");
        assert!(swallows(src, src), "onto itself is the same refusal");
        assert!(swallows(src, Path::new(r"C:\work\src\main")));
        assert!(swallows(src, Path::new(r"C:\work\src\main\java")));
        assert!(
            swallows(Path::new(r"C:\Work\Src"), Path::new(r"c:\work\src\main")),
            "one folder inside itself with a different shift key held"
        );
        assert!(
            !swallows(src, Path::new(r"C:\work\src2")),
            "a folder whose name starts the same way is not inside it"
        );
        assert!(!swallows(src, Path::new(r"C:\work")), "its parent is not");
        assert!(!swallows(src, Path::new(r"D:\work\src\main")));
        // A file cannot swallow anything: nothing is inside it.
        assert!(!swallows(
            Path::new(r"C:\work\one.txt"),
            Path::new(r"C:\work")
        ));
    }

    /// **The drops this program will not make, and why each one is refused.**
    ///
    /// See [`refuses`]. All of it is decided from what the destination *is*, so all of it is
    /// answerable while the drag is still moving — which is the whole point: a refusal after the
    /// button comes up is a dialog, and a refusal before it is a cursor, a sentence and a mark.
    ///
    /// The *reason* is asserted and not merely the refusal, because the reason is what the sentence
    /// is made of: [`Refused::Itself`] and [`Refused::Inside`] are two different mistakes and read
    /// as two different sentences.
    #[test]
    fn a_file_cannot_be_pinned_and_a_folder_cannot_go_inside_itself() {
        let file = PathBuf::from(r"C:\work\one.txt");
        let folder = PathBuf::from(r"C:\work\src");
        let a_file = [file.clone()];
        let a_folder = [folder.clone()];
        let both = [folder.clone(), file];
        let bookmarks = Onto::Bookmarks;
        let group = Onto::BookmarkGroup(0);

        // ---- The sidebar takes folders, and only folders ---------------
        assert_eq!(
            refuses(&bookmarks, &a_file, false),
            Some(Refused::AFile),
            "a file is not a place to go, so the sidebar will not have it"
        );
        assert_eq!(
            refuses(&group, &a_file, false),
            Some(Refused::AFile),
            "nor will a group"
        );
        assert_eq!(
            refuses(&bookmarks, &both, false),
            Some(Refused::AFile),
            "one file in the selection refuses the whole of it: half a gesture is worse than none"
        );
        assert_eq!(
            refuses(&bookmarks, &a_folder, true),
            None,
            "a folder is exactly what it is for"
        );

        // ---- A folder cannot go inside itself --------------------------
        assert_eq!(
            refuses(&Onto::Folder(folder.clone()), &a_folder, true),
            Some(Refused::Itself),
            "onto the very folder being dragged"
        );
        assert_eq!(
            refuses(&Onto::Folder(PathBuf::from(r"C:\WORK\SRC")), &a_folder, true),
            Some(Refused::Itself),
            "the same folder with a different shift key held"
        );
        let inside = Onto::Folder(PathBuf::from(r"C:\work\src\main"));
        assert_eq!(refuses(&inside, &a_folder, true), Some(Refused::Inside));
        assert_eq!(
            refuses(&Onto::Folder(PathBuf::from(r"C:\work\docs")), &a_folder, true),
            None,
            "a folder beside it is an ordinary destination"
        );

        // ---- And one such folder refuses the selection it is in --------
        //
        // The same rule as the sidebar's above, and for the same reason: the alternative is four of
        // five items moved and the fifth left where it was, with nothing on screen having said so.
        assert_eq!(
            refuses(&Onto::Folder(folder.clone()), &both, true),
            Some(Refused::Itself),
            "the folder in the selection cannot go into itself, so none of it goes"
        );
        assert_eq!(
            refuses(&inside, &both, true),
            Some(Refused::Inside),
            "nor into what is inside it, however much of the selection could have gone"
        );
        assert_eq!(
            refuses(&Onto::Folder(PathBuf::from(r"C:\work\docs")), &both, true),
            None,
            "and a folder beside them takes the whole selection"
        );

        // ---- The item a refusal is about, which is the one it names ----
        //
        // See [`culprit`]. The nearer mistake wins: with `src` and `C:\work` both in the air over
        // `C:\work\src`, it is `src` that cannot go into itself, and saying `C:\work` instead would
        // point at the wrong folder.
        let nested = [PathBuf::from(r"C:\work"), folder.clone()];
        assert_eq!(
            culprit(&nested, &folder).map(crate::fs::display_name).as_deref(),
            Some("src")
        );
        assert_eq!(
            refuses(&Onto::Folder(folder.clone()), &nested, true),
            Some(Refused::Itself),
            "and the refusal says the same thing the name does"
        );
        assert_eq!(
            culprit(&both, &PathBuf::from(r"C:\work\docs")),
            None,
            "nothing in the way, nothing to name"
        );

        // ---- And a drag that has said nothing refuses nothing ----------
        assert_eq!(
            refuses(&bookmarks, &[], false),
            None,
            "a source that has not rendered its files yet must not be refused for it"
        );
        assert_eq!(refuses(&inside, &[], false), None);
    }

    /// **Both ends of the sentence are the blue ones, and nothing else is.**
    ///
    /// What the accent is *for* here: the two names are the answer and the rest is grammar. A run
    /// marked wrong would put the verb in blue and the folder in grey, which reads as emphasis on
    /// the wrong half of the promise.
    #[test]
    fn the_two_names_are_the_blue_part_of_the_sentence() {
        let told = Told {
            doing: Doing::Move,
            refused: None,
            source: Some("one.txt".to_owned()),
            target: "docs".to_owned(),
        };
        fn blue_of(told: &Told) -> Vec<&str> {
            told.runs()
                .into_iter()
                .filter(|(_, blue)| *blue)
                .map(|(text, _)| text)
                .collect()
        }

        assert_eq!(told.sentence(), "Move one.txt into docs");
        assert_eq!(
            blue_of(&told),
            ["one.txt", "docs"],
            "the wrong runs are picked out"
        );

        // A refusal is the same sentence about the same gesture, with the verb negated and the
        // reason on the end of it — the two names are still the two names, so they are still blue.
        let inside = Told {
            refused: Some(Refused::Inside),
            ..told.clone()
        };
        assert_eq!(
            inside.sentence(),
            "Cannot move one.txt into docs, which is inside it"
        );
        assert_eq!(blue_of(&inside), ["one.txt", "docs"]);

        // Onto the folder being dragged, the far end is a word rather than the same name twice — so
        // there is one name in the sentence and one blue run.
        let itself = Told {
            refused: Some(Refused::Itself),
            ..told.clone()
        };
        assert_eq!(itself.sentence(), "Cannot move one.txt into itself");
        assert_eq!(blue_of(&itself), ["one.txt"]);

        // And the sidebar's refusal names nothing at all, so nothing in it is blue.
        let a_file = Told {
            doing: Doing::Pin,
            refused: Some(Refused::AFile),
            ..told.clone()
        };
        assert_eq!(a_file.sentence(), "Cannot pin a file");
        assert!(blue_of(&a_file).is_empty());

        // And with no near end, the one name there is stays the blue one — and the words either
        // side of the hole do not run together.
        let unnamed = Told {
            source: None,
            ..told
        };
        assert_eq!(unnamed.sentence(), "Move into docs");
        assert_eq!(blue_of(&unnamed), ["docs"]);
    }

    #[test]
    fn zones_resolve_the_front_one_first() {
        let region = |rect, path: &str| Region {
            rect,
            onto: Onto::Folder(PathBuf::from(path)),
            name: path.rsplit('\\').next().unwrap_or(path).to_owned(),
        };
        let under = region((0, 0, 100, 100), r"C:\under");
        let over = region((50, 50, 150, 150), r"C:\over");
        let targets = Targets {
            zones: vec![
                Region {
                    rect: (0, 0, 100, 100),
                    onto: Onto::Bookmarks,
                    name: "Bookmarks".to_owned(),
                },
                under.clone(),
                over.clone(),
            ],
            from: None,
        };
        assert_eq!(
            targets.at((60, 60)),
            Some(&over),
            "the later zone is the one in front"
        );
        assert_eq!(
            targets.at((10, 10)),
            Some(&under),
            "a listing over the sidebar's own zone means the listing"
        );
        assert_eq!(targets.at((200, 200)), None);
        // And the name comes back with it, which is what the pointer is told the drop will do.
        assert_eq!(targets.at((60, 60)).map(|region| region.name.as_str()), Some("over"));
    }
}
