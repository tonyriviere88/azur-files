//! The details view: a sticky header over virtualised rows.
//!
//! This is the hot loop, and it is built so that the cost of a frame depends on
//! the size of the *window*, never on the size of the folder:
//!
//! - **Only visible rows exist.** The scroll area is told the total height and asked
//!   for the visible range; a folder of 300,000 files draws the same forty rows as
//!   a folder of forty.
//! - **One widget for the whole list.** Not one per row: a single `interact` over
//!   the viewport, with the row derived from the pointer's `y`. Forty widget ids
//!   per frame per pane would otherwise be registered, hit-tested and animated for
//!   nothing.
//! - **No allocation per cell.** Sizes, dates and type names are written into one
//!   `String` that is cleared and reused, and egui's galley cache is keyed on the
//!   finished text — so a row that has not changed is a cache lookup, not a
//!   shaping pass.
//! - **Columns measured once.** Modified is a fixed-width format, Size is measured
//!   from the single longest value, and Type from the handful of distinct labels in
//!   the folder. None of it is re-measured while scrolling.
//!
//! The Name column takes what is left, and the other three are sized to their
//! content — and draggable, because a measurement is a starting point and not a
//! decision.

use azur_egui_theme::components::{galley_on_baseline, ink_baseline};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::{radius, space};
use egui::{pos2, vec2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};

use crate::app::Action;
use crate::fs::time::LocalZone;
use crate::fs::{fmt, Column};
use crate::icons;
use crate::pane::{PaneId, Tab, ROW_HEIGHT};
// `crate::icons` is this program's painted glyphs; this is the shell's icon service.
use crate::shell::icons as shell_icons;
use crate::shell::icons::Icons;
use crate::shell::links;
use crate::theme::Theme;
use crate::ui::{
    icon_rect, row_fill, row_fill_quiet, selection_bar, text_center, text_left, text_right,
    truncated,
};

mod band;
mod chain;
mod columns;
mod rename;
mod rows;
mod status;
#[cfg(test)]
mod tests;

// One file per part of a listing. [`show`] is the whole of the public surface, and these were all
// one module until it grew past three thousand lines, and the glob is what says
// so: the split is an arrangement of files, not a narrowing of what any part of a listing may
// reach. `crate::ui::grid` draws the same rows a different way and shares most of it.
pub(crate) use band::*;
pub(crate) use chain::*;
pub(crate) use columns::*;
pub(crate) use rename::*;
pub(crate) use rows::*;
pub(crate) use status::*;

/// What the listing wants the application to do that it cannot do itself.
pub struct Outcome {
    /// Prefetch this folder — the cursor has landed on it.
    pub prefetch: Option<std::path::PathBuf>,
    /// Where each visible folder row was drawn, so files can be dropped *into* it.
    ///
    /// Reported rather than resolved here: a drop target has to be answered synchronously from
    /// an OLE callback on another stack, so the application publishes rects rather than the
    /// listing being asked at the moment of the drop. Read one frame later than it is written,
    /// which is a frame the rows have not moved in.
    pub drop_rows: Vec<(Rect, std::path::PathBuf)>,
    /// The rows' own rectangle — where a drop into *this* folder lands, and what lights up
    /// for one.
    ///
    /// Not the pane: the column header sorts and the status line counts, and neither takes a
    /// drop, so promising them as the destination was promising the wrong thing. `NOTHING` for
    /// a pane too short to list a single row, which cannot show a highlight either.
    pub drop_area: Rect,
}

