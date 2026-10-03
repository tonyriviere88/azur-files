//! The two panels beside the tree: what a module **offers**, and what its importer **takes**.
//!
//! Both are about the row that is picked, and they are not two views of one list — that is the whole
//! reason there are two of them. `kernel32.dll` exports about seventeen hundred symbols; the program
//! that imports it uses forty. The export table answers "what is in here", and only the importing
//! binary's own table answers "what does this dependency actually exist for", which is the question
//! that makes a dependency graph worth walking.
//!
//! # Read on the click, and once
//!
//! Not during the walk. A graph of two hundred modules is a couple of million symbols and a hundred
//! megabytes of names, all to answer a question asked about one module at a time — so
//! [`crate::pe::exports`] and [`crate::pe::imported_from`] open the file again when a row is picked,
//! and [`Both`] holds the answer until a different row is. What that costs is a handful of reads and
//! one 16 KB window per table; see [`crate::pe`]'s `WINDOW`, which exists because doing it the
//! obvious way — one read per name — was forty milliseconds on a big DLL and a dropped frame on the
//! click that asked for it.
//!
//! # A row is prepared, not formatted while it is drawn
//!
//! Every row is a [`Line`]: three strings worked out once, when the module is picked. That is not
//! tidiness — two of the three are things that must not be done per frame. **Demangling** is a call
//! into `dbghelp` and an 8 KB buffer; and the **order** the rows are in is the order of the demangled
//! names, which cannot be known until they have been demangled. Drawing a scrolled panel is then a
//! slice of `shown` and nothing else, and so is the caption above it — see [`List::narrow`].
//!
//! # Denser rows than the tree, and monospaced
//!
//! 18 points against the tree's 24, with the caption font in them. The tree's rows carry a glyph and
//! a 14-point name and are the thing you scan; these are a *table*, read by looking down one column
//! for one name, and every point of row height is a symbol that did not fit in a panel that is
//! already the small half of a small panel.
//!
//! Monospaced, because the two things in these rows that are read character by character are a
//! C++ name and an address — `t.fonts.mono` is documented as being for exactly that, "paths, ids,
//! anything where alignment carries meaning".

use egui::{pos2, vec2, FontId, Id, Rect, Ui};

use azur_egui_theme::components::{galley_on_baseline, ink_baseline};

use crate::pe::{self, Graph, State};
use crate::theme::Theme;
use crate::ui::{text_center, truncated};

use super::{Pick, PAD};

/// One symbol. See the module header for why it is not the tree's row height.
pub const ROW: f32 = 18.0;

/// The smallest panel worth drawing.
///
/// **Both numbers are a floor on the answer rather than on the furniture.** A panel 100 points wide
/// holds `?Exec@App3` and an ellipsis, which is not a symbol name; a panel two rows tall shows two of
/// the seventeen hundred names in `kernel32.dll`, which is not a list. Below either, the tree gets
/// the room back and the panel is not drawn at all — see [`super::places`].
pub const MIN_W: f32 = 190.0;
pub const MIN_H: f32 = super::HEAD + ROW * 3.0;

/// The two columns to the right of a name.
///
/// `TAG_W` is separate from `TAIL_W` because it holds one short word rather than a ten-character
/// address: at 76 points it was taking 30 points off the name column for nothing, and the name is the
/// column that matters.
const NUMBER_W: f32 = 46.0;
const TAIL_W: f32 = 76.0;
const TAG_W: f32 = 46.0;

/// What the tail column grows to when there is room, and the width at which it does.
///
/// It only matters for forwarders, and only they need it: an address is ten characters and a
/// forwarding target is `api-ms-win-core-heap-l2-1-0.HeapAlloc`. Two tiers rather than a share of the
/// width, so the column does not move as the panel is resized past nothing in particular.
const TAIL_WIDE: f32 = 168.0;
const WIDE_AT: f32 = MIN_W + TAIL_WIDE;

/// One row, ready to draw. See the module header for why it is prepared rather than formatted.
struct Line {
    /// What the row says: the demangled name, the raw one when it is not a C++ name, or `#42` for a
    /// reference that named no name at all. Also what a search is asked about, which is the point —
    /// what can be read is what can be found.
    name: String,
    /// The number beside it — an export's `#ordinal`, an import's `@hint` — or nothing.
    number: String,
    /// The right-hand column: an entry point, what a forwarder really points at, or `delayed`.
    tail: String,
}

