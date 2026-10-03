//! The dependency tree: what a binary needs, and where each one came from.
//!
//! One of [`crate::ui::preview`]'s three views — the one a `.exe`, a `.dll`, a driver or a
//! control panel applet gets — so everything about *where* it is drawn, what its header says and
//! how it is opened belongs to the panel. This is the tree and nothing else.
//!
//! # What it is showing
//!
//! A tree of [`crate::pe`]'s graph, one row per module, with the folder each one was found in
//! beside it. The graph is a graph and not a tree — `kernel32.dll` is under nearly everything —
//! so the display is a walk of it that **stops where a module repeats along the branch it is
//! already on**. That is what keeps a cycle (and real graphs have them) from being an endless
//! row of the same two names, without hiding the fact that both things really do import it.
//!
//! # What it opens with
//!
//! The root expanded, and then **exactly the branches that lead to something that would stop the
//! program from starting** — see [`crate::pe::Graph::breaks_loading`]. Everything else stays
//! folded. A modern graph is a thousand modules; expanding it all would be a wall of `api-ms-`
//! rows with the one line that matters somewhere in the middle of it, which is the mistake the
//! original Dependency Walker is remembered for.
//!
//! # Everything in a row is on one baseline
//!
//! The design system's rule, and this tree is the case it is written for: **three texts at two
//! sizes beside two painted glyphs**, on a 24-point row. Two things have to be true at once, and
//! neither is what a rect-centred galley gives you.
//!
//! - The texts have to be level with the **glyphs**, which `azur::icons` centres on their own
//!   ink. A line box reserves room under the baseline for descenders that a DLL's name does not
//!   have, so text centred in the row hangs about a point and a half below a glyph centred in it.
//! - The 12-point location has to be level with the 14-point **name**. Centring both boxes in the
//!   same rect does not do that either, and the error is in the other direction: the two fonts
//!   have different line heights *and* different ascents, so their baselines end up 1.5 points
//!   apart and the eye sees the column step down as it crosses the row.
//!
//! So the row commits to one baseline — [`ink_baseline`], taken from the row's *principal* font,
//! which is the body one the names are in — and every text in it is drawn on that baseline by
//! [`galley_on_baseline`]. Text in the principal font lands exactly where centring would have put
//! it once the glyph correction is applied; everything else comes into line with it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use azur_egui_theme::components::{galley_on_baseline, ink_baseline};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::space;
use egui::{pos2, vec2, Color32, Rect, Sense, Ui};

use crate::icons;
use crate::pe::{self, Graph, State};
use crate::theme::Theme;
use crate::ui::{icon_rect, truncated};

/// One module. The listing's row height, so a pane and the panel inside it read as one window.
pub const ROW: f32 = crate::pane::ROW_HEIGHT;

/// One level of the tree.
///
/// Narrow on purpose. A dependency graph is a dozen levels deep in places, and 20 points a
/// level would put the tenth level's names off the right-hand side of the panel.
const INDENT: f32 = 12.0;

/// The expander, and the glyph after it.
const TWISTY: f32 = 10.0;
const GLYPH: f32 = 14.0;

/// The air at each end of a row, and either side of the columns.
const PAD: f32 = space::S2;

/// Where the Location column starts, as a share of the tree's width.
///
/// A fixed fraction rather than the widest name, because the point of the column is that the
/// locations line up: a walk whose modules came off four different directories is meant to be
/// readable as four groups at a glance, and a column that moves with the longest name in view
/// would reflow every time the tree is expanded.
const LOCATION_AT: f32 = 0.42;

/// Room at the right-hand end for the processor tag.
const ARCH: f32 = 52.0;

/// The most rows the tree will build.
///
/// Reached only by expanding, and expanding is a click per level — but the auto-expand in
/// [`View::new`] opens branches on its own, and a graph of a thousand modules with a missing one
/// under `shell32` would open a subtree of hundreds. Twenty thousand rows is more than anyone will
/// scroll and small enough to build in a frame.
const ROW_CAP: usize = 20_000;

