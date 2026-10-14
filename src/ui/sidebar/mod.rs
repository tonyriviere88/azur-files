//! The left panel: drives, bookmarks, places, WSL distributions, network locations.
//!
//! Five collapsible groups of rows, by default in the order they are worth scanning: the volumes on
//! this machine, the folders you chose, the ones the shell provides, the Linux distributions WSL has
//! installed, and — last, because it is the only group whose contents this machine may know nothing
//! about — the volumes on other machines.
//!
//! **That order is a default and not a rule.** A heading is a handle: dragging it moves the whole
//! group, and a right click on a heading or on the panel's empty space lists the groups with a tick
//! each, to take one off the panel or put it back. Both are in [`Sections`] and in the settings
//! file, by name. WSL is drawn only where a distribution is installed — see [`crate::fs::wsl`] for
//! how that is known without starting anything.
//!
//! Network is its own group rather than more Drives rows because nothing in it has a drive letter
//! — see [`crate::fs::drives::Drive::display_name`] — and a row named `fileserver` sitting among
//! rows named `Windows (C:)` under one heading would have to explain itself. Mostly it holds
//! *machines*, which are places to look rather than volumes; the exception is drawn by the same
//! [`drive_row`] a disk gets, because a mount has a capacity, a gauge and a menu exactly as a disk
//! does.
//!
//! Every row navigates the focused pane on a left click, opens in a new tab on a
//! middle click, and offers "open in a new pane" from its context menu — so the
//! split layout is reachable from here as well as by dragging a tab.
//!
//! The icons are the shell's, per *place* rather than per type — see
//! [`crate::shell::icons::Icons::place`], which is what makes Downloads look like Downloads
//! rather than like a folder.
//!
//! Bookmarks are a list the user arranges: dragging one reorders it, and dragging a folder
//! in from a listing pins it. The second is an OLE drop rather than a gesture of ours — the
//! group publishes itself as a drop zone, and [`crate::app::App`] turns a drop there into
//! a bookmark instead of a file operation.
//!
//! They can also be arranged *into* groups, which is a list of its own and lives in
//! [`bookmarks`] — the model, the rows, and the arithmetic of dropping one row between two
//! others. The `+` beside the Bookmarks heading is what makes a group; everything else about
//! the section is drawn from here through the same [`row`] every other line in the panel uses.

use azur_egui_theme::components::{ContextMenu, MenuItem};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::{radius, space, typography};
use egui::{pos2, vec2, CornerRadius, Id, Rect, Sense, Stroke, Ui};
use std::path::Path;

use crate::app::Action;
use crate::fs::drives::Drive;
use crate::fs::fmt;
use crate::fs::places::Place;
use crate::fs::wsl::Distro;
use crate::icons;
use crate::pane::{PaneId, Side};
use crate::theme::Theme;
use crate::ui::{icon_rect, row_fill, section_label, text_left, truncated};

pub mod bookmarks;

pub use bookmarks::{Bookmarks, Editing, Entry, Rename, Spot};

/// The Bookmarks section's own state, which is more than the list.
///
/// One struct rather than four more arguments to [`show`], and they belong together: all four
/// are about the one section, and three of them exist only because a *drag* is a thing that
/// spans frames.
pub struct Marks<'a> {
    pub list: &'a Bookmarks,
    /// A row being dragged, and a group being named.
    pub editing: &'a mut Editing,
    /// Where each group was drawn, refilled every frame, as `(the whole of it, its position in
    /// the list)`.
    ///
    /// So that a folder dragged in from a listing can land in the group it is dropped on rather
    /// than at the end of the list — [`crate::app::App`] publishes these as drop zones of their
    /// own, the way a listing publishes its folder rows. Written here because this is the only
    /// place that knows which rows a folded group did not draw.
    ///
    /// **The group's own row *and* the rows under it**, because a group is one thing: the
    /// highlight a drag is shown covers the block, so the block is what has to accept the drop.
    /// See [`bookmarks::Line::span`].
    pub rows: &'a mut Vec<(Rect, usize)>,
    /// Whether a drag from outside is over the window.
    ///
    /// Which takes the `+` away: it is a hover control, and during a drop it would be a button
    /// under the pointer that the pointer cannot press, sitting in the middle of a highlight
    /// that says something else is about to happen.
    pub dragging: bool,
}

/// A row: one body line and a point of air either side.
///
/// Dense on purpose. The panel is a list of names to scan rather than a set of controls
/// to aim at, and a 14-point line does not need 26 points of row to be legible — 22 puts
/// every drive, bookmark and place on screen at once in a 600-point window, which is the
/// default this opens at.
const PAD_Y: f32 = 1.0;
const ROW: f32 = typography::LINE_BODY + PAD_Y * 2.0;
/// How tall a drive's capacity bar is.
///
/// **Also the listing's share bar** — see `filelist::rows::share_bar`, whose whole justification is
/// that it is the same kind of statement said the same way, and one window should not have two
/// visual languages for "this much of that". One constant rather than the same 3.0 written twice with
/// a comment claiming they agree.
pub(crate) const GAUGE_HEIGHT: f32 = 3.0;
/// The gap between the text and the capacity bar under it.
const GAUGE_GAP: f32 = 1.0;
/// A drive row: the name, and the capacity bar under it. The numbers are a tooltip.
const DRIVE_ROW: f32 = PAD_Y + typography::LINE_BODY + GAUGE_GAP + GAUGE_HEIGHT + PAD_Y;
const HEADER: f32 = 20.0;
const GLYPH: f32 = 14.0;
/// Left inset for a row's glyph. The 2px selection bar lands inside it.
const INDENT: f32 = space::S3;
/// Left edge to where a row's label starts: the indent, the space a group's chevron
/// occupies, and the glyph. Known without a rect, which is what lets a drive row decide
/// how tall it is before it is allocated.
const TEXT_INSET: f32 = INDENT + 12.0 + space::S2 + GLYPH + space::S3;
/// How far a bookmark inside a group is set in from one that is not.
///
/// A chevron's width, which is what the group's own chevron occupies on the row above — so a
/// child's glyph starts where its group's name does, and the nesting is legible without a
/// guide line. One indent is all there is: a group holds folders and not groups, so no row is
/// ever further in than this. See [`bookmarks`].
const CHILD: f32 = 12.0;

/// One group of rows in the panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Drives,
    Bookmarks,
    Places,
    Wsl,
    Network,
}

