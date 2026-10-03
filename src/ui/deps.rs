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
mod tests {
    use super::*;

    /// A graph of this program's own test binary, walked for real.
    fn walked() -> Arc<Graph> {
        let me = std::env::current_exe().expect("a test process has an executable");
        Arc::new(pe::walk(&me, pe::BUDGET, pe::PATIENCE))
    }

    /// **Every ink this tree uses can be read on every surface it is drawn on**, in both themes.
    ///
    /// Measured rather than counted — `azur::contrast` is the ruler — and it found two real
    /// defects on the first run, both of which are the obvious way to write this:
    ///
    /// | ink | on a row | on a hovered row |
    /// | --- | --- | --- |
    /// | `text-primary` | 16.6 / 18.1 | 7.3 / 9.7 |
    /// | `text-secondary` | 6.9 / 7.4 | 3.0 / 4.0 |
    /// | `status-danger` | 4.9 / 4.4 | 2.1 / 2.4 |
    /// | ~~`text-tertiary`~~ | 3.8 / 5.8 | **1.7** / 3.1 |
    /// | ~~`text-disabled`~~ | 2.3 / 2.5 | **1.0** / 1.3 |
    /// | ~~`status-warning`~~ | 8.2 / **2.8** | 3.6 / **1.5** |
    ///
    /// `text-disabled` at 1.00:1 *is* the hover fill in the dark theme: an API set's location
    /// line was invisible for as long as the pointer was over the row it was on. And
    /// `status-warning` is a legitimate role that simply is not ink at 12 points on a light
    /// surface — it was the processor tag on a mismatched module, which is the one row in a
    /// dependency list you most need to be able to read.
    ///
    /// So the tree has three inks, and this is what keeps it that way. The two floors:
    /// **4.0:1 on the surface a row is normally read on**, and **2.0:1 in the transient hovered
    /// state** — the same argument, and a rung above the number, that `azur::desktop`'s own
    /// `a_row_can_still_be_read_when_it_is_hovered_or_pressed` settled on at 2.5.
    #[test]
    fn every_ink_in_the_tree_can_be_read() {
        use azur_egui_theme::contrast::ratio;

        const RESTING: f32 = 4.0;
        const HOVERED: f32 = 2.0;

        for t in [Theme::dark(), Theme::light()] {
            let name = if t.dark { "dark" } else { "light" };
            let hovered = crate::ui::hover_fill(&t);
            for (what, ink) in [
                ("text-primary", t.text.primary),
                ("text-secondary", t.text.secondary),
                ("status-danger", t.status.danger),
            ] {
                for (surface, fill, floor) in [
                    ("a row", t.bg.layer, RESTING),
                    ("a hovered row", hovered, HOVERED),
                ] {
                    let got = ratio(ink, fill);
                    assert!(
                        got >= floor,
                        "{name}: {what} on {surface} is {got:.2}:1, under {floor}"
                    );
                }
            }
            // And the glyphs, at the floor a *shape* gets rather than the one ink gets.
            for (what, ink, fill, surface) in [
                ("the module glyph", t.executable, t.bg.layer, "a row"),
                ("the module glyph", t.executable, hovered, "a hovered row"),
                ("the missing mark", t.status.danger, t.bg.layer, "a row"),
            ] {
                let got = ratio(ink, fill);
                assert!(
                    got >= azur_egui_theme::contrast::SHAPE,
                    "{name}: {what} on {surface} is {got:.2}:1, under the shape floor"
                );
            }
        }

        // And the three that were taken out really are the reason: each is under the hovered
        // floor on some surface. Across **both** themes rather than within one, which is the
        // whole point of checking both — `status-warning` is perfectly legible on the dark side
        // at 3.58:1 and 1.49:1 on the light one, and a rule derived from the dark theme alone is
        // how the two light-theme defects this project has already fixed got in.
        type Pick = fn(&Theme) -> Color32;
        for (what, pick) in [
            ("text-tertiary", (|t: &Theme| t.text.tertiary) as Pick),
            ("text-disabled", |t: &Theme| t.text.disabled),
            ("status-warning", |t: &Theme| t.status.warning),
        ] {
            let worst = [Theme::dark(), Theme::light()]
                .iter()
                .flat_map(|t| {
                    let ink = pick(t);
                    [t.bg.layer, crate::ui::hover_fill(t)].map(|fill| ratio(ink, fill))
                })
                .fold(f32::INFINITY, f32::min);
            assert!(
                worst < HOVERED,
                "{what} measures {worst:.2}:1 at worst and could be used after all"
            );
        }
    }

