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
//! **Or one flat list of every module**, which is the other thing the same graph is: the tree
//! answers "what pulled this in", and the list answers "what does this program load, in the end" —
//! and the list is the one that can be read to the bottom. See [`View::flat`].
//!
//! # What it opens with
//!
//! The root expanded, and then **exactly the branches that lead to a module that is not there** —
//! see [`crate::pe::Graph::missing`]. Everything else stays folded. A modern graph is a thousand
//! modules; expanding it all would be a wall of `api-ms-` rows with the one line that matters
//! somewhere in the middle of it, which is the mistake the original Dependency Walker is remembered
//! for.
//!
//! It is **every** missing module and not only the ones that would stop the program from starting,
//! for a reason worth knowing, because it was the other way round and that was a defect: the panel's
//! title says "5 missing", the count comes from [`crate::pe::Graph::tally`] which does not care how a
//! module was reached, and on an ordinary machine all five are delay-loaded from somewhere deep inside
//! `shell32` — so the tree opened no branch to any of them. A count of something the display will not
//! then show you is the one thing a dependency list must not do. What tells the two kinds apart is now
//! how the row is *drawn* rather than whether it can be found at all, and the title names them
//! outright: see [`about`].
//!
//! **What that costs is worth stating**, because it is the same wall this panel is otherwise careful
//! about: opening the way down to a module under `shell32` opens `shell32`, and `shell32` has 217
//! imports of its own. Walking this program's own binary, the tree therefore comes up with about 770
//! rows rather than 30. Two things make that the right trade anyway — the rows at the *top* are
//! unchanged, so what you see when the panel opens is still the root's own imports in order; and the
//! alternative was five modules that could not be reached by any amount of scrolling. The title says
//! which five they are, so you know what you are scrolling to, and [`ROW_CAP`] is what keeps the
//! pathological case bounded.
//!
//! # The row that is picked, and the two panels about it
//!
//! Clicking a row picks it, which is what the panels beside the tree are about: what that module
//! **exports**, and what the module above it **uses** out of it. Two lists and not one, because they
//! are two questions — `kernel32.dll` exports seventeen hundred symbols and the program that imports
//! it uses forty. See [`symbols`], and [`places`] for where they go and when there is no room for
//! them.
//!
//! Folding is the **expander** and a double click, not a click anywhere on the row, now that a click
//! anywhere on the row means something else. That is the platform's own tree vocabulary rather than
//! a choice made here.
//!
//! # Three search boxes, and only one of them is a filter
//!
//! One in each strip. The two symbol boxes narrow their list to the rows whose name holds what was
//! typed — see [`symbols::List::narrow`] — and the tree's does something else entirely, because a
//! filter over the rows on show would search whatever happens to be unfolded and the answer is nearly
//! always in a branch that is shut. It asks the graph and shows the way down to every match. See
//! [`View::find_modules`] and [`View::rebuild_found`].
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

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use azur_egui_theme::components::{galley_on_baseline, ink_baseline};
use azur_egui_theme::icons as azur_icons;
use azur_egui_theme::tokens::space;
use egui::{pos2, vec2, Color32, Id, Rect, Sense, Ui};

use crate::app::Action;
use crate::icons;
use crate::pe::{self, Graph, State};
use crate::theme::Theme;
use crate::ui::{icon_rect, seam, truncated, SEAM};

pub mod symbols;

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

/// The narrowest and shortest the tree will be squeezed to before the symbol panels are dropped
/// instead of it.
///
/// **The tree is what the panel is for**, so it is never the thing that gives way: a row is a name,
/// a folder and a processor tag, and under about 280 points the folder column — the reason the tree
/// is worth looking at — is an ellipsis. Four rows is the same floor stated for the other axis, and it
/// counts the strip over them: the tree pays [`HEAD`] off the top of whatever it is given, the same
/// way [`symbols::MIN_H`] does, and a floor that forgot that was a floor of three rows.
const TREE_MIN_W: f32 = 280.0;
const TREE_MIN_H: f32 = HEAD + ROW * 4.0;

/// How much of the canvas the symbol panels take, and how they divide it between them, before
/// anybody drags either — see [`Places`].
pub const SHARE: f32 = 0.40;
pub const SPLIT: f32 = 0.50;

/// How this panel is set up, as the window remembers it: which of the two views the tree is in, and
/// how much room the symbol panels have.
///
/// **One type rather than three fields on [`crate::ui::preview::Layout`]**, because three loose
/// scalars had to be written and read by hand twice over — and the second half was forgotten, so the
/// dragged sizes were saved on every quit and dropped on every launch. One type is one codec:
/// [`Sizes::as_str`] and [`Sizes::parse`], the shape `preview::Where`, `pane::FlatMode` and
/// `console::Kind` already use, and the round-trip test cannot pass while half of it is missing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sizes {
    /// Show a binary's dependencies as one flat list rather than as the tree.
    ///
    /// A preference and not per-view: it is a way of reading the same answer, and somebody who wants
    /// the list wants it for the next binary too.
    ///
    /// **Off by default** — the tree is what a dependency walk *is*, and it is the only one of the two
    /// that says what pulled a module in. The list is the one that can be read to the bottom, which is
    /// the other half of the question. See [`View::flat`].
    pub list: bool,
    /// The symbol panels' share of the canvas, and how the two of them divide it. Both dragged — see
    /// [`Places`] — and both here rather than in the view, because a panel that had to be re-dragged
    /// every time it opened would be a panel nobody dragged.
    pub share: f32,
    pub split: f32,
}

impl Default for Sizes {
    fn default() -> Self {
        Self {
            list: false,
            share: SHARE,
            split: SPLIT,
        }
    }
}

impl Sizes {
    /// One settings line: `list,share,split`.
    pub fn as_str(self) -> String {
        format!(
            "{},{:.3},{:.3}",
            u8::from(self.list),
            self.share,
            self.split
        )
    }

    /// Read that line back. Anything unreadable keeps its default rather than refusing the line, for
    /// the reason the rest of the settings file works that way: a broken line costs only itself.
    ///
    /// `is_finite` before the clamp, and that is not belt-and-braces: `"nan".parse::<f32>()` succeeds,
    /// `f32::clamp` passes a NaN straight through, and [`places`] would then hand out NaN rects — so
    /// the grip would never hit-test and neither the drag nor the double click that would put it back
    /// could be reached. The same guard, for the same reason, as `sidebar_width`.
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        let mut parts = text.split(',');
        if let Some(list) = parts.next() {
            out.list = list.trim() == "1";
        }
        for (part, into) in parts.zip([&mut out.share, &mut out.split]) {
            if let Ok(value) = part.trim().parse::<f32>() {
                if value.is_finite() {
                    *into = value.clamp(0.1, 0.9);
                }
            }
        }
        out
    }
}

