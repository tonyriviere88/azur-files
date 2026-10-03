//! The path bar: history buttons, a segmented breadcrumb, refresh and the filter.
//!
//! Modelled on Explorer's, which is still the best version of this control:
//!
//! - Every **segment** is a button that goes there.
//! - Every **chevron** between segments opens that folder's subfolders, so you can
//!   step sideways into a sibling without going up first.
//! - The **leading chevron** lists the drives.
//! - When the path is too long to fit, the leading segments collapse into a `…`
//!   that opens them as a menu — the current folder is never the part that
//!   disappears.
//!
//! With one difference from Explorer, and it is the reason the bar is drawn from
//! [`crate::pane::Tab::trail`] rather than from the current path: **going up does not trim
//! the bar.** Walk out of `src\ui` and the bar still reads `… › src › ui`, with the folder
//! you are now in in bold and the two you came out of still there to be clicked. Explorer
//! cuts them off, and then the only way back is to open a chevron and read a menu to find a
//! name that was on screen a moment ago.
//! - **Clicking the empty space** past the last segment turns the whole thing into
//!   an editable path field, as does `Ctrl+L`. `Enter` navigates, `Esc` puts it
//!   back — and what is typed into it completes, from the same dropdown a chevron
//!   opens. See [`PathComplete`]. Right-clicking it offers `Use / in path`, which is
//!   which slash it writes between the parts of a path — see [`slash_menu`].

use azur_egui_theme::components::{MenuItem, Size};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::{radius, space};
use egui::{pos2, vec2, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};
use std::path::{Path, PathBuf};

use crate::app::Action;
use crate::fs;
use crate::icons;
use crate::pane::{Lens, PaneId, Tab};
use crate::theme::Theme;
use crate::ui::{text_left, tool_button, truncated, TOOL_SIZE};

/// The bar's height: `tokens::control::MEDIUM`, so the buttons in it are Azur's
/// small size with room to breathe.
pub const HEIGHT: f32 = 32.0;

/// A point off the top of every segment label.
///
/// A line of text centred in a box sits centred on its *line box*, which includes the
/// descender space under the baseline — so beside a chevron, which is centred on its own
/// ink, the text reads a point low. Nudging the text rather than the chevrons because the
/// chevrons are what the eye tracks along the trail.
const TEXT_LIFT: f32 = 1.0;

/// And a point onto the chevrons between segments, for the same reason from the other
/// side. Together the two put a `›` on the middle of the names it separates.
const ARROW_DROP: f32 = 1.0;

/// Two points off the top of what is typed in the filter box, and of its placeholder.
///
/// [`TEXT_LIFT`]'s rule applied inside a field rather than beside one, and two points instead of one
/// because of what the box is *in*: a row of six painted glyphs — Back, Forward, Up, Refresh, the
/// filter's own funnel, the flatten and the eye — every one of them centred on its own ink. The
/// field's frame is what agrees with their boxes; the words in it were left agreeing with nothing,
/// two points under the line the rest of the bar reads along. It is the figure
/// [`crate::ui::filelist::CELL_LIFT`] and `chrome::TEXT_LIFT` already use for the same correction.
///
/// The box, the funnel and the ✕ stay put — see `TextField::text_lift`, which is what carries this
/// into the field, and which does not move the things that are already right.
const FILTER_TEXT_LIFT: f32 = 2.0;

/// The pen at the right-hand end, and the room reserved for it.
///
/// Larger than [`crate::ui::TOOL_ICON`], which every other glyph on this bar is drawn at, and
/// deliberately: the others are centred in a 24-point button whose fill and hover give them
/// their presence, and this one has no button around it at all. At 14 it read as a speck. 18 is
/// still under the bar's 24 points of room, so nothing about the bar's height changes.
///
/// Its space comes out of the trail's, so a path deep enough to fill the bar stops short of the
/// pen rather than running underneath it.
pub(crate) const PEN: f32 = 18.0;

/// Subfolders for whichever chevron dropdown is open.
///
/// One at a time, so one cache is enough. Read on the frame the menu opens rather
/// than on every frame it is showing — a popup body runs continuously, and reading
/// `C:\Windows\System32` sixty times a second to draw the same list would be a
/// self-inflicted stall.
#[derive(Default)]
pub struct CrumbMenu {
    path: Option<PathBuf>,
    items: Vec<(String, PathBuf)>,
    truncated: bool,
    /// Which of the bar's dropdowns is showing: the pane whose bar it is on, and the segment
    /// the chevron sits in front of — or [`OVERFLOW_MENU`] for the `…`.
    ///
    /// **This one field is also the bar's tracking mode.** While it is set, the whole bar
    /// behaves as one control: moving the pointer onto another segment or chevron brings the
    /// dropdown with it, without a second click. That is what Explorer's address bar does, and
    /// it is why the state cannot live where egui keeps a menu's open state by default — per
    /// trigger, toggled by that trigger's own click, which is right for a button and cannot
    /// say "the same menu, somewhere else along the bar". See
    /// [`azur_egui_theme::components::Menu::open`].
    ///
    /// It leaves on its own: clicking an entry, clicking away and `Escape` all close the popup,
    /// and the frame that notices puts this back to `None`.
    open: Option<(PaneId, usize)>,
}

/// The `…` button's place in [`CrumbMenu::open`].
///
/// It is one of the bar's dropdowns and takes part in the same tracking as the chevrons — it
/// simply has no segment of its own to be numbered after, being what stands in for the ones
/// that did not fit.
const OVERFLOW_MENU: usize = usize::MAX;

impl CrumbMenu {
    /// Fill the cache for `path`, unless it already holds it.
    fn ensure(&mut self, path: &Path) {
        if self.path.as_deref() == Some(path) {
            return;
        }
        // A directory of subdirectories is the only thing this menu shows, so the
        // listing is filtered as it is read rather than after.
        let dir = fs::scan::scan(path);
        let mut items: Vec<(String, PathBuf)> = Vec::new();
        for i in 0..dir.len() {
            let entry = &dir.entries[i];
            if !entry.is_dir() || entry.is_hidden() {
                continue;
            }
            items.push((dir.name(i).to_owned(), dir.target(i)));
        }
        items.sort_by(|a, b| fs::sort::natural_cmp(&a.0, &b.0));
        // A menu is for picking one of a few; past this it is a listing, and the
        // listing is what the pane behind it is for.
        self.truncated = items.len() > MENU_LIMIT;
        items.truncate(MENU_LIMIT);

        self.path = Some(path.to_path_buf());
        self.items = items;
    }

    /// What the open dropdown is listing: the folder it read, and how many subfolders it
    /// found. For the tests, which is how "the chevron opens and has content in it" is
    /// checked without hunting for a menu row's rect.
    #[cfg(test)]
    pub fn listing(&self) -> Option<(&Path, usize)> {
        self.path.as_deref().map(|path| (path, self.items.len()))
    }

    /// Which dropdown is showing, for the tests: the pane, and the segment its chevron is in
    /// front of. Also whether the bar is tracking, which is the same fact.
    #[cfg(test)]
    pub fn showing(&self) -> Option<(PaneId, usize)> {
        self.open
    }

    /// Put the dropdown away, and with it the tracking mode it turns on.
    ///
    /// The popup itself closes because `open` is what the `Menu` is told to read — see
    /// [`azur_egui_theme::components::Menu::open`] — so there is nothing else to dismiss.
    pub fn close(&mut self) {
        self.open = None;
    }
}

const MENU_LIMIT: usize = 200;

// ---------------------------------------------------------------------------
// Completing what is typed into the path field
// ---------------------------------------------------------------------------

/// How many offers the completion dropdown shows before it starts scrolling.
///
/// **Stated here rather than left to `Menu`'s own 320-point ceiling**, which is 11.43 rows — so the
/// twelfth was drawn as a two-point sliver along the bottom edge. That is worse than either answer
/// either side of it: a list you walk with the arrow keys should end where a *row* ends, because a
/// half-drawn row reads as a rendering fault, and saying "there is more below" is the scrollbar's
/// job and it already does it.
///
/// Ten is a number the eye takes in without counting. Past this many candidates the listing behind
/// the dropdown is the better way to find what you are after — which is the same reasoning
/// [`MENU_LIMIT`] rests on, two orders of magnitude further out.
pub(crate) const OFFERS_SHOWN: usize = 10;

/// What the path field is offering to complete, and which of it the keyboard is on.
///
/// **The chevron dropdown, opened by typing.** The bar already answers "what is inside that
/// folder" with a menu of folder rows carrying the shell's own icons, and a completion is the
/// same question asked with the keyboard instead of the pointer — so it is the same
/// [`azur_egui_theme::components::Menu`], the same [`MenuItem`]s and the same icons, and the only
/// new part is what moves down it:
///
/// - **Down** and **Up** walk the offers, and either one puts the dropdown up if it is not.
///   Past the last and before the first is *what you typed*, which is how you get back to your
///   own text without deleting anything.
/// - **Right** and **Tab** put the highlighted name in the field with the separator after it, so
///   what is offered next is what is inside it. That is the whole gesture: `Down Right Down
///   Right` walks a tree from the keyboard without a pointer or an `Enter` anywhere in it.
/// - **Enter** on an offer goes there. On nothing, it navigates to what is typed, exactly as it
///   did before any of this existed.
///
/// Matching is by prefix and **case-insensitive**, because a Windows file name is: having to
/// match the case of a folder whose name you are asking for would be no help at all.
///
/// **Nothing here reads the disk.** The folder is asked of [`crate::loader`] — the same service
/// the listings come from, which reads on a worker — and the offers are whatever has come back.
/// See [`crate::fs::typed_folder`] for why that matters more here than anywhere else in the
/// window: this runs on a keystroke, against whatever has been typed, and what has been typed
/// can name a share that is not there.
#[derive(Default)]
pub struct PathComplete {
    /// Whose field this is, and `None` when none is open.
    ///
    /// The reset is keyed on it: a field that has just opened starts with nothing highlighted and
    /// nothing showing, which is [`Self::hidden`]'s first job.
    pane: Option<PaneId>,
    /// The text the offers were worked out from. Anything else in the field means they are stale.
    typed: String,
    /// The folder already asked of the loader, so one it cannot read is asked for once.
    ///
    /// A failed read is deliberately not cached — see [`crate::loader::Loader`], and it is the
    /// right rule, because a share can come back. But it means `cached` keeps saying no, and an
    /// ask driven off that answer alone would be a fresh scan of a dead path on every frame,
    /// each one waking the window to ask again.
    asked: Option<PathBuf>,
    /// Whether the listing behind [`Self::offers`] had actually arrived. While it has not, the
    /// offers are rebuilt on every frame — which is two frames, the keystroke's and the one the
    /// scan lands on.
    ready: bool,
    /// Every child of the typed folder whose name starts with the typed name: what to show, and
    /// where it leads.
    offers: Vec<(String, PathBuf)>,
    /// Whether there were more than [`MENU_LIMIT`] of them.
    truncated: bool,
    /// Which offer the keyboard is on. `None` is what was typed, and is where it starts: `Enter`
    /// on a field you have only typed into has always gone where the text says, and a highlight
    /// that arrived by itself would quietly change what that key does.
    hot: Option<usize>,
    /// Put away — the field has only just opened, or something outside the dropdown was clicked.
    /// The next keystroke brings it back, and so does Down.
    hidden: bool,
    /// Bring the highlighted offer into view, on the frame the highlight moves and not on the
    /// ones after it: a list that scrolled itself every frame could not be scrolled by hand.
    follow: bool,
}