impl Section {
    /// Every group, in the order a first run shows them — see the module header for why that order.
    /// WSL sits between Places and Network: it is on this machine, so it is more certain than
    /// anything on the network, and less often wanted than the folders above it.
    pub const ALL: [Section; 5] = [
        Section::Drives,
        Section::Bookmarks,
        Section::Places,
        Section::Wsl,
        Section::Network,
    ];

    /// What the settings file calls it. A name rather than a position, so that a group added later
    /// cannot reinterpret an order somebody already arranged — the trap `sections=` has to step
    /// around by writing its flags out of panel order.
    pub fn key(self) -> &'static str {
        match self {
            Section::Drives => "drives",
            Section::Bookmarks => "bookmarks",
            Section::Places => "places",
            Section::Wsl => "wsl",
            Section::Network => "network",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Section::Drives => "Drives",
            Section::Bookmarks => "Bookmarks",
            Section::Places => "Places",
            Section::Wsl => "WSL",
            Section::Network => "Network",
        }
    }
}

/// How the panel is arranged: which groups are open, which are shown at all, and in what order.
/// Persisted, because each of the three is a decision about how you work rather than about this
/// session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sections {
    pub drives: bool,
    pub network: bool,
    pub bookmarks: bool,
    pub places: bool,
    pub wsl: bool,
    /// Top to bottom. Always every [`Section`] exactly once — see [`Self::set_order`], which is
    /// the only way anything outside this type writes it.
    pub order: [Section; 5],
    /// Taken off the panel from its right-click menu, indexed by `Section as usize`.
    pub hidden: [bool; 5],
}

impl Default for Sections {
    fn default() -> Self {
        Self {
            drives: true,
            network: true,
            bookmarks: true,
            places: true,
            wsl: true,
            order: Section::ALL,
            hidden: [false; 5],
        }
    }
}

impl Sections {
    /// Whether a group's rows are drawn under its heading.
    pub fn open_mut(&mut self, section: Section) -> &mut bool {
        match section {
            Section::Drives => &mut self.drives,
            Section::Bookmarks => &mut self.bookmarks,
            Section::Places => &mut self.places,
            Section::Wsl => &mut self.wsl,
            Section::Network => &mut self.network,
        }
    }

    pub fn shown(&self, section: Section) -> bool {
        !self.hidden[section as usize]
    }

    /// Put a group on the panel or take it off. Whether anything changed, which is whether the
    /// settings need writing.
    pub fn set_shown(&mut self, section: Section, shown: bool) -> bool {
        let was = self.shown(section);
        self.hidden[section as usize] = !shown;
        was != shown
    }

    /// Arrange the groups in `order`, which may name any of them in any number.
    ///
    /// Duplicates and unknown names are already gone by the time it gets here; what it has to settle
    /// is a group the order **does not mention** — a file written before that group existed, or one
    /// edited by hand. Such a group goes back in beside the one it follows in [`Section::ALL`], so
    /// an arrangement written before WSL existed gains WSL after Places, wherever Places was put.
    pub fn set_order(&mut self, order: impl IntoIterator<Item = Section>) {
        let mut list: Vec<Section> = Vec::with_capacity(Section::ALL.len());
        for section in order {
            if !list.contains(&section) {
                list.push(section);
            }
        }
        for (at, section) in Section::ALL.into_iter().enumerate() {
            if list.contains(&section) {
                continue;
            }
            let after = Section::ALL[..at]
                .iter()
                .rev()
                .find_map(|before| list.iter().position(|had| had == before));
            list.insert(after.map_or(0, |index| index + 1), section);
        }
        self.order.copy_from_slice(&list);
    }

    /// Take `from` out and put it on one side of `to`. Whether anything moved.
    pub fn move_beside(&mut self, from: Section, to: Section, after: bool) -> bool {
        if from == to {
            return false;
        }
        let before = self.order;
        let mut list: Vec<Section> = self.order.into_iter().filter(|s| *s != from).collect();
        let Some(at) = list.iter().position(|s| *s == to) else {
            return false;
        };
        list.insert(at + usize::from(after), from);
        self.order.copy_from_slice(&list);
        self.order != before
    }

    /// Put the groups back in the order a first run shows them. Whether anything moved.
    pub fn reset_order(&mut self) -> bool {
        let moved = self.order != Section::ALL;
        self.order = Section::ALL;
        moved
    }
}

/// Shell icons waiting to be drawn in one run.
///
/// egui starts a new draw call whenever the texture changes, and a shell icon is the only
/// thing in a sidebar row that does not come out of the font atlas — so drawing one inside
/// the row loop splits the frame's primitive stream at every row. Under `glow` that is not
/// merely slow, it leaks: see the note in [`crate::ui::filelist`] and `examples/spin.rs`.
type IconQueue = Vec<(egui::TextureId, Rect, Rect)>;

