//! The path bar: history buttons, a segmented breadcrumb, refresh and the filter.
//!
//! Modelled on Explorer's, which is still the best version of this control:
//!
//! - Every **segment** is a button that goes there.
//! - Every **chevron** between segments opens that folder's subfolders, so you can
//!   step sideways into a sibling without going up first.
//! - The **leading chevron** lists the drives. It is This PC's chevron, and it is all of This PC
//!   that the bar draws: a segment saying so in front of every path on the machine says nothing a
//!   drive letter does not, and clicking it did nothing this chevron does not. It appears as a
//!   segment only when it is the folder on show, because the bar always names that one.
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

/// Subfolders for whichever chevron dropdown is open.
///
/// One at a time, so one cache is enough. Built on the frame an answer lands rather
/// than on every frame the menu is showing — a popup body runs continuously, and
/// filtering and sorting `C:\Windows\System32` sixty times a second to draw the same
/// list would be a self-inflicted stall.
///
/// **Nothing here reads the disk.** The folder is asked of [`crate::loader`] — the same service
/// the listings come from and the same one the path field's completions use, which reads on a
/// worker — and the menu draws whatever has come back. See [`CrumbMenu::ensure`], which is a cache
/// probe and an ask, never a scan.
///
/// That is not a refinement, it is the difference between a window that paints and one that does
/// not. This used to call [`crate::fs::scan::scan`] from inside the popup body, which is the UI
/// thread on the frame the dropdown opens, and that function documents about itself that it only
/// ever runs on a loader worker: on the trail `\\server\share\a` the chevron between `server` and
/// `share` hands it a bare `\\server`, which is answered by `NetShareEnum` rather than by a
/// directory read — **22.1 seconds** for a name that does not resolve, measured in
/// [`crate::fs::drives::shares_on`] — and a share that has just gone away costs the same wait
/// through the redirector. The window stopped painting for all of it. Once per chevron the pointer
/// crossed, too, because [`CrumbMenu::open`] makes the whole bar one tracking control.
#[derive(Default)]
pub struct CrumbMenu {
    /// The folder [`Self::items`] were built from, and `None` while nothing has come back for the
    /// folder the open chevron points at.
    ///
    /// **This is the staleness check, and it needs no token to be one.** What fills the menu is a
    /// probe of the loader's cache for the folder *under the pointer* — an answer to any other
    /// question is not something this can be handed, because it is never handed anything: it
    /// looks up the one key it wants. A token would be re-checking that key. The loader's own
    /// tokens are matched in `App::collect_scans`, against the tab that navigated and never
    /// against this menu, which is also why a chevron on a share that wants credentials cannot
    /// raise the sign-in dialog.
    path: Option<PathBuf>,
    /// The folder already asked of the loader, so one it cannot read is asked for once.
    ///
    /// A failed read is deliberately not cached — see [`crate::loader::Loader`], and it is the
    /// right rule, because a share can come back. But it means [`crate::loader::Loader::cached`]
    /// keeps saying no, and an ask driven off that answer alone would queue a fresh scan of a dead
    /// path on every frame the menu is up, each one waking the window to ask again. `PathComplete`
    /// carries the same field for the same reason, and this menu asks about exactly the paths that
    /// field does.
    asked: Option<PathBuf>,
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

impl CrumbMenu {
    /// Fill the menu for `path` from the loader, asking for the folder if nobody has yet.
    ///
    /// Answers whether there is a listing to draw. While there is not — the answer is still out on
    /// a worker — the dropdown says so rather than drawing an empty body, which is a different
    /// claim; see where this is called.
    ///
    /// **[`crate::loader::Loader::prefetch`] rather than `request`**, which is what
    /// `PathComplete::refresh` does for the same dropdown reached by typing, and for two reasons
    /// beyond keeping the two halves of one control alike. It **does not queue a second scan of a
    /// folder already being read**, which matters here more than anywhere: the bar tracks, so the
    /// pointer can ask for a dozen folders in a second and cross the same chevron twice on the way
    /// back. And a chevron is a *guess* — a place somebody may be about to go — so it has no
    /// business jumping the queue in front of the folder a pane is waiting to show.
    fn ensure(&mut self, path: &Path, loader: &mut crate::loader::Loader) -> bool {
        if self.path.as_deref() == Some(path) {
            return true;
        }
        // **The rows in it belong to the last chevron, and they go now** rather than when the next
        // answer lands. A dropdown that kept them would spend the wait offering `MyTools`'
        // subfolders under `Sources` — every row a real folder, none of them in the folder the
        // pointer is on, which is a worse answer than saying nothing yet.
        //
        // The folder goes with them, and not only for tidiness: this field is what says *whose*
        // rows these are, so leaving it on the folder the menu is no longer showing would be the
        // staleness check itself reporting the stale answer.
        self.path = None;
        self.items.clear();
        self.truncated = false;
        // Asked once per folder, and once only for one that cannot be read. See [`Self::asked`].
        if self.asked.as_deref() != Some(path) {
            self.asked = Some(path.to_path_buf());
            loader.prefetch(path);
        }
        // Whatever the loader already has, which for a folder on the trail is usually everything:
        // walking down through it is what put it in the cache. A miss leaves the menu waiting, and
        // the worker wakes the window when it lands — see [`crate::loader::Loader::new`], which
        // holds the context for exactly that.
        let Some(dir) = loader.cached(path) else {
            return false;
        };
        // A directory of subdirectories is the only thing this menu shows, so the
        // listing is filtered as it is read rather than after.
        let mut items: Vec<(String, PathBuf)> = Vec::new();
        for i in 0..dir.len() {
            let entry = &dir.entries[i];
            if !entry.is_dir() || entry.is_hidden() {
                continue;
            }
            // The folder's own name, which is its name everywhere but the Recycle Bin — where a
            // row is named by the path it came from. See [`crate::fs::recycle`].
            items.push((dir.leaf(i).to_owned(), dir.target(i)));
        }
        items.sort_by(|a, b| fs::sort::natural_cmp(&a.0, &b.0));
        // A menu is for picking one of a few; past this it is a listing, and the
        // listing is what the pane behind it is for.
        self.truncated = items.len() > MENU_LIMIT;
        items.truncate(MENU_LIMIT);

        self.path = Some(path.to_path_buf());
        self.items = items;
        true
    }