/// A C++ name as a person reads it, or the raw one for everything that is not one.
///
/// **This is where the shape of the name is decided**, and it is a decision about a row rather than
/// about a binary — which is why the flag set it asks [`pe::demangle`] for leaves off the calling
/// convention, the return type and the access specifier. Two reasons, and the second decided it:
///
/// - A panel 300 points wide holds about forty characters. `public: int __cdecl` is nineteen of them
///   spent before the name starts, on every row.
/// - **It is what makes the list sortable.** The rows are ordered by what they say, so a name that
///   began with its access specifier would sort every `public:` in the DLL together and leave the
///   class names — the thing the eye is going down the column for — in no order at all.
///
/// Dependency Walker shows the whole declaration, and it has a resizable column of its own for it.
fn readable(raw: String) -> String {
    // By value: the name is already owned and about to be dropped, and the not-a-C++-name path is the
    // common one — for `kernel32.dll` it is every one of seventeen hundred.
    pe::demangle(&raw).unwrap_or(raw)
}

/// What the right-hand column of a list holds, which is what its width is decided from.
///
/// **A fact about the list rather than about a row**, and that is the point: a row that skipped the
/// column would hand its name the points the rows above and below gave up, and the numbers beside
/// them would step left and right down the panel. So the whole list decides, and one of the two
/// panels usually decides against — an import table with nothing delay-loaded in it is the ordinary
/// case, and the width goes to the names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tail {
    /// Nothing in it: no row has anything to put there.
    None,
    /// One short word — `delayed`.
    Tag,
    /// An entry point, or what a forwarder points at. The one that grows with the panel.
    Address,
}

/// One panel's rows, which of them the search has left, and what its strip says about that.
pub struct List {
    lines: Vec<Line>,
    /// Indices into `lines`, rebuilt only when the query changes.
    shown: Vec<usize>,
    /// The query `shown` and `caption` were built for, so it is known when they are stale.
    narrowed_for: String,
    /// What the strip says: the label and the count. Kept beside the count it describes rather than
    /// formatted per frame, for the same reason `shown` is — a caption is two allocations and the
    /// number in it changes only when a query does.
    caption: String,
    label: &'static str,
    /// Whether the table was cut off at [`pe::MAX_SYMBOLS`], which the caption then says out loud: a
    /// listing missing rows and not admitting it is worse than no listing, which is the rule this
    /// program holds itself to for a truncated walk and holds itself to here.
    cut_off: bool,
    /// Whether any row has a number, for the reason [`Tail`] gives about its own column.
    numbers: bool,
    tail: Tail,
}

impl List {
    fn new(label: &'static str, lines: Vec<Line>, tail: Tail) -> Self {
        let numbers = lines.iter().any(|line| !line.number.is_empty());
        let tail = if lines.iter().all(|line| line.tail.is_empty()) {
            Tail::None
        } else {
            tail
        };
        let mut list = Self {
            shown: (0..lines.len()).collect(),
            // Not the empty query: that is what `narrow` will be asked for first, and the guard there
            // would then skip the caption this has not built yet.
            narrowed_for: "\u{0}".to_owned(),
            caption: String::new(),
            label,
            cut_off: lines.len() >= pe::MAX_SYMBOLS,
            numbers,
            tail,
            lines,
        };
        list.narrow("");
        list
    }

    /// Keep the rows whose name the query keeps, and say how many that is.
    ///
    /// [`azur_egui_theme::filter::Query`] rather than a substring test of its own, so that a search
    /// box in this window means one thing wherever it is: every word must match, `!` excludes, `^` and
    /// `$` hold an end. It also folds once at `parse` and takes an ASCII byte path per candidate,
    /// where the obvious spelling — `name.to_lowercase().contains(..)` — allocates a `String` per row.
    ///
    /// Guarded on the query, because both the walk and the caption cost more than the comparison: a
    /// panel showing seventeen hundred names is asked this every frame and does the work on the frames
    /// that carry a keystroke.
    fn narrow(&mut self, query: &str) {
        if self.narrowed_for == query {
            return;
        }
        let filter = azur_egui_theme::filter::Query::parse(query);
        self.shown.clear();
        if filter.is_empty() {
            self.shown.extend(0..self.lines.len());
        } else {
            self.shown.extend(
                self.lines
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| filter.matches(&line.name))
                    .map(|(at, _)| at),
            );
        }
        self.narrowed_for = query.to_owned();
        self.caption = format!(
            "{} \u{2014} {}{}",
            self.label,
            if self.shown.len() == self.lines.len() {
                format!("{}", self.lines.len())
            } else {
                format!("{} of {}", self.shown.len(), self.lines.len())
            },
            if self.cut_off { ", cut off" } else { "" }
        );
    }
}

