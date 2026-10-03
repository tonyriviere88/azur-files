//! The left panel: drives, bookmarks, places.
//!
//! Three collapsible groups of rows, in the order they are worth scanning: the
//! volumes on the machine, the folders you chose, and the ones the shell provides.
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

use azur_egui_theme::components::{ContextMenu, MenuItem};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::{radius, space, typography};
use egui::{pos2, vec2, CornerRadius, Id, Rect, Sense, Ui};
use std::path::{Path, PathBuf};

use crate::app::Action;
use crate::fs::drives::Drive;
use crate::fs::places::Place;
use crate::fs::{display_name, fmt};
use crate::icons;
use crate::pane::{PaneId, Side};
use crate::theme::Theme;
use crate::ui::{icon_rect, row_fill, section_label, text_left, truncated};

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

/// Which groups are open. Persisted, because collapsing one is a decision about
/// how you work rather than about this session.
#[derive(Clone, Copy, Debug)]
pub struct Sections {
    pub drives: bool,
    pub bookmarks: bool,
    pub places: bool,
}

impl Default for Sections {
    fn default() -> Self {
        Self {
            drives: true,
            bookmarks: true,
            places: true,
        }
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
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    t: &Theme,
    drives: &[Drive],
    bookmarks: &[PathBuf],
    places: &[Place],
    current: &Path,
    focused: PaneId,
    sections: &mut Sections,
    icons_cache: &mut crate::shell::icons::Icons,
    reorder: &mut Option<usize>,
    scratch: &mut String,
    out: &mut Vec<Action>,
) -> Option<Rect> {
    let mut bookmarks_rect = None;
    let mut queue: IconQueue = Vec::new();
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

            // ---- Drives ---------------------------------------------------
            if group_header(ui, t, width, "Drives", &mut sections.drives) {
                for drive in drives {
                    let shell = shell_icon(ui, icons_cache, &drive.path);
                    drive_row(
                        ui, t, width, drive, shell, &mut queue, current, focused, scratch,
                        out,
                    );
                }
            }

            // ---- Bookmarks ------------------------------------------------
            ui.add_space(space::S2);
            let header = ui.cursor().min;
            if group_header(ui, t, width, "Bookmarks", &mut sections.bookmarks) {
                if bookmarks.is_empty() {
                    hint(
                        ui,
                        t,
                        width,
                        "Ctrl+D, or drag a folder here",
                    );
                }
                // Row rects as they are drawn, so the caret between them can be worked out
                // afterwards without laying anything out twice.
                let mut rows: Vec<Rect> = Vec::with_capacity(bookmarks.len());
                for (index, path) in bookmarks.iter().enumerate() {
                    let shell = shell_icon(ui, icons_cache, path);
                    let dragging = *reorder == Some(index);
                    let response = row(
                        ui,
                        t,
                        width,
                        ROW,
                        &icons::star_filled,
                        t.status.warning,
                        shell,
                        &mut queue,
                        &display_name(path),
                        path == current,
                        Id::new(("bookmark", path)),
                        // Only bookmarks can be dragged, and only to reorder themselves.
                        Sense::click_and_drag(),
                        dragging,
                    );
                    rows.push(response.rect);
                    if response.drag_started() {
                        *reorder = Some(index);
                    }
                    navigate_on(&response, focused, path, out);
                    ContextMenu::new(&response).show(ui.ctx(), |ui| {
                        menu_targets(ui, focused, path, out);
                        azur_egui_theme::components::menu_divider(ui);
                        if ui
                            .add(MenuItem::new("Remove bookmark").danger(true))
                            .clicked()
                        {
                            out.push(Action::RemoveBookmark(path.clone()));
                        }
                    });
                }
                reordering(ui, t, reorder, &rows, out);
                bookmarks_rect = Some(Rect::from_min_max(
                    pos2(header.x, header.y),
                    pos2(header.x + width, ui.cursor().min.y),
                ));
            }

            // ---- Places ---------------------------------------------------
            ui.add_space(space::S2);
            if group_header(ui, t, width, "Places", &mut sections.places) {
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
                        ROW,
                        glyph,
                        color,
                        shell,
                        &mut queue,
                        &place.label,
                        !place.shell_only && place.path == current,
                        Id::new(("place", &place.label)),
                        Sense::click(),
                        false,
                    );
                    if place.shell_only {
                        // Not a directory this program can list — hand it over.
                        if response.clicked() {
                            out.push(Action::Reveal(place.path.clone()));
                        }
                    } else {
                        navigate_on(&response, focused, &place.path, out);
                        ContextMenu::new(&response).show(ui.ctx(), |ui| {
                            menu_targets(ui, focused, &place.path, out);
                            azur_egui_theme::components::menu_divider(ui);
                            if ui
                                .add(MenuItem::new("Add to bookmarks").icon(&icons::star))
                                .clicked()
                            {
                                out.push(Action::AddBookmark(place.path.clone()));
                            }
                        });
                    }
                }
            }

            ui.add_space(space::S3);
            // Every row's shell icon, in one run — see [`IconQueue`]. Inside the scroll area,
            // so they are clipped with the rows they belong to.
            flush_icons(ui, &queue);
        });
    bookmarks_rect
}