/// What a key asked of the offers.
enum Pick {
    /// Put the highlighted name in the field, and offer what is inside it.
    Append,
    /// Go there.
    Go,
}

impl PathComplete {
    /// Work out what to offer for what is in the field.
    ///
    /// Called before anything is drawn and before a key is read, because both depend on it: what
    /// Down and Right do is a question about whether there is anything to move onto. Costs two
    /// comparisons on a frame where neither the text nor the listing has changed, which is nearly
    /// all of them.
    fn refresh(&mut self, pane: PaneId, text: &str, loader: &mut crate::loader::Loader) {
        let opened = self.pane != Some(pane);
        if opened || text != self.typed {
            self.pane = Some(pane);
            self.typed = text.to_owned();
            self.hot = None;
            self.ready = false;
            // **Nothing is offered for a path that has only been shown.** `Ctrl+L` fills the
            // field with where you already are, so on the frame it opens the one thing that
            // matches is the folder you are standing in — a dropdown in the way, saying
            // something you can already read on the bar behind it. The first keystroke brings it
            // up, and so does Down.
            self.hidden = opened;
        }

        let (prefix, leaf) = split_typed(text);
        let folder = fs::typed_folder(prefix);
        // Asked once per folder, and once only for one that cannot be read. See `asked`.
        if self.asked != folder {
            self.asked = folder.clone();
            if let Some(folder) = &folder {
                loader.prefetch(folder);
            }
        }
        if self.ready {
            return;
        }

        self.offers.clear();
        self.truncated = false;
        match &folder {
            // Nothing names a folder yet, and what a path typed from nothing can still become is
            // a volume: `d` offers `D:`. Explorer offers your history here as well; this program
            // keeps none, and a guess about where you meant is worse than a short list of places
            // that are certainly there.
            None => {
                for drive in fs::drives::list_letters() {
                    if starts_with_folded(&drive.letter, leaf) {
                        self.offers.push((drive.letter, drive.path));
                    }
                }
            }
            Some(folder) => {
                // Whatever the loader already has. A miss leaves the offers empty and `ready`
                // false, and the worker wakes the window when it lands.
                let Some(dir) = loader.cached(folder) else {
                    return;
                };
                for i in 0..dir.len() {
                    let entry = &dir.entries[i];
                    // Folders only. The field takes a file too — `Enter` on one opens it — but a
                    // completion that offered every file in `C:\Windows` would bury the four
                    // folders in it, and it is the folders you are typing through.
                    if !entry.is_dir() {
                        continue;
                    }
                    // **Hidden folders are offered once a name is being typed, and not before.**
                    // With nothing after the separator this is the chevron menu's question and
                    // gets the chevron menu's answer, which leaves `$Recycle.Bin` and `System
                    // Volume Information` off the top of every drive. With a name half typed it
                    // is a different question, and a folder that exists and is not offered reads
                    // as a bug in the completion.
                    if entry.is_hidden() && leaf.is_empty() {
                        continue;
                    }
                    let name = dir.name(i);
                    if !starts_with_folded(name, leaf) {
                        continue;
                    }
                    self.offers.push((name.to_owned(), dir.target(i)));
                }
                self.offers.sort_by(|a, b| fs::sort::natural_cmp(&a.0, &b.0));
                // A menu is for picking one of a few, exactly as it is for a chevron.
                self.truncated = self.offers.len() > MENU_LIMIT;
                self.offers.truncate(MENU_LIMIT);
            }
        }
        self.ready = true;
    }

    /// Move the highlight, and put the dropdown up if it was down.
    ///
    /// Past either end is `None` — what was typed — rather than a wrap straight round to the
    /// other end: the text you wrote is one of the choices, and a list that stepped over it would
    /// leave `Escape`, which throws the whole field away, as the only way back to it.
    fn step(&mut self, down: bool) {
        self.hidden = false;
        self.follow = true;
        let last = self.offers.len().saturating_sub(1);
        self.hot = match (self.hot, down) {
            (None, true) => Some(0),
            (None, false) => Some(last),
            (Some(at), true) if at >= last => None,
            (Some(at), true) => Some(at + 1),
            (Some(0), false) => None,
            (Some(at), false) => Some(at - 1),
        };
    }

    /// Put the highlighted offer in the field: the prefix exactly as it was typed, the offer's
    /// name, and the separator that starts the next one.
    ///
    /// **The prefix is not rewritten**, which is why [`split_typed`] cuts the text rather than the
    /// resolved path: somebody who typed `%appdata%\` or `~\` keeps what they typed, and the field
    /// stays something they can read. Returns where the field now points, or `None` if nothing was
    /// highlighted.
    fn accept(&self, text: &mut String, slashes: bool) -> Option<PathBuf> {
        let (name, path) = self.offers.get(self.hot?)?;
        let (prefix, _) = split_typed(text);
        // The separator they have been using. A path typed with forward slashes lists, navigates
        // and draws a breadcrumb — `fs::normalize` is what sees to that at the door — so turning
        // one into a mixture of both at the moment of helping would be this program's own doing.
        //
        // With none used yet the setting decides, and that is not a corner: a path typed from
        // nothing completes to a *drive* first, so `d` becomes `D:/` with `Use / in path` on and
        // `D:\` with it off. From then on there is a prefix again, carrying the separator this
        // put there. See [`with_separator`].
        let sep = match prefix.chars().next_back() {
            Some('/') => '/',
            Some('\\') => '\\',
            _ if slashes => '/',
            _ => std::path::MAIN_SEPARATOR,
        };
        let next = format!("{prefix}{name}{sep}");
        *text = next;
        Some(path.clone())
    }

    /// The field on `pane` has gone. Forget what it was offering, so the next one opens fresh.
    ///
    /// Keyed on the pane because both panes draw their own bar every frame, and the one without a
    /// field open must not clear the state of the one that has.
    fn close(&mut self, pane: PaneId) {
        if self.pane != Some(pane) {
            return;
        }
        *self = Self::default();
    }

    /// Show the offers as though the last character had been typed rather than put there.
    ///
    /// For `--path=`, which fills the field from outside and would otherwise get the dropdown a
    /// freshly opened one has: down, because a path that has only been *shown* has nothing to say.
    /// Claiming the pane here is what makes [`Self::refresh`] read the text as a change rather than
    /// as an opening — the same distinction, approached from the other side.
    pub fn type_ahead(&mut self, pane: PaneId) {
        self.pane = Some(pane);
        self.typed.clear();
        self.hidden = false;
    }

    /// The field's text has been rewritten from outside and it is the same path: leave the dropdown
    /// exactly as it was, up or down.
    ///
    /// [`Self::type_ahead`]'s opposite, and for the one thing that does this — `Use / in path`,
    /// ticked with a field open, which swaps every separator in it. To [`Self::refresh`] that is
    /// indistinguishable from a keystroke, and a keystroke puts the dropdown up: tick the setting on
    /// a field holding where you are and a list of the folder you are standing in appears under it,
    /// which is precisely the list `Ctrl+L` goes out of its way not to show.
    ///
    /// Claiming the text is what does it — `refresh` reads an unchanged field as nothing having
    /// happened. The offers are still rebuilt, because they carry the old text's prefix, and the
    /// highlight goes because they are about to move under it.
    pub fn rewritten(&mut self, pane: PaneId, text: &str) {
        if self.pane != Some(pane) {
            return;
        }
        self.typed = text.to_owned();
        self.ready = false;
        self.hot = None;
    }

    /// What is on offer and which of it is highlighted, for the tests.
    #[cfg(test)]
    pub fn offering(&self) -> (Vec<&str>, Option<usize>) {
        (
            self.offers.iter().map(|(name, _)| name.as_str()).collect(),
            self.hot,
        )
    }

    /// Whether the dropdown is up, which is the same question as whether there is anything in it
    /// that has not been put away.
    #[cfg(test)]
    pub fn showing(&self) -> bool {
        !self.hidden && !self.offers.is_empty()
    }
}

/// Split what has been typed into the folder part and the name being typed inside it.
///
/// The separator stays with the folder, so the two halves put back together are exactly the text
/// that came in — which is what lets [`PathComplete::accept`] leave everything in front of the
/// name alone.
fn split_typed(text: &str) -> (&str, &str) {
    match text.rfind(['\\', '/']) {
        // Both separators are one byte, so this is a character boundary.
        Some(at) => text.split_at(at + 1),
        None => ("", text),
    }
}

/// Whether `name` begins with `typed`, whatever case either of them is in.
///
/// Character by character rather than `to_lowercase()` on both, because this runs once per entry
/// per keystroke over folders that can hold thousands of them: the allocation is the expensive
/// part, and a comparison stops at the first character that differs. `char::to_lowercase` rather
/// than the ASCII fold, because a folder can be named in any language and this is the one place
/// where getting that wrong means a folder you can see is not offered.
fn starts_with_folded(name: &str, typed: &str) -> bool {
    let mut name = name.chars().flat_map(char::to_lowercase);
    let mut typed = typed.chars().flat_map(char::to_lowercase);
    loop {
        match (typed.next(), name.next()) {
            // Everything typed has been matched.
            (None, _) => return true,
            // The name ran out first, or the two disagree.
            (Some(_), None) => return false,
            (Some(a), Some(b)) => {
                if a != b {
                    return false;
                }
            }
        }
    }
}

/// Put the caret at the end of the field.
///
/// For the one moment the field's text is changed by something other than the field: a name has
/// just been appended, and egui keeps a `TextEdit`'s caret in its own memory, where it knows
/// nothing about a buffer written from outside. Without this the caret stays where the typing left
/// it and the next keystroke lands in the middle of the name that was just completed.
///
/// Stored after the widget has run, so it is the *next* frame that reads it — which is the frame
/// the next keystroke arrives in.
fn caret_to_end(ctx: &egui::Context, id: Id, text: &str) {
    use egui::text::{CCursor, CCursorRange};

    let Some(mut state) = egui::TextEdit::load_state(ctx, id) else {
        return;
    };
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(text.chars().count()))));
    state.store(ctx, id);
}

/// A path written with the separator the path field is set to show.
///
/// Both directions, because the setting can be turned off as well as on, and exact either way:
/// **neither slash can appear in a Windows file name**, so every one of them in a path is a
/// separator and nothing else. It is the same length in bytes as what came in, which is why nothing
/// has to be done about the caret afterwards — the character it sits in front of is still there.
///
/// Not [`crate::fs::normalize`], which is the same swap in the one direction the disk cares about
/// and hands back a `PathBuf`. This is about what the field *shows*, and what the field holds is a
/// `String` somebody may be halfway through typing.
pub(crate) fn with_separator(text: &str, slashes: bool) -> String {
    if slashes {
        text.replace('\\', "/")
    } else {
        text.replace('/', "\\")
    }
}

/// A menu entry with a tick in its icon slot when it is on.
///
/// The icon slot rather than a checkbox, which is what every desktop menu does with a toggle —
/// and `MenuItem` reserves the slot for every entry, so the labels line up whether or not
/// anything is ticked.
fn ticked<'a>(item: MenuItem<'a>, on: bool) -> MenuItem<'a> {
    if on {
        item.icon(&azur_egui_theme::icons::check)
    } else {
        item
    }
}