/// Both lists for one picked module, read once.
pub struct Both {
    /// Which pick this is the answer to, so a list is never drawn against a different row. The panel
    /// re-reads when this stops matching.
    pub of: Pick,
    /// What the picked module exports, ordered by name. `Err` with why for anything there is no
    /// export table to read — and there are five separate ways for that to be true, which is why it
    /// is a `Result` rather than an empty list.
    exports: Result<List, &'static str>,
    /// What the module above it in the tree uses out of it, in import-table order.
    used: Result<List, &'static str>,
}

impl Both {
    /// Open both files, read both tables, and prepare every row.
    pub fn read(graph: &Graph, of: Pick) -> Self {
        let module = &graph.modules[of.module];
        let exports = match (&module.path, module.state) {
            (Some(path), State::Found) => pe::exports(path),
            // Every other state is a reason there is nothing to read, and saying which one is the
            // whole content of the panel in that case.
            (_, State::ApiSet) => {
                Err("An API set is a name in a schema, not a file with symbols in it.")
            }
            (_, State::Missing) => Err("Not found, so there is no export table to read."),
            (_, State::Unreadable(why)) => Err(why),
            (_, State::Unvisited) => Err("The walk stopped before reaching it."),
            (None, State::Found) => Err("Found, but with no path to read it back from."),
        };
        let exports = exports.map(|list| {
            let mut lines: Vec<Line> = list.into_iter().map(offered).collect();
            // Ordered by name, which is a departure from the file's own order and the right one
            // here: a list of seventeen hundred symbols is searched for a name, and ordinal order
            // makes that a scan of the whole panel. By the *demangled* name, which is the one on
            // screen — see [`readable`], whose flag set is chosen for exactly this.
            lines.sort_by(|a, b| crate::fs::sort::natural_cmp(&a.name, &b.name));
            List::new("Exports", lines, Tail::Address)
        });

        let used = match of.from.map(|from| &graph.modules[from]) {
            // The importing module's *own* file is what holds this answer, so the failure here is
            // about that file and not about the one that is picked.
            Some(from) => match &from.path {
                Some(path) => pe::imported_from(path, &module.name),
                None => {
                    Err("The module above it is not on disk, so its import table cannot be read.")
                }
            },
            None => Err("Nothing above it: this is the binary the walk started from."),
        };
        // The label is a fact about the pick, so it is settled here rather than rebuilt per frame.
        let label = match of.from {
            Some(from) => format!("Used by {}", graph.modules[from].name),
            None => "Used by".to_owned(),
        };
        let used = used.map(|list| {
            // Left in import-table order, which is the order the loader binds them in and the only
            // order this list has ever had. It is short enough to read.
            List::new(
                // Leaked, because the two labels a `List` can carry are a `&'static str` and this
                // one, and the alternative is an owned `String` on every list for the sake of one.
                // A `Both` is built per pick and lives until the next one; this is a few dozen bytes
                // per picked row, once.
                Box::leak(label.into_boxed_str()),
                list.into_iter().map(taken).collect(),
                Tail::Tag,
            )
        });

        Self { of, exports, used }
    }

    /// How many of each, or `None` where there was nothing to read. For the tests; the captions read
    /// the lists.
    #[cfg(test)]
    pub fn counts(&self) -> (Option<usize>, Option<usize>) {
        (
            self.exports.as_ref().ok().map(|list| list.lines.len()),
            self.used.as_ref().ok().map(|list| list.lines.len()),
        )
    }

    /// What the exports panel would draw for this query, in the order it would draw it. For the
    /// tests, which is the only way to ask the search anything without a frame to draw it in.
    #[cfg(test)]
    pub fn exports_shown(&mut self, query: &str) -> Vec<&str> {
        let Ok(list) = &mut self.exports else {
            return Vec::new();
        };
        list.narrow(query);
        list.shown
            .iter()
            .filter_map(|&at| list.lines.get(at))
            .map(|line| line.name.as_str())
            .collect()
    }