/// Draw the sidebar's contents into the current `Ui`, which the panel has already
/// sized.
///
/// The groups are drawn in [`Sections::order`], skipping any taken off the panel and WSL when
/// nothing is installed. A heading is also a handle: dragging one moves its group, and a right
/// click on a heading or on the panel's empty space offers the list of groups to tick. Both are
/// pushed as actions rather than done here — see [`crate::app::Action::MoveSection`].
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    t: &Theme,
    drives: &[Drive],
    shares: &[Drive],
    servers: &[std::path::PathBuf],
    // Machines a browse found, drawn muted, and whether that browse is still running. Empty until
    // the button on the Network heading is pressed — see [`discover_button`].
    found: &[std::path::PathBuf],
    finding: bool,
    marks: &mut Marks<'_>,
    places: &[Place],
    distros: &[Distro],
    current: &Path,
    focused: PaneId,
    sections: &mut Sections,
    icons_cache: &mut crate::shell::icons::Icons,
    scratch: &mut String,
    out: &mut Vec<Action>,
) -> Option<Rect> {
    let mut bookmarks_rect = None;
    let mut queue: IconQueue = Vec::new();

    // **The panel's empty space, for the right click that picks which groups are shown.** Interacted
    // before anything in the scroll area, which is what puts it underneath: egui hands a click to
    // the last widget that claimed the pointer's position, so every row and heading drawn after
    // this keeps its own menu, and only the space below the last group falls through to here.
    let back = ui.interact(ui.max_rect(), Id::new("sidebar-back"), Sense::click());
    let wsl = !distros.is_empty();
    sections_menu(&back, sections, wsl, out);

    // **A subtle scrollbar: shown only while the pointer is over the panel.** egui calls a bar
    // "dormant" when the pointer is outside its scroll area, and this one fills the panel, so a
    // dormant handle at zero opacity is a bar that appears on hovering the sidebar. Floating, too:
    // a solid bar allocates its gutter whether or not it is showing, and an invisible bar that
    // still narrows every row is not subtle. Nothing is drawn into `ui` after this.
    let scroll = &mut ui.style_mut().spacing.scroll;
    scroll.floating = true;
    scroll.dormant_handle_opacity = 0.0;

    egui::ScrollArea::vertical()
        .id_salt("sidebar")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width();
            // Rows are contiguous. The design system's 8-point item spacing is right for
            // controls and wrong for a list of names: the hover fill *is* the row, and a
            // gap between rows turns a list into a column of buttons and costs a third of
            // the panel. The groups get their air explicitly instead.
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(space::S1);

            // Refilled by the Bookmarks group when it is drawn, and empty when it is not: a group
            // taken off the panel is not somewhere a drag from a listing can land.
            marks.rows.clear();
            // Where each group was drawn, the gap above it included, for a heading being dragged.
            let mut spans: Vec<(Section, Rect)> = Vec::with_capacity(Section::ALL.len());
            // The heading under the pointer's drag, and whether this is the frame it was let go.
            let mut held: Option<(Section, bool)> = None;

            for section in sections.order {
                if !sections.shown(section) || (section == Section::Wsl && !wsl) {
                    continue;
                }
                let top = ui.cursor().min;
                if !spans.is_empty() {
                    ui.add_space(space::S2);
                }
                let reserve = match section {
                    Section::Bookmarks | Section::Network => ADD,
                    _ => 0.0,
                };
                let head = group_header(
                    ui,
                    t,
                    width,
                    section.label(),
                    sections.open_mut(section),
                    reserve,
                );
                sections_menu(&head.response, sections, wsl, out);
                if head.response.drag_stopped() {
                    held = Some((section, true));
                } else if head.response.dragged() {
                    held = Some((section, false));
                }

                match section {
                    // ---- Drives -----------------------------------------------
                    Section::Drives if head.open => {
                        for drive in drives {
                            // Not the shell's icon for a mapping that is not mounted: asking the
                            // shell about `H:\` is a way of touching it, and the painted glyph says
                            // enough.
                            let shell = if drive.disconnected {
                                None
                            } else {
                                shell_icon(ui, icons_cache, &drive.path)
                            };
                            drive_row(
                                ui, t, width, drive, shell, &mut queue, current, focused,
                                scratch, out,
                            );
                        }
                    }

                    // ---- Bookmarks --------------------------------------------
                    Section::Bookmarks => {
                        if head.open {
                            bookmarks::section(
                                ui, t, width, marks, icons_cache, &mut queue, current, focused,
                                out,
                            );
                        }
                        // The section as a whole, which is both what a drag from a listing may be
                        // dropped on and the area the `+` answers to the pointer within. Known only
                        // now, after the rows — the alternative is last frame's rect, and a control
                        // that appears a frame late is a control that flickers as the pointer
                        // crosses into the section.
                        let area = down_to_cursor(ui, head.rect.min, width);
                        if head.open {
                            bookmarks_rect = Some(area);
                        }
                        // The one control in this panel that makes something, and it is on the
                        // heading rather than in a menu because a group is the only thing here that
                        // has to be *made* before it can be used — every other row in the sidebar
                        // already exists. Drawn whether the section is folded or not: making a group
                        // is how somebody would open it again.
                        new_group_button(ui, t, head.rect, area, marks, out);
                    }

                    // ---- Places -----------------------------------------------
                    Section::Places if head.open => {
                        for place in places {
                            let glyph = icons::for_place(place.icon);
                            let color = match crate::fs::places::kind_of(place.icon) {
                                fmt::Kind::Folder => t.folder,
                                _ => t.text.secondary,
                            };
                            let shell = shell_icon(ui, icons_cache, &place.path);
                            let response = row(
                                ui,
                                t,
                                width,
                                0.0,
                                ROW,
                                glyph,
                                color,
                                shell,
                                &mut queue,
                                &place.label,
                                place.path == current,
                                Id::new(("place", &place.label)),
                                Sense::click(),
                                false,
                                false,
                            );
                            // The Recycle Bin too, which used to be handed to Explorer — see
                            // [`crate::fs::recycle`] for how it is listed here now.
                            navigate_on(&response, focused, &place.path, out);
                            place_menu(ui, &response, focused, &place.path, out);
                        }
                    }

                    // ---- WSL --------------------------------------------------
                    //
                    // The distributions installed for this user, read from the registry and never
                    // from WSL — see [`crate::fs::wsl`] for what asking WSL would cost, and why the
                    // icon is a file beside the distribution's disk and not the shell's answer for
                    // its share. Not drawn at all where nothing is installed.
                    Section::Wsl if head.open => {
                        for distro in distros {
                            let shell = distro
                                .icon
                                .as_deref()
                                .and_then(|icon| shell_icon(ui, icons_cache, icon));
                            let response = row(
                                ui,
                                t,
                                width,
                                0.0,
                                ROW,
                                &icons::terminal,
                                t.text.secondary,
                                shell,
                                &mut queue,
                                &distro.name,
                                distro.path == current,
                                Id::new(("wsl", &distro.name)),
                                Sense::click(),
                                false,
                                false,
                            );
                            navigate_on(&response, focused, &distro.path, out);
                            place_menu(ui, &response, focused, &distro.path, out);
                        }
                    }

                    // ---- Network ----------------------------------------------
                    //
                    // **Last** by default, after Places, because it is the only group whose
                    // contents this machine may know nothing about: the ones above it are always
                    // there and always right, and a panel is scanned from the top.
                    //
                    // Three kinds of row, in descending certainty:
                    //
                    // - **A machine this window is connected to** — `fileserver`. Opening it lists
                    //   every share the server offers; see [`crate::fs::drives::server_dir`]. One
                    //   line, no gauge, because a machine has no capacity.
                    // - **A machine that is merely *there*** — drawn in `text-tertiary` to say so,
                    //   because it has been seen and nothing more. Two things put one here and the
                    //   row does not distinguish them, since clicking either resolves it the same
                    //   way: the refresh button on the heading, which is an SSDP or WSD
                    //   announcement; and a machine whose connection dropped but which still
                    //   answers on the SMB port, probed at startup. See
                    //   [`crate::loader::Volumes::found`]. Clicking one opens it, which is what
                    //   asks for a connection: a machine that wants credentials refuses the share
                    //   list, and refusing the share list is what raises Windows' credential
                    //   prompt. See [`crate::app::connect`].
                    // - **A network location that no machine here can reach** — a connection to a
                    //   DFS path, whose first component is a domain rather than a server. Drawn by
                    //   the same [`drive_row`] a disk gets, because it is a mount with the same
                    //   capacity to show.
                    //
                    // The group is drawn even when all three are empty, because an absent group
                    // answers "does this program see my share at all" with silence.
                    Section::Network => {
                        if head.open {
                            if servers.is_empty() && shares.is_empty() && found.is_empty() {
                                hint(ui, t, width, 0.0, "No network locations");
                            }
                            for server in servers {
                                machine_row(
                                    ui, t, width, server, false, icons_cache, &mut queue,
                                    current, focused, out,
                                );
                            }
                            // The found ones after the connected ones, and never a machine that is
                            // already above: a row drawn twice, once certain and once dimmed, would
                            // read as two places.
                            for machine in found {
                                if servers.iter().any(|had| {
                                    had.as_os_str().eq_ignore_ascii_case(machine.as_os_str())
                                }) {
                                    continue;
                                }
                                machine_row(
                                    ui, t, width, machine, true, icons_cache, &mut queue,
                                    current, focused, out,
                                );
                            }
                            for share in shares {
                                let shell = shell_icon(ui, icons_cache, &share.path);
                                drive_row(
                                    ui, t, width, share, shell, &mut queue, current, focused,
                                    scratch, out,
                                );
                            }
                        }
                        // The area the button answers to the pointer within, which is the heading
                        // and the rows under it — the same span the `+` on Bookmarks uses, and for
                        // the same reason.
                        let area = down_to_cursor(ui, head.rect.min, width);
                        discover_button(ui, t, head.rect, area, finding, out);
                    }

                    // A folded group: its heading is all there is.
                    _ => {}
                }

                spans.push((section, down_to_cursor(ui, top, width)));
            }

            if let Some((from, released)) = held {
                moving_section(ui, t, &spans, from, released, out);
            }

            ui.add_space(space::S3);
            // Every row's shell icon, in one run — see [`IconQueue`]. Inside the scroll area,
            // so they are clipped with the rows they belong to.
            flush_icons(ui, &queue);
        });
    bookmarks_rect
}

