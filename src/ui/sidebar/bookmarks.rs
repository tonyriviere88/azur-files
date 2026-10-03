//! The bookmarks: the list the user arranges, and the groups they arrange it into.
//!
//! Two halves in one file, deliberately. The model below is a list of entries where an entry
//! is either a folder or a named box of folders, and every gesture the panel offers — drag a
//! row into a group, drag it back out, fold a group away, take a group apart — is one method
//! on it. The drawing further down is the only caller. Keeping them together is what makes
//! the insertion arithmetic checkable without a window: [`Bookmarks::move_to`] and
//! [`landing`] are the two places an off-by-one silently reverses a list, and both are pure.
//!
//! # One level, and no more
//!
//! A group holds folders. It does not hold groups. That is a decision rather than an
//! omission: a sidebar 200 points wide has room for one indent, and a tree of bookmarks is a
//! place to lose bookmarks in. [`Bookmarks::move_to`] refuses a group dropped into a group
//! rather than flattening it, so the rule is kept in the one place a nested group could ever
//! come from.
//!
//! # A group is not a place
//!
//! Clicking one folds it, because there is nowhere to go. Which is also why it does not wear
//! the shell's folder: the rows under it are real folders and Windows draws those itself, so a
//! group drawn the same way would be a folder that does not open. It gets our own glyph
//! instead — see [`crate::icons::bookmark_group`].

use azur_egui_theme::components::{ContextMenu, MenuItem};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::{radius, space};
use egui::{pos2, vec2, CornerRadius, Id, Rect, Sense, Ui};
use std::path::{Path, PathBuf};

use super::{
    hint, menu_targets, navigate_on, row, shell_icon, IconQueue, Marks, CHILD, GLYPH, INDENT, ROW,
};
use crate::app::Action;
use crate::fs::display_name;
use crate::icons;
use crate::pane::PaneId;
use crate::theme::Theme;
use crate::ui::{icon_rect, row_fill, text_left, text_right, truncated};

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

/// One line of the Bookmarks section: a folder, or a named box of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Mark(PathBuf),
    Group(Group),
}

/// A group of bookmarks: a name, whether its rows are showing, and what is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    /// Whether its rows are drawn. Persisted, for the same reason a section's own fold is:
    /// which groups you keep shut is a decision about how you work, not about this session.
    pub open: bool,
    pub marks: Vec<PathBuf>,
}

/// The bookmarks, in the order the sidebar shows them.
///
/// The list is private and every change goes through a method, because two invariants have to
/// hold and neither is local: **a folder appears at most once anywhere**, and **a group holds
/// no groups**. A caller holding the `Vec` could break either without meaning to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bookmarks {
    entries: Vec<Entry>,
}

/// What a new group is called until it is named.
///
/// It is created with its name field open and the text selected, so this is a prompt to type
/// over rather than a label — but it has to be *something*, because a group with a blank name
/// is a row you cannot read and cannot aim at.
pub const NEW_GROUP: &str = "New group";

/// How long a group's name may be.
///
/// Not a storage limit — it is what stops a pasted paragraph becoming one line of the settings
/// file and one row of nothing but ellipsis. Three times what the panel shows at its default
/// width, so widening it still reveals more.
const NAME_LIMIT: usize = 64;

/// Where a row is, and where a drag is going.
///
/// `group` is a position in [`Bookmarks::entries`] whose entry is a [`Group`]; `None` is the
/// top level. `index` counts within whichever of the two it names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Spot {
    pub group: Option<usize>,
    pub index: usize,
}

impl Spot {
    pub fn top(index: usize) -> Self {
        Self { group: None, index }
    }

    pub fn in_group(group: usize, index: usize) -> Self {
        Self {
            group: Some(group),
            index,
        }
    }
}

impl Bookmarks {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The group at `index`, if that is what is there.
    pub fn group(&self, index: usize) -> Option<&Group> {
        match self.entries.get(index) {
            Some(Entry::Group(group)) => Some(group),
            _ => None,
        }
    }