/// The preview button's own menu: whether the panel is showing, and where it goes.
///
/// **Sticky**, which is `azur::components::ContextMenu`'s word for "these are settings, not
/// commands": ticking one of three positions and having the menu vanish means reopening it to see
/// what you did. A menu of commands should close — dismissal is how a reader knows the command was
/// taken — and this one is not.
///
/// Where the panel goes is the *window's* preference and not this folder's, which is why `layout`
/// comes in from `App` while `open` comes off the tab. A radio group in a menu reads as a setting,
/// and a setting that only applied to the folder you happened to be in when you chose it would be
/// a setting nobody could rely on.
fn position_menu(
    ui: &Ui,
    trigger: &egui::Response,
    pane: PaneId,
    open: bool,
    layout: &mut crate::ui::preview::Layout,
    out: &mut Vec<Action>,
) {
    use azur_egui_theme::components::{collection_label, menu_divider, ContextMenu};

    ContextMenu::new(trigger)
        .sticky(true)
        .show(ui.ctx(), |ui| {
            if ui
                .add(ticked(
                    MenuItem::new("Show preview").shortcut("Ctrl+P"),
                    open,
                ))
                .clicked()
            {
                out.push(Action::TogglePreview(pane));
            }
            menu_divider(ui);
            collection_label(ui, "Position");
            for at in crate::ui::preview::Where::ALL {
                if ui
                    .add(ticked(MenuItem::new(at.label()), layout.at == at))
                    .clicked()
                {
                    layout.at = at;
                    out.push(Action::RememberLayout);
                }
            }
        });
}

/// The flatten button's own menu: whether the tree is showing, and which of the two ways.
///
/// The same shape as [`position_menu`] and for the same reasons — **sticky**, because these are
/// settings rather than commands, and the radio group is the *window's* preference while the
/// toggle above it is this folder's. A right click on the control that opens a thing is where
/// people look for the settings of that thing, and it keeps two radio buttons off a path bar with
/// no room for them.
///
/// Picking a mode does not turn the view on, exactly as picking a preview position does not open
/// the panel: the entry above is what does that, it is right there, and a menu whose settings
/// silently perform actions is a menu you stop opening to look at. What it *does* do is take
/// effect at once on every pane already showing a tree — see [`crate::app::Action::SetFlatMode`].
///
/// **Regroup single folders** is the third setting and it belongs to the tree rather than to the
/// button, which is why it sits under the two modes rather than beside the toggle at the top: it is
/// what a `Tree` looks like, and in a `List` there is no shape for it to change. Ticked while it is
/// on, which it is by default — see [`crate::config::Config::regroup`] — and enabled either way,
/// because a setting that greys out in the mode you are not in is a setting you cannot find when you
/// go looking for why the last tree looked like that.
///
/// On This PC there is no menu, because there is no button: `tool_button` senses hover alone while
/// it is disabled, so a right click there never reaches this. That is the same answer the button
/// gives — a machine's volumes are not a tree, and each of them is a place to flatten of its own.
fn flatten_menu(
    ui: &Ui,
    trigger: &egui::Response,
    pane: PaneId,
    flat: bool,
    mode: crate::pane::FlatMode,
    regroup: bool,
    out: &mut Vec<Action>,
) {
    use azur_egui_theme::components::{collection_label, menu_divider, ContextMenu};

    ContextMenu::new(trigger)
        .sticky(true)
        .show(ui.ctx(), |ui| {
            if ui
                .add(ticked(
                    MenuItem::new("Flatten this folder").shortcut("Ctrl+E"),
                    flat,
                ))
                .clicked()
            {
                out.push(Action::ToggleFlat(pane));
            }
            menu_divider(ui);
            collection_label(ui, "Show as");
            for as_what in crate::pane::FlatMode::ALL {
                if ui
                    .add(ticked(MenuItem::new(as_what.label()), mode == as_what))
                    .clicked()
                {
                    out.push(Action::SetFlatMode(as_what));
                }
            }
            // What the tree does with a folder that holds nothing but one folder. **Under the modes
            // but behind a rule**, because it is neither of the two things above it: not a third mode
            // — the group it would join is a radio group, and a tick in the middle of one reads as a
            // mode you can have as well as `Tree` — and not a command like the toggle at the top. Its
            // own line says so before the words do.
            menu_divider(ui);
            if ui
                .add(ticked(MenuItem::new("Regroup single folders"), regroup))
                .clicked()
            {
                out.push(Action::SetRegroup(!regroup));
            }
        });
}

/// The funnel's menu: the listings a *name* cannot ask for.
///
/// **A left click, and `Menu` rather than `ContextMenu`.** The two menus above it hang off buttons
/// that already do something, so theirs is the second gesture and the right button is where it
/// belongs. This button has no first gesture — opening this *is* what it does — and a control whose
/// only purpose is behind the button most people never try there is a control nobody finds. That is
/// the whole reason the `@git` word became this: what it cost was every reader who never learnt it.
///
/// **Not sticky**, unlike those two, because these are commands rather than settings: each entry
/// rebuilds the listing behind the menu, and what a reader wants next is to see it. Dismissal is
/// also how they know the command was taken.
///
/// Ticked, and **ticking the one on show is how it is turned off** — a listing you are already
/// looking at cannot be asked for again, so the tick is the only thing the entry can usefully mean
/// the second time. One at a time for the reason [`Lens`] gives: they are two questions, not two
/// halves of one.
fn funnel_menu(
    ui: &Ui,
    trigger: &egui::Response,
    pane: PaneId,
    lens: Option<Lens>,
    out: &mut Vec<Action>,
) {
    azur_egui_theme::components::Menu::new(trigger).show(ui.ctx(), |ui| {
        for which in Lens::ALL {
            let on = lens == Some(which);
            if ui.add(ticked(MenuItem::new(which.label()), on)).clicked() {
                out.push(Action::SetLens {
                    pane,
                    lens: (!on).then_some(which),
                });
            }
        }
    });
}

/// The path field's own menu: which slash it writes between the parts of a path.
///
/// **Sticky**, like the two button menus above and for the same reason — it holds a setting rather
/// than a command, and a menu that vanished on the tick would have to be reopened to see what the
/// tick did. Here that matters more than it does up there, because what it did is *behind* the menu:
/// the path in the field, rewritten.
///
/// One entry, and it hangs off the field rather than off the bar because the field is the only thing
/// the setting is about — a breadcrumb has no separators in it to change. Which slash you want is a
/// question about where the path is going next: `\` is what Windows shows and what its own dialogs
/// take, and `/` is what a shell, a URL and nearly every source file want, which is a conversion
/// otherwise done by hand every time a path is copied out of here.
///
/// Returns where the menu is while it is showing, because [`edit_field`] has to know whether a click
/// that took the keyboard off the field landed in here or somewhere else entirely.
fn slash_menu(
    ui: &Ui,
    trigger: &egui::Response,
    slashes: bool,
    out: &mut Vec<Action>,
) -> Option<Rect> {
    use azur_egui_theme::components::ContextMenu;

    ContextMenu::new(trigger)
        .sticky(true)
        .show(ui.ctx(), |ui| {
            if ui
                .add(ticked(MenuItem::new("Use / in path"), slashes))
                .clicked()
            {
                out.push(Action::SetForwardSlashes(!slashes));
            }
        })
        .map(|shown| shown.response.rect)
}