/// And how deep the tree goes. Real graphs are under twenty; this is a guard.
const DEPTH_CAP: usize = 32;

/// One row of the tree: which module, and where it sits in the walk.
///
/// Rebuilt from the graph and the expansion set whenever either changes, and not otherwise —
/// so scrolling a thousand-module graph is arithmetic over this vector rather than a fresh
/// traversal per frame.
struct Row {
    module: usize,
    depth: u16,
    /// Whether the reference that got here is delay-loaded.
    delayed: bool,
    /// It has imports, and this row is somewhere they can be shown.
    expandable: bool,
    open: bool,
    /// This module is already an ancestor of this row: the branch stops here rather than
    /// going round again.
    cyclic: bool,
}

/// A walked graph, and how much of it is unfolded.
pub struct View {
    graph: Arc<Graph>,
    /// Which modules are expanded, by index into the graph.
    ///
    /// By module and not by position in the tree, so expanding `shell32.dll` expands it
    /// wherever else it appears. That is the behaviour a graph wants: the question is what a
    /// module needs, and the answer does not depend on which of its importers you asked from.
    expanded: HashSet<usize>,
    rows: Vec<Row>,
    stale: bool,
}

impl View {
    /// Take a finished walk and open the branches worth looking at.
    pub fn new(graph: Arc<Graph>) -> Self {
        let mut expanded = HashSet::from([0]);
        for chain in graph.breaks_loading() {
            // Every module on the way down, but not the missing one itself — it has nothing
            // under it to show.
            for &at in chain.iter().rev().skip(1) {
                expanded.insert(at);
            }
        }
        let mut view = Self {
            graph,
            expanded,
            rows: Vec::new(),
            stale: true,
        };
        view.rebuild();
        view
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// How many rows are on show. For the tests, which is where a fold driven by a real click
    /// can be seen from.
    #[cfg(test)]
    pub fn shown(&self) -> usize {
        self.rows.len()
    }

    /// The first row a click would actually unfold, as a position in what is on show.
    ///
    /// For the test that drives a real click at a row's coordinates, which needs a row where
    /// unfolding is a thing that can happen — expandable, not already open, and not a branch that
    /// stops because the module is its own ancestor.
    ///
    /// It exists because that test used to assume the row under the root was one, and that is not a
    /// fact about this program at all: the panel is pointed at the **test binary itself**, so the
    /// order of the rows is the order the linker wrote that binary's import table in, and any change
    /// to this crate can rearrange it. It did — `api-ms-win-core-synch-l1-2-0.dll` arrived at the
    /// top, and an API set has nothing under it to show, so the click landed on a row that could not
    /// unfold and the test failed for a reason that had nothing to do with whether the click reached
    /// it. Which is the only thing it was ever trying to prove.
    ///
    /// **`rows` is how many of them the panel can actually show**, and it is the second half of the
    /// same lesson. A row is only clickable if it is on screen, and the panel along the *bottom* of a
    /// pane holds about nine — so a foldable row at index twenty is a click into the canvas below the
    /// last row, which does nothing. That is what happened when this crate gained `mfplat.dll` and
    /// `d3d11.dll` for the video player: two more imports at the top of the table pushed the first
    /// foldable one out of sight, and a test about whether a click reaches a row failed because the
    /// row it had chosen was not there to be reached.
    /// **Its name comes back with it**, and that is the third half of the same lesson. Knowing
    /// *which* row to click is no use without knowing where it was drawn, and a caller that works
    /// that out from [`ROW`] and the panel's own constants is off by however much furniture sits
    /// between the panel's edge and the first row — which is exactly the mistake that made a click
    /// intended for row one land on row two. The name is what a test can find among the drawn text,
    /// so the click goes where the row actually is.
    #[cfg(test)]
    pub fn first_foldable(&self, rows: usize) -> Option<(usize, &str)> {
        let at = self
            .rows
            .iter()
            .take(rows)
            .position(|row| row.expandable && !row.open && !row.cyclic)?;
        let module = self.rows.get(at).map(|row| row.module)?;
        Some((at, self.graph.modules.get(module)?.name.as_str()))
    }

    /// Fold a module open or shut.
    fn toggle(&mut self, module: usize) {
        if !self.expanded.remove(&module) {
            self.expanded.insert(module);
        }
        self.stale = true;
    }

    /// Flatten the graph into the rows on show.
    fn rebuild(&mut self) {
        self.stale = false;
        self.rows.clear();
        let graph = self.graph.clone();
        // An explicit stack rather than recursion: the depth is bounded by the branch rule
        // below, but that bound is the module count, and a thousand frames of a UI thread's
        // stack is not a bound worth relying on.
        //
        // Each item is a row to emit and the branch it sits on, and `branch` is what makes the
        // walk finite: a module that is already above this row is drawn and not descended into.
        let mut stack: Vec<(usize, u16, bool, Vec<usize>)> = vec![(0, 0, false, Vec::new())];
        while let Some((module, depth, delayed, branch)) = stack.pop() {
            if self.rows.len() >= ROW_CAP {
                break;
            }
            let cyclic = branch.contains(&module);
            let expandable = !graph.modules[module].imports.is_empty()
                && !cyclic
                && (depth as usize) < DEPTH_CAP;
            let open = expandable && self.expanded.contains(&module);
            self.rows.push(Row {
                module,
                depth,
                delayed,
                expandable,
                open,
                cyclic,
            });
            if !open {
                continue;
            }
            let mut below = branch;
            below.push(module);
            // Pushed in reverse, since the stack hands them back the other way round and the
            // import table's order is the order worth showing.
            for edge in graph.modules[module].imports.iter().rev() {
                stack.push((edge.to, depth + 1, edge.delayed, below.clone()));
            }
        }
    }
}

/// The whole answer, for the panel's title tooltip: what the walk found, how long it took, and
/// where every name was looked for.
///
/// All three are here because none of them fits on a bar a few hundred points wide, and because
/// they belong together: a location means nothing without the list of places that were tried —
/// the interesting answer is usually "it came off the third one" — and the list means nothing
/// without what it does *not* model, which is the last paragraph.
///
/// The timing is not decoration either. A program that claims to walk a thousand modules quickly
/// should be willing to be checked, and a walk that suddenly takes two seconds is how you find out
/// a `PATH` entry has gone.
pub fn about(graph: &Graph) -> String {
    /// How many of the search directories are listed. A real `PATH` has thirty entries and the
    /// ones that matter are at the front: the binary's own folder, then `System32`, then whatever
    /// was installed most recently.
    const SHOWN: usize = 10;
    use std::fmt::Write as _;

    let (files, api_sets, missing) = graph.tally();
    let mut text = format!(
        "{files} file{}, {api_sets} API set{}, {missing} missing \u{2014} {:.0} ms{}\n\nLooked for in order:",
        if files == 1 { "" } else { "s" },
        if api_sets == 1 { "" } else { "s" },
        graph.micros as f64 / 1000.0,
        if graph.truncated {
            "\nThe walk stopped at its budget or its patience, so this is incomplete."
        } else {
            ""
        }
    );
    for dir in graph.search.iter().take(SHOWN) {
        let _ = write!(text, "\n    {}", dir.display());
    }
    if graph.search.len() > SHOWN {
        let _ = write!(
            text,
            "\n    …and {} more from PATH",
            graph.search.len() - SHOWN
        );
    }
    text.push_str(
        "\n\nKnownDLLs, side-by-side assemblies and manifest redirection\n\
         are not modelled, and win over all of these at load time.",
    );
    text
}

/// Draw the tree at `rect`, and answer the pointer.
pub fn show(ui: &mut Ui, t: &Theme, rect: Rect, view: &mut View) {
    if view.stale {
        view.rebuild();
    }
    let graph = view.graph.clone();
    let count = view.rows.len();

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    // Rows are painted at exact rects, so the scroll extent has to be the rows and nothing
    // else — see the same line in `filelist`, and the same reason: the installed style's item
    // spacing would make the extent taller than what is drawn and put every row a little
    // further from where the pointer thinks it is.
    child.spacing_mut().item_spacing = egui::Vec2::ZERO;

    let mut toggled = None;
    let scroll = egui::ScrollArea::vertical()
        .id_salt(("deps-rows", rect.min.x as i32, rect.min.y as i32))
        .auto_shrink([false, false]);
    scroll.show_rows(&mut child, ROW, count, |ui, range| {
        let first = range.start;
        // The right-hand edge is the scroll area's, not the panel's: a `ScrollArea` keeps a
        // few points for its bar and clips to what is left, so a row measured against the
        // panel's own width puts its last column under that clip — which is how the processor
        // tag came out as `x6`.
        let right_edge = ui.clip_rect().right().min(rect.right());
        let visible = Rect::from_min_max(
            pos2(rect.left(), ui.min_rect().top()),
            pos2(right_edge, ui.min_rect().top() + range.len() as f32 * ROW),
        );
        // One interaction for the whole block, and the row worked out from the pointer, for
        // the reason `filelist` gives: a widget per row would be an id, a hit test and an
        // animation slot each for a highlight that arithmetic gives away.
        let response = ui.interact(
            visible,
            egui::Id::new(("deps-hit", rect.min.x as i32, rect.min.y as i32)),
            Sense::click(),
        );
        let last = range.len().saturating_sub(1);
        let hovered = response.hover_pos().and_then(|at| {
            (visible.contains(at) && !range.is_empty())
                .then(|| first + (((at.y - visible.top()) / ROW) as usize).min(last))
        });

        let location_x = (visible.left() + visible.width() * LOCATION_AT).round();
        let arch_right = visible.right() - PAD;
        let location_right = arch_right - ARCH;

        for at in range.clone() {
            let Some(row) = view.rows.get(at) else {
                continue;
            };
            let module = &graph.modules[row.module];
            let rect = Rect::from_min_size(
                pos2(visible.left(), visible.top() + (at - first) as f32 * ROW),
                vec2(visible.width(), ROW),
            );
            // The one line every text in this row sits on, whatever font or size it is in, and
            // level with the two glyphs beside them. See the module header. Per row rather than
            // hoisted, because it is snapped to the pixel grid and a scrolled row's top is not
            // a whole number of points — it costs a cached layout lookup.
            let baseline = ink_baseline(ui.painter(), &t.fonts.body, rect.top(), ROW);

            if hovered == Some(at) {
                ui.painter()
                    .rect_filled(rect, egui::CornerRadius::ZERO, crate::ui::hover_fill(t));
            }

            let mut x = rect.left() + PAD + row.depth as f32 * INDENT;

            // The expander. Only where there is something under it, and the space is kept
            // either way so that the names of a level line up whether or not each one has
            // children — a tree whose leaves start a few points left of its branches reads as
            // a mistake.
            let twisty = icon_rect(rect, x, TWISTY);
            if row.expandable {
                let over = hovered == Some(at);
                let glyph: azur_egui_theme::icons::Icon<'_> = if row.open {
                    &azur_icons::chevron_down
                } else {
                    &azur_icons::chevron_right
                };
                glyph(
                    ui.painter(),
                    twisty,
                    if over { t.text.primary } else { t.text.secondary },
                );
            }
            x = twisty.right() + PAD;

            // The glyph, which is where the module's state is said first: a binary, or the error
            // mark for something that is not there or cannot be read.
            //
            // **A missing module that is delay-loaded gets the mark in a neutral ink**, because
            // it is not a fault: Windows ships stubs for features that are not installed, and
            // nothing opens a delay-loaded DLL until something calls into it. See
            // `pe::Graph::breaks_loading` for the five this program's own binary finds.
            let (glyph, ink): (azur_egui_theme::icons::Icon<'_>, Color32) = match module.state {
                State::Found => (&icons::executable, t.executable),
                State::ApiSet => (&icons::executable, t.text.secondary),
                State::Missing if row.delayed => (&azur_icons::error, t.text.secondary),
                State::Missing | State::Unreadable(_) => (&azur_icons::error, t.status.danger),
                State::Unvisited => (&azur_icons::ellipsis, t.text.secondary),
            };
            let box_rect = icon_rect(rect, x, GLYPH);
            glyph(ui.painter(), box_rect, ink);
            x = box_rect.right() + PAD;

            // ---- Name ----
            //
            // A missing module's name is the one thing in the tree worth colouring, and an API
            // set or a delay-loaded reference is one step quieter: neither is part of what has
            // to be on disk for the program to start.
            //
            // Only three inks appear here, and that is measured rather than tidy — see
            // `every_ink_in_the_tree_can_be_read`. `text-tertiary` is 1.65:1 on a hovered row
            // in the dark theme and `text-disabled` is 1.00:1, which is to say it is the fill.
            let name_color = match module.state {
                State::Missing if !row.delayed => t.status.danger,
                State::Missing | State::ApiSet | State::Unvisited => t.text.secondary,
                _ if row.delayed => t.text.secondary,
                _ => t.text.primary,
            };
            let name_right = location_x - PAD;
            let galley = truncated(
                ui.painter(),
                &module.name,
                t.fonts.body.clone(),
                name_color,
                (name_right - x).max(0.0),
            );
            galley_on_baseline(ui.painter(), x, baseline, galley);

            // ---- Location ----
            //
            // The folder, not the whole path: the name is already in the column to the left,
            // and repeating it costs the width that tells you the two `VCRUNTIME140.dll`s in a
            // graph came off different directories.
            let (where_, tint) = location_of(module, row, t);
            let galley = truncated(
                ui.painter(),
                &where_,
                t.fonts.caption.clone(),
                tint,
                (location_right - location_x).max(0.0),
            );
            galley_on_baseline(ui.painter(), location_x, baseline, galley);

            // ---- The processor ----
            //
            // In the danger colour when it is not the root's, because Windows will not load
            // it — which is the second most useful thing this tree can tell you.
            if module.machine != 0 {
                let foreign = graph.foreign(row.module);
                let galley = truncated(
                    ui.painter(),
                    pe::machine_name(module.machine),
                    t.fonts.caption.clone(),
                    if foreign {
                        t.status.danger
                    } else {
                        t.text.secondary
                    },
                    ARCH,
                );
                let width = galley.size().x;
                galley_on_baseline(ui.painter(), arch_right - width, baseline, galley);
            }
        }

        // Clicking a row folds it, which is the whole vocabulary this tree needs. Answered
        // after the loop so the row that was drawn is the row that was clicked.
        if response.clicked() {
            if let Some(at) = hovered {
                if let Some(row) = view.rows.get(at) {
                    if row.expandable {
                        toggled = Some(row.module);
                    }
                }
            }
        }
    });