    /// The tree is a walk of a graph, and the two things that make that finite: a module
    /// already on the branch is not descended into, and expansion is what decides the rest.
    #[test]
    fn the_tree_folds_and_never_goes_round_a_cycle() {
        let graph = walked();
        let mut view = View::new(graph.clone());

        assert_eq!(view.rows[0].module, 0, "the root is the first row");
        assert_eq!(view.rows[0].depth, 0);
        assert!(view.rows[0].open, "the root opens expanded");
        assert_eq!(
            view.rows.len(),
            1 + graph.modules[0].imports.len(),
            "a freshly walked binary shows its own imports and no more"
        );

        // Folding the root leaves one row; unfolding puts them back.
        view.toggle(0);
        view.rebuild();
        assert_eq!(view.rows.len(), 1);
        assert!(!view.rows[0].open);
        view.toggle(0);
        view.rebuild();
        assert!(view.rows.len() > 1);

        // Expand everything there is, which is what would run away if either rule were
        // missing: this graph has cycles in it — `kernel32` and `kernelbase` refer to each
        // other — so a walk with no branch rule would not come back.
        for i in 0..graph.modules.len() {
            view.expanded.insert(i);
        }
        view.rebuild();
        assert!(view.rows.len() > graph.modules[0].imports.len());
        assert!(view.rows.len() <= ROW_CAP);
        for row in &view.rows {
            assert!((row.depth as usize) <= DEPTH_CAP);
            // Nothing is both a repeat and expanded: that is the rule, stated as an assertion.
            assert!(!(row.cyclic && row.open));
        }
        // And something really was a repeat, or the rule was never exercised.
        assert!(
            view.rows.iter().any(|row| row.cyclic),
            "no cycle was reached, so the branch rule proves nothing here"
        );
    }

    /// A walk opens the branches that lead to something that would stop the program starting,
    /// and nothing else.
    #[test]
    fn a_walk_opens_only_what_is_worth_looking_at() {
        let graph = walked();
        let view = View::new(graph.clone());

        // Every module opened is either the root or on a chain to something missing.
        let chains = graph.breaks_loading();
        for &module in &view.expanded {
            let on_a_chain = chains
                .iter()
                .any(|chain| chain[..chain.len() - 1].contains(&module));
            assert!(
                module == 0 || on_a_chain,
                "{} was opened for no reason",
                graph.modules[module].name
            );
        }
        // And every chain really is open, all the way down.
        for chain in &chains {
            for &module in &chain[..chain.len() - 1] {
                assert!(
                    view.expanded.contains(&module),
                    "the way down to a missing module is not open at {}",
                    graph.modules[module].name
                );
            }
        }
    }

    /// The search order is reported with what it does not model, because a location means
    /// nothing without the list of places that were tried.
    #[test]
    fn the_whole_answer_is_on_the_title() {
        let graph = walked();
        let text = about(&graph);
        let (files, _, _) = graph.tally();
        assert!(
            text.starts_with(&format!("{files} file")),
            "the counts are not the first thing said: {text}"
        );
        assert!(text.contains(" ms"), "the timing is gone");
        assert!(text.contains("Looked for in order:"));
        assert!(
            text.contains(&graph.search[0].display().to_string()),
            "the first place looked is not in the list"
        );
        assert!(
            text.contains("KnownDLLs"),
            "the caveats are gone, and the list now reads as the whole rule"
        );
    }
}