/// The canvas width at which the symbol panels go **beside** the tree rather than under it.
///
/// Not a floor — the two fit in far less than this, and [`TREE_MIN_W`] with [`symbols::MIN_W`] is
/// where that floor is. This is the width at which a column *reads*: the tree keeps its Location
/// column, the panel keeps enough width for a C++ name, and both keep the full height of the panel to
/// scroll in. Under it the panels go along the bottom instead, where a narrow dock has height to
/// spare and no width at all.
const BESIDE_AT: f32 = 800.0;

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
    /// The module whose imports this row is one of. `None` for the root, and for every row of the
    /// flat list — which shows a module once rather than once per importer, so there is no one
    /// importer for it to be about.
    from: Option<usize>,
    depth: u16,
    /// Whether the reference that got here is delay-loaded.
    delayed: bool,
    /// It has imports, and this row is somewhere they can be shown.
    expandable: bool,
    open: bool,
    /// This module is already an ancestor of this row: the branch stops here rather than
    /// going round again.
    cyclic: bool,
    /// The row this one hangs under, or `usize::MAX` for the root. See [`Step::under`], which is what
    /// it is for: the ancestry a cycle is detected against.
    under: usize,
    /// Only on the way to an answer rather than being one — a row a search kept because something
    /// under it matched, not because it did.
    ///
    /// Drawn in the quiet ink for that reason. Searching `crypt` in this program's own graph shows
    /// `RPCRT4.dll` and `SspiCli.dll` among the answers, and they are there because `bcryptPrimitives`
    /// and `CRYPTBASE` are under them: the tree is the route, and the route is not the answer.
    faint: bool,
}

/// One row still to emit, and where it hangs. [`View::walk`]'s stack.
struct Step {
    module: usize,
    from: Option<usize>,
    depth: u16,
    delayed: bool,
    /// The row this one goes under, as a position in the rows already emitted, or `usize::MAX` for the
    /// root.
    ///
    /// **The ancestry is this chain and not a copy of it.** Each `Step` used to carry a `Vec` of every
    /// module above it, cloned per child pushed — one allocation per row emitted, up to [`ROW_CAP`] of
    /// them alive at once, for a list only ever asked `contains`. Walking up from here answers the same
    /// question in at most [`DEPTH_CAP`] index steps and allocates nothing.
    under: usize,
}

/// What a set of rows was built from. See [`View::built`].
#[derive(Default, PartialEq)]
struct Built {
    flat: bool,
    query: String,
}

/// Which row is picked: the module, and the one that imports it.
///
/// **By module and importer rather than by row number**, because the rows are rebuilt whenever
/// anything is folded, and a row number would then be a pick of whatever moved into that position.
///
/// The importer is part of it because one of the two panels is about the *edge* and not the module:
/// what `shell32.dll` uses out of `kernel32.dll` is not what the root uses out of it. The
/// consequence is that the same module picked under two different importers is two different picks,
/// which is right — they are two different answers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pick {
    pub module: usize,
    pub from: Option<usize>,
}

/// A walked graph, and how much of it is unfolded.
pub struct View {
    graph: Arc<Graph>,
    /// The whole answer for the panel's title, worked out once.
    ///
    /// It is [`about`]'s string, and it was being rebuilt from the graph on every frame the pointer
    /// rested on the title — two breadth-first walks of every edge, a `HashSet` and a 1.5 KB string,
    /// sixty times a second, for an answer that cannot change: the graph behind an `Arc` is never
    /// touched again after the walk.
    about: String,
    /// Which modules are opened before `main` runs — [`crate::pe::Graph::upfront`], asked once. It
    /// depends only on the graph, and the flat list was deriving it on every rebuild.
    upfront: Vec<bool>,
    /// Which modules are expanded, by index into the graph.
    ///
    /// By module and not by position in the tree, so expanding `shell32.dll` expands it
    /// wherever else it appears. That is the behaviour a graph wants: the question is what a
    /// module needs, and the answer does not depend on which of its importers you asked from.
    expanded: HashSet<usize>,
    rows: Vec<Row>,
    /// What [`View::rows`] were built from: the view they are in, and the query that narrowed them.
    ///
    /// **One staleness key rather than three idioms.** This module had a mirror of the flat-list
    /// preference compared in [`show`], a `clone()` of the query compared in [`tree`] — an allocation
    /// per frame to detect a keystroke — and `List::narrowed_for` doing the same job a third way for
    /// the panels. The rows are rebuilt when this stops matching what was asked for, and not
    /// otherwise.
    built: Built,
    picked: Option<Pick>,
    /// The picked module's symbols, read when it is picked and kept until it is not. See
    /// [`symbols::Both`].
    symbols: Option<symbols::Both>,
    /// What the tree is searching for.
    ///
    /// **A query on a tree is not a filter on its rows**, and this is the field where that decision
    /// lives. Filtering what is unfolded would search thirty rows out of a thousand — the answer is
    /// nearly always in a branch that is shut, which is the one place a filter cannot look. So a query
    /// asks the *graph* instead, and the tree becomes the chains from the root down to every module
    /// whose name matches: not "which of these rows match" but "where does this come in, and through
    /// what". See [`View::walk`]'s `Only::Chains`.
    find_modules: String,
    /// And what each of the two symbol panels is searching for.
    ///
    /// Here rather than beside the lists, which are dropped and re-read whenever another row is
    /// picked: a query that vanished on the next pick would be no use for the thing these are for,
    /// which is following one symbol from module to module.
    find_used: String,
    find_exports: String,
    /// How many modules the tree's query matched, or `None` when there is no query.
    ///
    /// Counted by the rebuild rather than by the strip that shows it, because the strip cannot: in the
    /// tree a matching module is not one row, it is a row per place it is imported from, and the
    /// interesting number is how many *modules* answered.
    matched: Option<usize>,
}

impl View {
    /// Take a finished walk and open the branches worth looking at.
    pub fn new(graph: Arc<Graph>) -> Self {
        let mut expanded = HashSet::from([0]);
        for chain in graph.missing() {
            // Every module on the way down, but not the missing one itself — it has nothing
            // under it to show.
            for &at in chain.iter().rev().skip(1) {
                expanded.insert(at);
            }
        }
        let mut view = Self {
            about: about(&graph),
            upfront: graph.upfront(),
            graph,
            expanded,
            rows: Vec::new(),
            // Anything but the default, so the first `sync` builds rather than believing the rows it
            // has not made yet.
            built: Built {
                flat: true,
                query: "\u{0}".to_owned(),
            },
            picked: None,
            symbols: None,
            find_modules: String::new(),
            find_used: String::new(),
            find_exports: String::new(),
            matched: None,
        };
        view.sync(false);
        view
    }