/// A bookmark being dragged to a new position: the caret while it is held, and the move
/// when it is let go.
///
/// The list is short and always fully drawn, so the insertion point is simply the first row
/// whose middle the pointer is above — no hit-testing and no scrolling to account for.
fn reordering(
    ui: &mut Ui,
    t: &Theme,
    reorder: &mut Option<usize>,
    rows: &[Rect],
    out: &mut Vec<Action>,
) {
    let Some(from) = *reorder else { return };
    if from >= rows.len() {
        *reorder = None;
        return;
    }

    let Some(pointer) = ui.ctx().pointer_interact_pos() else {
        *reorder = None;
        return;
    };
    let mut to = rows.len();
    for (index, rect) in rows.iter().enumerate() {
        if pointer.y < rect.center().y {
            to = index;
            break;
        }
    }

    if ui.input(|i| i.pointer.any_down()) {
        // The caret goes between rows: at the top of the row it would push down, or under
        // the last one when it is going to the end.
        let y = rows
            .get(to)
            .map(|rect| rect.top())
            .unwrap_or_else(|| rows[rows.len() - 1].bottom());
        let band = rows[0];
        ui.painter().rect_filled(
            Rect::from_min_max(
                pos2(band.left() + INDENT, y - 1.0),
                pos2(band.right() - space::S3, y + 1.0),
            ),
            CornerRadius::same(radius::CIRCULAR),
            t.accent.default,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        return;
    }

    // Dropping a row on itself, or immediately below itself, means nothing.
    if to != from && to != from + 1 {
        out.push(Action::MoveBookmark { from, to });
    }
    *reorder = None;
}

/// A group heading that folds its rows away. Returns whether they should be drawn.
fn group_header(ui: &mut Ui, t: &Theme, width: f32, label: &str, open: &mut bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(width, HEADER), Sense::hover());
    let response = ui.interact(rect, Id::new(("sidebar-group", label)), Sense::click());
    if response.clicked() {
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
    *open
}

/// One row. Returns its response so the caller can wire clicks and a menu.
#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut Ui,
    t: &Theme,
    width: f32,
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

    let glyph_rect = icon_rect(rect, rect.left() + INDENT + 12.0 + space::S2, GLYPH);
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

/// The name a drive row shows: its label and its letter.
fn drive_name(drive: &Drive) -> String {
    format!("{} ({})", drive.label, drive.letter)
}

/// What a drive's tooltip says: its name, and the numbers behind the bar.
///
/// Both numbers, since nothing has to fit beside anything here — and the name as well,
/// because in a narrow panel the row's own label is the part that got truncated.
fn drive_tooltip(drive: &Drive, out: &mut String) {
    out.clear();
    out.push_str(&drive_name(drive));
    out.push_str(" — ");
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
    let name = drive_name(drive);
    let text_width = (width - TEXT_INSET - space::S3).max(0.0);

    let (rect, _) = ui.allocate_exact_size(vec2(width, DRIVE_ROW), Sense::hover());
    let response = ui.interact(rect, Id::new(("drive", &drive.letter)), Sense::click());
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
    ContextMenu::new(&response).show(ui.ctx(), |ui| {
        menu_targets(ui, focused, &drive.path, out);
        azur_egui_theme::components::menu_divider(ui);
        if ui.add(MenuItem::new("Open terminal here")).clicked() {
            out.push(Action::OpenTerminal(drive.path.clone()));
        }
    });
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

/// A one-line note where a group has nothing in it.
fn hint(ui: &mut Ui, t: &Theme, width: f32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
    let inner = Rect::from_min_max(
        pos2(rect.left() + INDENT + 12.0 + space::S2, rect.top()),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::drives::DriveKind;

    fn drive(total: u64, free: u64) -> Drive {
        Drive {
            path: PathBuf::from("D:\\"),
            letter: "D:".to_owned(),
            label: "Data".to_owned(),
            kind: DriveKind::Fixed,
            total,
            free,
            described: true,
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
}