    /// Every bookmarked folder, wherever it sits.
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> + '_ {
        self.entries.iter().flat_map(|entry| match entry {
            Entry::Mark(path) => std::slice::from_ref(path),
            Entry::Group(group) => group.marks.as_slice(),
        })
    }

    /// Whether a folder is bookmarked — at the top level or in any group, which is the only
    /// question `Ctrl+D` and the shell's `Pin to Quick access` ever ask.
    pub fn contains(&self, path: &Path) -> bool {
        self.paths().any(|mark| mark == path)
    }

    /// Pin a folder at the end of the top level. `false` if there was nothing to pin, or it
    /// was pinned already.
    ///
    /// The empty path is "This PC", which is not a folder anybody can bookmark — the guard
    /// lives here rather than at each of the call sites that could forget it.
    pub fn add(&mut self, path: PathBuf) -> bool {
        if path.as_os_str().is_empty() || self.contains(&path) {
            return false;
        }
        self.entries.push(Entry::Mark(path));
        true
    }

    /// The same, into the group at `index`.
    pub fn add_in(&mut self, index: usize, path: PathBuf) -> bool {
        if path.as_os_str().is_empty() || self.contains(&path) {
            return false;
        }
        match self.entries.get_mut(index) {
            Some(Entry::Group(group)) => {
                group.marks.push(path);
                true
            }
            _ => false,
        }
    }

    /// The same again, into the group added most recently — which is how the settings file
    /// spells membership: a `bookmark_group=` line, and the `bookmark_in=` lines under it.
    ///
    /// A `bookmark_in=` with no group before it means a hand-edited file, and a folder is
    /// worth more than the line it was written on, so it becomes a top-level bookmark.
    pub fn add_in_last(&mut self, path: PathBuf) -> bool {
        match self
            .entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::Group(_)))
        {
            Some(index) => self.add_in(index, path),
            None => self.add(path),
        }
    }

    /// Unpin a folder, wherever it is.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.paths().count();
        self.entries.retain(|entry| match entry {
            Entry::Mark(mark) => mark != path,
            Entry::Group(_) => true,
        });
        for entry in &mut self.entries {
            if let Entry::Group(group) = entry {
                group.marks.retain(|mark| mark != path);
            }
        }
        self.paths().count() != before
    }

    /// Add a group at the end, and say where it landed.
    pub fn add_group(&mut self, name: &str, open: bool) -> usize {
        self.entries.push(Entry::Group(Group {
            name: sane_name(name),
            open,
            marks: Vec::new(),
        }));
        self.entries.len() - 1
    }

    /// Rename the group at `index`. `false` if there is no group there, or the name is
    /// nothing but space — a blank row is not a name, and keeping the old one is the only
    /// other answer there is.
    pub fn rename_group(&mut self, index: usize, name: &str) -> bool {
        let name = sane_name(name);
        if name.is_empty() {
            return false;
        }
        match self.entries.get_mut(index) {
            Some(Entry::Group(group)) if group.name != name => {
                group.name = name;
                true
            }
            _ => false,
        }
    }

    /// Fold the group at `index` away, or open it again.
    pub fn toggle_group(&mut self, index: usize) -> bool {
        match self.entries.get_mut(index) {
            Some(Entry::Group(group)) => {
                group.open = !group.open;
                true
            }
            _ => false,
        }
    }

    /// Take the group apart, leaving what was in it where it was.
    ///
    /// The counterpart to [`Self::remove_group`], and the reason both exist: "I do not want
    /// this box" and "I do not want these bookmarks" are two different wishes, and one menu
    /// entry for both would have to guess which was meant. There is no undo here.
    pub fn ungroup(&mut self, index: usize) -> bool {
        let Some(Entry::Group(group)) = self.entries.get_mut(index) else {
            return false;
        };
        let marks = std::mem::take(&mut group.marks);
        self.entries
            .splice(index..=index, marks.into_iter().map(Entry::Mark));
        true
    }

    /// Remove the group at `index`, and the bookmarks in it with it.
    pub fn remove_group(&mut self, index: usize) -> bool {
        if !matches!(self.entries.get(index), Some(Entry::Group(_))) {
            return false;
        }
        self.entries.remove(index);
        true
    }

    /// How many rows a container holds: the top level, or one group's marks.
    ///
    /// Which is where the end of that list is, and that is where a drop onto a folded group
    /// goes.
    pub fn len_of(&self, group: Option<usize>) -> usize {
        match group {
            None => self.entries.len(),
            Some(index) => self.group(index).map_or(0, |group| group.marks.len()),
        }
    }

    /// Whether the row at `spot` is a group rather than a bookmark.
    pub fn is_group(&self, spot: Spot) -> bool {
        spot.group.is_none() && matches!(self.entries.get(spot.index), Some(Entry::Group(_)))
    }

    /// Move the row at `from` to `to`, where `to` is an insertion point in the list **as it
    /// stands** — the position the row there would be pushed down from.
    ///
    /// Returns whether anything moved, which is also what says the settings are worth writing
    /// again. Every way of getting it wrong is a no-op rather than a panic: a stale drag from
    /// a list that has changed since, a group dropped into a group, a spot past the end of
    /// its container.
    pub fn move_to(&mut self, from: Spot, mut to: Spot) -> bool {
        // A group holds folders and not groups. See the module header.
        if self.is_group(from) && to.group.is_some() {
            return false;
        }
        // Onto itself, or immediately after itself: both mean the list it is already in.
        if from.group == to.group && (to.index == from.index || to.index == from.index + 1) {
            return false;
        }
        // Past the end of the container it is going into — which cannot come from a row that
        // is on screen, so it is a drag from a list that has changed since. Refused rather
        // than clamped: an instruction about a list this is not is not an instruction to put
        // something at the bottom of this one. The end itself is `len`, and is allowed.
        if to.index > self.len_of(to.group) {
            return false;
        }

        let taken = match from.group {
            None if from.index < self.entries.len() => self.entries.remove(from.index),
            None => return false,
            Some(index) => match self.entries.get_mut(index) {
                Some(Entry::Group(group)) if from.index < group.marks.len() => {
                    Entry::Mark(group.marks.remove(from.index))
                }
                _ => return false,
            },
        };

        // Taking it out shifts everything after it in the same container down one...
        if to.group == from.group && to.index > from.index {
            to.index -= 1;
        }
        // ...and, when it came out of the top level, it shifts the *groups* as well, since a
        // group is named by its position in that same list.
        if let (None, Some(group)) = (from.group, &mut to.group) {
            if *group > from.index {
                *group -= 1;
            }
        }

        match to.group {
            None => {
                let at = to.index.min(self.entries.len());
                self.entries.insert(at, taken);
                true
            }
            // Only a mark can get this far — a group was refused above — and only into a group
            // that is still there. Anything else puts the row back rather than dropping it on
            // the floor.
            Some(index) => match (taken, self.entries.get_mut(index)) {
                (Entry::Mark(path), Some(Entry::Group(group))) => {
                    let at = to.index.min(group.marks.len());
                    group.marks.insert(at, path);
                    true
                }
                (taken, _) => {
                    let at = from.index.min(self.entries.len());
                    self.entries.insert(at, taken);
                    false
                }
            },
        }
    }
}