    /// The whole answer for the panel's title. Worked out once — see [`View::about`].
    pub fn about(&self) -> &str {
        &self.about
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Whether the rows in hand are the flat list. What they were built from, which is the only
    /// authority the drawing and the layout should be asking — see [`View::built`].
    fn flat(&self) -> bool {
        self.built.flat
    }

    /// How many rows are on show. For the tests; the panel reads the rows.
    #[cfg(test)]
    pub fn shown(&self) -> usize {
        self.rows.len()
    }

    /// Which module is picked. For the tests, which are the only thing outside this module with any
    /// business knowing.
    #[cfg(test)]
    pub fn picked(&self) -> Option<Pick> {
        self.picked
    }

    /// How many symbols were read for it: what it exports, and what the row above it uses — each
    /// `None` where there was nothing to read. `None` at the top for a view that has not read any.
    #[cfg(test)]
    pub fn symbol_counts(&self) -> Option<(Option<usize>, Option<usize>)> {
        self.symbols.as_ref().map(symbols::Both::counts)
    }

    /// The two lists as the panels would draw them, for a search. For the tests.
    #[cfg(test)]
    pub fn symbols_mut(&mut self) -> Option<&mut symbols::Both> {
        self.symbols.as_mut()
    }

    /// Type into the tree's search box, and rebuild as the frame would. For the tests.
    #[cfg(test)]
    pub fn search_modules(&mut self, query: &str) {
        self.find_modules = query.to_owned();
        let flat = self.flat();
        self.sync(flat);
    }

    /// What the tree is showing, as the name of each row and whether it is only on the way to an
    /// answer. For the tests.
    #[cfg(test)]
    pub fn shown_rows(&self) -> Vec<(&str, bool)> {
        self.rows
            .iter()
            .map(|row| (self.graph.modules[row.module].name.as_str(), row.faint))
            .collect()
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
    ///
    /// **All of them and not the first one**, which is the fourth lesson and the same one again. A
    /// module appears wherever it is imported, so a name that is on screen twice gives a caller
    /// looking for "where was this drawn" two answers and no way to choose — and *which* names repeat
    /// near the top is not a fact about this program either. It changed the day the tree began opening
    /// the way down to every missing module: `advapi32.dll` is one of the root's own imports and also
    /// one of `shell32`'s, and both were suddenly on screen.
    ///
    /// So the choice is the caller's, because only the caller can see what was actually drawn — which
    /// is also the one honest answer to how many rows fit, a question this cannot answer and the
    /// caller was previously estimating from the panel's height.
    #[cfg(test)]
    pub fn foldable(&self, rows: usize) -> Vec<(usize, &str)> {
        self.rows
            .iter()
            .take(rows)
            .enumerate()
            .filter(|(_, row)| row.expandable && !row.open && !row.cyclic)
            .map(|(at, row)| (at, self.graph.modules[row.module].name.as_str()))
            .collect()
    }

    /// Pick the row of the module with this name, if one is on show. Whether one was.
    ///
    /// For `--deps=<module>`: the two panels are behind a click on a row, and a capture run has no
    /// mouse — the same reason `--rename` and `--preview` exist. By name because that is the only
    /// thing a person on a command line has; the first row of that module, because a module under two
    /// importers is two rows and either of them answers the question the flag is asking.
    pub fn pick_named(&mut self, name: &str) -> bool {
        let found = self.rows.iter().find(|row| {
            self.graph.modules[row.module]
                .name
                .eq_ignore_ascii_case(name)
        });
        let Some(row) = found else {
            return false;
        };
        self.picked = Some(Pick {
            module: row.module,
            from: row.from,
        });
        true
    }

    /// Fold a module open or shut, and rebuild the rows it changes.
    fn toggle(&mut self, module: usize) {
        if !self.expanded.remove(&module) {
            self.expanded.insert(module);
        }
        self.rebuild();
    }

    /// Whether a row is the picked one.
    ///
    /// In the flat list the importer is not compared, because a flat row has none: the same module
    /// picked in the tree is still the module picked here, and a pick that vanished when the toggle
    /// was pressed would make the toggle a thing you learn not to press.
    fn is_picked(&self, row: &Row) -> bool {
        self.picked.is_some_and(|pick| {
            pick.module == row.module && (self.flat() || pick.from == row.from)
        })
    }

    /// Read the picked module's symbols, unless they are already in hand.
    ///
    /// Called from [`show`] and only when a panel is going to be drawn: the read is two files
    /// reopened and a couple of hundred kilobytes off the disk, which is nothing on a click and
    /// pointless in a panel too small to show the answer.
    fn ensure_symbols(&mut self) {
        let Some(pick) = self.picked else {
            self.symbols = None;
            return;
        };

        if self.symbols.as_ref().is_some_and(|both| both.of == pick) {
            return;
        }
        self.symbols = Some(symbols::Both::read(&self.graph, pick));
    }

    /// Build the rows for `flat` and the query in hand, unless they are already what is in hand.
    ///
    /// The one place staleness is decided — see [`View::built`].
    fn sync(&mut self, flat: bool) {
        if self.built.flat == flat && self.built.query == self.find_modules {
            return;
        }
        self.built = Built {
            flat,
            query: self.find_modules.clone(),
        };
        self.rebuild();
    }

    /// Build the rows from scratch, for whatever [`View::built`] now says they are.
    fn rebuild(&mut self) {
        self.rows.clear();
        self.matched = None;
        let graph = self.graph.clone();
        let query = azur_egui_theme::filter::Query::parse(&self.built.query);
        if self.built.flat {
            self.rebuild_flat(&graph, &query);
            return;
        }
        if query.is_empty() {
            self.walk(&graph, Only::Everything);
            return;
        }
        // **The whole graph is searched and the tree is the answer**, which is the point
        // [`find_modules`] argues: a filter over the rows on show would search whatever happens to be
        // unfolded, and what somebody typing `msvcp` wants to know is where it comes in — a chain from
        // the root, through whichever module pulled it in, down to the match.
        //
        // [`crate::pe::Graph::chains_to`] does the finding, which is the same breadth-first walk that
        // opens the way down to the missing modules.
        //
        // [`find_modules`]: View::find_modules
        let chains = graph.chains_to(|module| query.matches(&module.name));
        let on_a_chain: HashSet<usize> = chains.iter().flatten().copied().collect();
        let found: HashSet<usize> = chains.iter().filter_map(|c| c.last().copied()).collect();
        self.matched = Some(found.len());
        self.walk(&graph, Only::Chains(&on_a_chain, &found));
        // The root on its own says nothing: it is the file being looked at, and it leads every chain
        // whether anything matched or not.
        if self.rows.len() == 1 && !found.contains(&0) {
            self.rows.clear();
        }
    }

    /// Walk the graph into rows, depth first, taking the children [`Only`] allows.
    ///
    /// An explicit stack rather than recursion: the depth is bounded by the branch rule below, but that
    /// bound is the module count, and a thousand frames of a UI thread's stack is not a bound worth
    /// relying on.
    ///
    /// One walk for both the whole tree and a search's chains, because everything except which
    /// children to follow is the same: the row cap, the depth cap, the cycle rule, and — the one that
    /// matters — [`order`], so a search's rows are ordered exactly like the tree they are a view of.
    fn walk(&mut self, graph: &Graph, only: Only<'_>) {
        let mut stack: Vec<Step> = vec![Step {
            module: 0,
            from: None,
            depth: 0,
            delayed: false,
            under: usize::MAX,
        }];
        let mut kids: Vec<pe::Edge> = Vec::new();
        while let Some(step) = stack.pop() {
            if self.rows.len() >= ROW_CAP {
                break;
            }
            let module = step.module;
            let cyclic = self.above(step.under, module);
            let deep = (step.depth as usize) >= DEPTH_CAP;
            // What the two walks disagree about, and all they disagree about: the whole tree folds and
            // a search does not — every row of a search is on the way to an answer, so there is nothing
            // under it the search left out, and a match itself is where the answer stops.
            let (expandable, open, faint) = match only {
                Only::Everything => {
                    let expandable =
                        !graph.modules[module].imports.is_empty() && !cyclic && !deep;
                    (expandable, expandable && self.expanded.contains(&module), false)
                }
                Only::Chains(_, found) => {
                    (false, !found.contains(&module) && !deep, !found.contains(&module))
                }
            };
            let at = self.rows.len();
            self.rows.push(Row {
                module,
                from: step.from,
                depth: step.depth,
                delayed: step.delayed,
                under: step.under,
                expandable,
                open,
                cyclic,
                faint,
            });
            if !open {
                continue;
            }
            // Sorted by [`order`], and pushed in reverse since the stack hands them back the other way
            // round.
            kids.clear();
            kids.extend(
                graph.modules[module]
                    .imports
                    .iter()
                    .filter(|edge| match only {
                        Only::Everything => true,
                        // A chain's own modules only, and never back up the branch it is on.
                        Only::Chains(on_a_chain, _) => {
                            on_a_chain.contains(&edge.to) && !self.above(at, edge.to)
                        }
                    })
                    .copied(),
            );
            let beside = folder_of(&graph.modules[module]);
            kids.sort_by(|a, b| order(graph, beside, a.to, b.to));
            for edge in kids.iter().rev() {
                stack.push(Step {
                    module: edge.to,
                    from: Some(module),
                    depth: step.depth + 1,
                    delayed: edge.delayed,
                    under: at,
                });
            }
        }
    }

    /// Whether `module` is already on the branch that ends at row `from`.
    ///
    /// What keeps the walk finite: a module that is above a row is drawn and not descended into. Up the
    /// [`Row::under`] chain rather than through a copy of the branch — bounded by [`DEPTH_CAP`], and
    /// see [`Step::under`] for what the copies were costing.
    fn above(&self, from: usize, module: usize) -> bool {
        let mut at = from;
        for _ in 0..=DEPTH_CAP {
            let Some(row) = self.rows.get(at) else {
                return false;
            };
            if row.module == module {
                return true;
            }
            at = row.under;
        }
        false
    }

    /// Every module once, in one list. See [`Sizes::list`].
    fn rebuild_flat(&mut self, graph: &Graph, query: &azur_egui_theme::filter::Query) {
        // A query is a plain filter here, which is all it can be and all it needs to be: every module
        // is already a row, so there is nothing hidden for it to fail to look in. The tree's own search
        // is the one that has to work differently — see [`View::rebuild`].
        let mut order: Vec<usize> = (0..graph.modules.len())
            .filter(|&at| query.matches(&graph.modules[at].name))
            .collect();
        // Every module is a row here, so the two numbers are the same one.
        self.matched = (!query.is_empty()).then_some(order.len());

        // **The folder test is decorated, not asked per comparison.** `order` parses a path and
        // compares it against `beside`, and a comparator pays that twice per comparison — nineteen
        // thousand `Path::parent()` parses to sort a thousand modules. Once each, into a flag the
        // comparator reads, is the same answer for one parse per module.
        let beside = folder_of(&graph.modules[0]);
        let same: Vec<bool> = (0..graph.modules.len())
            .map(|at| beside_of(graph, beside, at))
            .collect();
        order.sort_by(|&a, &b| {
            same[b].cmp(&same[a]).then_with(|| {
                crate::fs::sort::natural_cmp(&graph.modules[a].name, &graph.modules[b].name)
            })
        });

        self.rows.extend(order.into_iter().take(ROW_CAP).map(|at| Row {
            module: at,
            from: None,
            depth: 0,
            delayed: at != 0 && !self.upfront[at],
            under: usize::MAX,
            expandable: false,
            open: false,
            cyclic: false,
            faint: false,
        }));
    }
}

/// Which of a module's imports a walk follows.
enum Only<'a> {
    /// All of them: the tree, folded where it is folded.
    Everything,
    /// Only the modules on a chain to a match, with the matches themselves — the shape a search
    /// leaves. See [`View::rebuild`].
    Chains(&'a HashSet<usize>, &'a HashSet<usize>),
}

/// The strip over a list of anything: the box that narrows it, the room its caption goes in, and the
/// room left under it for the rows.
///
/// **Here rather than beside either list**, because all three strips in this panel are the same strip:
/// the tree's and the two symbol panels'. It lived in [`symbols`] and the tree reached down into it,
/// which is a module admitting it is in the wrong file.
///
/// **The caption is the caller's to paint**, and not because it is shorter that way: what it says is
/// how many rows the query left, and the query is only known once the field here has run.
///
/// **The box gives way before the caption is dropped, and then the caption gives way to the box.** A
/// narrow panel keeps its search field and loses its label: the label is a reminder of which list this
/// is, which the rows under it also say, and the field is the only way to find one symbol in seventeen
/// hundred.
fn strip(ui: &mut Ui, t: &Theme, rect: Rect, query: Option<(Id, &mut String)>) -> Strip {
    /// What the field needs before it is worth drawing, and what it takes when it is.
    const FIELD_MIN: f32 = 96.0;
    const FIELD: f32 = 150.0;

    let strip = Rect::from_min_size(rect.min, vec2(rect.width(), HEAD.min(rect.height())));
    ui.painter()
        .rect_filled(strip, egui::CornerRadius::ZERO, t.bg.layer_alt);
    let mut right = strip.right() - PAD;

    if let Some((id, query)) = query {
        let width = FIELD.min(strip.width() - PAD * 2.0);
        if width >= FIELD_MIN {
            let field = Rect::from_min_size(
                pos2((right - width).round(), (strip.center().y - HEAD * 0.5).round()),
                vec2(width, HEAD),
            );
            // Square, like everything else welded into a bar in this window. See
            // [`crate::ui::squared`], and [`crate::ui::nudge_caret`] for the mark taken first.
            let first_shape = crate::ui::shape_mark(ui);
            // **Under `push_id`**, which is what makes the field's own id this caller's rather than a
            // number counted off the widget sequence. This module's rows were once hit-tested through
            // an id derived that way, and adding one button to the bar above moved it — the rows drew,
            // hovered, and stopped answering clicks. A field that lost its id would lose the keyboard
            // instead, mid-word.
            ui.push_id(id, |ui| {
                crate::ui::squared(ui, |ui| {
                    ui.put(
                        field,
                        azur_egui_theme::components::TextField::new(query)
                            .placeholder("Search")
                            .clearable(true)
                            .size(azur_egui_theme::components::Size::Small)
                            .width(width)
                            .text_lift(TEXT_LIFT),
                    )
                })
            });
            crate::ui::nudge_caret(
                ui,
                first_shape,
                crate::ui::CARET_SHORTER,
                crate::ui::CARET_LOWER,
            );
            right = field.left() - PAD;
        }
    }

    Strip {
        body: Rect::from_min_max(pos2(rect.left(), strip.bottom()), rect.max),
        caption: Rect::from_min_max(
            pos2(rect.left() + PAD, strip.top()),
            pos2(right - PAD, strip.bottom()),
        ),
    }
}

/// What [`strip`] leaves its caller: where the caption goes, and where the rows go.
struct Strip {
    caption: Rect,
    body: Rect,
}

/// What a strip says, on the line the rest of it reads along.
fn caption(ui: &Ui, t: &Theme, at: Rect, text: &str) {
    let baseline = ink_baseline(ui.painter(), &t.fonts.caption, at.top(), at.height());
    let galley = truncated(
        ui.painter(),
        text,
        t.fonts.caption.clone(),
        t.text.secondary,
        at.width().max(0.0),
    );
    galley_on_baseline(ui.painter(), at.left(), baseline, galley);
}

/// The strip's own height: [`crate::ui::TOOL_SIZE`], which is what the search field in it is.
pub const HEAD: f32 = crate::ui::TOOL_SIZE;

/// A point and a half off the top of what is typed in a search box, and of its placeholder.
///
/// The same correction, for the same reason, as the filter box on the path bar — see
/// `crate::ui::breadcrumb`'s `FILTER_TEXT_LIFT`: the field's frame is centred in the strip and the
/// words inside it sit below the line the text beside them reads along.
///
/// **1.5 and not that one's 2.0**, because the line being joined is a different line. The filter box
/// comes up to a row of painted glyphs, each centred on its own ink; this comes up to a caption on
/// [`ink_baseline`], which is half a point further down. Measured rather than chosen —
/// `everything_in_a_dependency_row_sits_on_one_line` compares the two texts in a strip exactly, and
/// 2.0 put the placeholder half a point above the caption beside it.
const TEXT_LIFT: f32 = 1.5;

/// Where the expander sits in a row at this depth.
///
/// One function because two things have to agree about it: it is drawn there, and it is the one part
/// of a row that folds on a single click — everywhere else picks. A hit test that worked the position
/// out for itself would be a fold that misses by however much the two formulas drifted apart.
fn twisty_x(left: f32, depth: u16) -> f32 {
    left + PAD + depth as f32 * INDENT
}

/// The folder a module was found in, which is what [`order`] groups by.
fn folder_of(module: &pe::Module) -> Option<&Path> {
    module.path.as_deref().and_then(Path::parent)
}

/// How the modules of one level are ordered: **the ones from the same folder first, then by name.**
///
/// Which replaces the import table's own order, and deliberately. That order is the linker's, and
/// for a program that ships DLLs beside itself it means its own are scattered through fifty system
/// modules in an order nobody can predict or scan. Grouping by folder puts them at the top, together
/// — and the folder compared against is the **importing module's**, not the root's, so the rule reads
/// the same at every level of the tree: the ones that live next to it come first.
///
/// Anything that is not a file at all — an API set, a module that is missing — is in no folder and so
/// is never in the first group. It sorts alphabetically among the rest, which is where a name you are
/// looking up expects to find it.
///
/// [`crate::fs::sort::natural_cmp`] and not a plain compare, so that a name orders here exactly as it
/// would in a pane: case-insensitively, and with `vcruntime140_1` after `vcruntime140`.
/// Over a level of a dozen edges, which is what a tree node has. The flat list has a thousand and
/// decorates instead — see [`View::rebuild_flat`], which is the same rule with the folder test lifted
/// out of the comparison.
fn order(graph: &Graph, beside: Option<&Path>, a: usize, b: usize) -> Ordering {
    // `b` against `a`, because `false` sorts first and being in the same folder has to.
    beside_of(graph, beside, b)
        .cmp(&beside_of(graph, beside, a))
        .then_with(|| crate::fs::sort::natural_cmp(&graph.modules[a].name, &graph.modules[b].name))
}

/// Whether module `at` was found in `beside`, which is the first half of [`order`].
fn beside_of(graph: &Graph, beside: Option<&Path>, at: usize) -> bool {
    folder_of(&graph.modules[at])
        .zip(beside)
        .is_some_and(|(mine, theirs)| mine.as_os_str().eq_ignore_ascii_case(theirs.as_os_str()))
}

/// The whole answer, for the panel's title tooltip: what the walk found, **which modules are not
/// there**, how long it took, and where every name was looked for.
///
/// All of it is here because none of it fits on a bar a few hundred points wide, and because it
/// belongs together: a location means nothing without the list of places that were tried — the
/// interesting answer is usually "it came off the third one" — and the list means nothing without
/// what it does *not* model, which is the last paragraph.
///
/// **The missing ones are named.** A count on its own — "5 missing" — is a thing the reader then has
/// to go and find, and before [`crate::pe::Graph::missing`] existed the tree would not even open the
/// branches to them. Each is named with the module that wants it and whether anything wants it up
/// front, because that last part is the difference between "this program will not start" and "one
/// feature of it will not work, later, somewhere else".
///
/// The timing is not decoration either. A program that claims to walk a thousand modules quickly
/// should be willing to be checked, and a walk that suddenly takes two seconds is how you find out
/// a `PATH` entry has gone.
pub fn about(graph: &Graph) -> String {
    /// How many of the search directories are listed. A real `PATH` has thirty entries and the
    /// ones that matter are at the front: the binary's own folder, then `System32`, then whatever
    /// was installed most recently.
    const SHOWN: usize = 10;
    /// And how many missing modules. More than this and the file is broken in a way one line each
    /// will not explain.
    const NAMED: usize = 12;
    use std::fmt::Write as _;

    let (files, api_sets, missing) = graph.tally();
    let mut text = format!(
        "{files} file{}, {api_sets} API set{}, {missing} missing \u{2014} {:.0} ms{}",
        if files == 1 { "" } else { "s" },
        if api_sets == 1 { "" } else { "s" },
        graph.micros as f64 / 1000.0,
        if graph.truncated {
            "\nThe walk stopped at its budget or its patience, so this is incomplete."
        } else {
            ""
        }
    );

    // Which ones, and who wants them. `upfront` is the graph's own answer to "is this opened before
    // `main`" — one rule, asked here and by the flat list, which were deriving it two different ways
    // and disagreeing about the same module.
    let chains = graph.missing();
    if !chains.is_empty() {
        let upfront = graph.upfront();
        text.push_str("\n\nNot found:");
        for chain in chains.iter().take(NAMED) {
            let Some(&at) = chain.last() else { continue };
            let by = chain
                .get(chain.len().wrapping_sub(2))
                .map(|&from| graph.modules[from].name.as_str())
                .unwrap_or("this file");
            let _ = write!(
                text,
                "\n    {} \u{2014} wanted by {by}{}",
                graph.modules[at].name,
                if upfront[at] { ", at start-up" } else { ", delay-loaded" }
            );
        }
        if chains.len() > NAMED {
            let _ = write!(text, "\n    …and {} more", chains.len() - NAMED);
        }
    }

    text.push_str("\n\nLooked for in order:");
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

/// Where the three areas go on a canvas of this shape, and how much of it each takes.
///
/// **The symbol panels are a column down the right-hand side.** That is where they are unless the
/// canvas cannot hold one — the tree needs [`TREE_MIN_W`] and a panel needs [`symbols::MIN_W`], and
/// [`BESIDE_AT`] is the width at which the two fit *comfortably* rather than merely fit. Under that
/// they go in a band along the bottom instead, and under both floors they are not drawn at all: the
/// tree is what this panel is for, so it is never the thing that gives way.
///
/// **How much they take is dragged, not decided here.** `share` is the column's share of the width
/// (or the band's share of the height), and `split` is how the two panels divide what that leaves —
/// both out of [`crate::ui::preview::Layout`], both clamped so that neither area can be dragged under
/// its floor, and both persisted, because a panel that had to be re-dragged every time it opened
/// would be a panel nobody dragged.
struct Places {
    tree: Rect,
    /// What the module above the picked one uses out of it. The first to go when there is only room
    /// for one panel: it is the one that is not there in the flat list either, so it is the one the
    /// reader is not relying on being present.
    used: Option<Rect>,
    exports: Option<Rect>,
    /// The two seams, which are also the two grips: the one between the tree and the panels, and the
    /// one between the panels. `None` where that boundary does not exist.
    seams: [Option<Seam>; 2],
}

/// A boundary between two areas: where it is drawn, and which way a drag on it moves.
#[derive(Clone, Copy)]
struct Seam {
    at: Rect,
    /// Whether dragging it moves left and right. A *vertical* line is dragged horizontally, so this
    /// is about the gesture rather than about the line.
    sideways: bool,
    /// The length of the axis the share is a fraction of.
    span: f32,
    /// How far the share may travel before an area is under its floor.
    bounds: (f32, f32),
}

fn places(rect: Rect, want_exports: bool, want_used: bool, sizes: Sizes) -> Places {
    let mut out = Places {
        tree: rect,
        used: None,
        exports: None,
        seams: [None, None],
    };
    // Nothing is picked, so there is nothing for either panel to be about and the tree has the
    // canvas. The one case that is not about how much room there is.
    if !want_exports {
        return out;
    }

    // Which arrangements the canvas can hold at all, and then which of them it is wide enough for.
    let beside = rect.width() - symbols::MIN_W - SEAM >= TREE_MIN_W && rect.height() >= symbols::MIN_H;
    let under = rect.height() - symbols::MIN_H - SEAM >= TREE_MIN_H && rect.width() >= symbols::MIN_W;
    if !beside && !under {
        return out;
    }

    // The column down the right-hand side, or a band along the bottom for a canvas too narrow for
    // one. Either way it is the same cut on the other axis, which is what [`cut`] is for — with the
    // floors of *that* axis, the tree's being 280 points one way and four rows the other.
    let sideways = beside && (rect.width() >= BESIDE_AT || !under);
    let (mine, theirs) = if sideways {
        (TREE_MIN_W, symbols::MIN_W)
    } else {
        (TREE_MIN_H, symbols::MIN_H)
    };
    let (tree, panels, seam_) = cut(rect, sideways, mine, theirs, sizes.share);
    out.tree = tree;
    out.seams[0] = Some(seam_);

    // And the two panels inside it: stacked when the panels are a column — used above exports, the
    // order Dependency Walker puts them in and the order they are read in — side by side when they are
    // a band, there being no height to stack them in.
    let (floor, room) = if sideways {
        (symbols::MIN_H, panels.height())
    } else {
        (symbols::MIN_W, panels.width())
    };
    if want_used && room >= floor * 2.0 + SEAM {
        let (used, exports, seam_) = cut(panels, !sideways, floor, floor, 1.0 - sizes.split);
        out.used = Some(used);
        out.exports = Some(exports);
        out.seams[1] = Some(seam_);
    } else {
        out.exports = Some(panels);
    }
    out
}

/// Divide an area in two along one axis, and describe the boundary.
///
/// `share` is the *second* piece's share of what is left after the seam, clamped so that neither piece
/// is under its floor — and the clamp is in the same terms the grip drags in, which is the point of
/// doing this in one place: four copies of `(floor / room, (room - floor) / room)` were four chances
/// for a seam that could be dragged past a floor.
fn cut(area: Rect, sideways: bool, first: f32, second: f32, share: f32) -> (Rect, Rect, Seam) {
    let span = if sideways { area.width() } else { area.height() };
    let room = span - SEAM;
    // `max` on the ceiling and not on both: an area too small for the two floors together would put
    // the ceiling under the floor, and `f32::clamp` panics on that rather than picking one. The caller
    // does not ask for a cut it has no room for — [`places`] returns early instead — and this is here
    // so that getting that wrong is a cramped panel rather than a crash. It was: the floors of the
    // *width* were handed to a cut down the height, and 190 of 299 against 19 of 299 panicked.
    let bounds = (second / room, ((room - first) / room).max(second / room));
    let size = (room * share.clamp(bounds.0, bounds.1)).round();
    let at = if sideways {
        area.right() - size
    } else {
        area.bottom() - size
    };
    let (a, b, line) = if sideways {
        (
            Rect::from_min_max(area.min, pos2(at - SEAM, area.bottom())),
            Rect::from_min_max(pos2(at, area.top()), area.max),
            Rect::from_min_max(pos2(at - SEAM, area.top()), pos2(at, area.bottom())),
        )
    } else {
        (
            Rect::from_min_max(area.min, pos2(area.right(), at - SEAM)),
            Rect::from_min_max(pos2(area.left(), at), area.max),
            Rect::from_min_max(pos2(area.left(), at - SEAM), pos2(area.right(), at)),
        )
    };
    (
        a,
        b,
        Seam {
            at: line,
            sideways,
            span: room,
            bounds,
        },
    )
}

/// One seam, drawn and dragged: the same gesture as the panel's own edge, the sidebar's splitter and
/// the column edges in the listing — drag to size, double click to put it back.
///
/// Reaching a few points either side of the one-point line, because a one-point grab target is a
/// one-point grab target. Whether the share moved, which is what the caller remembers.
fn grip(ui: &mut Ui, t: &Theme, seam_: Seam, id: Id, share: &mut f32, default: f32) -> bool {
    let reach = 3.0;
    let band = if seam_.sideways {
        seam_.at.expand2(vec2(reach, 0.0))
    } else {
        seam_.at.expand2(vec2(0.0, reach))
    };
    ui.painter()
        .rect_filled(seam_.at, egui::CornerRadius::ZERO, seam(t));
    let response = ui.interact(band, id, Sense::click_and_drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(if seam_.sideways {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::ResizeVertical
        });
    }
    let mut moved = false;
    if response.dragged() {
        // A share rather than points, so the panels keep their proportions when the pane is resized
        // — the same reasoning as the preview panel's own grip, which says it at length.
        //
        // Negated because the share is the size of the piece *after* the seam: dragging the boundary
        // right or down makes that piece smaller. The caller that measures the piece before it — the
        // split between the two panels — passes its share the other way up, which is where the
        // `1.0 - split` in [`places`] comes from.
        let along = if seam_.sideways {
            response.drag_delta().x
        } else {
            response.drag_delta().y
        };
        let was = *share;
        *share = (*share - along / seam_.span.max(1.0)).clamp(seam_.bounds.0, seam_.bounds.1);
        moved = *share != was;
    }
    if response.double_clicked() {
        moved = *share != default;
        *share = default;
    }
    moved
}

/// Draw the tree — and the panels about the picked row — at `rect`, and answer the pointer.
///
/// `id` is the tile this is in, which the three search boxes and the two grips are named after: a
/// widget's id has to be the same one next frame or the box loses the keyboard mid-word, and a rect
/// is not that — a panel being dragged has a different rect every frame, which is exactly when the
/// grip must not change its mind about what it is.
///
/// `layout` is the window's preferences: which of the two views the tree is in, and how much room the
/// panels have. Returns whether a drag moved either share, so the caller can have it remembered.
pub fn show(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    id: Id,
    view: &mut View,
    sizes: &mut Sizes,
    out: &mut Vec<Action>,
) {
    view.sync(sizes.list);

    // The panels, and therefore the shape of the tree, depend on what is picked. The `used` one
    // needs an importer to be about, which the flat list does not have and the root never has.
    let picked = view.picked;
    let places = places(
        rect,
        picked.is_some(),
        picked.is_some_and(|pick| !view.flat() && pick.from.is_some()),
        *sizes,
    );
    if places.exports.is_some() {
        view.ensure_symbols();
    }

    tree(ui, t, places.tree, id, view);

    // The seams, which are also the grips. Drawn after the tree so that a seam is never under the
    // scroll bar the tree keeps at its own right-hand edge.
    //
    // The first seam's share is the panels' and the second's is the *upper* panel's, so the two are
    // dragged in opposite directions — which is why the split goes in and comes back inverted. See
    // [`grip`], whose one rule is that a share measures the piece after the seam.
    let mut moved = false;
    for (which, seam_) in places.seams.iter().enumerate() {
        let Some(seam_) = seam_ else { continue };
        let id = id.with(("deps-grip", which));
        moved |= if which == 0 {
            grip(ui, t, *seam_, id, &mut sizes.share, SHARE)
        } else {
            let mut room = 1.0 - sizes.split;
            let moved = grip(ui, t, *seam_, id, &mut room, 1.0 - SPLIT);
            sizes.split = 1.0 - room;
            moved
        };
    }
    if moved {
        out.push(Action::RememberLayout);
    }

    // Borrowed after the tree has been drawn, because the tree takes the view mutably — a pick made
    // by this frame's click lands in it, and the panels are about the pick as it was when the rows
    // were painted.
    let View {
        symbols,
        find_used,
        find_exports,
        ..
    } = view;
    if let Some(both) = symbols {
        if let Some(at) = places.exports {
            symbols::exports(ui, t, at, id.with("deps-exports"), both, find_exports);
        }
        if let Some(at) = places.used {
            symbols::used(ui, t, at, id.with("deps-used"), both, find_used);
        }
    }
}

/// The rows, and the pointer.
fn tree(ui: &mut Ui, t: &Theme, whole: Rect, id: Id, view: &mut View) {
    // The tree's own strip: how many modules there are — which is the one number a dependency panel
    // is *about* — and the box that narrows them. See [`View::find_modules`] for what a query does to
    // a tree, which is not what it does to a list.
    let flat = view.flat();
    let strip = strip(
        ui,
        t,
        whole,
        Some((id.with("deps-modules"), &mut view.find_modules)),
    );
    // Rebuilt here rather than next frame, so the rows under the box are the rows the box asks for.
    view.sync(flat);
    // **Modules and not rows**, and the two differ: a module is a row wherever it is imported, so a
    // tree of 746 rows is 937 modules seen more than once. The number here is the graph's, which is
    // the number the panel's own title reports, and the one a search narrows.
    //
    // Counted after the box has been read, so the number is this keystroke's rather than the last
    // one's. See [`strip`], which hands back the room for exactly that reason.
    let all = view.graph.modules.len();
    caption(
        ui,
        t,
        strip.caption,
        &match view.matched {
            Some(found) => format!("{found} of {all} modules"),
            None => format!("{all} module{}", if all == 1 { "" } else { "s" }),
        },
    );

    let rect = strip.body;
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
    let mut took = None;
    let scroll = egui::ScrollArea::vertical()
        .id_salt(id.with("deps-rows"))
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
        let response = ui.interact(visible, id.with("deps-hit"), Sense::click());
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

            // The picked row wears the listing's own selection: the accent fill and the bar down
            // its left edge, which is what this window means by "selected" everywhere else.
            let picked = view.is_picked(row);
            if let Some(fill) = crate::ui::row_fill(t, picked, hovered == Some(at)) {
                ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, fill);
            }
            if picked {
                crate::ui::selection_bar(ui.painter(), rect, t);
            }

            let x = twisty_x(rect.left(), row.depth);

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
                    if over || picked {
                        t.text.primary
                    } else {
                        t.text.secondary
                    },
                );
            }
            let mut x = twisty.right() + PAD;

            // The two inks everything in this row is in, which on a picked row are one ink. See
            // [`inks`], where the measurement is.
            let (quiet, loud) = inks(t, picked);

            // The glyph, which is where the module's state is said first: a binary, or the error
            // mark for something that is not there or cannot be read.
            //
            // **A missing module that is delay-loaded gets the mark in a neutral ink**, because
            // it is not a fault: Windows ships stubs for features that are not installed, and
            // nothing opens a delay-loaded DLL until something calls into it. See
            // `pe::Graph::upfront`, which is the other half of that and finds none of the five this
            // program's own binary is missing.
            let (glyph, ink): (azur_egui_theme::icons::Icon<'_>, Color32) = match module.state {
                State::Found => (&icons::executable, t.executable),
                State::ApiSet => (&icons::executable, quiet),
                State::Missing if row.delayed => (&azur_icons::error, quiet),
                State::Missing | State::Unreadable(_) => (&azur_icons::error, loud),
                State::Unvisited => (&azur_icons::ellipsis, quiet),
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
            let name_color =
                ink_for(quiet, loud, t.text.primary, module.state, row.delayed || row.faint);
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
            let (where_, bad) = location_of(module, row);
            let galley = truncated(
                ui.painter(),
                &where_,
                t.fonts.caption.clone(),
                if bad { loud } else { quiet },
                (location_right - location_x).max(0.0),
            );
            galley_on_baseline(ui.painter(), location_x, baseline, galley);

            // ---- The processor ----
            //
            // In the danger colour when it is not the root's, because Windows will not load
            // it — which is the second most useful thing this tree can tell you.
            if module.machine != 0 {
                let galley = truncated(
                    ui.painter(),
                    pe::machine_name(module.machine),
                    t.fonts.caption.clone(),
                    if graph.foreign(row.module) { loud } else { quiet },
                    ARCH,
                );
                let width = galley.size().x;
                galley_on_baseline(ui.painter(), arch_right - width, baseline, galley);
            }
        }

        // ---- What a click means ----
        //
        // **The expander folds; the row picks.** Which it has to be, now that there are two things
        // a row can do: a click anywhere used to fold, and that was the whole vocabulary the tree
        // had. A double click folds as well, wherever it lands, because that is what a tree does on
        // this platform and because the expander is a ten-point target.
        //
        // Answered after the loop so that the row that was drawn is the row that was clicked.
        if let Some(at) = hovered.filter(|_| response.clicked() || response.double_clicked()) {
            if let Some(row) = view.rows.get(at) {
                let on_twisty = response.interact_pointer_pos().is_some_and(|pos| {
                    // A little past the glyph's own right edge, because a 10-point target is small
                    // and the space after it belongs to nothing else.
                    pos.x < twisty_x(visible.left(), row.depth) + TWISTY + PAD
                });
                if row.expandable && (on_twisty || response.double_clicked()) {
                    toggled = Some(row.module);
                } else {
                    took = Some(Pick {
                        module: row.module,
                        from: row.from,
                    });
                }
            }
        }
    });

    if let Some(module) = toggled {
        view.toggle(module);
    }
    if let Some(pick) = took {
        // A second click on the picked row puts it away, which is the only way back to a tree with
        // the whole canvas to itself.
        view.picked = (view.picked != Some(pick)).then_some(pick);
    }
}