/// From `top` down to where the next row would go, across the panel: a group, or its heading and
/// rows.
fn down_to_cursor(ui: &Ui, top: egui::Pos2, width: f32) -> Rect {
    Rect::from_min_max(top, pos2(top.x + width, ui.cursor().min.y))
}

/// The menu a place or a distribution offers: the ways to open it, and pinning it.
fn place_menu(
    ui: &Ui,
    response: &egui::Response,
    focused: PaneId,
    path: &Path,
    out: &mut Vec<Action>,
) {
    ContextMenu::new(response).show(ui.ctx(), |ui| {
        menu_targets(ui, focused, path, out);
        azur_egui_theme::components::menu_divider(ui);
        if ui
            .add(MenuItem::new("Add to bookmarks").icon(&icons::star))
            .clicked()
        {
            out.push(Action::AddBookmark(path.to_path_buf()));
        }
    });
}

/// Which groups the panel shows, as a list of ticks — on the empty space, and on every heading.
///
/// **Sticky**, because these are settings rather than commands: taking two groups off would
/// otherwise mean opening the menu twice. WSL is listed only where something is installed; a
/// ticked entry that puts nothing on the panel would read as a setting that does not work.
fn sections_menu(target: &egui::Response, sections: &Sections, wsl: bool, out: &mut Vec<Action>) {
    ContextMenu::new(target).sticky(true).show(&target.ctx, |ui| {
        for section in sections.order {
            if section == Section::Wsl && !wsl {
                continue;
            }
            let shown = sections.shown(section);
            if ui
                .add(MenuItem::new(section.label()).selected(shown))
                .clicked()
            {
                out.push(Action::ShowSection {
                    section,
                    shown: !shown,
                });
            }
        }
        // Only once there is an order to undo. Dragging is the one way to arrange the groups, and
        // dragging five of them back by hand is the chore this saves.
        if sections.order != Section::ALL {
            azur_egui_theme::components::menu_divider(ui);
            if ui.add(MenuItem::new("Reset order")).clicked() {
                out.push(Action::ResetSectionOrder);
                ui.close();
            }
        }
    });
}

/// A heading being dragged: where it would go while it is held, and the move when it is let go.
///
/// The same accent caret a bookmark being dragged shows between rows, drawn on the edge of a
/// whole group.
fn moving_section(
    ui: &Ui,
    t: &Theme,
    spans: &[(Section, Rect)],
    from: Section,
    released: bool,
    out: &mut Vec<Action>,
) {
    let Some(pointer) = ui.ctx().pointer_interact_pos() else {
        return;
    };
    let Some((to, span, after)) = section_landing(spans, pointer.y) else {
        return;
    };
    if released {
        if to != from {
            out.push(Action::MoveSection { from, to, after });
        }
        return;
    }
    let y = if after { span.bottom() } else { span.top() };
    ui.painter().rect_filled(
        Rect::from_min_max(
            pos2(span.left() + INDENT, y - 1.0),
            pos2(span.right() - space::S3, y + 1.0),
        ),
        CornerRadius::same(radius::CIRCULAR),
        t.accent.default,
    );
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
}

/// Which group a dragged heading is going beside, where that group was drawn, and whether it goes
/// after it.
///
/// The groups tile the panel top to bottom with no gaps — each one's span takes in the air above
/// it — so this is one comparison per group: the first one the pointer is above the bottom of, and
/// which half of it. Below the last group is after the last group.
fn section_landing(spans: &[(Section, Rect)], y: f32) -> Option<(Section, Rect, bool)> {
    let &(last, span) = spans.last()?;
    match spans.iter().find(|(_, span)| y < span.bottom()) {
        Some(&(section, span)) => Some((section, span, y >= span.center().y)),
        None => Some((last, span, true)),
    }
}