/// Draw the bar inside `rect`.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &mut Tab,
    menu: &mut CrumbMenu,
    complete: &mut PathComplete,
    loader: &mut crate::loader::Loader,
    icons_cache: &mut crate::shell::icons::Icons,
    layout: &mut crate::ui::preview::Layout,
    flat_mode: crate::pane::FlatMode,
    regroup: bool,
    slashes: bool,
    out: &mut Vec<Action>,
) {
    // **The filter, applied once the typing stops.** Before anything is drawn, so the listing
    // below is laid out from the order this settles on rather than a frame behind it.
    //
    // The repaint is not optional: this program is idle between events, so a keystroke's frame is
    // the last one there will be until something else happens. Without asking for the frame that
    // notices the deadline, a filter typed and left alone would apply whenever the pointer next
    // moved. Asked for again on every frame that is early, because a frame that arrives for some
    // other reason at 40 ms does not stop the clock — and egui only promises *no later than*.
    if let Some(left) = tab.settle_filter(ui.input(|i| i.time)) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(left));
    }

    // The bar's own surface, and the colour of a selected tab: the tab in the strip above is
    // welded to the bar directly under it, so the two are one surface with the pane's listing
    // hanging off it. `crate::ui::seam` is where that colour is decided, once.
    let surface = crate::ui::seam(t);
    ui.painter().rect_filled(rect, CornerRadius::ZERO, surface);

    let center = rect.center().y;
    let button = |x: f32| {
        Rect::from_min_size(
            pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
            vec2(TOOL_SIZE, TOOL_SIZE),
        )
    };

    // ---- Getting about ---------------------------------------------------
    //
    // Back, Forward, Up, Refresh. Refresh is here rather than out at the right-hand end beside
    // the filter because it is the same kind of thing as the other three — it acts on the folder
    // you are looking at — and because that end is the end that gets given up on a narrow pane.
    // It is part of the group that is never dropped now, which is the point of moving it.
    let mut x = rect.left() + space::S2;
    for (glyph, tip, enabled, action) in [
        (
            &icons::arrow_left as azur_icons::Icon<'_>,
            "Back (Alt+Left)",
            tab.can_go_back(),
            Action::Back(pane),
        ),
        (
            &icons::arrow_right,
            "Forward (Alt+Right)",
            tab.can_go_forward(),
            Action::Forward(pane),
        ),
        (
            &icons::arrow_up,
            "Up (Alt+Up)",
            fs::parent_of(&tab.path).is_some(),
            Action::Up(pane),
        ),
        (
            &icons::refresh,
            "Refresh (F5)",
            true,
            Action::Refresh(pane),
        ),
    ] {
        if tool_button(
            ui,
            t,
            button(x),
            Id::new(("nav", pane, tip)),
            glyph,
            tip,
            enabled,
            false,
            surface,
        )
        .clicked()
        {
            out.push(action);
        }
        x += TOOL_SIZE;
    }

    x += space::S2;
    ui.painter().rect_filled(
        Rect::from_min_size(pos2(x.round(), center - 8.0), vec2(1.0, 16.0)),
        CornerRadius::ZERO,
        t.stroke.subtle,
    );
    x += 1.0 + space::S2;

    // ---- The right-hand end -----------------------------------------------
    //
    // The filter, and nothing else now. Measured before the path and given up when the pane is
    // narrow: a filter box on top of a truncated path is worse than a path you can read. The
    // buttons at the left are never dropped — Back with nowhere to click is the one thing a path
    // bar cannot do without.
    let mut right = rect.right() - space::S2;
    /// The path needs at least this much to be worth showing at all.
    const MIN_PATH: f32 = 96.0;
    let room_for = |right: f32, want: f32| right - want - x >= MIN_PATH;

    // **A box with something in it is not dropped**, whether that something is typed or is a lens on
    // the funnel: a listing narrowed by a control that has gone off the bar is the trap the flatten
    // button is kept for, and the funnel is the only way back off a lens. The lens is a stronger case
    // than the text, in fact — a filter you cannot see is at least a filter you know you typed.
    let filter_width = if !tab.filter.is_empty() || tab.lens.is_some() || rect.width() > 460.0 {
        160.0_f32.min((rect.width() - 260.0).max(0.0))
    } else {
        0.0
    };
    if filter_width >= 90.0 && room_for(right, filter_width) {
        let field = Rect::from_min_size(
            pos2(
                (right - filter_width).round(),
                (center - TOOL_SIZE * 0.5).round(),
            ),
            vec2(filter_width, TOOL_SIZE),
        );
        // Square, like the bar it sits in. See [`crate::ui::squared`].
        //
        // The mark is taken before the field paints anything, so [`crate::ui::nudge_caret`] has a
        // place to start looking for the caret afterwards. See [`crate::ui::CARET_SHORTER`].
        let first_shape = crate::ui::shape_mark(ui);
        let response = crate::ui::squared(ui, |ui| {
            ui.put(
                field,
                azur_egui_theme::components::TextField::new(&mut tab.filter)
                    .placeholder("Filter")
                    // The funnel's room, and the funnel itself drawn further down rather than here:
                    // it is a *button* now. See [`funnel_menu`], and the block that puts it there.
                    .prefix_room(true)
                    .clearable(true)
                    .size(Size::Small)
                    .width(filter_width)
                    // See [`FILTER_TEXT_LIFT`]: the words come up to the line the bar's glyphs
                    // are on, and the box stays where it is.
                    .text_lift(FILTER_TEXT_LIFT),
            )
        });
        crate::ui::nudge_caret(
            ui,
            first_shape,
            crate::ui::CARET_SHORTER,
            crate::ui::CARET_LOWER,
        );
        // **What the box understands, where it can be found.** Every word narrows, `!` excludes and
        // `^` and `$` hold an end — invisible otherwise, because a filter field looks exactly the
        // same whether it takes one substring or four terms. Every marker is worded by
        // `azur_egui_theme::filter` rather than by this program, and the list is that library's
        // straight through: the syntax is the component's, so the field says the same thing in every
        // window that has one and the two cannot drift. What this window adds to a filter is not
        // syntax at all — it is on the funnel at the head of this box. See [`crate::pane::Lens`].
        //
        // Drawn rather than handed over as a string, because it is a table: the markers are
        // `text-secondary` against their meanings' `text-primary`, and a meaning has to start at the
        // same x on every line. The caption face is not monospaced, so that column is measured and
        // painted — [`crate::ui::tooltip_table`], which is the listing's row tooltip's table as well.
        let response = azur_egui_theme::components::tooltip_ui(response, |ui| {
            ui.label(
                egui::RichText::new(azur_egui_theme::filter::RULE)
                    .font(t.fonts.caption.clone())
                    .color(t.text.primary),
            );
            ui.add_space(azur_egui_theme::tokens::space::S1);
            crate::ui::tooltip_table(ui, t, azur_egui_theme::filter::MARKERS);
        });
        if response.changed() {
            // Noted, not applied — see `Tab::settle_filter` at the top of this function, and
            // `pane::FILTER_DELAY` for the 240 ms one pass can cost.
            tab.filter_changed(ui.input(|i| i.time));
        }
        // `Ctrl+F` puts the caret here without the user having to find it, and so does **`F3`** —
        // one key, no chord, and the key a great many Windows programs have meant "find" with
        // since long before `Ctrl+F` was the convention. Nothing else in this window wants it:
        // `F2` renames and `F5` re-reads, and both of those are in `App::keyboard` where a
        // shortcut with no field to hand focus to belongs. These two are here because they need
        // the field's own response, and they arrive with it — so, like the box itself, they are
        // there when the pane is wide enough to draw it and not when it is not.
        //
        // A `TextField` selects what it holds when focus arrives — including focus handed to it
        // this way, after the widget has already run, which is what `focus_arrived` is for — so
        // either key lands ready for a query to be typed over the last one.
        // **`F3` stands aside while another field has the keyboard**, and `Ctrl+F` does not. That is
        // the difference between a bare key and a chord: a chord is unambiguous wherever it is
        // pressed, and a bare key belongs to whatever is being typed into. The panel's find bar is
        // the field this is for — `F3` there means *find next*, which is what the key has meant in
        // every Windows program for thirty years, and it would be unreachable if the path bar took
        // the keystroke first. Which it would: the bar is drawn before the panel.
        //
        // Focus rather than a flag from the panel, because the rule is about any field and not that
        // one. And this field is excepted from its own guard: `F3` with the caret already here should
        // not be inert, it should simply be where it already is.
        let elsewhere = ui
            .ctx()
            .memory(|m| m.focused())
            .is_some_and(|id| id != response.id);
        if ui.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::F,
            )) || (!elsewhere
                && i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::NONE,
                    egui::Key::F3,
                )))
        }) {
            response.request_focus();
        }

        // ---- The funnel, which is a button -----------------------------------
        //
        // In the room the field kept for it — `prefix_room` above, and
        // `components::prefix_rect` is where that room is — so it sits inside the box's own frame
        // exactly where the glyph did. Drawn *after* the field, which is what makes it clickable at
        // all: egui gives a point to the last widget registered over it, and the field's input covers
        // this corner. It is the ordering the ✕ at the other end relies on too.
        //
        // A 24-point button will not fit inside a 24-point field, so it is the ✕'s 18 — the design
        // system's `.clearButton` size, and this is the same kind of thing at the other end of the
        // same box. Everything else about it is [`crate::ui::tool_button`]'s: subtle at rest so the
        // box still reads as a box, the bar's own hover and press, and **latched while a lens is on**,
        // which is the one thing on screen that says a listing has been narrowed by something you
        // cannot see in the text.
        let funnel = azur_egui_theme::components::prefix_rect(field, Size::Small).expand(2.0);
        let response = tool_button(
            ui,
            t,
            funnel,
            Id::new(("filter-lens", pane)),
            &icons::filter,
            // What the menu is for, rather than a name for the button. The box beside it filters by
            // the name; this is where the other questions are, and saying so is what stops it reading
            // as decoration on a field.
            "Filter by something other than the name",
            true,
            tab.lens.is_some(),
            surface,
        );
        funnel_menu(ui, &response, pane, tab.lens, out);
        right = field.left() - space::S2;
    }

    // Flatten, immediately before the filter — the two are the same kind of thing, a question
    // asked of the folder you are looking at rather than somewhere to go, and both are given up
    // together when the pane is too narrow for the path. Latched while it is on, which is what
    // says the listing on show is not this folder's own children.
    //
    // It is *not* dropped with the filter when the box itself is hidden but the room is there:
    // a 24-point button is affordable long after a 160-point field is not, and a flatten you can
    // turn on and not off would be a trap. The order of the two `room_for` tests below is what
    // that comes down to.
    //
    // **It carries a context menu**, which is where the choice between the two flatten modes
    // lives: one flat list of everything under the folder, or the tree it came from. The same
    // arrangement as the preview button's below, for the same reason — a right click on the
    // control that opens a thing is where the settings of that thing belong.
    if room_for(right, TOOL_SIZE) {
        let rect = button(right - TOOL_SIZE);
        let response = tool_button(
            ui,
            t,
            rect,
            Id::new(("flatten", pane)),
            &icons::flatten,
            // What the button *does* rather than which mode it will do it in: the mode is a
            // setting one right click away, and a tooltip that changed its wording underneath a
            // button that behaves the same either way would read as two different buttons.
            "Flatten this folder's whole tree (Ctrl+E)",
            // Nothing to flatten on This PC, whose rows are drives — see `Tab::toggle_flat`.
            !tab.path.as_os_str().is_empty(),
            tab.flat,
            surface,
        );
        if response.clicked() {
            out.push(Action::ToggleFlat(pane));
        }
        flatten_menu(ui, &response, pane, tab.flat, flat_mode, regroup, out);
        right = rect.left() - space::S2;
    }

    // The preview toggle, before the flatten one. Both are questions asked about the folder
    // rather than places to go, so they belong at this end — and this one is furthest from the
    // filter because it is the least to do with it.
    //
    // **It carries a context menu**, which is where the panel's position lives: show or hide,
    // and then Right, Bottom or Auto. A right click on the control that opens a thing is where
    // people look for the settings of that thing, and it keeps three radio buttons off a path
    // bar that has no room for them.
    if room_for(right, TOOL_SIZE) {
        let rect = button(right - TOOL_SIZE);
        let response = tool_button(
            ui,
            t,
            rect,
            Id::new(("preview", pane)),
            &icons::eye,
            "Preview the selected file (Ctrl+P)",
            true,
            tab.preview.open,
            surface,
        );
        if response.clicked() {
            out.push(Action::TogglePreview(pane));
        }
        position_menu(ui, &response, pane, tab.preview.open, layout, out);
        right = rect.left() - space::S2;
    }

    // Refresh used to be here, and is now in the group at the left. Nothing else is: the star
    // that pinned the folder to Bookmarks is gone. `Ctrl+D` still does it, and so does the
    // folder's own context menu, which is where the rest of what you can do to a folder lives —
    // a toggle button whose two states are a hollow star and a filled one was a permanent
    // fixture spending most of its life saying nothing.

    // ---- The path itself -------------------------------------------------
    let path_rect = Rect::from_min_max(pos2(x, rect.top()), pos2(right.max(x), rect.bottom()));
    if tab.editing_path {
        edit_field(
            ui, t, path_rect, pane, tab, complete, loader, icons_cache, slashes, out,
        );
    } else {
        // No field on this pane, so nothing it was offering is still wanted. A no-op for the
        // pane that does have one open — see [`PathComplete::close`].
        complete.close(pane);
        segments(ui, t, path_rect, pane, tab, menu, icons_cache, slashes, out);
    }
}