/// The two inks anything in a row is drawn in: the quiet one, and the one that means *this is
/// wrong* — and **on a picked row they are one ink**.
///
/// Which is measured rather than a simplification. The fill under a picked row is the accent, the
/// same surface a selected row in the listing wears, and on it `status-danger` is **2.33:1** in the
/// dark theme and 1.77:1 with the pointer over it — under the 3:1 a *shape* needs, let alone ink.
/// `text-secondary` is 3.31:1, under the 4:1 this tree holds itself to. So neither is available
/// there, and the row goes to `text-primary` throughout: see
/// `every_ink_in_the_tree_can_be_read`, which measures all four surfaces and asserts both halves.
///
/// Nothing is lost by it. The error *mark* beside the name is still the error mark, the location
/// column still reads `not found`, and the row you have picked is the one row whose state you have
/// just read — it is the two hundred rows you have not picked that need the colour.
fn inks(t: &Theme, picked: bool) -> (Color32, Color32) {
    if picked {
        (t.text.primary, t.text.primary)
    } else {
        (t.text.secondary, t.status.danger)
    }
}

/// What ink a module's name is in, out of the pair [`inks`] gave the row.
///
/// Takes the pair rather than the theme and the row's state: on a picked row the two are one ink and
/// every arm here returns it, so a version that took `picked` had a branch that could not fire and
/// computed the pair a second time to prove it.
///
/// `quiet` covers the row a search kept only because something under it matched, for the same reason
/// it covers a delay-loaded one — neither is the answer.
fn ink_for(quiet: Color32, loud: Color32, plain: Color32, state: State, quietly: bool) -> Color32 {
    match state {
        State::Missing if !quietly => loud,
        State::Missing | State::ApiSet | State::Unvisited => quiet,
        _ if quietly => quiet,
        // The one text in the tree that is fully inked. On a picked row all three are one colour, which
        // is what [`inks`] is for.
        _ => plain,
    }
}