/// A group's name as it is allowed to be stored: on one line, trimmed, and bounded.
///
/// One line because the settings file is one `key=value` per line, and a name with a newline
/// in it would come back as two settings and neither of them right. The field cannot produce
/// one — egui's single-line editor drops them — but a paste and a hand-edited file both can.
fn sane_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(NAME_LIMIT)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A flat list of marks, from the paths alone.
///
/// So that a list built in a test reads as the list it is — `["a", "b"].iter().map(PathBuf::from).collect()`
/// — and goes through [`Bookmarks::add`] on the way in, which is what keeps a duplicate out of
/// one.
impl FromIterator<PathBuf> for Bookmarks {
    fn from_iter<I: IntoIterator<Item = PathBuf>>(paths: I) -> Self {
        let mut marks = Self::default();
        for path in paths {
            marks.add(path);
        }
        marks
    }
}

// ---------------------------------------------------------------------------
// What the panel is in the middle of
// ---------------------------------------------------------------------------

/// The bookmark list mid-gesture: a row being dragged, and a group being named.
///
/// One struct rather than two fields on [`crate::app::App`], because they are the same kind of
/// thing — state that belongs to a gesture rather than to the settings — and it is easier to
/// see that they are mutually exclusive when they sit together.
#[derive(Clone, Debug, Default)]
pub struct Editing {
    /// The row a drag picked up, if one is in flight.
    pub drag: Option<Spot>,
    /// The group whose name is being typed.
    pub rename: Option<Rename>,
}