/// The editable path field, and the completions under it.
#[allow(clippy::too_many_arguments)]
fn edit_field(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &mut Tab,
    complete: &mut PathComplete,
    loader: &mut crate::loader::Loader,
    icons_cache: &mut crate::shell::icons::Icons,
    slashes: bool,
    out: &mut Vec<Action>,
) {
    let field = Rect::from_min_size(
        pos2(rect.left(), (rect.center().y - TOOL_SIZE * 0.5).round()),
        vec2(rect.width(), TOOL_SIZE),
    );

    // **What is on offer, before anything is drawn and before a key is read.** Both of the things
    // below need it: what Down and Right do is a question about whether there is anything to move
    // onto. See [`PathComplete::refresh`], which is cheap on the frames where nothing changed.
    complete.refresh(pane, &tab.edit_text, loader);

    // ---- The keys the dropdown takes -------------------------------------
    //
    // Read *and consumed* here, above the widget, because every one of them already means
    // something to a `TextEdit`: Up and Down move the caret, Right moves it a character, Enter
    // surrenders the keyboard and Tab hands it to the next widget in the window. A field that saw
    // them first would answer them first, and this is the only place they can be taken out of the
    // queue before it runs.
    //
    // Right and Enter are only taken while an offer is highlighted. With nothing highlighted they
    // are the field's own, which is the whole reason the highlight starts at nothing: `Enter` on a
    // path you have typed out in full goes there, as it always has, and `Right` moves the caret.
    // Nothing is ever highlighted while the dropdown is down — every path that puts it away clears
    // the highlight with it — so those two need no separate test for that.
    let mut pick: Option<Pick> = None;
    if !complete.offers.is_empty() {
        ui.input_mut(|i| {
            const NONE: egui::Modifiers = egui::Modifiers::NONE;
            // Down and Up put the dropdown up if it is not up *and* land on an offer, in the one
            // press: an arrow that only revealed a list, and then had to be pressed again to move
            // into it, would be a wasted press every single time.
            if i.consume_key(NONE, egui::Key::ArrowDown) {
                complete.step(true);
            }
            if i.consume_key(NONE, egui::Key::ArrowUp) {
                complete.step(false);
            }
            // **Tab completes with nothing highlighted and with the dropdown still down**, taking
            // the first offer — which is what Tab has meant in every shell for forty years, and
            // what somebody who typed `Ctrl+L`, three letters and Tab is asking for. It is also
            // why the field has to hold on to the key; see [`keep_tab`].
            if i.consume_key(NONE, egui::Key::Tab) {
                if complete.hot.is_none() {
                    complete.step(true);
                }
                pick = Some(Pick::Append);
            }
            if complete.hot.is_some() {
                if i.consume_key(NONE, egui::Key::ArrowRight) {
                    pick = Some(Pick::Append);
                }
                if i.consume_key(NONE, egui::Key::Enter) {
                    pick = Some(Pick::Go);
                }
            }
        });
    }

    // Where the field now points, if a key sent it somewhere.
    let mut go: Option<PathBuf> = None;
    // Whether the text was written from out here, and so whether the caret has to be moved.
    let mut appended = false;
    match pick {
        Some(Pick::Append) => {
            if complete.accept(&mut tab.edit_text, slashes).is_some() {
                appended = true;
                // What is on offer now is what is inside the folder just named. Refreshed here
                // rather than left to the next frame, so the dropdown never spends a frame
                // showing the old folder's children under a field that has moved on.
                complete.refresh(pane, &tab.edit_text, loader);
            }
        }
        Some(Pick::Go) => {
            go = complete
                .hot
                .and_then(|at| complete.offers.get(at))
                .map(|(_, path)| path.clone());
        }
        None => {}
    }

    // The dropdown hangs off *this* rather than off the field's own response: a `TextField` hands
    // back its inner `TextEdit`'s, whose rect is the field minus its padding, and a menu aligned
    // to that sits a few points in from the edge the eye reads the field by. Registered before the
    // field, and sensing hover only, so the field is what the pointer lands on.
    let anchor = ui.interact(field, Id::new(("crumb-complete", pane)), Sense::hover());

    // Square, like the breadcrumb it replaces. See [`crate::ui::squared`].
    let response = crate::ui::squared(ui, |ui| {
        ui.put(
            field,
            azur_egui_theme::components::TextField::new(&mut tab.edit_text)
                .size(Size::Small)
                .width(rect.width()),
        )
    });
    // The field is created and focused in the same frame it is opened.
    if !response.has_focus() && !response.lost_focus() {
        response.request_focus();
    }
    if appended {
        caret_to_end(ui.ctx(), response.id, &tab.edit_text);
    }
    keep_tab(ui, response.id);

    // ---- The field's own menu --------------------------------------------
    //
    // Which slash the field writes, and nothing else in it. See [`slash_menu`].
    let menu_rect = slash_menu(ui, &response, slashes, out);
    // **A click in that menu is not a click away from the field.**
    //
    // A field surrenders the keyboard to a click outside it — egui's rule, and the right one — and a
    // tick in a popup is a click outside it. So the frame that ticks the entry would also be the
    // frame that closes the field and puts the breadcrumb back, which is the one thing this setting
    // has to be able to show: the path, still there, now written with the other slash. The field
    // asks for the keyboard back as soon as it notices it has gone, and this is what keeps it from
    // being thrown away in between.
    //
    // Keyed on the pointer being *in* the menu rather than on the menu being open, because those
    // are not the same question. `egui::Popup::show` decides to close after its body has run, so
    // the frame a click *outside* the popup lands on is a frame the popup is still open for — an
    // open menu is exactly what clicking away into the listing looks like as well, and where the
    // pointer is is the only thing that tells the two apart. egui goes on reporting `lost_focus`
    // for as long as nothing else has taken the keyboard, so the coarser test closes the field a
    // frame late rather than never; a frame late is still a frame spent holding the keyboard over a
    // listing somebody has already clicked in, and resting on that is resting on a detail of how
    // egui keeps its focus history.
    let in_menu = menu_rect.is_some_and(|rect| {
        ui.input(|i| i.pointer.interact_pos())
            .is_some_and(|at| rect.contains(at))
    });

    // ---- The offers ------------------------------------------------------
    //
    // Built every frame the field is up, open or not, for the same reason the chevrons' menus are:
    // the popup is what puts itself up, so one constructed only once it is already open is one
    // nothing can open. `open` comes back `false` when it has put itself away — `Escape`, or a
    // click outside it — which is how this learns to stay down until asked again.
    //
    // **Assembled here rather than with `azur::components::Menu`, and the whole reason is the
    // height.** The frame, the rows and the widths are that component's, taken from it directly so
    // this dropdown and a chevron's cannot drift apart; what it cannot express is a list that has
    // to be a stated number of rows tall *whatever it was tall a moment ago*. See
    // [`offers_height`], which is the bug this is written around.
    let showing = !complete.hidden && !complete.offers.is_empty();
    let mut open = showing;
    let hot = complete.hot;
    let follow = std::mem::take(&mut complete.follow);
    let mut clicked: Option<PathBuf> = None;
    let width = field.width();
    let wanted = offers_height(complete.offers.len());
    egui::Popup::menu(&anchor)
        .open_bool(&mut open)
        // **Under the field, always.** A popup that does not fit picks another side by itself, and
        // for a bar along the top of a pane the other side is two rows of nothing above it. There
        // is never more room up there, so there is never a decision to make.
        .align(egui::RectAlign::BOTTOM_START)
        .align_alternatives(&[])
        .frame(azur_egui_theme::components::menu_frame(t))
        .gap(space::S1)
        // As wide as the field, so the two read as one control rather than as a menu that happens
        // to have opened nearby.
        .width(width)
        .show(|ui| {
            // **The room the rows need, taken rather than asked for.**
            //
            // An `Area` hands its content *last frame's size* as this frame's `max_rect` — see
            // `egui::Area::content_ui` — and a `ScrollArea` fits itself into whatever room it is
            // given without ever asking for more. Put those two together and a dropdown gets stuck
            // at the size of the first list it ever showed: typing `d` offers one drive, so the
            // popup is one row tall, and when `d:/` turns that into fifteen folders the scroll area
            // shrinks to the one row of room, so the area never grows, so the room never grows.
            // Two rows and a scrollbar, for ever. That was the bug.
            //
            // So the height is claimed instead: a child `Ui` of exactly the size the rows want, and
            // the cursor advanced past it afterwards so the area sizes itself to what was taken.
            // Right on the first frame, which matters — the list changes on a keystroke, and there
            // is no second frame coming until the next one.
            let taken = Rect::from_min_size(ui.max_rect().min, vec2(width, wanted));
            let mut list = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(taken)
                    .layout(egui::Layout::top_down_justified(egui::Align::Min)),
            );
            // `gap: space-1` between entries, as `Menu` sets it — a menu's rows are flush.
            list.spacing_mut().item_spacing.y = 0.0;
            // A non-floating scrollbar allocates its width whether or not it is showing, which
            // would put a 14-point gutter of nothing down the right edge of every dropdown and
            // ellipsize the widest name into it.
            list.style_mut().spacing.scroll.floating = true;
            egui::ScrollArea::vertical()
                .id_salt(("crumb-complete", pane))
                .max_height(wanted)
                .show(&mut list, |ui| {
                    offers(ui, t, complete, hot, follow, icons_cache, &mut clicked);
                });
            ui.advance_cursor_after_rect(taken);
        });
    // The popup has put itself away — `Escape`, or a click outside it. Recorded, or it would come
    // straight back up on the next frame; the next keystroke brings it back, and so does Down.
    //
    // **Unless the click was in the field**, which is somebody placing the caret rather than
    // dismissing anything. It *is* a click outside the popup, so the popup was right to close, and
    // this is the only place that knows better. No frame is lost putting it back: a popup draws
    // itself before it decides to go.
    if showing && !open && !response.clicked() {
        complete.hidden = true;
        complete.hot = None;
    }
    // Somewhere the field is to go, by the arrow keys or by a click on an offer.
    let go = go.or(clicked);

    let (enter, escape) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    // `Escape` with the menu up closes the menu, which egui has already done by here. One press
    // dismisses one thing; the next one throws the field away, as it always has.
    let escape = escape && menu_rect.is_none();
    if escape || response.lost_focus() && !enter && !in_menu {
        tab.editing_path = false;
    }
    // An offer was chosen. No `resolve_input`, and so no question asked of the disk: the offer came
    // out of a listing, and it carries the path it came from.
    if let Some(path) = go {
        tab.editing_path = false;
        out.push(Action::Navigate { pane, path });
    } else if enter {
        tab.editing_path = false;
        match fs::resolve_input(&tab.edit_text) {
            Some(path) if path.is_file() => out.push(Action::Open(path)),
            Some(path) => out.push(Action::Navigate { pane, path }),
            // Nothing there. Leave the text as typed so it can be corrected.
            None => tab.editing_path = true,
        }
    }
    if !tab.editing_path {
        complete.close(pane);
    }
}

/// How tall the dropdown is for a given number of offers.
///
/// [`OFFERS_SHOWN`] rows, or fewer if there are fewer — it is a ceiling and not a size, because a
/// menu padded out to a fixed height with empty space would be a menu claiming to hold something it
/// does not.
///
/// **A whole number of rows**, which is why the figure is stated rather than left to
/// `Menu`'s own 320-point default: that is 11.43 rows, so the twelfth came out as a two-point sliver
/// along the bottom edge. A list you walk with the arrow keys should end where a row ends — a
/// half-drawn one reads as a rendering fault, and "there is more below" is the scrollbar's job.
fn offers_height(count: usize) -> f32 {
    count.clamp(1, OFFERS_SHOWN) as f32 * azur_egui_theme::components::menu_item_height()
}

/// The rows of the dropdown.
///
/// Split out of [`edit_field`] only because the popup body it goes in is already three closures
/// deep; nothing here is reusable and nothing else calls it.
fn offers(
    ui: &mut Ui,
    t: &Theme,
    complete: &PathComplete,
    hot: Option<usize>,
    follow: bool,
    icons_cache: &mut crate::shell::icons::Icons,
    clicked: &mut Option<PathBuf>,
) {
    for (index, (name, path)) in complete.offers.iter().enumerate() {
        // Reserved so the keyboard's highlight can be painted *behind* the row: by the
        // time the row has been added its label is already down, and a fill over the top
        // of it is a fill over the top of the name. The same trick the open chevron's pair
        // uses in `segments`.
        let slot = ui.painter().add(egui::Shape::Noop);
        // Windows' own icon, exactly as a chevron dropdown gets it: by path for a volume,
        // because that is the only way to a drive's own glyph, and by kind for a folder,
        // which is one lookup shared by every row here and for the rest of the session.
        let icon = if path.parent().is_none() {
            icons_cache.place(path)
        } else {
            icons_cache.kind("", true)
        };
        let texture = icon.and_then(|icon| icons_cache.uv(ui.ctx(), icon));
        let paint = |painter: &egui::Painter, rect: Rect, color: egui::Color32| {
            match texture {
                Some((texture, uv)) => {
                    painter.image(texture, rect, uv, egui::Color32::WHITE);
                }
                None => icons::folder(painter, rect, color),
            }
        };
        let row = ui.add(MenuItem::new(name.clone()).icon(&paint));
        if hot == Some(index) {
            // `control-hover`, which is the fill `MenuItem` gives a row under the pointer:
            // the keyboard is doing the pointer's job here, and two different greys for
            // one meaning would read as two different things.
            ui.painter().set(
                slot,
                egui::epaint::RectShape::filled(
                    row.rect,
                    CornerRadius::ZERO,
                    t.bg.control_hover,
                ),
            );
            if follow {
                row.scroll_to_me(Some(egui::Align::Center));
            }
        }
        // Clicking an offer goes there, which is what clicking an entry in any of this
        // bar's dropdowns does.
        if row.clicked() {
            *clicked = Some(path.clone());
        }
    }
    if complete.truncated {
        azur_egui_theme::components::menu_divider(ui);
        ui.add(
            azur_egui_theme::components::Text::new(format!("First {MENU_LIMIT} shown"))
                .color(azur_egui_theme::components::TextColor::Tertiary),
        );
    }
}