    /// Ask for the open chevron's folder again the next time the menu is drawn.
    ///
    /// For a click on a chevron, which is somebody asking afresh: the folder may have gained a
    /// subfolder since the last time it was used. Both fields, and the second is the one that does
    /// any work — clearing [`Self::path`] only rebuilds the rows from what the loader holds, and a
    /// folder whose read *failed* is not in the loader at all, so without clearing [`Self::asked`]
    /// a second click on a share that has come back would never try it again.
    ///
    /// What this deliberately does **not** do is [`crate::loader::Loader::invalidate`] the folder.
    /// How stale a listing may get is the loader's business — the watcher drops the copy of a
    /// folder that changed under it, and so does every file operation this program performs — and
    /// the alternative is a menu that reads the disk on every click of the one control whose
    /// dropdowns open by the dozen, with a `Reading…` in it each time where the rows used to be
    /// instant. F5 is the gesture for "read it all again", here as everywhere else.
    fn reopen(&mut self) {
        self.path = None;
        self.asked = None;
    }

    /// What the open dropdown is listing: the folder it read, and how many subfolders it
    /// found. For the tests, which is how "the chevron opens and has content in it" is
    /// checked without hunting for a menu row's rect.
    ///
    /// `None` while the read is still out, which is what a test has to wait through now that the
    /// folder is read on a worker — the same wait `Harness::settle` does for a pane's listing.
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

// ---------------------------------------------------------------------------
// Completing what is typed into the path field
// ---------------------------------------------------------------------------

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
    // **A folder diff's half has the path and the four buttons that get about, and nothing else** —
    // no filter, no flatten, no preview. Each half is always a tree, has no panel beside it, and a
    // filter on one side would make the two sides disagree about what they are comparing. See
    // [`crate::diff`].
    let bare = tab.diff.is_some();
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
    let surface = crate::ui::bar(t);
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
    //
    // A right click on Back or Forward lists the history either side — see [`history_menu`].
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
        let history = matches!(action, Action::Back(_) | Action::Forward(_));
        let response = tool_button(
            ui,
            t,
            button(x),
            Id::new(("nav", pane, tip)),
            glyph,
            tip,
            enabled,
            false,
            surface,
        );
        if response.clicked() {
            out.push(action);
        }
        // Only an enabled button senses a click at all, so a menu never opens empty.
        if history {
            history_menu(ui, t, &response, pane, &tab.history, tab.at, out);
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
    let filter_width = if bare {
        0.0
    } else if !tab.filter.is_empty() || tab.lens.is_some() || rect.width() > 460.0 {
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
                    // **The bar it sits in, not `background-control`.** A field two points in
                    // from each end of a bar, painted a different colour from it, reads as a hole
                    // in the bar rather than as a control on it — and in a palette where the bar
                    // is the lightest surface in the window there is nothing lighter for the
                    // field to be. `crate::ui::bar` is the one place that colour is decided, so
                    // the tab, the bar and this box cannot drift apart.
                    .fill(crate::ui::bar(t))
                    // **And outlined in the seam's colour, where that reads against the bar.**
                    // With the fill matching the bar the border is the whole of what makes the box
                    // a box, so which colour it takes is not a detail —
                    // `crate::ui::field_outline` is where that is decided and why. Only at rest:
                    // hover, focus and a bad regex still say so in Azur's own colours. See
                    // `azur::components::FieldLook::border`.
                    .border(crate::ui::field_outline(t))
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

    // **A folder diff's one control**, where the filter box is on any other bar: which rows both halves
    // show. Three states, stepped through by clicking and chosen by name from its right-click menu —
    // see [`crate::diff::Show`]. Latched while it is narrowing anything, for the reason the funnel
    // is latched under a lens: it is the only thing on screen that says rows have been left out.
    if let Some(show) = tab.diff.as_ref().map(|d| d.show) {
        if room_for(right, TOOL_SIZE) {
            let rect = button(right - TOOL_SIZE);
            let tip = match show {
                crate::diff::Show::All => {
                    "Showing all files — click to show only what differs, right-click to choose"
                }
                crate::diff::Show::Changes => {
                    "Showing only changes: names, types, sizes and dates — click to show only \
                     added or missing names"
                }
                crate::diff::Show::Names => {
                    "Showing only names on one side and not the other — click to show all files"
                }
            };
            let response = tool_button(
                ui,
                t,
                rect,
                Id::new(("diff-show", pane)),
                &icons::diff,
                tip,
                true,
                show != crate::diff::Show::All,
                surface,
            );
            if response.clicked() {
                out.push(Action::SetDiffShow {
                    pane,
                    show: show.next(),
                });
            }
            diff_show_menu(ui, &response, pane, show, out);
            right = rect.left() - space::S2;
        }
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
    if !bare && room_for(right, TOOL_SIZE) {
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
            // Nothing to flatten on This PC, whose rows are drives, or in the Recycle Bin, whose
            // rows already carry the whole path they came from — see `Tab::toggle_flat`.
            !crate::fs::is_synthetic(&tab.path),
            tab.flat,
            surface,
        );
        if response.clicked() {
            out.push(Action::ToggleFlat(pane));
        }
        flatten_menu(ui, &response, pane, tab.flat, flat_mode, regroup, out);
        right = rect.left() - space::S2;
    }

    // Measure is **not** here, and was for an afternoon. It is on the status line between the view
    // switch and the console's — see [`crate::ui::filelist::status_line`] — because what it turns on
    // is a *column*, and because the figure it reports goes next to the scan's own timing, which is
    // on that bar. This one is where the questions about *where you are* live.

    // The preview toggle, before the flatten one. Both are questions asked about the folder
    // rather than places to go, so they belong at this end — and this one is furthest from the
    // filter because it is the least to do with it.
    //
    // **It carries a context menu**, which is where the panel's position lives: show or hide,
    // and then Right, Bottom or Auto. A right click on the control that opens a thing is where
    // people look for the settings of that thing, and it keeps three radio buttons off a path
    // bar that has no room for them.
    if !bare && room_for(right, TOOL_SIZE) {
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
        segments(
            ui, t, path_rect, pane, tab, menu, loader, icons_cache, slashes, out,
        );
    }

    // **A diff's second folder, asked for** — an outline round the path in the accent's mark, held
    // and then faded. Drawn last, over the crumbs, and hit-tests nothing. See [`crate::diff::Flash`].
    let now = ui.input(|i| i.time);
    if let Some(strength) = tab.diff.as_mut().and_then(|d| d.flash.strength(now)) {
        ui.painter().rect_stroke(
            path_rect.shrink2(vec2(0.0, 3.0)),
            CornerRadius::ZERO,
            egui::Stroke::new(2.0, t.accent.mark.gamma_multiply(strength)),
            egui::StrokeKind::Inside,
        );
        // This window is idle between events, and a fade is nothing but frames.
        ui.ctx().request_repaint();
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

/// Switch the bar into its editable form, prefilled with the current path.
///
/// The current path, not the trail: the field is for going somewhere, and what it should
/// open showing is where you are.
///
/// With the separator the field is set to write, which is the whole of what `Use / in path` does to
/// a field that is opening — see [`with_separator`], and [`slash_menu`] for where it is ticked.
pub fn start_editing(tab: &mut Tab, slashes: bool) {
    // The two places that are not paths open under the names the field takes back. See
    // [`crate::fs::resolve_input`].
    tab.edit_text = if crate::fs::is_synthetic(&tab.path) {
        crate::fs::display_name(&tab.path)
    } else {
        with_separator(&tab.path.to_string_lossy(), slashes)
    };
    tab.editing_path = true;
}

#[cfg(test)]
mod tests;

mod complete;
mod menus;
mod segments;

// The bar is one row on screen and its pieces are not independent — the crumbs decide where the
// path field opens, and the field decides whether there are crumbs at all. The glob says so.
pub(crate) use complete::*;
pub(crate) use menus::*;
pub(crate) use segments::*;