/// Draw the header and the rows inside `rect`, and the status line in `floor`.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    t: &Theme,
    zone: &LocalZone,
    rect: Rect,
    // The status line's own band, across the whole width of the pane and under everything in it —
    // including the preview panel, whichever side that is docked to.
    //
    // Handed in rather than taken off the bottom of `rect`, which is what this used to do. `rect` is
    // the listing's region, and when the preview panel is docked to the *right* that region is a
    // column: a status line inside it stopped where the panel began and the pane had two different
    // things along its bottom edge. It is the pane's floor rather than the listing's, so the pane is
    // what decides where it goes — see `App::pane`.
    floor: Rect,
    pane: PaneId,
    tab: &mut Tab,
    focused: bool,
    icons_cache: &mut crate::shell::icons::Icons,
    links_cache: &mut crate::shell::links::Links,
    // The shell's thumbnails, for the tiles. Handed in whichever view is showing, because the
    // switch is on the status line and the next frame may be the other one.
    thumbs: &mut crate::shell::thumbs::Thumbs,
    cut: &[std::path::PathBuf],
    status: Option<&str>,
    // Whether this pane's console is open, for the switch at the left of the status line.
    //
    // Told rather than inferred from `reserved`: a pane too short to give the console a band leaves
    // that at zero, and a switch that unlatched itself when the pane was squeezed would be reporting
    // the pane's height as the console's state.
    console_open: bool,
    // The window's rule for opening a folder as tiles, for the view switch's context menu. Nothing
    // in a listing reads it — it is consulted once, when a folder lands, by
    // [`crate::pane::Tab::choose_view`] — so it is here only to be ticked. See [`tiles_menu`].
    auto_tiles: crate::pane::AutoTiles,
    // And which file types this machine can draw a picture of, which is the other half of what that
    // rule counts — the same menu reports what the two of them make of this folder.
    providers: &mut crate::shell::providers::Providers,
    // The band at the bottom of `rect` that belongs to somebody else — the console panel.
    //
    // The rows stop short of it. Without this they are laid out across the console and the console
    // merely paints over them — which reads as a floating overlay rather than as a panel in the stack,
    // and hit-tests as one too.
    reserved: f32,
    scratch: &mut String,
    out: &mut Vec<Action>,
) -> Outcome {
    // The frame's clock, for the one thing here that depends on how long something has taken:
    // whether a scan has been slow enough to admit to. See [`crate::pane::SLOW_SCAN`].
    let now = ui.input(|i| i.time);
    let mut outcome = Outcome {
        prefetch: None,
        drop_rows: Vec::new(),
        drop_area: Rect::NOTHING,
    };

    // **Tiles have no column header.** Four draggable dividers over a grid would be four dividers
    // about nothing: there are no columns under them to size. Which does cost something and it is
    // worth naming — the header is where sorting is done, so in this view the sort is whatever the
    // details view was last set to. It is per tab and it survives the switch, so setting it once is
    // enough; a *sort* control for the grid would be new furniture and is not what was asked for.
    let tiles = tab.view_mode.is_icons();
    let header = Rect::from_min_size(
        rect.min,
        vec2(rect.width(), if tiles { 0.0 } else { HEADER_HEIGHT }),
    );
    // A pane squeezed shorter than its own furniture would give an inverted body
    // rect, which turns into an empty visible range and an underflow downstream. The
    // header and the status line are worth more than a row nobody could read.
    if rect.bottom() - reserved <= header.bottom() + ROW_HEIGHT {
        if !tiles {
            header_strip(ui, t, header, pane, tab, &resolved_widths(tab, rect.width()), out);
        }
        status_line(
            ui, t, floor, pane, tab, console_open, auto_tiles, providers, status, now, scratch, out,
        );
        return outcome;
    }
    let body = Rect::from_min_max(
        header.left_bottom(),
        pos2(rect.right(), rect.bottom() - reserved),
    );
    outcome.drop_area = body;

    // Columns have to be resolved before the header can be drawn, and measuring
    // needs a painter — so this happens first, once per listing. Skipped entirely for tiles: it is a
    // pass over the folder for three widths nothing is going to use.
    let widths = if tiles {
        [0.0; 4]
    } else {
        if !tab.widths_measured {
            measure_columns(ui, t, tab, scratch);
        }
        let widths = resolved_widths(tab, body.width());
        header_strip(ui, t, header, pane, tab, &widths, out);
        widths
    };

    // Whether any rows were drawn, and so whether anything is listening for a click over
    // the body. See [`bare_body`].
    let mut listed = false;

    if tab.dir.as_ref().is_some_and(|d| d.error.is_some()) {
        let message = tab
            .dir
            .as_ref()
            .and_then(|d| d.error.clone())
            .unwrap_or_default();
        text_center(
            ui.painter(),
            body,
            t.fonts.body.clone(),
            t.status.danger,
            &message,
        );
    } else if tab.dir.is_none() {
        // A scan that has not landed yet, and **only once it has been long enough to notice**.
        // A local folder comes back in single-digit milliseconds, so this used to be a word that
        // flashed up and vanished on every navigation, in the place the listing was about to be.
        // See [`crate::pane::SLOW_SCAN`]; the frame that gets here at the right moment is booked
        // by `App::start_scans`.
        if tab.waiting_visibly(now) {
            text_center(
                ui.painter(),
                body,
                t.fonts.body.clone(),
                t.text.tertiary,
                "Reading…",
            );
        }
    } else if tab.order.is_empty() {
        // "Empty" and "everything is filtered out" are different facts, and telling
        // them apart is the difference between a dead end and a hint.
        //
        // Four of them now, and the lens is the one that had to be added: with it on and the box
        // empty, this said *everything here is hidden* and sent the reader to `Ctrl+H` over a folder
        // with plenty in it. The text comes first because it is the thing most recently typed and the
        // first thing anybody would clear — and each lens words its own answer, which is
        // [`crate::pane::Lens::nothing_found`].
        let message = if tab.dir.as_ref().is_some_and(|d| d.is_empty()) {
            "This folder is empty"
        } else if !tab.filter.is_empty() {
            "Nothing matches the filter"
        } else if let Some(lens) = tab.lens {
            lens.nothing_found()
        } else {
            "Everything here is hidden — Ctrl+H shows it"
        };
        text_center(
            ui.painter(),
            body,
            t.fonts.body.clone(),
            t.text.tertiary,
            message,
        );
    } else if tiles {
        crate::ui::grid::show(
            ui, t, zone, body, pane, tab, focused, icons_cache, thumbs, cut, out, &mut outcome,
        );
        listed = true;
    } else {
        rows(
            ui,
            t,
            zone,
            body,
            pane,
            tab,
            focused,
            &widths,
            icons_cache,
            links_cache,
            cut,
            scratch,
            out,
            &mut outcome,
        );
        listed = true;
    }
    if !listed {
        bare_body(ui, body, pane, tab, out);
    }

    status_line(
        ui, t, floor, pane, tab, console_open, auto_tiles, providers, status, now, scratch, out,
    );
    outcome
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// Ask for the shell context menu on a right click.
///
/// The menu itself is Explorer's — see [`crate::shell::menu`] — so this only has to
/// A body with no rows in it still answers a right click.
///
/// [`rows`] is what wires the listing's clicks up, and a listing with nothing in it never gets
/// there: an empty folder, a folder filtered down to nothing, one still being read, and one
/// that refused to be read all draw a line of text over a body that nothing was listening to.
/// So right-clicking an empty folder did nothing at all — and an empty folder is precisely
/// where `New folder` and `Paste` are most wanted.
///
/// The folder's own menu, the same one the space below a short listing gives.
fn bare_body(ui: &Ui, body: Rect, pane: PaneId, tab: &Tab, out: &mut Vec<Action>) {
    if body.height() <= 1.0 || body.width() <= 1.0 {
        return;
    }
    // Clicks only. A drag over an empty body is a rubber band around nothing, and leaving
    // dragging alone keeps the drop zone underneath able to take files dropped in.
    let response = ui.interact(body, Id::new(("bare-body", pane)), Sense::click());
    if response.clicked() || response.secondary_clicked() {
        out.push(Action::Focus(pane));
    }
    context_menu(ui, &response, pane, tab, None, out);
}

/// decide *what* it is for and *where* it goes. Showing it is deferred to the
/// application through an action, because `TrackPopupMenuEx` is modal: it must not run
/// with the listing borrowed and half-drawn.
pub(crate) fn context_menu(
    ui: &Ui,
    response: &egui::Response,
    pane: PaneId,
    tab: &Tab,
    row: Option<usize>,
    out: &mut Vec<Action>,
) {
    if !response.secondary_clicked() {
        return;
    }
    // Where the pointer was when the button went down, in screen pixels — a Win32 menu
    // is positioned in physical coordinates, and egui works in points.
    let at = response
        .interact_pointer_pos()
        .or_else(|| ui.ctx().pointer_interact_pos())
        .unwrap_or(response.rect.center());
    let scale = ui.ctx().pixels_per_point();

    // `row` is what the menu is for, and by the time this is called the selection already agrees
    // with it — the caller selected the row the click landed on, or cleared the selection and
    // passed `None`. So the selection is the answer, with the row itself as the fallback for the
    // case that cannot happen: a row that is not selected and not selectable, because the listing
    // went away between the click and here.
    let items = match row {
        Some(_) if tab.selected_count > 0 => tab.selection_paths(),
        Some(row) => tab.target_at(row).into_iter().collect(),
        // The background: the folder's own menu, which is the only one New is on.
        None => Vec::new(),
    };

    out.push(Action::ShellMenu {
        pane,
        items,
        at: ((at.x * scale) as i32, (at.y * scale) as i32),
    });
}

// ---------------------------------------------------------------------------
// Status line
// ---------------------------------------------------------------------------