/// A group's name, while it is being edited.
#[derive(Clone, Debug)]
pub struct Rename {
    /// Which group, as a position in [`Bookmarks::entries`].
    pub group: usize,
    pub text: String,
    /// Whether the field still has to take the keyboard and select what is in it. True for one
    /// frame — the one the field first appears in.
    pub fresh: bool,
}

impl Rename {
    pub fn new(group: usize, text: String) -> Self {
        Self {
            group,
            text,
            fresh: true,
        }
    }
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// A row as it was drawn, so a drag can be resolved once they all are.
///
/// Recorded rather than recomputed: the rows come out of a loop over a list that folds groups
/// away, and working out afterwards where the pointer is means knowing where the rows went —
/// not doing that fold again.
#[derive(Clone, Copy, Debug)]
struct Line {
    rect: Rect,
    /// **What a drag treats this row as**: its own rect, except for a group's row, where it is
    /// the group and everything under it.
    ///
    /// A group is one thing, so a drag has to see one thing. Two places read this rather than
    /// [`Self::rect`], and both were wrong before it existed: the highlight for a drop *into* a
    /// group covered the name and left the bookmarks it was about to join outside it, and the
    /// caret for a drop *after* a group was drawn under its name — which is in the middle of its
    /// rows, where it reads as landing between two of them.
    span: Rect,
    /// Where in the model this row is.
    spot: Spot,
    /// Set when the row *is* a group's own row, to that group's position.
    header: Option<usize>,
}

/// Where a drag would land: into a group, or between two rows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Landing {
    /// Into the group at this position — with the whole of it, which is what gets the highlight.
    Into { group: usize, rect: Rect },
    /// At this spot, with the caret along `edge`.
    Between { spot: Spot, edge: Rect },
}

/// The Bookmarks section: the rows, the groups, and the drag in flight over them.
#[allow(clippy::too_many_arguments)]
pub(super) fn section(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
    marks: &mut Marks<'_>,
    icons_cache: &mut crate::shell::icons::Icons,
    queue: &mut IconQueue,
    current: &Path,
    focused: PaneId,
    out: &mut Vec<Action>,
) {
    // The list, out of the bundle: it is a shared reference, so holding a copy of it leaves the
    // two `&mut` fields beside it free to be written while the rows are drawn.
    let list = marks.list;
    if list.is_empty() {
        hint(ui, t, width, 0.0, "Ctrl+D, or drag a folder here");
        return;
    }

    let mut lines: Vec<Line> = Vec::with_capacity(list.entries().len());
    for (index, entry) in list.entries().iter().enumerate() {
        match entry {
            Entry::Mark(path) => {
                let spot = Spot::top(index);
                let rect = mark_row(
                    ui, t, width, 0.0, spot, path, list, marks.editing, icons_cache, queue,
                    current, focused, out,
                );
                lines.push(Line {
                    rect,
                    span: rect,
                    spot,
                    header: None,
                });
            }
            Entry::Group(group) => {
                let rect =
                    group_row(ui, t, width, index, group, list, marks.editing, current, out);
                // Where its own line is, so the whole of it can be written back once its rows
                // have been drawn and there is a whole of it to write.
                let own = lines.len();
                lines.push(Line {
                    rect,
                    span: rect,
                    spot: Spot::top(index),
                    header: Some(index),
                });
                if group.open {
                    if group.marks.is_empty() {
                        // Recorded as the group's row is, so that dropping on the words that ask
                        // for one does what they say. Aiming at a hint that turned out not to be
                        // a target would be the panel inviting a gesture it then ignored.
                        let rect = hint(ui, t, width, CHILD, "drop a bookmark in");
                        lines.push(Line {
                            rect,
                            span: rect,
                            spot: Spot::in_group(index, 0),
                            header: Some(index),
                        });
                    }
                    for (at, path) in group.marks.iter().enumerate() {
                        let spot = Spot::in_group(index, at);
                        let rect = mark_row(
                            ui, t, width, CHILD, spot, path, list, marks.editing, icons_cache,
                            queue, current, focused, out,
                        );
                        lines.push(Line {
                            rect,
                            span: rect,
                            spot,
                            header: None,
                        });
                    }
                }

                // The group as one block: its own row and everything under it, which is what a
                // drag is shown and what a drop is answered from. See [`Line::span`]. The last
                // line is the last row of this group, or the group's own where it drew none.
                let bottom = lines.last().expect("the group's own line").rect.bottom();
                let span = Rect::from_min_max(rect.min, pos2(rect.max.x, bottom));
                lines[own].span = span;
                // Published as a drop zone of its own — see [`super::Marks::rows`] — over the
                // whole of it rather than over its name, so that a folder dragged in from a
                // listing lands where the highlight said it would.
                marks.rows.push((span, index));
            }
        }
    }
    dragging(ui, t, list, marks.editing, &lines, out);
}