/// Let the path field keep `Tab`, so completing with it does not also leave the field.
///
/// Consuming the key is not enough, and this is the one place in the window where that is true.
/// egui decides whether an arrow or a `Tab` **moves the focus** at the top of the frame, from the
/// raw events, before a single widget has run — so by the time this bar can consume anything, the
/// keyboard is already on its way to the next widget. What that decision reads is the focused
/// widget's `EventFilter`, which the widget itself set on the frame before.
///
/// So the filter is set here, over the one a `TextEdit` sets for itself, and it differs from it in
/// exactly one bit: `tab`. The rest is copied rather than defaulted, because dropping
/// `horizontal_arrows` or `vertical_arrows` would hand `Left` and `Right` to the focus machinery
/// and the caret would stop moving.
///
/// It does *not* make the field insert a tab character. That is decided by the filter the widget
/// passes to `filtered_events`, which is its own and still says no.
///
/// **Set whenever the field is open, offers or none**, so the rule is one line long: while you are
/// typing a path, `Tab` is the completion key. The price is that a `Tab` with nothing to complete
/// does nothing at all rather than moving the keyboard on, and that is the cheaper half — the
/// alternative is a key whose meaning depends on whether a background scan has landed yet, and
/// there is nowhere in this window a `Tab` out of the path bar usefully goes.
fn keep_tab(ui: &Ui, id: Id) {
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                // `Escape` stays the focus machinery's, which is what closes the field.
                escape: false,
            },
        );
    });
}

/// Which segment of the trail is the folder actually being shown.
///
/// Not the last one: [`crate::pane::Tab::trail`] can run deeper than the current folder,
/// which is the whole point of it. The fallback is the end of the trail, for the case that
/// should not arise — a path that is not on its own trail — because a bar with nothing bold
/// on it is a worse answer than a bar with the wrong thing bold.
pub(crate) fn active_index(crumbs: &[(String, PathBuf)], path: &Path) -> usize {
    crumbs
        .iter()
        .position(|(_, crumb)| crumb == path)
        .unwrap_or(crumbs.len() - 1)
}

/// The segmented breadcrumb.
#[allow(clippy::too_many_arguments)]
fn segments(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &mut Tab,
    menu: &mut CrumbMenu,
    icons_cache: &mut crate::shell::icons::Icons,
    slashes: bool,
    out: &mut Vec<Action>,
) {
    // A segment is something you hover in order to go somewhere, like a row in the listing or in
    // the sidebar, so it takes the same hover they do rather than a toolbar button's.
    //
    // The pair an open dropdown makes — a name welded to the chevron after it — wears that same
    // grey, and so do the entries inside the dropdown. The three are one gesture: the pointer
    // moves along the bar, into the list that opened under it, and down the folders in it, and a
    // darker "pressed" grey under an open menu read as the bar having been dented rather than as
    // the trail carrying on into the list.
    let hover_fill = crate::ui::hover_fill(t);
    // The *pressed* fill still comes from the ladder, and from `control_fills` rather than
    // straight off the theme: the bar these sit on is `crate::ui::seam`, not `background-layer`,
    // and in the light theme where the seam and `control-active` are the same grey a pressed
    // segment was the same colour as the bar, so the press showed as the fill going away.
    let (_, pressed_fill) = crate::ui::control_fills(t, crate::ui::seam(t));

    const CHEVRON: f32 = 16.0;
    const OVERFLOW: f32 = 22.0;

    // The two things that are painted *behind* the segments, reserved here and filled in once
    // the loop has worked out where they go.
    //
    // Both are backgrounds for something drawn later — the wash that says a click will open the
    // path field, and the fill that welds an open chevron to the segment it belongs to — and
    // neither can be positioned until the segments have been laid out, which is the same pass
    // that draws their text. A reserved slot is how a painter draws in one order and composites
    // in another; the alternative is measuring the whole bar twice.
    let field_hint = ui.painter().add(egui::Shape::Noop);
    let pair_hint = ui.painter().add(egui::Shape::Noop);
    let corner = CornerRadius::same(radius::SMALL);

    // The whole bar, and then the part of it the trail is laid out in: the pen at the
    // right-hand end is reserved out of the segments' room, so a deep path runs up to it and
    // never over it. See where the pen is drawn.
    let bar = rect;
    let rect = Rect::from_min_max(
        rect.min,
        pos2((rect.right() - PEN - space::S2).max(rect.left()), rect.bottom()),
    );

    // The trail rather than the current folder: walking up leaves the deeper part of it on
    // the bar so it can be clicked again. See [`crate::pane::Tab::trail`].
    let crumbs = fs::breadcrumb_segments(&tab.trail);
    let center = rect.center().y;
    let deepest = crumbs.len() - 1;
    let active = active_index(&crumbs, &tab.path);

    // Whether this bar had a dropdown open when the frame began, which is the same question as
    // whether the pointer moving along it should carry that dropdown with it. Read once, before
    // anything can change it: a click *during* this frame must not also count as a hover.
    let tracking = menu.open.is_some_and(|(owner, _)| owner == pane);
    // Which dropdown the pointer is asking for, whether one was clicked, and whether the one
    // that is open was drawn — all three settled during the loop and acted on after it.
    let mut wanted: Option<usize> = None;
    let mut clicked = false;
    let mut shown = false;

    // Natural width of every segment, so the overflow can be decided before
    // anything is drawn.
    //
    // The current folder is measured in `body-strong`, which is what it is *drawn* in —
    // measuring it in the regular face would leave its name a few points short and
    // truncate it with room to spare.
    let widths: Vec<f32> = crumbs
        .iter()
        .enumerate()
        .map(|(index, (label, _))| {
            let font = if index == active {
                t.fonts.body_strong.clone()
            } else {
                t.fonts.body.clone()
            };
            let galley =
                ui.painter()
                    .layout_no_wrap(label.clone(), font, egui::Color32::PLACEHOLDER);
            // A rounded-up point of slack, so a hinted glyph advance never spills
            // past the width it was measured at.
            (galley.size().x + space::S3 * 2.0).ceil() + 1.0
        })
        .collect();

    // Every segment is followed by a chevron, and the trail starts with one for the
    // drives.
    //
    // Measured up to the current folder and no further: the part of the trail *past* it is
    // a convenience and takes whatever room is left over, so a long tail can never push the
    // folder you are actually in off the front of the bar. Whatever does not fit at the
    // right-hand end is simply not drawn — the loop below stops when it runs out of room.
    let cost: f32 = widths[..=active].iter().sum::<f32>() + (active + 1) as f32 * CHEVRON;
    let mut first = 0;
    if cost > rect.width() {
        // Drop from the front until it fits, leaving room for the `…`.
        let mut used = cost + OVERFLOW;
        while first < active && used > rect.width() {
            used -= widths[first] + CHEVRON;
            first += 1;
        }
    }

    let mut x = rect.left();

    if rect.width() < CHEVRON + 24.0 {
        // Not even one segment fits. Nothing is better than a chevron on its own.
        return;
    }

    if first > 0 {
        let overflow = Rect::from_min_size(
            pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
            vec2(OVERFLOW, TOOL_SIZE),
        );
        let response = tool_button(
            ui,
            t,
            overflow,
            Id::new(("crumb-overflow", pane)),
            &azur_icons::ellipsis,
            "",
            true,
            false,
            crate::ui::seam(t),
        );
        if response.clicked() {
            clicked = true;
            menu.open = (menu.open != Some((pane, OVERFLOW_MENU)))
                .then_some((pane, OVERFLOW_MENU));
        } else if tracking && response.hovered() {
            wanted = Some(OVERFLOW_MENU);
        }
        let mut open = menu.open == Some((pane, OVERFLOW_MENU));
        if open {
            shown = true;
            // The same fill an open chevron wears, from behind, because `tool_button` has
            // already painted its glyph by now. Not its `active` state, which is Azur's accent
            // and means *latched* everywhere else in this window.
            ui.painter().set(
                pair_hint,
                egui::epaint::RectShape::filled(overflow, corner, hover_fill),
            );
        }
        azur_egui_theme::components::Menu::new(&response)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                for (label, path) in crumbs.iter().take(first) {
                    if ui.add(MenuItem::new(label.clone())).clicked() {
                        out.push(Action::Navigate {
                            pane,
                            path: path.clone(),
                        });
                    }
                }
            });
        if !open && menu.open == Some((pane, OVERFLOW_MENU)) {
            menu.open = None;
        }
        x += OVERFLOW;
    }

    // A leading chevron for the very first visible segment, which for a full path
    // is This PC and so lists the drives.
    let mut pending_chevron = Some(if first == 0 {
        PathBuf::new()
    } else {
        crumbs[first - 1].1.clone()
    });

    // Where the segment before the chevron about to be drawn was, so the two can be filled as
    // one shape. `None` for the leading chevron, whose segment is in the overflow or is This PC
    // itself — it is highlighted alone, having nothing to be welded to.
    let mut previous: Option<Rect> = None;

    for (index, (label, path)) in crumbs.iter().enumerate().skip(first) {
        if let Some(parent) = pending_chevron.take() {
            if x + CHEVRON > rect.right() {
                break;
            }
            let chevron = Rect::from_min_size(
                pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
                vec2(CHEVRON, TOOL_SIZE),
            );
            let response = ui.interact(
                chevron,
                Id::new(("crumb-chevron", pane, index)),
                Sense::click(),
            );
            if response.clicked() {
                clicked = true;
                // A click on the chevron whose dropdown is already up closes it, which is what
                // clicking an open menu's button does everywhere.
                menu.open = (menu.open != Some((pane, index))).then_some((pane, index));
                // Re-read on each open: the folder may have gained a subfolder since the
                // last time this chevron was used.
                menu.path = None;
            } else if tracking && response.hovered() {
                wanted = Some(index);
            }

            let mut open = menu.open == Some((pane, index));
            if open {
                shown = true;
                // **The chevron and the text before it, as one shape.** They are one control:
                // the chevron lists that folder's subfolders, so the pair is "this folder, and
                // what is inside it". Two fills would put a seam down the middle of it — and
                // this has to go *behind* the segment, whose label was painted a step ago.
                ui.painter().set(
                    pair_hint,
                    egui::epaint::RectShape::filled(
                        match previous {
                            Some(text) => text.union(chevron),
                            None => chevron,
                        },
                        corner,
                        hover_fill,
                    ),
                );
            } else if response.hovered() {
                ui.painter().rect_filled(chevron, corner, hover_fill);
            }
            // Down when the menu is showing, right when it is not — the same
            // rotation Explorer uses to say "this opens".
            //
            // A point lower than centre, which is where it sits beside a line of text:
            // the label's ink is above the middle of its line box, and matching the
            // chevron to the *text* beats matching it to the box.
            let glyph_rect = Rect::from_center_size(
                chevron.center() + vec2(0.0, ARROW_DROP),
                vec2(12.0, 12.0),
            );
            if open {
                azur_icons::chevron_down(ui.painter(), glyph_rect, t.text.primary);
            } else {
                azur_icons::chevron_right(ui.painter(), glyph_rect, t.text.tertiary);
            }

            // Built every frame, open or not.
            //
            // `Menu` is what puts the popup up — it hangs an `egui::Popup` off the trigger
            // response — so a menu that is only constructed once it is *already* open is a
            // menu nothing can ever open. That is exactly what this was, and why the
            // chevrons did nothing.
            //
            // Driven from [`CrumbMenu::open`] rather than from egui's own per-trigger memory,
            // because the bar is one control while a dropdown is up. `open` comes back `false`
            // when the popup has closed itself — an entry clicked, a click outside, `Escape` —
            // which is how the bar learns to stop tracking.
            //
            // The folder is read inside the closure, which egui runs only while the popup
            // is showing, and [`CrumbMenu`] holds the result — so the scan happens on the
            // frame it opens rather than sixty times a second while it is up.
            azur_egui_theme::components::Menu::new(&response)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                menu.ensure(&parent);
                if menu.items.is_empty() {
                    ui.add(
                        azur_egui_theme::components::Text::new("No subfolders")
                            .color(azur_egui_theme::components::TextColor::Tertiary),
                    );
                }
                for (name, target) in &menu.items {
                    let here = *target == *path;
                    // Windows' icon, which for the leading chevron — the one that lists
                    // the volumes — is the difference between a row of drives and a row of
                    // identical folders.
                    //
                    // A *drive* is asked about by path, because that is the only way to get
                    // its own icon, and there are at most twenty-six of them. A folder is
                    // asked about by kind, which is one lookup shared by every folder for
                    // the rest of the session: this menu can hold two hundred rows, and a
                    // per-path lookup each would be two hundred questions for the
                    // generic folder icon two hundred times over.
                    let icon = if target.parent().is_none() {
                        icons_cache.place(target)
                    } else {
                        icons_cache.kind("", true)
                    };
                    let texture = icon
                        .and_then(|icon| icons_cache.uv(ui.ctx(), icon));
                    let paint = |painter: &egui::Painter, rect: Rect, color: egui::Color32| {
                        match texture {
                            Some((texture, uv)) => {
                                painter.image(texture, rect, uv, egui::Color32::WHITE);
                            }
                            None => icons::folder(painter, rect, color),
                        }
                    };
                    if ui
                        .add(MenuItem::new(name.clone()).icon(&paint).selected(here))
                        .clicked()
                    {
                        out.push(Action::Navigate {
                            pane,
                            path: target.clone(),
                        });
                    }
                }
                if menu.truncated {
                    azur_egui_theme::components::menu_divider(ui);
                    ui.add(
                        azur_egui_theme::components::Text::new(format!("First {MENU_LIMIT} shown"))
                            .color(azur_egui_theme::components::TextColor::Tertiary),
                    );
                }
            });
            // The popup has closed itself: an entry was clicked, or something outside it was, or
            // `Escape`. That is the end of the bar's tracking mode, and this is the only place
            // it is reported.
            if !open && menu.open == Some((pane, index)) {
                menu.open = None;
            }
            x += CHEVRON;
        }

        let width = widths[index].min((rect.right() - x).max(0.0));
        if width <= 0.0 {
            break;
        }
        let segment = Rect::from_min_size(
            pos2(x.round(), (center - TOOL_SIZE * 0.5).round()),
            vec2(width, TOOL_SIZE),
        );
        previous = Some(segment);
        let current = index == active;
        let response = ui.interact(
            segment,
            Id::new(("crumb", pane, index)),
            Sense::click(),
        );
        // A click on a segment ends the tracking rather than moving it: the popup closes itself,
        // and the hover below — the pointer is on this segment, since it was just clicked —
        // would otherwise put the dropdown straight back up on the way out.
        if response.clicked() || response.middle_clicked() {
            clicked = true;
        }
        // Hovering a segment while the bar is tracking opens *its* chevron — the one after it,
        // which is the one that lists this folder's subfolders. The deepest segment on the
        // trail has no chevron after it, so there is nothing for it to open and whatever is
        // showing stays where it is.
        if tracking && response.hovered() && index != deepest {
            wanted = Some(index + 1);
        }
        // Part of the open pair: filled as one shape with the chevron after it, from behind, so
        // it wears nothing of its own here. See where `pair_hint` is set.
        let paired = menu.open == Some((pane, index + 1));
        if !paired && (response.hovered() || response.is_pointer_button_down_on()) {
            ui.painter().rect_filled(
                segment,
                corner,
                if response.is_pointer_button_down_on() {
                    pressed_fill
                } else {
                    hover_fill
                },
            );
        }
        // Bold and full-contrast marks the folder being shown, which is the only cue that it
        // is not simply the end of the trail. Everything else on the bar is a link, on both
        // sides of it.
        let (font, color) = if current {
            (t.fonts.body_strong.clone(), t.text.primary)
        } else if response.hovered() || paired {
            (t.fonts.body.clone(), t.text.primary)
        } else {
            (t.fonts.body.clone(), t.text.secondary)
        };
        let galley = truncated(
            ui.painter(),
            label,
            font,
            color,
            width - space::S3 * 2.0,
        );
        text_left(
            ui.painter(),
            Rect::from_min_max(
                pos2(segment.left() + space::S3, segment.top() - TEXT_LIFT),
                pos2(segment.right() - space::S3, segment.bottom() - TEXT_LIFT),
            ),
            galley,
        );
        if response.clicked() && !current {
            out.push(Action::Navigate {
                pane,
                path: path.clone(),
            });
        }
        // Middle click opens an ancestor in its own tab, as it does everywhere.
        if response.middle_clicked() {
            out.push(Action::NavigateNewTab {
                pane,
                path: path.clone(),
            });
        }

        x += width;
        // A chevron between every pair of segments, listing the left one's subfolders. The
        // one after the current folder is the useful case and the reason this is keyed on
        // the end of the trail rather than on the current folder.
        if index != deepest {
            pending_chevron = Some(path.clone());
        }
    }

    // ---- Where the open dropdown goes next -------------------------------
    //
    // Applied here rather than where the hover was noticed, so that the frame which notices is
    // drawn whole: switching mid-loop would leave the dropdown that is going away already
    // painted for this frame with the one arriving painted over it. One frame of delay, and the
    // repaint is asked for because the pointer has very likely stopped moving by now — it
    // arrived somewhere and stayed.
    match wanted.filter(|_| !clicked) {
        Some(index) => {
            if menu.open != Some((pane, index)) {
                menu.open = Some((pane, index));
                ui.ctx().request_repaint();
            }
        }
        // A dropdown recorded as open whose chevron was not drawn at all — the pane has since
        // been narrowed past it, or the pointer asked for one that turned out not to fit. It is
        // showing nothing, and leaving it recorded would leave the bar tracking a menu that
        // cannot be seen or closed.
        None if tracking && !shown => menu.open = None,
        None => {}
    }

    // ---- The rest of the bar: click it to type a path --------------------
    //
    // Including the pen's own strip, which is part of the same target: it is a drawing rather
    // than a button, and a hint you cannot click would be a strange thing to draw.
    let field = Rect::from_min_max(
        pos2(bar.left(), (bar.center().y - TOOL_SIZE * 0.5).round()),
        pos2(bar.right(), (bar.center().y + TOOL_SIZE * 0.5).round()),
    );
    let empty = Rect::from_min_max(pos2(x, bar.top()), bar.max);
    let over_field = if empty.width() > 4.0 {
        let response = ui.interact(empty, Id::new(("crumb-empty", pane)), Sense::click());
        if response.clicked() {
            start_editing(tab, slashes);
        }
        response.hovered()
    } else {
        false
    };
    if over_field {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        // **The border and nothing else.** The one place on the bar where a click does
        // something other than navigate says so before the click — and says it the way a field
        // says it, with the outline a field wears under the pointer, over the whole shape the
        // field is about to take. A fill as well made the bar change colour to announce
        // something that is only an announcement.
        //
        // From behind, and this is the reason `field_hint` was reserved: the outline spans the
        // whole bar, and by the time the empty space at the end of it is known to be hovered,
        // every segment's label has been painted.
        ui.painter().set(
            field_hint,
            egui::epaint::RectShape::stroke(field, corner, Stroke::new(1.0, pen_ink(t, true)), StrokeKind::Inside),
        );
    }

    // ---- The pen ---------------------------------------------------------
    //
    // A hint, not a control: it says the bar can be typed into, which is the one thing about
    // this bar that nothing else on it advertises. Always drawn, because a hint that only
    // appears once the pointer is already there is not a hint — and in the same ink as the
    // outline, brighter when the pointer is over the bar, so the two read as one cue.
    //
    // Its space is taken out of the trail's before the segments are laid out, so a path deep
    // enough to fill the bar stops short of it rather than running underneath it.
    icons::pencil(
        ui.painter(),
        Rect::from_center_size(
            pos2((bar.right() - PEN * 0.5).round(), field.center().y.round()),
            egui::vec2(PEN, PEN),
        ),
        pen_ink(t, over_field),
    );
}