/// A machine: a name and an icon, and nothing else because a machine has nothing else.
///
/// `found` is whether it came out of a network browse rather than out of this machine's own
/// connections, which is the whole difference between the two kinds of row — and it is said in the
/// ink rather than with a badge or a second line. See [`row`]'s `muted`.
#[allow(clippy::too_many_arguments)]
fn machine_row(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
    machine: &Path,
    found: bool,
    icons_cache: &mut crate::shell::icons::Icons,
    queue: &mut IconQueue,
    current: &Path,
    focused: PaneId,
    out: &mut Vec<Action>,
) {
    // **No shell icon for a machine nobody is connected to.** Asking the shell about a path is
    // asking it to go and look at that path, and for an unreachable one that is a question with a
    // network timeout in it — on the icon worker, which every row in the window shares. A connected
    // machine has already answered, so it may as well wear its own icon.
    let shell = if found {
        None
    } else {
        shell_icon(ui, icons_cache, machine)
    };
    let response = row(
        ui,
        t,
        width,
        0.0,
        ROW,
        &icons::drive_network,
        if found { t.text.tertiary } else { t.text.secondary },
        shell,
        queue,
        &crate::fs::display_name(machine),
        machine == current,
        Id::new(("server", machine)),
        Sense::click(),
        false,
        found,
    );
    navigate_on(&response, focused, machine, out);
    ContextMenu::new(&response).show(ui.ctx(), |ui| {
        menu_targets(ui, focused, machine, out);
    });
}

/// The refresh on the Network heading: go and look for machines.
///
/// The `+` on Bookmarks in every respect that is about the panel — a hover control rather than
/// permanent furniture, drawn after the heading so a click cannot fold the section on its way — and
/// different in the one respect that is about the network: it says whether it is still looking.
///
/// **The only thing in this program that starts a browse.** See [`crate::loader::Volumes::discover`]
/// for why nothing else does: it reaches the network, it takes as long as the network takes, and
/// what it answers is a list of other people's computers.
fn discover_button(
    ui: &mut Ui,
    t: &Theme,
    heading: Rect,
    area: Rect,
    finding: bool,
    out: &mut Vec<Action>,
) {
    // Shown while looking even with the pointer away, which is the one place this parts company
    // with the `+`: a control that vanishes mid-answer takes the only sign that anything is
    // happening with it.
    let pointer = ui.ctx().pointer_interact_pos();
    if !finding && !pointer.is_some_and(|at| area.contains(at)) {
        return;
    }
    let rect = Rect::from_center_size(
        pos2(heading.right() - space::S2 - ADD * 0.5, heading.center().y),
        vec2(ADD, ADD),
    );

    // **A spinner in the button's place while it is looking**, rather than the refresh glyph in a
    // pressed state. A browse takes 14 seconds on this machine — see
    // [`crate::fs::drives::discover::machines`] — and for that long a latched button is
    // indistinguishable from one that has stuck: it is the only thing on screen that could say
    // work is happening, and a static glyph says the opposite.
    //
    // Drawn here rather than with Azur's `Spinner`, which allocates its own space in a layout and
    // cannot be put at a rect on top of a heading. Its shape though: a `stroke-subtle` ring with
    // one accent three-quarter arc, 0.8s a turn.
    if finding {
        let response = ui.interact(rect, Id::new("network-discover"), Sense::hover());
        if response.hovered() {
            azur_egui_theme::components::tooltip(
                response,
                "Looking for machines on the network…",
            );
        }
        spinner(ui, t, rect);
        return;
    }

    let response = crate::ui::tool_button(
        ui,
        t,
        rect,
        Id::new("network-discover"),
        // The same glyph the path bar's Refresh wears, because it is the same verb: go and look
        // again. See [`crate::ui::breadcrumb`].
        &icons::refresh,
        "Look for machines on the network",
        true,
        false,
        t.bg.layer_alt,
    );
    if response.clicked() {
        out.push(Action::DiscoverNetwork);
    }
}

/// A turning ring, for as long as the browse runs.
///
/// The one animation in this panel, and it asks for the repaint that keeps it turning — without
/// that, an idle window draws the arc once and leaves it stopped, which reads as a hang rather than
/// as work.
fn spinner(ui: &Ui, t: &Theme, rect: Rect) {
    /// A turn every 0.8 seconds, which is Azur's `azur-spinner-spin`.
    const PERIOD: f32 = 0.8;
    let width = 2.0;
    let radius = (rect.width() - width) * 0.5;
    ui.painter().circle_stroke(
        rect.center(),
        radius,
        Stroke::new(width, t.stroke.subtle),
    );

    let time = ui.input(|i| i.time) as f32;
    let start = (time / PERIOD) * std::f32::consts::TAU;
    let sweep = std::f32::consts::TAU * 0.75;
    const STEPS: usize = 24;
    let points: Vec<egui::Pos2> = (0..=STEPS)
        .map(|step| {
            let angle = start + sweep * (step as f32 / STEPS as f32);
            rect.center() + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        Stroke::new(width, t.accent.mark),
    ));
    // It turns, so the next frame has to come.
    ui.ctx().request_repaint();
}

/// The square the `+` on the Bookmarks heading takes, and what the label gives up for it.
///
/// 18 in a 20-point heading, which is as much of a target as the row has to give and still
/// leaves a point of air above and below the button's own fill.
const ADD: f32 = 18.0;

/// `+` at the right of the Bookmarks heading: a new group.
///
/// **Only while the pointer is in the section**, `area`. A permanent button on a heading is a
/// permanent piece of furniture in a panel whose whole job is to be a list of names — the same
/// argument that took the star off the path bar — and making a group is a rare thing to do. It
/// appears where the hand already is, and the panel is otherwise exactly as quiet as it was.
///
/// And **not while anything is being dragged**: a drag from a listing is on its way to being
/// pinned, so a button under the pointer in the middle of a drop highlight would be advertising
/// a press that cannot happen. A bookmark being dragged within the list is the same thing.
///
/// Interacted *after* the heading it sits on, which is what puts it on top — egui hands a
/// click to the last widget that claimed the pointer's position. [`group_header`] is told how
/// much of its right edge this takes, both so the label stops short of it and so a click that
/// lands here cannot also fold the section away.
fn new_group_button(
    ui: &mut Ui,
    t: &Theme,
    heading: Rect,
    area: Rect,
    marks: &Marks<'_>,
    out: &mut Vec<Action>,
) {
    if marks.dragging || marks.editing.drag.is_some() {
        return;
    }
    let pointer = ui.ctx().pointer_interact_pos();
    if !pointer.is_some_and(|at| area.contains(at)) {
        return;
    }
    let rect = Rect::from_center_size(
        pos2(heading.right() - space::S2 - ADD * 0.5, heading.center().y),
        vec2(ADD, ADD),
    );
    let response = crate::ui::tool_button(
        ui,
        t,
        rect,
        Id::new("bookmark-group-add"),
        &icons::plus,
        "New group of bookmarks",
        true,
        false,
        t.bg.layer_alt,
    );
    if response.clicked() {
        out.push(Action::AddBookmarkGroup);
    }
}