    if let Some(module) = toggled {
        view.toggle(module);
    }
}

/// What goes in the Location column, and in what ink.
fn location_of(module: &pe::Module, row: &Row, t: &Theme) -> (String, Color32) {
    if row.cyclic {
        // Already above this row: the branch stops, and saying why is better than a leaf that
        // looks like it has nothing under it.
        return ("(already above)".to_owned(), t.text.secondary);
    }
    match module.state {
        State::Found => match module.path.as_deref().and_then(Path::parent) {
            Some(folder) => (folder.to_string_lossy().into_owned(), t.text.secondary),
            None => (String::new(), t.text.secondary),
        },
        // Not a file, and the honest answer is what resolves it rather than a path.
        State::ApiSet => (
            "resolved by the API set schema".to_owned(),
            t.text.secondary,
        ),
        // Delay-loaded and missing is worth saying in full, because it is the difference between
        // "this will not run" and "one feature will not work, later, somewhere else".
        State::Missing if row.delayed => ("not found — delay-loaded".to_owned(), t.text.secondary),
        State::Missing => ("not found".to_owned(), t.status.danger),
        State::Unreadable(why) => (why.to_owned(), t.status.danger),
        State::Unvisited => (
            "not visited — the walk stopped".to_owned(),
            t.text.secondary,
        ),
    }
}

#[cfg(test)]
mod tests;