/// One bookmark. Returns the row it was drawn in, for the drag.
#[allow(clippy::too_many_arguments)]
fn mark_row(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
    indent: f32,
    spot: Spot,
    path: &Path,
    marks: &Bookmarks,
    editing: &mut Editing,
    icons_cache: &mut crate::shell::icons::Icons,
    queue: &mut IconQueue,
    current: &Path,
    focused: PaneId,
    out: &mut Vec<Action>,
) -> Rect {
    let shell = shell_icon(ui, icons_cache, path);
    let response = row(
        ui,
        t,
        width,
        indent,
        ROW,
        &icons::star_filled,
        t.status.warning,
        shell,
        queue,
        &display_name(path),
        path == current,
        Id::new(("bookmark", path)),
        // Only bookmarks and their groups can be dragged, and only within this list.
        Sense::click_and_drag(),
        editing.drag == Some(spot),
    );
    if response.drag_started() {
        editing.drag = Some(spot);
    }
    navigate_on(&response, focused, path, out);
    ContextMenu::new(&response).show(ui.ctx(), |ui| {
        menu_targets(ui, focused, path, out);
        azur_egui_theme::components::menu_divider(ui);
        move_items(ui, marks, spot, out);
        if ui
            .add(MenuItem::new("Remove bookmark").danger(true))
            .clicked()
        {
            out.push(Action::RemoveBookmark(path.to_path_buf()));
        }
    });
    response.rect
}

/// The ways to move a bookmark between the top level and a group without dragging it.
///
/// Dragging is how this list is arranged, and it is the one gesture that cannot put a row into
/// a group that is folded shut: the rows to aim between are not on screen. So the menu says it
/// in words too — which is also how anybody discovers that a group takes bookmarks at all.
fn move_items(ui: &mut Ui, marks: &Bookmarks, spot: Spot, out: &mut Vec<Action>) {
    let groups: Vec<(usize, &str)> = marks
        .entries()
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| match entry {
            Entry::Group(group) if Some(index) != spot.group => Some((index, group.name.as_str())),
            _ => None,
        })
        .collect();

    if !groups.is_empty() {
        azur_egui_theme::components::submenu(ui, MenuItem::new("Move to group"), |ui| {
            for (index, name) in groups.iter().copied() {
                if ui
                    .add(MenuItem::new(name).icon(&icons::bookmark_group))
                    .clicked()
                {
                    out.push(Action::MoveBookmark {
                        from: spot,
                        to: Spot::in_group(index, marks.len_of(Some(index))),
                    });
                }
            }
        });
    }
    if spot.group.is_some() && ui.add(MenuItem::new("Take out of the group")).clicked() {
        out.push(Action::MoveBookmark {
            from: spot,
            to: Spot::top(marks.len_of(None)),
        });
    }
    if !groups.is_empty() || spot.group.is_some() {
        azur_egui_theme::components::menu_divider(ui);
    }
}