/// A group's heading, as it was drawn.
struct Head {
    rect: Rect,
    /// Whether its rows should be drawn.
    open: bool,
    /// For the right-click menu and the drag that moves the group.
    response: egui::Response,
}

/// A group heading that folds its rows away, and the handle that moves the group.
///
/// `reserve` is how much of the right edge belongs to something else — a control drawn on the
/// heading afterwards, which is the `+` on Bookmarks and the refresh on Network: the label stops
/// short of it, and a click that lands in it does not fold the section.
///
/// **A click folds and a drag moves**, which egui tells apart by distance: a press that travels
/// less than a few points and comes back up is a click. The heading being dragged goes quiet the
/// way a dragged bookmark does, and stays where it is until it is let go.
fn group_header(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
    label: &str,
    open: &mut bool,
    reserve: f32,
) -> Head {
    let (rect, _) = ui.allocate_exact_size(vec2(width, HEADER), Sense::hover());
    let response = ui.interact(
        rect,
        Id::new(("sidebar-group", label)),
        Sense::click_and_drag(),
    );
    if response.dragged() {
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, t.bg.control_active);
    }
    // Where the click landed, and not merely that one did: a control on the heading is on top
    // of it, so egui gives it the click — but this is the belt to that braces, because a
    // heading that folds itself away under a button is a fault nothing else would catch.
    let on_reserve = response
        .interact_pointer_pos()
        .is_some_and(|at| at.x > rect.right() - reserve);
    if response.clicked() && !on_reserve {
        *open = !*open;
    }

    let chevron = icon_rect(rect, rect.left() + INDENT, 12.0);
    let color = if response.hovered() {
        t.text.secondary
    } else {
        t.text.tertiary
    };
    if *open {
        azur_icons::chevron_down(ui.painter(), chevron, color);
    } else {
        azur_icons::chevron_right(ui.painter(), chevron, color);
    }
    section_label(
        ui.painter(),
        Rect::from_min_max(
            pos2(chevron.right() + space::S2, rect.top()),
            pos2(rect.right() - space::S3, rect.bottom()),
        ),
        t,
        label,
    );
    Head {
        rect,
        open: *open,
        response,
    }
}

/// One row. Returns its response so the caller can wire clicks and a menu.
///
/// `indent` is how far in from the panel's edge the row's contents start — [`CHILD`] for a
/// bookmark inside a group, and nothing for everything else.
#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
    indent: f32,
    height: f32,
    glyph: azur_icons::Icon<'_>,
    glyph_color: egui::Color32,
    shell: Option<(egui::TextureId, Rect)>,
    queue: &mut IconQueue,
    label: &str,
    current: bool,
    id: Id,
    sense: Sense,
    dragging: bool,
    // A row less certain than the rest of the panel: a machine the network has merely announced.
    // One step down the text ladder, so it reads as "there, but not yet yours" — and it still
    // lights to full contrast under the pointer, because it is as clickable as any other row and
    // one that stayed dim under the hand would read as disabled.
    muted: bool,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let response = ui.interact(rect, id, sense);

    // **The pointer's own highlight and nothing else: the left panel does not mark the folder
    // that is open.** A band down a sidebar row was competing with the listing's selection for
    // the same colour and the same meaning, and only one of the two is something the user
    // selected. Where you are is still said — the label goes `text-primary`, and the breadcrumb
    // above the listing is the bar whose entire job that is.
    if let Some(fill) = row_fill(t, false, response.hovered()) {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    }
    // The row being dragged stays where it is and goes quiet, so the list does not reflow
    // under the pointer mid-gesture — the same treatment a tab gets.
    if dragging {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg.control_active);
    }

    let glyph_rect = icon_rect(rect, rect.left() + indent + INDENT + 12.0 + space::S2, GLYPH);
    draw_icon(ui, glyph_rect, glyph, glyph_color, shell, queue);

    let text_left_x = glyph_rect.right() + space::S3;
    // Lifted two points, the same as every cell in the listing — see `filelist::CELL_LIFT`.
    // A line of text centred in its row sits a shade low against the icon beside it, because the
    // font's ascent and descent are not symmetrical about the middle.
    let text_rect = Rect::from_min_max(
        pos2(text_left_x, rect.top() - crate::ui::filelist::CELL_LIFT),
        pos2(rect.right() - space::S3, rect.bottom() - crate::ui::filelist::CELL_LIFT),
    );
    let color = if current || response.hovered() {
        t.text.primary
    } else if muted {
        t.text.tertiary
    } else {
        t.text.secondary
    };

    let galley = truncated(
        ui.painter(),
        label,
        t.fonts.body.clone(),
        color,
        text_rect.width(),
    );
    text_left(ui.painter(), text_rect, galley);
    response
}

/// Windows' own icon for a row, once the shell has answered.
///
/// The sidebar shows the same icons Explorer's navigation pane does — the Downloads arrow,
/// the Pictures thumbnail, the Recycle Bin, a network volume's plug, a drive with a custom
/// `autorun.inf` icon. Every one of those is the shell's answer about *that place*, which
/// no amount of drawing here could reproduce.
///
/// `None` until it arrives, which is a frame or two after the first sighting and never
/// again for the rest of the session; the painted glyph stands in until then. See
/// [`crate::shell::icons::Icons::place`].
fn shell_icon(
    ui: &Ui,
    icons_cache: &mut crate::shell::icons::Icons,
    path: &Path,
) -> Option<(egui::TextureId, Rect)> {
    let icon = icons_cache.place(path)?;
    icons_cache.uv(ui.ctx(), icon)
}