    /// And the same for the used panel, whose rows are the ones that carry a tag.
    #[cfg(test)]
    pub fn used_shown(&mut self, query: &str) -> Vec<(&str, bool)> {
        let Ok(list) = &mut self.used else {
            return Vec::new();
        };
        list.narrow(query);
        list.shown
            .iter()
            .filter_map(|&at| list.lines.get(at))
            .map(|line| (line.name.as_str(), !line.tail.is_empty()))
            .collect()
    }
}

/// One row of the exports list.
///
/// **The ordinal and the address, and nothing else.** The hint was here too and came out: three
/// numbers beside a name is a table of numbers with a name in it, and of the three the hint is the one
/// that answers a question nobody asks of this panel — it is the *importer's* record of where a name
/// sat, so it earns a column in the list of what an importer uses and not in the list of what a module
/// offers.
fn offered(export: pe::Export) -> Line {
    Line {
        name: match export.name {
            Some(name) => readable(name),
            // Not nameless in the file — it has no entry in the *name* table, which is a different
            // thing and worth saying rather than leaving a blank row.
            None => "(by ordinal only)".to_owned(),
        },
        number: format!("#{}", export.ordinal),
        tail: match export.bound {
            // A forwarder has no entry point: what the address field holds is the `DLL.Function` it
            // really points at, which is what belongs in this column.
            pe::Bound::Forward(to) => format!("\u{2192} {to}"),
            pe::Bound::Entry(address) => format!("{address:#010x}"),
        },
    }
}

/// One row of the used list.
fn taken(symbol: pe::Symbol) -> Line {
    Line {
        // Imported by name, or by ordinal — and in the second case the number *is* the whole
        // reference, so it goes where the name would have been.
        name: match (symbol.name, symbol.ordinal) {
            (Some(name), _) => readable(name),
            (None, Some(ordinal)) => format!("#{ordinal}"),
            (None, None) => "(unreadable)".to_owned(),
        },
        // The hint: the exporting module's name-table index as this binary was built against it.
        // Worth a column here because a hint that no longer matches the DLL on disk is how a binary
        // built against another version of it shows up.
        number: symbol.hint.map(|hint| format!("@{hint}")).unwrap_or_default(),
        // **Delay-loaded per symbol and not per module**, which is why it is said here as well as on
        // the tree's row: a binary can import from one DLL both ways at once, and this is the panel
        // where that is visible.
        tail: if symbol.delayed {
            "delayed".to_owned()
        } else {
            String::new()
        },
    }
}

/// What the picked module exports.
pub fn exports(ui: &mut Ui, t: &Theme, rect: Rect, id: Id, both: &mut Both, query: &mut String) {
    match &mut both.exports {
        Ok(list) => panel(ui, t, rect, id, list, query),
        Err(why) => empty(ui, t, rect, "Exports", why),
    }
}

/// What the module above the picked one uses out of it.
pub fn used(ui: &mut Ui, t: &Theme, rect: Rect, id: Id, both: &mut Both, query: &mut String) {
    match &mut both.used {
        Ok(list) => panel(ui, t, rect, id, list, query),
        Err(why) => empty(ui, t, rect, "Used by", why),
    }
}