/// A group's own row: the chevron that folds it, our glyph, the name, and how many are inside
/// when they are not on show.
///
/// Painted here rather than through [`row`] for three reasons a "roughly one line" helper
/// cannot carry: the chevron, the count that a long name has to give way to, and the name
/// turning into a field while it is being typed.
#[allow(clippy::too_many_arguments)]
fn group_row(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
    index: usize,
    group: &Group,
    marks: &Bookmarks,
    editing: &mut Editing,
    current: &Path,
    out: &mut Vec<Action>,
) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
    let response = ui.interact(
        rect,
        Id::new(("bookmark-group", index)),
        Sense::click_and_drag(),
    );
    let naming = editing.rename.as_ref().is_some_and(|r| r.group == index);

    // The pointer's own highlight, as on every other row here — see [`row`]. A group being
    // renamed wears none of it: the field standing in the row is what says which one is being
    // named, and a hover fill under it reads as two states at once.
    if let Some(fill) = row_fill(t, false, response.hovered() && !naming) {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    }
    if editing.drag == Some(Spot::top(index)) {
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, t.bg.control_active);
    }

    // The chevron goes in the slot every row leaves for it — the same twelve points a
    // section's heading uses, which is what puts a group's glyph in the same column as the
    // bookmarks' rather than beside it.
    let chevron = icon_rect(rect, rect.left() + INDENT, 12.0);
    let ink = if response.hovered() {
        t.text.secondary
    } else {
        t.text.tertiary
    };
    if group.open {
        azur_icons::chevron_down(ui.painter(), chevron, ink);
    } else {
        azur_icons::chevron_right(ui.painter(), chevron, ink);
    }

    let glyph = icon_rect(rect, rect.left() + INDENT + 12.0 + space::S2, GLYPH);
    icons::bookmark_group(ui.painter(), glyph, t.status.warning);

    // How many are in a folded group, right-aligned — so a long name loses width to the count
    // rather than running under it.
    let mut right = rect.right() - space::S3;
    if !group.open && !group.marks.is_empty() {
        let galley = truncated(
            ui.painter(),
            &group.marks.len().to_string(),
            t.fonts.caption.clone(),
            t.text.tertiary,
            40.0,
        );
        let at = Rect::from_min_max(
            pos2(right - galley.size().x, rect.top()),
            pos2(right, rect.bottom()),
        );
        text_right(ui.painter(), at, galley);
        right = at.left() - space::S2;
    }

    // Lifted the two points every other row in the window lifts its text by — see
    // `filelist::CELL_LIFT`.
    let text = Rect::from_min_max(
        pos2(
            glyph.right() + space::S3,
            rect.top() - crate::ui::filelist::CELL_LIFT,
        ),
        pos2(right, rect.bottom() - crate::ui::filelist::CELL_LIFT),
    );
    if naming {
        name_field(ui, t, index, text, editing, out);
        return rect;
    }

    let galley = truncated(
        ui.painter(),
        &group.name,
        t.fonts.body.clone(),
        if response.hovered() {
            t.text.primary
        } else {
            t.text.secondary
        },
        text.width(),
    );
    text_left(ui.painter(), text, galley);

    // A group is not a place, so a click folds it rather than going anywhere.
    if response.clicked() {
        out.push(Action::ToggleBookmarkGroup(index));
    }
    if response.drag_started() {
        editing.drag = Some(Spot::top(index));
    }
    ContextMenu::new(&response).show(ui.ctx(), |ui| {
        if ui.add(MenuItem::new("Rename").icon(&icons::pencil)).clicked() {
            out.push(Action::BeginRenameBookmarkGroup(index));
        }
        // Only when there is a folder to add and it is pinned nowhere at all — not merely
        // nowhere in *this* group, since a folder is bookmarked once. A menu entry that does
        // nothing is worse than one that is not there.
        if !current.as_os_str().is_empty()
            && !marks.contains(current)
            && ui
                .add(MenuItem::new("Add the current folder").icon(&icons::star))
                .clicked()
        {
            out.push(Action::AddBookmarkIn {
                group: index,
                path: current.to_path_buf(),
            });
        }
        azur_egui_theme::components::menu_divider(ui);
        if !group.marks.is_empty()
            && ui
                .add(MenuItem::new("Ungroup, keeping the bookmarks"))
                .clicked()
        {
            out.push(Action::UngroupBookmarks(index));
        }
        if ui
            .add(MenuItem::new(remove_label(group)).danger(true))
            .clicked()
        {
            out.push(Action::RemoveBookmarkGroup(index));
        }
    });
    rect
}