/// A row's icon: the shell's bitmap if there is one, and the painted glyph if not.
fn draw_icon(
    ui: &Ui,
    rect: Rect,
    glyph: azur_icons::Icon<'_>,
    color: egui::Color32,
    shell: Option<(egui::TextureId, Rect)>,
    queue: &mut IconQueue,
) {
    match shell {
        // Queued rather than drawn — see [`IconQueue`]. The painted fallback below is drawn
        // where it stands, because it comes out of the same atlas as the text beside it.
        Some((texture, uv)) => queue.push((texture, rect, uv)),
        None => glyph(ui.painter(), rect, color),
    }
}

/// Draw everything the rows queued, in one run.
fn flush_icons(ui: &Ui, queue: &IconQueue) {
    let painter = ui.painter();
    for (texture, rect, uv) in queue {
        painter.image(*texture, *rect, *uv, egui::Color32::WHITE);
    }
}

/// What a drive's tooltip says: its name, and the numbers behind the bar.
///
/// Both numbers, since nothing has to fit beside anything here — and the name as well,
/// because in a narrow panel the row's own label is the part that got truncated.
fn drive_tooltip(drive: &Drive, out: &mut String) {
    out.clear();
    out.push_str(&drive.display_name());
    out.push_str(" — ");
    if drive.disconnected {
        out.push_str("disconnected");
        return;
    }
    match drive.used_fraction() {
        Some(_) => {
            fmt::size(drive.free, out);
            out.push_str(" free of ");
            fmt::size(drive.total, out);
        }
        // Nothing in the slot, or a volume it would have meant waking hardware to
        // measure. Saying so beats a tooltip that says nothing.
        None => out.push_str("not measured"),
    }
}

/// A drive: its name, how much room is left, and a bar showing it.
///
/// Painted here rather than through [`row`] because the elements have to clear each other
/// exactly — a capacity bar drawn a few points too high strikes through the caption above
/// it, which is the kind of thing a "roughly two lines" helper cannot promise.
///
/// The free-space **numbers are a tooltip**, not a caption. Inline they charged every drive
/// row for a second piece of text that had to fit beside the name — which in a panel this
/// narrow it often did not, so the caption came and went as the panel was dragged, and the
/// name it belonged to lost width to it whenever it stayed. The bar under the name already
/// carries the answer at a glance; the digits are what you go looking for, and going
/// looking is what a hover is.
#[allow(clippy::too_many_arguments)]
fn drive_row(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
    drive: &Drive,
    shell: Option<(egui::TextureId, Rect)>,
    queue: &mut IconQueue,
    current: &Path,
    focused: PaneId,
    scratch: &mut String,
    out: &mut Vec<Action>,
) {
    let name = drive.display_name();
    let text_width = (width - TEXT_INSET - space::S3).max(0.0);

    let (rect, _) = ui.allocate_exact_size(vec2(width, DRIVE_ROW), Sense::hover());
    // Keyed on the path rather than the letter: every network location has an empty letter, so a
    // letter-keyed id would give every row in the Network group the same one — and two widgets
    // sharing an id in egui share their hover, their tooltip and their click.
    let response = ui.interact(rect, Id::new(("drive", &drive.path)), Sense::click());
    let here = drive.path == *current;

    // As on every other row here: the pointer's highlight only, never a mark for the folder that
    // happens to be open. See `row`.
    if let Some(fill) = row_fill(t, false, response.hovered()) {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    }

    let glyph = icon_rect(rect, rect.left() + INDENT + 12.0 + space::S2, GLYPH);
    draw_icon(
        ui,
        glyph,
        icons::for_drive(drive.kind),
        t.text.secondary,
        shell,
        queue,
    );

    let left = glyph.right() + space::S3;
    let right = rect.right() - space::S3;

    // The name, then the bar, placed from the measured height of the name rather than at a
    // guessed offset — a capacity bar a few points too high strikes through the text.
    let name = truncated(
        ui.painter(),
        &name,
        t.fonts.body.clone(),
        if here || response.hovered() {
            t.text.primary
        } else if drive.disconnected {
            // A step down, as a found machine is in `row`: there, but not connected.
            t.text.tertiary
        } else {
            t.text.secondary
        },
        text_width,
    );
    let mut y = rect.top() + PAD_Y;
    let name_height = name.size().y;
    ui.painter()
        .galley(pos2(left, y.round()), name, egui::Color32::PLACEHOLDER);
    y += name_height + GAUGE_GAP;

    if let Some(used) = drive.used_fraction() {
        let bar = Rect::from_min_size(pos2(left, y.round()), vec2((right - left).max(0.0), GAUGE_HEIGHT));
        if bar.width() > 8.0 {
            let corner = CornerRadius::same(radius::CIRCULAR);
            ui.painter().rect_filled(bar, corner, t.gauge_track);
            ui.painter().rect_filled(
                Rect::from_min_size(bar.min, vec2((bar.width() * used).max(2.0), bar.height())),
                corner,
                t.gauge(used),
            );
        }
    }

    // The numbers, on hover. Guarded rather than left to the tooltip's own hover test,
    // because the formatting is the cost and there is no sense paying it per drive per
    // frame for text nobody is looking at.
    if response.hovered() {
        drive_tooltip(drive, scratch);
        azur_egui_theme::components::tooltip(response.clone(), scratch);
    }

    navigate_on(&response, focused, &drive.path, out);
    // **Windows' own menu for the drive**, the one Explorer's navigation pane shows — Eject,
    // Format, Disconnect, BitLocker, Properties and whatever is installed — rather than a short one
    // of ours. Built off the UI thread like every shell menu, so a drive that is slow to answer
    // costs a busy cursor and not a frozen window. See [`Action::DriveMenu`].
    if response.secondary_clicked() {
        // Physical pixels inside the window, as `filelist::context_menu` sends them.
        let at = response
            .interact_pointer_pos()
            .unwrap_or(response.rect.center());
        let scale = ui.ctx().pixels_per_point();
        out.push(Action::DriveMenu {
            pane: focused,
            drive: drive.path.clone(),
            at: ((at.x * scale) as i32, (at.y * scale) as i32),
        });
    }
}

/// Left click navigates, middle click opens a tab.
fn navigate_on(response: &egui::Response, focused: PaneId, path: &Path, out: &mut Vec<Action>) {
    if response.clicked() {
        out.push(Action::Navigate {
            pane: focused,
            path: path.to_path_buf(),
        });
    }
    if response.middle_clicked() {
        out.push(Action::NavigateNewTab {
            pane: focused,
            path: path.to_path_buf(),
        });
    }
}