/// What goes in the Location column, and whether it is saying something is wrong.
///
/// The ink is the caller's, out of [`inks`] — this decides *which of the two*, and not what either
/// of them is, because that answer depends on the row's surface rather than on the module.
/// `Cow`, because this is called for every visible row on every frame and all but one of its answers
/// is a literal: the folder is borrowed straight out of the module on Windows, where a path that is
/// valid UTF-8 costs `to_string_lossy` nothing, and the six other arms borrow a `&'static str`.
fn location_of<'a>(module: &'a pe::Module, row: &Row) -> (Cow<'a, str>, bool) {
    if row.cyclic {
        // Already above this row: the branch stops, and saying why is better than a leaf that
        // looks like it has nothing under it.
        return ("(already above)".into(), false);
    }
    match module.state {
        State::Found => match module.path.as_deref().and_then(Path::parent) {
            Some(folder) => (folder.to_string_lossy(), false),
            None => ("".into(), false),
        },
        // Not a file, and the honest answer is what resolves it rather than a path.
        State::ApiSet => ("resolved by the API set schema".into(), false),
        // Delay-loaded and missing is worth saying in full, because it is the difference between
        // "this will not run" and "one feature will not work, later, somewhere else".
        State::Missing if row.delayed => ("not found — delay-loaded".into(), false),
        State::Missing => ("not found".into(), true),
        State::Unreadable(why) => (why.into(), true),
        State::Unvisited => ("not visited — the walk stopped".into(), false),
    }
}

#[cfg(test)]
mod tests;