/// What removing a group is called, which depends on what goes with it.
///
/// A menu entry that says `Remove group` while it is about to take four bookmarks with it is
/// one that lies by omission — and there is no undo here to make that recoverable.
fn remove_label(group: &Group) -> String {
    match group.marks.len() {
        0 => "Remove group".to_owned(),
        1 => "Remove group and its bookmark".to_owned(),
        count => format!("Remove group and its {count} bookmarks"),
    }
}

/// The field a group is named in: over the row, where the name is.
///
/// The same in-place editing the listing does — see [`crate::ui::filelist::rename_field`],
/// which carries the argument and both points of alignment. Simpler here, because there is no
/// extension to keep out of the selection and no column for the field to outgrow: a group's
/// name is one word in a panel of a known width.
fn name_field(
    ui: &mut Ui,
    t: &Theme,
    index: usize,
    at: Rect,
    editing: &mut Editing,
    out: &mut Vec<Action>,
) {
    let Some(rename) = editing.rename.as_mut() else {
        return;
    };
    let fresh = rename.fresh;
    let id = Id::new(("bookmark-group-name", index));

    // **The name does not move when the field opens**, which is the whole of this arithmetic and
    // the one thing about an in-place editor that is invisible in the source and obvious on
    // screen. A field is a box with its own padding and its own idea of where a line sits inside
    // it, so the name it stands in for jumps a point or two the instant it appears — on the one
    // word you are looking at. `filelist::rename_field` measured that flinch off a screenshot;
    // this is the same fix.
    //
    // So: no margin at all, and the box placed so that `Align::Center` inside it lands the line
    // exactly where [`crate::ui::text_left`] would have put the label's first pixel — which
    // means measuring the text in the font the field will lay it out in, and snapping to *device*
    // pixels rather than to points, because half a point is a blurred word.
    let font = egui::TextStyle::Body.resolve(ui.style());
    let line = ui
        .painter()
        .layout_no_wrap(rename.text.clone(), font, egui::Color32::PLACEHOLDER)
        .size();
    use egui::emath::GuiRounding as _;
    let top = (at.center().y - line.y * 0.5).round_to_pixels(ui.painter().pixels_per_point());
    let height = crate::ui::filelist::FIELD_HEIGHT;
    let field = Rect::from_min_size(
        pos2(at.left(), top - (height - line.y) * 0.5),
        vec2(at.width().max(MIN_NAME), height),
    );

    // Azur's focus ring taken off the way a rename in the listing takes it off, and for the
    // same reason: the field is already an obviously editable box, on a row whose own
    // highlight has been removed for it, and the ring on top was the loudest thing in the
    // window. The *width* only — the colour that stroke carries is what selected text is drawn
    // in, and zeroing the whole thing paints the selected name in nothing.
    let ring = ui.visuals().selection.stroke;
    ui.visuals_mut().selection.stroke = egui::Stroke::new(0.0, ring.color);
    let response = crate::ui::squared(ui, |ui| {
        ui.put(
            field,
            egui::TextEdit::singleline(&mut rename.text)
                .id(id)
                .margin(egui::Margin::ZERO)
                .vertical_align(egui::Align::Center)
                .background_color(t.bg.layer)
                .desired_width(field.width()),
        )
    });
    ui.visuals_mut().selection.stroke = ring;

    if fresh {
        response.request_focus();
        // All of it: `New group` is a prompt to type over, and a group has no extension to
        // preserve — which is what the `true` says.
        crate::ui::filelist::select_stem(ui.ctx(), id, &rename.text, true);
        rename.fresh = false;
    }

    let (enter, escape) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if escape {
        out.push(Action::CancelRenameBookmarkGroup);
    } else if enter || response.lost_focus() {
        out.push(Action::CommitRenameBookmarkGroup {
            group: index,
            name: rename.text.clone(),
        });
    }
}

/// Narrow enough to type a name in, in a panel dragged as narrow as it goes.
const MIN_NAME: f32 = 60.0;

// ---------------------------------------------------------------------------
// The drag
// ---------------------------------------------------------------------------