/// The three ways to open a place, shared by every context menu here.
fn menu_targets(ui: &mut Ui, focused: PaneId, path: &Path, out: &mut Vec<Action>) {
    if ui.add(MenuItem::new("Open").icon(&icons::folder_open)).clicked() {
        out.push(Action::Navigate {
            pane: focused,
            path: path.to_path_buf(),
        });
    }
    if ui.add(MenuItem::new("Open in new tab")).clicked() {
        out.push(Action::NavigateNewTab {
            pane: focused,
            path: path.to_path_buf(),
        });
    }
    if ui
        .add(MenuItem::new("Open in a pane to the right").icon(&icons::split_side))
        .clicked()
    {
        out.push(Action::OpenInSplit {
            pane: focused,
            path: path.to_path_buf(),
            side: Side::Right,
        });
    }
    if ui
        .add(MenuItem::new("Open in a pane below").icon(&icons::split_down))
        .clicked()
    {
        out.push(Action::OpenInSplit {
            pane: focused,
            path: path.to_path_buf(),
            side: Side::Bottom,
        });
    }
}

/// A one-line note where a group has nothing in it. Returns the row it took, which an empty
/// group's hint needs — see [`bookmarks::section`], where it is also a place to drop one.
fn hint(ui: &mut Ui, t: &Theme, width: f32, indent: f32, text: &str) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
    let inner = Rect::from_min_max(
        pos2(rect.left() + indent + INDENT + 12.0 + space::S2, rect.top()),
        pos2(rect.right() - space::S3, rect.bottom()),
    );
    let galley = truncated(
        ui.painter(),
        text,
        t.fonts.caption.clone(),
        t.text.disabled,
        inner.width(),
    );
    text_left(ui.painter(), inner, galley);
    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::drives::DriveKind;
    use std::path::PathBuf;

    fn drive(total: u64, free: u64) -> Drive {
        Drive {
            path: PathBuf::from("D:\\"),
            letter: "D:".to_owned(),
            label: "Data".to_owned(),
            kind: DriveKind::Fixed,
            total,
            free,
            described: true,
            disconnected: false,
        }
    }

    #[test]
    fn the_tooltip_names_the_drive_and_both_numbers() {
        let mut out = String::new();
        drive_tooltip(&drive(1_000_000_000_000, 713_000_000_000), &mut out);
        // The name too: in a narrow panel it is the row's label that got truncated, and
        // this is the only other place it is written.
        assert!(out.starts_with("Data (D:) — "), "{out}");
        assert!(out.contains(" free of "), "{out}");
        // Both numbers, since nothing has to fit beside anything in a tooltip.
        assert_eq!(
            out.matches("GB").count() + out.matches("TB").count(),
            2,
            "{out}"
        );
    }

    #[test]
    fn an_unmeasured_drive_still_says_something() {
        // An empty card reader, or a share that has not answered yet. A tooltip that came
        // up blank would read as a bug in the tooltip.
        let mut out = String::new();
        drive_tooltip(&drive(0, 0), &mut out);
        assert_eq!(out, "Data (D:) — not measured");
    }

    /// A Network row is the same row, and its tooltip names it the same way.
    ///
    /// Both halves matter here. The name, because a share whose row is truncated to
    /// `web on file…` in a narrow panel has nowhere else to say where it is; and "not
    /// measured", because a disconnected share is exactly the case that never gets a gauge —
    /// which without this reads as a row that has stopped working rather than a share that is
    /// down.
    #[test]
    fn a_network_row_is_named_after_its_machine() {
        let share = Drive {
            path: PathBuf::from("\\\\fileserver\\web"),
            letter: String::new(),
            label: "web".to_owned(),
            kind: DriveKind::Network,
            total: 0,
            free: 0,
            described: true,
            disconnected: false,
        };
        let mut out = String::new();
        drive_tooltip(&share, &mut out);
        assert_eq!(out, "web on fileserver — not measured");
    }

    /// Dragging a heading moves its group to the side of the one it lands on, and only that.
    #[test]
    fn a_dragged_group_lands_beside_the_one_it_is_dropped_on() {
        use Section::*;
        let mut sections = Sections::default();
        // Network to the top, before Drives.
        assert!(sections.move_beside(Network, Drives, false));
        assert_eq!(sections.order, [Network, Drives, Bookmarks, Places, Wsl]);
        // Drives after Places.
        assert!(sections.move_beside(Drives, Places, true));
        assert_eq!(sections.order, [Network, Bookmarks, Places, Drives, Wsl]);
        // Dropped where it already is, or on itself: nothing moved, so nothing to save.
        assert!(!sections.move_beside(Drives, Places, true));
        assert!(!sections.move_beside(Wsl, Wsl, false));
        assert!(sections.reset_order());
        assert_eq!(sections.order, Section::ALL);
        assert!(!sections.reset_order());
    }

    /// Where a heading being dragged would land, read off the groups' spans.
    #[test]
    fn a_drag_lands_by_which_half_of_a_group_the_pointer_is_in() {
        let span = |top: f32, bottom: f32| Rect::from_min_max(pos2(0.0, top), pos2(200.0, bottom));
        let (drives, places) = (span(0.0, 100.0), span(100.0, 140.0));
        let spans = [(Section::Drives, drives), (Section::Places, places)];
        assert_eq!(section_landing(&spans, 10.0), Some((Section::Drives, drives, false)));
        assert_eq!(section_landing(&spans, 60.0), Some((Section::Drives, drives, true)));
        assert_eq!(section_landing(&spans, 110.0), Some((Section::Places, places, false)));
        // Below the last group is after it.
        assert_eq!(section_landing(&spans, 500.0), Some((Section::Places, places, true)));
        assert_eq!(section_landing(&[], 10.0), None);
    }

    /// A mapping Windows has not reconnected says so, rather than "not measured" — which would
    /// read as a measurement still on its way, and none ever is.
    #[test]
    fn a_disconnected_mapping_says_so() {
        let mapped = Drive {
            path: PathBuf::from("H:\\"),
            letter: "H:".to_owned(),
            label: "Common".to_owned(),
            kind: DriveKind::Network,
            total: 0,
            free: 0,
            described: false,
            disconnected: true,
        };
        let mut out = String::new();
        drive_tooltip(&mapped, &mut out);
        assert_eq!(out, "Common (H:) — disconnected");
    }
}