/// One panel: the strip, then the rows in a scroll area of their own.
///
/// Rows are painted at exact rects for the reason [`super::show`] gives — item spacing would put
/// every row a little further from where the pointer thinks it is, and these two are inside a panel
/// inside a pane, where that error accumulates.
fn panel(ui: &mut Ui, t: &Theme, rect: Rect, id: Id, list: &mut List, query: &mut String) {
    // **The box first, the count second.** The caption says how many rows survived the query, and the
    // query is not known until the field has been drawn — so a caption painted before it trails what
    // was typed by one keystroke. [`super::strip`] hands back where the caption goes and this paints
    // it once the number is right.
    let strip = super::strip(ui, t, rect, Some((id, query)));
    list.narrow(query);
    super::caption(ui, t, strip.caption, &list.caption);
    if strip.body.height() < ROW {
        return;
    }
    let body = strip.body;
    if list.shown.is_empty() {
        // A search that matched nothing, which is worth saying: an empty panel under a box with
        // something typed in it reads as a panel that has broken.
        text_center(
            ui.painter(),
            body,
            t.fonts.caption.clone(),
            t.text.secondary,
            "Nothing matches",
        );
        return;
    }

    let font = mono(t);
    // The two columns' widths, decided once for the whole panel rather than per row — see [`Tail`].
    // Neither is dropped on a narrow panel any more: the panels are resizable now, so dragging one
    // wider is the answer to a cramped column, and a column that came and went with the width was a
    // table whose shape changed while you dragged it.
    let tail = match list.tail {
        Tail::None => 0.0,
        Tail::Tag => TAG_W,
        Tail::Address if rect.width() >= WIDE_AT => TAIL_WIDE,
        Tail::Address => TAIL_W,
    };
    let number = if list.numbers { NUMBER_W } else { 0.0 };

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(body.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing = egui::Vec2::ZERO;

    let scroll = egui::ScrollArea::vertical()
        .id_salt(id.with("rows"))
        .auto_shrink([false, false]);
    let count = list.shown.len();
    scroll.show_rows(&mut child, ROW, count, |ui, range| {
        let first = range.start;
        // The scroll area's right edge and not the panel's: it keeps a few points for its bar and
        // clips to what is left, so a column measured against the panel would sit under that clip.
        let edge = ui.clip_rect().right().min(body.right());
        for at in range.clone() {
            let Some(line) = list.shown.get(at).and_then(|&at| list.lines.get(at)) else {
                continue;
            };
            let row = Rect::from_min_size(
                pos2(body.left(), ui.min_rect().top() + (at - first) as f32 * ROW),
                vec2(edge - body.left(), ROW),
            );
            // Zebra stripes, because these rows are read *across*: a name on the left and a number
            // 200 points away on the right, at 12 points, with no glyph in between to hold the eye
            // on the line. `row_alt` is one step off the surface for exactly this — see the token,
            // which says why a stripe that announces itself is harder to read across, not easier.
            if at % 2 == 1 {
                ui.painter()
                    .rect_filled(row, egui::CornerRadius::ZERO, t.row_alt);
            }

            let baseline = ink_baseline(ui.painter(), &font, row.top(), ROW);
            let mut right = row.right() - PAD;
            // The tail, and then the number, right to left — each kept as a *column* whether this
            // row has anything to put in it, for the reason [`Tail`] gives.
            if tail > 0.0 {
                right = column(ui, &line.tail, &font, t.text.secondary, right, baseline, tail);
            }
            if number > 0.0 {
                right = column(
                    ui,
                    &line.number,
                    &font,
                    t.text.secondary,
                    right,
                    baseline,
                    number,
                );
            }
            let x = row.left() + PAD;
            let galley = truncated(
                ui.painter(),
                &line.name,
                font.clone(),
                t.text.primary,
                (right - PAD - x).max(0.0),
            );
            galley_on_baseline(ui.painter(), x, baseline, galley);
        }
    });
}

/// A panel with the strip and the reason there is nothing under it.
///
/// **No search box**, because there is nothing to search: a field over a list that does not exist is
/// a control that cannot do anything, which is the argument the panel's own bar makes about every
/// button it drops.
fn empty(ui: &mut Ui, t: &Theme, rect: Rect, label: &str, why: &str) {
    let strip = super::strip(ui, t, rect, None);
    super::caption(ui, t, strip.caption, label);
    let body = strip.body;
    if body.height() >= t.fonts.caption.size {
        // One line, so the reasons are written short enough to be one — `text_center` does not wrap.
        text_center(
            ui.painter(),
            body,
            t.fonts.caption.clone(),
            t.text.secondary,
            why,
        );
    }
}

/// One right-aligned column of `width`, returning where the next one to its left ends.
fn column(
    ui: &Ui,
    text: &str,
    font: &FontId,
    ink: egui::Color32,
    right: f32,
    baseline: f32,
    width: f32,
) -> f32 {
    if text.is_empty() {
        return right - width - PAD;
    }
    let galley = truncated(ui.painter(), text, font.clone(), ink, width);
    let x = right - galley.size().x;
    galley_on_baseline(ui.painter(), x, baseline, galley);
    // The *column's* left edge and not the text's, so the names to the left of it line up whatever
    // each row's number happens to be.
    right - width - PAD
}

/// The monospaced face at the caption size: see the module header for both halves of that.
fn mono(t: &Theme) -> FontId {
    FontId::new(t.fonts.caption.size, t.fonts.mono.family.clone())
}