/// A row being dragged: the mark while it is held, and the move when it is let go.
fn dragging(
    ui: &mut Ui,
    t: &Theme,
    marks: &Bookmarks,
    editing: &mut Editing,
    lines: &[Line],
    out: &mut Vec<Action>,
) {
    let Some(from) = editing.drag else { return };
    let Some(pointer) = ui.ctx().pointer_interact_pos() else {
        editing.drag = None;
        return;
    };
    let Some(landing) = landing(lines, pointer, marks.is_group(from), marks.len_of(None)) else {
        editing.drag = None;
        return;
    };

    if ui.input(|i| i.pointer.any_down()) {
        match landing {
            // A group accepts what is dropped on it, so it lights up the way every other drop
            // target in this window does rather than showing a caret beside itself.
            Landing::Into { rect, .. } => crate::ui::drop_target(ui.painter(), rect, t),
            Landing::Between { edge, .. } => {
                ui.painter().rect_filled(
                    edge,
                    CornerRadius::same(radius::CIRCULAR),
                    t.accent.default,
                );
            }
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        return;
    }

    let to = match landing {
        // The end of the group: a folded one has no rows to aim between.
        Landing::Into { group, .. } => Spot::in_group(group, marks.len_of(Some(group))),
        Landing::Between { spot, .. } => spot,
    };
    out.push(Action::MoveBookmark { from, to });
    editing.drag = None;
}

/// Where the pointer says a dragged row is going.
///
/// The rows are short, contiguous and all drawn, so this is arithmetic over their rects rather
/// than hit-testing: which row the pointer is in, and which half of it.
///
/// Three rules that the shape of it does not say on its own:
///
/// **A group's own row means "into it"** rather than "beside it" — which is the only way to
/// fill a group that is folded shut, and the reason a group is a drop target and not merely a
/// row to insert before. The rows *inside* an open one still take a caret of their own, indented
/// with them: that is how a bookmark is put in a particular place in a group, and it is the only
/// thing those rows could usefully mean.
///
/// **A group is one block when a group is what is being dragged.** A group holds no groups, so
/// the rows inside one are not places another group can go — they are excluded from the search
/// entirely, and what is left of the group is [`Line::span`], the whole of it. Aiming at the top
/// half of any part of it lands before it and the bottom half after it, and the caret is drawn on
/// the edge of the block rather than under the group's name, which is a line in the middle of its
/// rows.
///
/// **Below the last row is the end of the top level**, whatever container that last row
/// belongs to. Without it, a list ending in an open group has no reachable "put it at the
/// bottom".
fn landing(lines: &[Line], pointer: egui::Pos2, moving_group: bool, top: usize) -> Option<Landing> {
    // What each row is, for the drag in flight. A group being dragged sees blocks — its own rows
    // are gone from the list and a group is the whole of itself; anything else sees rows, because
    // the rows inside a group are somewhere a bookmark can go. Either way what is left tiles the
    // section from top to bottom with no gaps, which is what makes the search below one
    // comparison per row.
    let reach = |line: &Line| if moving_group { line.span } else { line.rect };
    let rows: Vec<&Line> = lines
        .iter()
        .filter(|line| !moving_group || line.spot.group.is_none())
        .collect();
    let last = *rows.last()?;

    let Some(row) = rows
        .iter()
        .copied()
        .find(|line| pointer.y < reach(line).bottom())
    else {
        let block = reach(last);
        return Some(Landing::Between {
            spot: Spot::top(top),
            edge: caret(block, block.bottom(), 0.0),
        });
    };

    if let Some(group) = row.header {
        if !moving_group {
            return Some(Landing::Into {
                group,
                // The whole group, which is more than the row this was decided from: what the
                // drop is *about* is the group, so that is what lights up. See [`Line::span`].
                rect: row.span,
            });
        }
    }

    let block = reach(row);
    let above = pointer.y < block.center().y;
    Some(Landing::Between {
        spot: Spot {
            group: row.spot.group,
            index: row.spot.index + usize::from(!above),
        },
        edge: caret(
            block,
            if above { block.top() } else { block.bottom() },
            if row.spot.group.is_some() { CHILD } else { 0.0 },
        ),
    })
}

/// The two points of accent that say a row is going between these two.
fn caret(row: Rect, y: f32, indent: f32) -> Rect {
    Rect::from_min_max(
        pos2(row.left() + INDENT + indent, y - 1.0),
        pos2(row.right() - space::S3, y + 1.0),
    )
}

#[cfg(test)]
mod tests;