/// The ink the pen and the field's outline share.
///
/// One colour for both, because they are one hint: the outline says "this is a field" and the pen
/// says "this is where you write". `stroke-strong` is what Azur gives a *field* under the pointer
/// — the same border the path field itself will wear a moment later — and `stroke-control` is that
/// border at rest, which is what the pen sits at until the pointer arrives.
///
/// Not `stroke-subtle`, which is the obvious name for something subtle and is invisible here:
/// [`crate::ui::seam`] — the bar's own fill — *is* `stroke-subtle`, so a hairline in it is a
/// hairline drawn in the colour behind it. It was, for as long as this hint was a hairline.
pub(crate) fn pen_ink(t: &Theme, lit: bool) -> egui::Color32 {
    if lit {
        t.stroke.strong
    } else {
        t.stroke.control
    }
}

/// Switch the bar into its editable form, prefilled with the current path.
///
/// The current path, not the trail: the field is for going somewhere, and what it should
/// open showing is where you are.
///
/// With the separator the field is set to write, which is the whole of what `Use / in path` does to
/// a field that is opening — see [`with_separator`], and [`slash_menu`] for where it is ticked.
pub fn start_editing(tab: &mut Tab, slashes: bool) {
    tab.edit_text = if tab.path.as_os_str().is_empty() {
        "This PC".to_owned()
    } else {
        with_separator(&tab.path.to_string_lossy(), slashes)
    };
    tab.editing_path = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The box's tooltip is the design system's syntax and nothing this window invented, and the
    /// listings it does add are the funnel's.**
    ///
    /// This used to assert the opposite: the list was the library's three with `@git` appended, and
    /// what needed testing was that the appended pair reached the tooltip at all. The word is a menu
    /// now, so the claim is inverted — a marker of this program's own here would send a reader to the
    /// box to type something it no longer understands, which is the one wrong answer a syntax tooltip
    /// can give.
    ///
    /// Pairs and not lines, because what makes the two columns possible is that the key and the
    /// meaning never become one string. A marker smuggled in as `"!word — …"` would draw as one long
    /// key in `text-secondary` with an empty column beside it, which is the failure this catches — the
    /// design system tests that its own three are *present*
    /// (`filter::tests::the_markers_are_all_documented_as_pairs`), not that they are still two halves
    /// by the time this window draws them.
    #[test]
    fn the_filter_tooltip_is_the_design_systems_and_the_lenses_are_the_funnels() {
        let markers = azur_egui_theme::filter::MARKERS;
        for want in ["!word", "^word", "word$"] {
            let (key, meaning) = markers
                .iter()
                .find(|(key, _)| *key == want)
                .unwrap_or_else(|| panic!("`{want}` is not in the tooltip: {markers:?}"));
            assert!(
                !meaning.trim().is_empty(),
                "`{key}` is in the tooltip with nothing said about it"
            );
            // Neither half may carry the other: that is what makes them two columns.
            assert!(
                !key.contains('—') && !meaning.contains('—') && !meaning.contains(key),
                "`{key}`/`{meaning}` has been folded into one string"
            );
        }

        // Nothing in the box's syntax is about git or about a kind of file: both are questions about
        // this program's data, and the funnel is where they are asked.
        for (key, meaning) in markers {
            assert!(
                !key.starts_with('@') && !meaning.contains("git"),
                "`{key}`/`{meaning}` is a lens dressed up as syntax"
            );
        }

        // And each lens says what it does, because the menu is the only place either is named.
        let labels: Vec<&str> = Lens::ALL.iter().map(|lens| lens.label()).collect();
        assert_eq!(labels.len(), 2, "{labels:?}");
        assert!(
            labels.iter().all(|label| label.starts_with("Show ")),
            "an entry that does something is a sentence, not a value: {labels:?}"
        );
        assert_ne!(labels[0], labels[1]);
    }

    #[test]
    fn the_bold_segment_is_the_folder_being_shown() {
        let trail = fs::breadcrumb_segments(Path::new(r"C:\a\b\c"));
        // This PC, C:\, a, b, c.
        assert_eq!(trail.len(), 5, "{trail:?}");

        // At the end of the trail it is the last segment, as it always used to be.
        assert_eq!(active_index(&trail, Path::new(r"C:\a\b\c")), 4);
        // Walked up two: the trail still shows five, and `a` is the one in bold.
        assert_eq!(active_index(&trail, Path::new(r"C:\a")), 2);
        // All the way up. This PC is a segment like any other.
        assert_eq!(active_index(&trail, Path::new("")), 0);
    }

    #[test]
    fn a_path_that_is_not_on_the_trail_falls_back_to_its_end() {
        // Should not happen -- `Tab::go_to` keeps the two in step -- but a bar with nothing
        // bold on it would be worse than this.
        let trail = fs::breadcrumb_segments(Path::new(r"C:\a\b"));
        assert_eq!(active_index(&trail, Path::new(r"D:\somewhere")), trail.len() - 1);
    }

    /// The cut has to be lossless: the two halves put back together are the text that came in,
    /// which is what lets a completion leave `%appdata%\` alone instead of resolving it away.
    #[test]
    fn the_typed_path_splits_at_the_last_separator() {
        assert_eq!(split_typed(r"C:\Users\to"), (r"C:\Users\", "to"));
        assert_eq!(split_typed(r"C:\Users\"), (r"C:\Users\", ""));
        assert_eq!(split_typed(r"C:\"), (r"C:\", ""));
        // Either slash, because the field takes either.
        assert_eq!(split_typed("D:/Sources/My"), ("D:/Sources/", "My"));
        // Nothing names a folder yet, so all of it is the name being typed.
        assert_eq!(split_typed("Doc"), ("", "Doc"));
        assert_eq!(split_typed(""), ("", ""));
        // What was typed is never rewritten on the way through.
        assert_eq!(split_typed(r"%APPDATA%\Mic"), (r"%APPDATA%\", "Mic"));
        for text in [r"C:\Users\to", "Doc", "", r"~\", "D:/x/y"] {
            let (a, b) = split_typed(text);
            assert_eq!(format!("{a}{b}"), text, "the split lost something");
        }
    }

    #[test]
    fn matching_a_name_ignores_its_case() {
        assert!(starts_with_folded("Windows", "win"));
        assert!(starts_with_folded("windows", "WIN"));
        assert!(starts_with_folded("Windows", ""));
        assert!(starts_with_folded("Windows", "Windows"));
        assert!(!starts_with_folded("Windows", "Windowsx"));
        assert!(!starts_with_folded("Windows", "ind"), "this matches prefixes, not parts");
        // Not the ASCII fold: a folder can be named in any language, and one you can see that is
        // not offered reads as a broken completion.
        assert!(starts_with_folded("Étude", "é"));
        assert!(starts_with_folded("étude", "É"));
    }

    /// Two offers and the text that made them, which is enough to drive the highlight and the
    /// append without a window or a disk.
    fn sample() -> PathComplete {
        PathComplete {
            pane: None,
            typed: r"C:\Users\to".to_owned(),
            asked: None,
            ready: true,
            offers: vec![
                ("tony".to_owned(), PathBuf::from(r"C:\Users\tony")),
                ("tools".to_owned(), PathBuf::from(r"C:\Users\tools")),
            ],
            truncated: false,
            hot: None,
            hidden: false,
            follow: false,
        }
    }

    /// Down and Up walk the offers, and both ends come back to what was typed rather than
    /// wrapping straight round — `Escape` throws the field away, so it cannot be the only way
    /// back to your own text.
    #[test]
    fn the_highlight_walks_the_offers_and_back_to_what_was_typed() {
        let mut complete = sample();
        assert_eq!(complete.hot, None, "nothing starts highlighted");

        complete.step(true);
        assert_eq!(complete.hot, Some(0));
        complete.step(true);
        assert_eq!(complete.hot, Some(1));
        complete.step(true);
        assert_eq!(complete.hot, None, "past the last offer is what was typed");
        complete.step(true);
        assert_eq!(complete.hot, Some(0), "and then round again");

        complete.hot = None;
        complete.step(false);
        assert_eq!(complete.hot, Some(1), "Up from nothing starts at the end");
        complete.step(false);
        assert_eq!(complete.hot, Some(0));
        complete.step(false);
        assert_eq!(complete.hot, None);
    }

    /// An arrow puts the dropdown back up, which is the only way back to one that was put away
    /// without deleting what you typed.
    #[test]
    fn an_arrow_opens_the_dropdown_and_lands_on_an_offer_at_once() {
        let mut complete = sample();
        complete.hidden = true;
        complete.step(true);
        assert!(!complete.hidden, "Down did not bring the dropdown back");
        assert_eq!(
            complete.hot,
            Some(0),
            "the same press has to land on an offer, or every one of them costs two"
        );
        assert!(complete.follow, "the highlight has to be scrolled into view");
    }

    /// Appending keeps the prefix exactly as it was typed and adds the separator that starts the
    /// next name, so the offers after it are what is inside the folder just chosen.
    #[test]
    fn accepting_an_offer_appends_a_name_and_a_separator() {
        let mut complete = sample();
        let mut text = complete.typed.clone();

        assert_eq!(
            complete.accept(&mut text, false),
            None,
            "with nothing highlighted there is nothing to append"
        );
        assert_eq!(text, r"C:\Users\to", "and the text is left alone");

        complete.hot = Some(1);
        assert_eq!(
            complete.accept(&mut text, false),
            Some(PathBuf::from(r"C:\Users\tools"))
        );
        assert_eq!(text, "C:\\Users\\tools\\");
    }

    /// A path typed with forward slashes stays typed with forward slashes: it lists and navigates
    /// perfectly — `fs::normalize` sees to that at the door — so mixing the two would be this
    /// program's own doing.
    ///
    /// **The prefix wins over the setting**, both ways round, which is the point: `Use / in path`
    /// says what the field is *filled* with, and once there is a path in it the separators in that
    /// path are the better evidence of what somebody wants.
    #[test]
    fn appending_keeps_the_separator_that_was_being_used() {
        let mut complete = sample();
        complete.offers = vec![("src".to_owned(), PathBuf::from(r"D:\Sources\src"))];
        complete.hot = Some(0);

        for slashes in [false, true] {
            let mut text = "D:/Sources/s".to_owned();
            complete.accept(&mut text, slashes);
            assert_eq!(text, "D:/Sources/src/");

            let mut text = r"D:\Sources\s".to_owned();
            complete.accept(&mut text, slashes);
            assert_eq!(text, "D:\\Sources\\src\\");
        }
    }

    /// With no separator typed yet the setting decides, which is a path typed from nothing: the
    /// first offer is a *drive*, and the separator after it is the first one the field will hold.
    ///
    /// The only place `Use / in path` reaches the completion, and it has to — a field filled with
    /// `/` and a `Tab` that answers `D:\` would be the program disagreeing with its own setting on
    /// the first keystroke.
    #[test]
    fn appending_to_a_bare_name_uses_the_slash_the_setting_asks_for() {
        let mut complete = sample();
        complete.offers = vec![("D:".to_owned(), PathBuf::from("D:\\"))];
        complete.hot = Some(0);

        let mut text = "d".to_owned();
        complete.accept(&mut text, true);
        assert_eq!(text, "D:/");

        let mut text = "d".to_owned();
        complete.accept(&mut text, false);
        assert_eq!(text, "D:\\");
    }

    /// The swap the field's separator setting is: both ways, exact, and the same length either way.
    #[test]
    fn the_separator_swap_is_lossless_in_both_directions() {
        assert_eq!(with_separator(r"D:\Sources\ui", true), "D:/Sources/ui");
        assert_eq!(with_separator("D:/Sources/ui", false), r"D:\Sources\ui");
        // Already the way it was asked for: nothing to do, and nothing done.
        assert_eq!(with_separator("D:/Sources", true), "D:/Sources");
        assert_eq!(with_separator(r"D:\Sources", false), r"D:\Sources");
        // A mixture is what half-typing a path leaves, and it comes out consistent.
        assert_eq!(with_separator(r"D:\Sources/ui\x", true), "D:/Sources/ui/x");
        // A share. Both leading separators are separators, so both turn.
        assert_eq!(with_separator(r"\\nas\music", true), "//nas/music");
        assert_eq!(with_separator("//nas/music", false), r"\\nas\music");
        // `This PC` has nothing in it to swap, and neither does a name being typed.
        assert_eq!(with_separator("This PC", true), "This PC");
        // Length preserved, which is what lets the caret stay where it was.
        for text in [r"D:\a\b", "D:/a/b", r"\\nas\music", "This PC", ""] {
            for slashes in [false, true] {
                assert_eq!(with_separator(text, slashes).len(), text.len(), "{text}");
            }
        }
        // And a round trip is the identity for a path written either way.
        for text in [r"D:\a\b", r"\\nas\music"] {
            assert_eq!(with_separator(&with_separator(text, true), false), text);
        }
    }

    /// A field rewritten from outside is not somebody typing, so the dropdown stays as it was.
    ///
    /// Which is the whole of [`PathComplete::rewritten`]: `refresh` reads any change to the text as
    /// a keystroke and puts the offers up, and ticking `Use / in path` on a field holding where you
    /// already are would raise a list of the folder you are standing in — the one list `Ctrl+L` is
    /// careful not to show.
    #[test]
    fn rewriting_the_field_from_outside_leaves_the_dropdown_down() {
        let mut complete = sample();
        complete.pane = Some(1);
        complete.hidden = true;
        complete.hot = Some(1);

        // The wrong pane's field: not this one's business. The other pane draws its own bar on
        // every frame of a split window.
        complete.rewritten(2, "C:/Users/to");
        assert_eq!(complete.typed, r"C:\Users\to");

        complete.rewritten(1, "C:/Users/to");
        assert_eq!(
            complete.typed, "C:/Users/to",
            "the new text was not claimed, so `refresh` will read it as a keystroke"
        );
        assert!(complete.hidden, "the dropdown came up on its own");
        assert!(!complete.ready, "the offers carry the old prefix and were kept");
        assert_eq!(complete.hot, None, "the highlight stayed on offers that are about to move");
    }

    /// The one pane that has a field open keeps it while the other pane's bar is drawn, which
    /// happens on every frame of a split window.
    #[test]
    fn the_other_panes_bar_does_not_clear_this_ones_offers() {
        let mut complete = sample();
        complete.pane = Some(1);
        complete.hot = Some(0);

        complete.close(2);
        assert_eq!(complete.hot, Some(0), "the wrong pane cleared the field's state");

        complete.close(1);
        assert!(complete.offers.is_empty(), "its own pane did not clear it");
        assert_eq!(complete.pane, None);
    }
}
