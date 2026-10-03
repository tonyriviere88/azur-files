//! The rows themselves: one widget for all of them, formatting into a buffer that is reused.
//!
//! Only what is visible is drawn, and nothing here asks the disk or the shell a question — every
//! cell comes off the [`crate::fs::Dir`] the scan already produced.

use super::*;

/// The glyph in a row.
///
/// 16, which is the size the shell small image list is drawn at — so a row does not
/// reflow when a painted fallback glyph is replaced by the real icon a frame later.
///
/// Reachable from [`crate::ui::grid`], which draws a tree's *folder* rows exactly as this view does
/// — the whole point of that mode is that only the files become tiles.
pub(crate) const GLYPH: f32 = 16.0;

/// Space either side of a cell's text.
pub(crate) const CELL_PAD: f32 = space::S3;

/// Two points off the top of every cell's text in a row.
///
/// Text centred in a box is centred on its *line box*, which reserves room under the
/// baseline for descenders — so a line of mostly-x-height text put beside a 16px icon
/// centred on its own ink reads two points low. Which it did: a guide drawn through the
/// icon's centre passed above the x-height of every name in the listing.
///
/// Applied to all four cells rather than to the name alone, or the columns of one row would
/// no longer sit on the same line as each other. The icon stays where it is — it is the
/// thing the text is being brought level with.
pub const CELL_LIFT: f32 = 2.0;

/// The twisty's box: the chevron that opens and shuts a folder in a
/// [`crate::pane::FlatMode::Tree`] listing.
///
/// Fourteen — [`crate::ui::TOOL_ICON`]'s size, which is what every other painted glyph in this
/// window's chrome is drawn at — in a box the height of the row. What it is *hit* on is that whole
/// box and not the chevron's ink: a 6-point arrowhead is not a click target.
pub(crate) const TWISTY: f32 = 14.0;

/// Where a row's own column starts: indented by how far down the tree it is.
///
/// `row.left() + CELL_PAD + 2.0` for every row of every other listing, where `depth` is 0 — the
/// two modes share one expression rather than branching, so an ordinary listing cannot drift from
/// the top level of a tree.
pub(crate) fn stem_x(row: Rect, depth: usize) -> f32 {
    row.left() + CELL_PAD + 2.0 + depth as f32 * INDENT
}

/// The box a tree row's twisty is drawn and clicked in: the row's own indent, one level wide.
///
/// One statement of it, because three things have to agree about where the chevron is — the
/// painting, the click, and the test that checks the click lands on it rather than beside it. A
/// test that restated the arithmetic would happily pass a layout that had drifted.
pub(crate) fn twisty_rect(row: Rect, depth: usize) -> Rect {
    Rect::from_min_size(pos2(stem_x(row, depth), row.top()), vec2(TWISTY, ROW_HEIGHT))
}

/// How far one level of a tree steps in: **exactly the twisty's width.**
///
/// So a child's chevron sits under its parent's icon, and the chevrons down one branch make a
/// single straight column rather than a stagger. Any other number and the ladder wanders.
///
/// It also keeps the step cheap, which matters because the indent comes out of the Name column and
/// nothing else: a tree eight folders deep at Explorer's own 19-point step would have spent 152
/// points before the first letter of a name.
pub(crate) const INDENT: f32 = TWISTY;

/// How big a git badge is, and where on the row's icon it sits.
///
/// Eleven points of a sixteen-point icon, in the bottom-left corner: the shell's own proportion and
/// the shell's own corner — Windows puts its sync overlays there, TortoiseSVN puts its ticks there,
/// so a badge here lands where an eye trained on Explorer already looks. It is also the emptiest
/// corner of every glyph in [`crate::icons`], the page's fold being at the top right.
///
/// **Inside the icon, not beside it.** Hanging it off the left edge is what the first version did, and
/// at the left edge of a row is where the selection bar lives and where the pane's own border is a few
/// points further on — so the badge read as clipped even when it was not.
pub(crate) const BADGE: f32 = 10.0;

pub(crate) fn badge_rect(icon: Rect) -> Rect {
    Rect::from_min_size(
        pos2(icon.left() - 1.0, icon.bottom() - BADGE + 3.0),
        egui::Vec2::splat(BADGE),
    )
}

/// A glyph on the status line: the branch, an arrow, the tick, the limit's warning.
///
/// Eleven rather than the 14 a toolbar uses. These sit *in* a line of 12-point text rather than in a
/// button of their own, and a glyph taller than the capitals beside it reads as an icon that has
/// wandered in.
pub(crate) const MARK: f32 = 11.0;

/// What separates two of the greys on the right of the status line.
///
/// Air alone is not enough between `0.3 ms` and `1016 KB`: two figures in the same ink with a gap
/// between them read as one phrase. The counts need no separator in front of them — they are a
/// different colour, which is a stronger boundary than any character.
pub(crate) const SEPARATOR: &str = "   ·   ";

/// How far above the middle of the bar everything on it sits.
///
/// **Asked for by eye, and it is the last point of an argument arithmetic cannot finish.** The line is
/// centred by its ink rather than by its line box — see [`status_geometry`], which is what stopped it
/// reading a point and a half low at 1.5× — and one point above *that* is where it was wanted. The
/// bar's bottom edge is not its bottom edge: the window's own 1px border is drawn over it, and
/// `background-layer-alt` against `stroke-default` is a boundary the eye reads as the end of the band
/// while the arithmetic does not.
///
/// It moves the whole line and not only the words: the switch, the glyphs and the text all go up
/// together, or the level they were brought to would be given away a point at a time.
pub(crate) const NUDGE: f32 = 1.0;

/// The least air between the status line's two groups.
///
/// **Wider than a separator, because it is a bigger boundary than one.** Without it a narrow pane
/// brought `13 changed` and `0.3 ms` to within eight points of each other and the line read as one
/// long phrase in three colours. The right-hand group gives way instead — that is what having
/// priority means — so what is on the bar is always either separated or absent.
pub(crate) const GROUP_GAP: f32 = space::S6;

/// Nothing under the last file — a row and a half of it.
///
/// So that **the folder is always somewhere to right-click.** The menu for the folder itself —
/// where `New folder`, `Paste` and `Refresh` live — is the one you get by right-clicking a part
/// of the listing that is not a file, and in a folder taller than the pane there was no such
/// part: every pixel from the header to the status line was a row. The only way to reach it was
/// to scroll to the end and find the gap, if the last row happened to leave one.
///
/// Enough to be unmissable rather than merely present, which is also what makes a rubber band easy
/// to start from below the files. It was **three rows**, and three is more than that argument buys:
/// the end of a long folder read as though the listing had stopped short of the pane, and the slack
/// is charged twice over because it is content — a folder that nearly fills its pane grows a
/// scrollbar for it. Half of it is still one and a half times a click target.
///
/// Content and not a margin: at the top of a long folder every pixel of the pane is still rows, and
/// the space appears as you reach the end. In points rather than in rows because half a row is not a
/// row — see the extent in [`rows`], which is `ScrollArea::show_rows`' arithmetic with this in place
/// of a whole number of them.
pub const TAIL: f32 = ROW_HEIGHT * 1.5;

/// The dashed rectangle that marks the row the keyboard is on but has not selected.
///
/// Square corners, because it is drawn one pixel inside a row whose fill has none — a rounded
/// ring inside a square edge reads as a mistake at this size.
///
/// One closed polyline rather than four dashed edges: `dashed_line` walks the points it is
/// given, so a rectangle handed over as five points comes back with its dashes in step all the
/// way round instead of restarting at every corner.
pub(crate) fn cursor_ring(painter: &egui::Painter, rect: Rect, color: Color32) {
    const DASH: f32 = 2.0;
    const GAP: f32 = 2.0;
    // Half-pixel centres, so a one-pixel stroke lands on one row of pixels rather than
    // straddling two and coming out grey and two wide.
    let r = Rect::from_min_max(
        pos2(rect.left() + 0.5, rect.top() + 0.5),
        pos2(rect.right() - 0.5, rect.bottom() - 0.5),
    );
    painter.extend(egui::Shape::dashed_line(
        &[
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ],
        Stroke::new(1.0, color),
        DASH,
        GAP,
    ));
}

/// How far off the bottom of the row the bar in a Size cell sits.
///
/// Its *height* is the sidebar's [`crate::ui::sidebar::GAUGE_HEIGHT`] — the capacity bar under a drive
/// row — because it is the same kind of thing said the same way, and one window should not have two
/// visual languages for "this much of that".
///
/// The room comes out of the two points every cell's text is lifted by, plus the slack a
/// 16-point caption line leaves in a 24-point row: the text's line box ends 6 points off the
/// bottom, and its descenders a point and a half above that. So the bar sits under the number
/// rather than behind it, and nothing has to be moved to make space.
const SHARE_DROP: f32 = 2.0;

/// The least a Size cell can be and still carry a bar worth reading.
///
/// Under this the track is shorter than a couple of dozen pixels, at which point a 3% share and a
/// 12% one are the same one-pixel stub and the bar is decoration. The column is draggable, so it
/// can be squeezed to anything.
const SHARE_MIN: f32 = 24.0;

/// The bar in a Size cell: how much of what is on show this row is.
///
/// **Track and fill, not fill alone.** A bar with nothing behind it has no reference to be read
/// against — 26% of an invisible extent is a stub of unknown meaning — and the track is what makes
/// the cell's own width the hundred percent. It is `stroke-subtle`, the quietest line in the
/// palette and the same one the drive gauge uses, so a column of forty of them does not read as
/// ruling.
///
/// The fill is `accent-mark`, which is the accent as *ink on a surface* rather than as a surface:
/// that is the rung that stays legible on a selected row, where the row's own fill is the accent's
/// subtle end. A hidden row's bar goes the way its text does — see the `dim` argument — because a
/// full-strength bar on a faded row would be the loudest thing in it.
///
/// A share that rounds to nothing still gets a point of fill. A folder that holds a thousandth of
/// the listing is not the same as one that has not been counted, and a bar that vanished at some
/// threshold would make it look like one.
fn share_bar(painter: &egui::Painter, t: &Theme, cell: Rect, share: f32, dim: bool) {
    if cell.width() < SHARE_MIN {
        return;
    }
    let height = crate::ui::sidebar::GAUGE_HEIGHT;
    let top = (cell.bottom() - SHARE_DROP - height).round();
    let track = Rect::from_min_size(
        pos2(cell.left().round(), top),
        vec2(cell.width().round(), height),
    );
    let corner = CornerRadius::same(radius::CIRCULAR);
    painter.rect_filled(track, corner, t.gauge_track);
    painter.rect_filled(
        Rect::from_min_size(
            track.min,
            vec2((track.width() * share).max(1.0), track.height()),
        ),
        corner,
        if dim { t.text.disabled } else { t.accent.mark },
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn rows(
    ui: &mut Ui,
    t: &Theme,
    zone: &LocalZone,
    body: Rect,
    pane: PaneId,
    tab: &mut Tab,
    focused: bool,
    widths: &[f32; 4],
    icons_cache: &mut crate::shell::icons::Icons,
    links_cache: &mut crate::shell::links::Links,
    cut: &[std::path::PathBuf],
    scratch: &mut String,
    out: &mut Vec<Action>,
    outcome: &mut Outcome,
) {
    let Some(dir) = tab.dir.clone() else { return };
    let count = tab.order.len();

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(body.intersect(ui.clip_rect()));
    // The extent and the band below are counted in whole [`ROW_HEIGHT`]s, and the installed style's
    // item spacing is `space-3`. Anything egui adds per item would put the two out of step the moment
    // something scrolled — rows landing in the wrong place and the pointer hit-testing a different one
    // than it looks like, which is what this cost when `ScrollArea::show_rows` was reserving
    // `row_height + item_spacing.y` a row. Rows are painted at exact rects here, so it is nothing.
    child.spacing_mut().item_spacing = egui::Vec2::ZERO;

    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("rows", pane))
        .auto_shrink([false, false]);

    // Keyboard movement has to bring the cursor with it.
    //
    // Nudged into view rather than centred: a listing that recentres on every arrow
    // press is unreadable. `ScrollArea` takes an absolute offset, so the nudge is
    // computed against last frame's, which the pane records below.
    if let Some(offset) = tab.scroll_to.take() {
        // The rubber-band's auto-scroll: an absolute offset, which wins over the
        // cursor nudge because the pointer is the thing being followed.
        scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        tab.scroll_to_cursor = false;
    } else if tab.scroll_to_cursor {
        if let Some(at) = tab.cursor {
            let top = at as f32 * ROW_HEIGHT;
            let bottom = top + ROW_HEIGHT;
            let view = body.height();
            let mut offset = tab.scroll_y;
            if top < offset {
                offset = top;
            } else if bottom > offset + view {
                offset = bottom - view;
            }
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        tab.scroll_to_cursor = false;
    }

    // **`ScrollArea::show_rows`, with a fractional row at the end of it.** That is the whole reason
    // this is `show_viewport` and not the one-liner: `show_rows` counts the extent in whole rows, and
    // [`TAIL`] is a row and a half. Everything below is its own arithmetic — the extent, the band of
    // rows the viewport crosses, and one row past the end of it so a row half off the bottom edge is
    // still drawn — with the tail in points instead of rows.
    //
    // The band stops at the rows that exist, and the slack under them is deliberately not in it: the
    // extra is scrollable space and not a row, and anything drawn or hit-tested there would be a
    // file, where what is wanted is the folder. `first.min(last)` is that same rule for the
    // degenerate case — a viewport shorter than the tail, scrolled past every row — which asks for an
    // empty band rather than for a negative one.
    let output = scroll.show_viewport(&mut child, |ui, viewport| {
        ui.set_height(count as f32 * ROW_HEIGHT + TAIL);
        // `saturating_add`, because the offset is not always a sane one: the rubber band's
        // auto-scroll and `Tab::scroll_to` hand over whatever they were asked for, and egui does not
        // clamp it until after the viewport has been worked out. `show_rows`' own `+ 1` panics there.
        let last = ((viewport.max.y / ROW_HEIGHT).ceil() as usize)
            .saturating_add(1)
            .min(count);
        let first = ((viewport.min.y / ROW_HEIGHT).floor().max(0.0) as usize).min(last);
        let band = Rect::from_x_y_ranges(
            ui.max_rect().x_range(),
            (ui.max_rect().top() + first as f32 * ROW_HEIGHT)
                ..=(ui.max_rect().top() + last as f32 * ROW_HEIGHT),
        );
        // A child over the band, rather than `scope_builder`'s closure: the rows are painted at
        // explicit rects and the extent is the `set_height` above, so there is nothing for a scope to
        // measure and hand back — and this keeps `ui` meaning the band all the way down.
        let mut band_ui = ui.new_child(egui::UiBuilder::new().max_rect(band));
        let ui = &mut band_ui;
        // The widget ids in here are per row, so they have to come out the same at every scroll
        // position — which is what `show_rows` uses this for at exactly this point.
        ui.skip_ahead_auto_ids(first);
        let range = first..last;
        let visible = Rect::from_min_max(
            pos2(body.left(), ui.min_rect().top()),
            pos2(body.right(), ui.min_rect().top() + range.len() as f32 * ROW_HEIGHT),
        );
        // One interaction for the whole visible block, and the row worked out from
        // the pointer. Forty per-row widgets would cost forty ids, forty hit-tests
        // and forty animation slots for a hover highlight that arithmetic gives for
        // nothing.
        let response = ui.interact(
            visible,
            Id::new(("rows-hit", pane)),
            Sense::click_and_drag(),
        );
        let last_visible = range.len().saturating_sub(1);
        let row_at = |at: egui::Pos2| {
            (visible.contains(at) && !range.is_empty())
                .then(|| first + (((at.y - visible.top()) / ROW_HEIGHT) as usize).min(last_visible))
        };
        let hovered_row = response.hover_pos().and_then(row_at);

        // Where the button went down, while it is down.
        //
        // Not the same row as the one under the pointer, and that difference is the whole
        // point: egui only calls a press a drag once it has *travelled*, by which time the
        // pointer is a row or two along. Deciding from the pointer would pick up the file
        // the drag arrived at rather than the one it started on.
        let press = ui.input(|i| {
            i.pointer
                .any_down()
                .then(|| i.pointer.press_origin())
                .flatten()
        });
        let pressed_row = press.and_then(row_at);

        let edges = column_x(visible, widths);
        let name_font = t.fonts.body.clone();
        let meta_font = t.fonts.caption.clone();

        // Every row's shell icon, drawn in one batch once the rows are done.
        //
        // **Why this exists.** egui starts a new draw call whenever the texture changes, and a
        // shell icon is the only thing in a row that is not the font atlas — the fills, the
        // rules, the painted glyphs and all four columns of text come out of that one texture.
        // Drawing an icon inside the row loop therefore cuts the frame's primitive stream in
        // two at every row: forty rows become eighty-odd draw calls instead of a handful.
        //
        // That is not merely slow, it *leaks*, and not in this program: a plain eframe window
        // drawing forty textured quads holds steady, and the same forty interleaved with text
        // grows by 190 MB in thirty seconds. `examples/spin.rs` is that measurement. Batching
        // the icons is the workaround available from here, and it is what a hand-painted
        // listing should have been doing anyway.
        //
        // Drawing them last is invisible: an icon sits in its own column, over the row fill
        // and clear of the text.
        let mut deferred: Vec<(egui::TextureId, Rect, Rect, Color32)> = Vec::new();
        // The row being renamed, if one of the visible ones is, and where its name cell was.
        //
        // Drawn after the loop for the same reason the icons are, but to a different end: the
        // field is allowed to be wider than the Name column, and a widget drawn in the middle
        // of the loop would have the next three columns of its own row painted on top of it.
        let mut renaming: Option<(usize, f32, f32, Rect)> = None;
        // A git badge per row that has one, drawn after the icons for the plain reason that it goes
        // *on top* of one — a shell icon is a textured quad and would cover a badge painted first.
        //
        // The row's own fill travels with it: the badge is punched out of whatever it is sitting on,
        // and on a selected row that is the accent rather than the surface.
        let mut badges: Vec<(Rect, crate::git::State, Color32)> = Vec::new();
        // What git said about this folder, taken once. An `Arc` clone rather than a borrow, because
        // the loop below needs `&mut Tab` for the icon and shortcut columns it fills in as it goes.
        let git = tab.git.clone();
        // What a row actually has *ink* on, cell by cell, so a click can tell "on the file" from
        // "on the row it happens to be in". Two gestures ask:
        //
        // - A **drag**, which picks the file up from its ink and draws a rubber band from the
        //   space around it. That asks about the row the button went *down* on.
        // - A **right click**, which is the file's menu on its ink and the folder's menu off it.
        //   That asks about the row under the pointer, because a click arrives on the release,
        //   by which time no button is down and there is no press to ask about.
        //
        // So both rows are collected — at most two, and their rects cannot be confused for each
        // other's, since a row is a horizontal band and the two are at different heights.
        let mut ink: Vec<Rect> = Vec::with_capacity(10);
        // Whether this listing is a flattened tree rather than a flat list of the same rows, taken
        // once: it decides the indent, the twisty and what the Name column says after the name.
        // See [`crate::pane::FlatMode`].
        let tree = tab.is_tree();
        // Every visible twisty's box and the row it opens, for the click below. A click on one is
        // *not* a click on the row — it opens a folder rather than selecting it — so it has to be
        // tested before the selection is touched, and only the boxes actually drawn are here: a
        // folder with nothing in it has no twisty and nothing to hit.
        let mut twisties: Vec<(Rect, usize)> = Vec::new();
        // The joined folders in front of a merged chain's own name — `src > main > java > `. One
        // buffer down the whole loop, so a tree of chains allocates once for the frame rather than
        // once a row. Left holding whatever the last row put in it, like `scratch`.
        let mut chain_text = String::new();

        for position in range.clone() {
            let entry_index = tab.order[position] as usize;
            let entry = &dir.entries[entry_index];
            let row = Rect::from_min_size(
                pos2(
                    visible.left(),
                    visible.top() + (position - first) as f32 * ROW_HEIGHT,
                ),
                vec2(visible.width(), ROW_HEIGHT),
            );
            // Every cell's text is centred in this instead of in the row — see [`CELL_LIFT`].
            // Fills, the selection bar, the cursor ring and the drag ink all stay on `row`.
            let text_row = row.translate(vec2(0.0, -CELL_LIFT));

            let selected = tab.selected.get(entry_index).copied().unwrap_or(false);
            let is_hovered = hovered_row == Some(position);
            // The two rows whose ink is worth collecting: see where `ink` is declared.
            let inked = pressed_row == Some(position) || is_hovered;
            let renaming_here = matches!(tab.renaming, Some((at, _)) if at == entry_index);
            // The row being renamed wears none of it. The field has a border and a focus ring of
            // its own, and a selected row's fill and accent bar sit right behind them competing
            // for the same edge -- so the one row you are actually looking at is the one that
            // reads worst. Explorer drops the highlight while renaming too.
            //
            // What the row ends up filled with is kept: a git badge is punched out of it.
            let mut under = t.bg.layer;
            if !renaming_here {
                // A selection in a listing that does not have the keyboard goes quiet. `focused` is
                // false both when another pane has it and when *this* pane's console does, which is
                // the same statement either way: these rows are still selected, and the arrow keys
                // are not about them. See [`crate::ui::row_fill_quiet`].
                let fill = match (selected, focused) {
                    (true, false) => Some(row_fill_quiet(t)),
                    _ => row_fill(t, selected, is_hovered),
                };
                if let Some(fill) = fill {
                    ui.painter().rect_filled(row, CornerRadius::ZERO, fill);
                    under = fill;
                }
                if selected {
                    selection_bar(ui.painter(), row, t);
                }
            }
            // The keyboard cursor, when it is not simply the selection.
            //
            // Dashed and grey rather than a solid accent outline. The accent is what this
            // window says "selected" with — the fill and the bar down the left edge of a
            // selected row — and spending it on a row that is *not* selected said the opposite
            // of what it meant. A dashed grey rectangle is what every list on the platform has
            // marked the focused-not-selected row with since long before any of them had a
            // theme, and it cannot be confused with a selection at a glance.
            if focused && tab.cursor == Some(position) && !selected {
                cursor_ring(ui.painter(), row.shrink(1.0), t.stroke.strong);
            }

            // A hidden or system entry is dimmed rather than hidden-when-shown:
            // seeing that it *is* hidden is the point of showing it. A row waiting on a
            // paste is dimmed for a different reason — it is going somewhere — and
            // Explorer marks it the same way, so the two share the treatment.
            let pending_cut = crate::ui::is_cut(cut, &dir.path, dir.name(entry_index));
            let dim = entry.is_hidden() || pending_cut;
            let name_color = if dim { t.text.tertiary } else { t.text.primary };
            let meta_color = if dim { t.text.disabled } else { t.text.secondary };

            // ---- Name ----
            let kind = fmt::kind_of(dir.ext(entry_index), entry.is_dir());
            // How far down the tree this row is drawn, and so how far in its column starts. Zero for
            // every row of every other listing. `Tab::row_depth` and not `Dir::depth`, because a
            // merged chain of folders stands where the first of them stood — see `sort::TreeRow`.
            //
            // The twisty's width is reserved whether or not one is drawn, so that a folder's
            // children line up with each other whether they are folders or files.
            let depth = tab.row_depth(position);
            // How many folders are merged into this row's name, which is what turns it from `com`
            // into `src > main > java > com`. Never anything but zero outside a tree.
            let merged = tab.row_merged(position);
            let stem = stem_x(row, depth);
            let glyph_x = if tree { stem + TWISTY } else { stem };
            let box_rect = icon_rect(row, glyph_x, GLYPH);

            // The twisty, on a folder that has something under it.
            //
            // **Which is asked of the display order, not of the file system** — see
            // `Tab::has_children_below`, which is where that reasoning and its edge cases live. A
            // folder the user has shut has no children in the order at all, which is why that is the
            // other half of the test rather than a special case: it is shut, so it opens.
            //
            // The name it is shut *under* is the row's own — the innermost folder of a merged chain,
            // which is the one whose children the twisty is about.
            if tree && entry.is_dir() {
                let shut = tab.collapsed.contains(dir.name(entry_index));
                let has_kids = shut || tab.has_children_below(position);
                if has_kids {
                    let hit = twisty_rect(row, depth);
                    let chevron: azur_icons::Icon<'_> = if shut {
                        &azur_icons::chevron_right
                    } else {
                        &azur_icons::chevron_down
                    };
                    // `text-secondary`, so the ladder of chevrons down a branch does not compete
                    // with the names beside it — and `text-primary` under the pointer, which is
                    // the only affordance a painted glyph with no button around it has.
                    chevron(
                        ui.painter(),
                        icon_rect(row, hit.left(), TWISTY),
                        if is_hovered {
                            t.text.primary
                        } else {
                            t.text.secondary
                        },
                    );
                    twisties.push((hit, position));
                }
            }

            // The dimmed half of the Name cell: **what this row points at, or where it is.**
            //
            // A shortcut's target wins when it is known, because that is what the row *is* — a
            // name standing for somewhere else — and in an ordinary listing every row shares
            // the same parent anyway, so the target is the only context there is to give. The
            // parent is what a flattened listing shows, and it is the answer for every row in
            // one that is not a shortcut.
            //
            // **With the command line, where there is one.** Two shortcuts to `cmd.exe` with
            // different arguments are two different things, and a row showing only the target
            // draws them identically — see [`links::Target::line`].
            //
            // Asked once per row per view and answered on a worker thread: reading a `.lnk`
            // means COM, and one pointing at a share that is not currently reachable is the
            // classic Explorer hang. Until the answer lands the row draws its name alone, which
            // is what it did before this existed. See `crate::shell::links`.
            let context: Option<std::borrow::Cow<'_, str>> = {
                let row_index = entry_index as u32;
                match links::kind_of(dir.ext(entry_index), entry.is_link()) {
                    Some(kind) => {
                        if !tab.links.contains_key(&row_index)
                            && links_cache.request(
                                tab.view,
                                row_index,
                                dir.target(entry_index),
                                kind,
                            )
                        {
                            // Present means asked, so the row does not ask again next frame.
                            tab.links.insert(row_index, None);
                        }
                        // Cloned rather than borrowed: the map is behind the same `&mut Tab`
                        // that the next row's request writes to, and one short path per
                        // shortcut row on screen is not a cost worth threading a lifetime for.
                        tab.links
                            .get(&row_index)
                            .and_then(|target| target.as_ref())
                            .map(|target| std::borrow::Cow::Owned(target.line().into_owned()))
                    }
                    None => None,
                }
                // Where the row is, for a flattened listing. `""` in every other one — and
                // deliberately nothing in a **tree**, where the row is already sitting under the
                // folder it is in: the indent says it, and saying it twice is a column of dimmed
                // paths repeating what the shape of the listing already shows.
                .or_else(|| match dir.within(entry_index) {
                    _ if tree => None,
                    "" => None,
                    parent => Some(std::borrow::Cow::Borrowed(parent)),
                })
            };

            // The shell icon if one is known, and the painted glyph until it is. Nothing
            // here blocks or allocates: the per-file answer is a lookup in the tab's own
            // four-bytes-a-row column, and the per-type one is a small map.
            let shell_icon = match dir.explicit_target(entry_index) {
                // A row that stands for somewhere else — a volume under This PC. Its icon
                // is its own, not its type's, and there are at most a couple of dozen of
                // them, so this is the one listing that can afford to ask per path.
                Some(target) => icons_cache.place(target),
                None => {
                    let ext = dir.ext(entry_index);
                    // A file whose icon lives inside it: the answer belongs to this view of
                    // this folder, and the path to ask with is built once per file rather
                    // than once per file per frame.
                    let own = if !entry.is_dir() && Icons::is_per_file(ext) {
                        match tab.file_icons.get(entry_index).copied() {
                            Some(shell_icons::UNASKED) => {
                                let path = dir.path.join(dir.name(entry_index));
                                if icons_cache.request_file(
                                    tab.view,
                                    entry_index as u32,
                                    path,
                                ) {
                                    tab.file_icons[entry_index] = shell_icons::ASKED;
                                }
                                None
                            }
                            Some(index) if index >= 0 => Some(shell_icons::Icon { index }),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    // Falling through to the type's icon means an `.exe` shows the generic
                    // application glyph rather than nothing while its own is fetched.
                    own.or_else(|| icons_cache.kind(ext, entry.is_dir()))
                }
            }
            .and_then(|icon| icons_cache.uv(ui.ctx(), icon));

            match shell_icon {
                Some((texture, uv)) => {
                    // Held back and drawn after the loop — see `deferred`. A shell icon is a
                    // textured quad and everything else on a row comes out of the font atlas,
                    // so drawing it here would split the frame's primitive stream in two at
                    // every row.
                    deferred.push((
                        texture,
                        uv,
                        box_rect,
                        // A hidden entry is faded rather than recoloured: a shell icon
                        // carries its own colours, and tinting them would misreport
                        // what kind of file it is.
                        if dim {
                            Color32::from_white_alpha(110)
                        } else {
                            Color32::WHITE
                        },
                    ));
                }
                None => {
                    let glyph: azur_icons::Icon<'_> = if entry.is_dir() && entry.is_link() {
                        &icons::folder_link
                    } else {
                        icons::for_kind(kind)
                    };
                    let glyph_color = if dim { t.text.disabled } else { t.kind(kind) };
                    glyph(ui.painter(), box_rect, glyph_color);
                }
            }

            // **What git says about this row**, by the name the row is showing — which in a
            // flattened listing is a path, and git speaks in paths, so both work out of the same
            // lookup. One hash lookup per visible row per frame, against a map built once when the
            // answer landed; nothing here walks the repository's changes.
            if let Some(state) = git.as_ref().and_then(|repo| repo.state(dir.name(entry_index))) {
                badges.push((badge_rect(box_rect), state, under));
            }

            let name_left = glyph_x + GLYPH + space::S3;
            let name_right = edges[1] - CELL_PAD;
            // A folder row is somewhere files can be dropped. Recorded for every visible one,
            // because that is what makes dragging onto a folder mean "into that folder" rather
            // than "into the folder I am looking at".
            if entry.is_dir() {
                outcome.drop_rows.push((row, dir.target(entry_index)));
            }
            if inked {
                // The icon counts as the file too: it is the most obvious thing to take
                // hold of, and it is what Explorer's own drag handle is.
                ink.push(box_rect);
            }
            if renaming_here {
                // Held back until every column of every row has been drawn, so that a field
                // wider than the Name column covers Size, Type and Modified instead of being
                // painted under them. See where `renaming` is declared.
                // `text_row`, not `row`: the field has to put its text exactly where the label
                // would have gone, and the label is centred in `text_row`. See `rename_field`.
                //
                // **A merged chain renames its last folder**, because that folder is what the row is
                // — its columns are its, opening it goes there — so the field opens over that
                // segment rather than over the whole chain. The folders in front stay on screen
                // beside it, which is also what says which of them is being renamed.
                let ahead = if merged > 0 {
                    let (_, chain) = chain_split(dir.name(entry_index), merged);
                    chain_ahead(chain_folders(chain), false, &mut chain_text);
                    ui.painter()
                        .layout_no_wrap(chain_text.clone(), name_font.clone(), meta_color)
                        .size()
                        .x
                } else {
                    0.0
                };
                renaming = Some((
                    entry_index,
                    (name_left + ahead).min(name_right),
                    name_right,
                    text_row,
                ));
            } else if name_right > name_left {
                let galley = if merged > 0 {
                    let (_, chain) = chain_split(dir.name(entry_index), merged);
                    chain_galley(
                        ui.painter(),
                        chain,
                        name_font.clone(),
                        name_color,
                        meta_color,
                        name_right - name_left,
                        &mut chain_text,
                    )
                } else {
                    name_galley(
                        ui.painter(),
                        dir.leaf(entry_index),
                        context.as_deref(),
                        name_font.clone(),
                        name_color,
                        meta_color,
                        name_right - name_left,
                    )
                };
                if inked {
                    ink.push(Rect::from_min_max(
                        pos2(name_left, row.top()),
                        pos2((name_left + galley.size().x).min(name_right), row.bottom()),
                    ));
                }
                text_left(
                    ui.painter(),
                    Rect::from_min_max(
                        pos2(name_left, text_row.top()),
                        pos2(name_right, text_row.bottom()),
                    ),
                    galley,
                );
            }

            // ---- Size ----
            //
            // A file's own bytes, as ever — and, while the status line's measure button is on, a
            // folder's counted ones with a bar under them saying how much of the listing that is.
            // `Tab::size_shown` is what decides whether there is a number at all; `None` is the
            // blank cell a folder has always had. See [`crate::sizes`].
            if widths[1] > 0.0 {
                if let Some(bytes) = tab.size_shown(entry_index) {
                    let cell = Rect::from_min_max(
                        pos2(edges[1] + CELL_PAD, text_row.top()),
                        pos2(edges[2] - CELL_PAD, text_row.bottom()),
                    );
                    // Under the text rather than behind it, and on `row` rather than `text_row`:
                    // the bar belongs to the row's own bottom edge, where the two points every
                    // cell's text is lifted by are exactly the room it needs. See [`share_bar`].
                    if let Some(share) = tab.sizes.share(bytes) {
                        share_bar(
                            ui.painter(),
                            t,
                            Rect::from_min_max(
                                pos2(cell.left(), row.top()),
                                pos2(cell.right(), row.bottom()),
                            ),
                            share,
                            dim,
                        );
                    }
                    scratch.clear();
                    fmt::size(bytes, scratch);
                    let galley = truncated(
                        ui.painter(),
                        scratch,
                        meta_font.clone(),
                        meta_color,
                        cell.width(),
                    );
                    if inked {
                        // Right-aligned, so its ink is against the right edge of the cell.
                        ink.push(Rect::from_min_max(
                            pos2((cell.right() - galley.size().x).max(cell.left()), row.top()),
                            pos2(cell.right(), row.bottom()),
                        ));
                    }
                    text_right(ui.painter(), cell, galley);
                }
            }

            // ---- Type ----
            if widths[2] > 0.0 {
                scratch.clear();
                fmt::type_label(dir.ext(entry_index), entry.is_dir(), scratch);
                let cell = Rect::from_min_max(
                    pos2(edges[2] + CELL_PAD, text_row.top()),
                    pos2(edges[3] - CELL_PAD, text_row.bottom()),
                );
                let galley = truncated(
                    ui.painter(),
                    scratch,
                    meta_font.clone(),
                    meta_color,
                    cell.width(),
                );
                if inked {
                    // The ink is what a drag is measured against, so it stays on the row
                    // rather than following the text's two-point lift.
                    ink.push(Rect::from_min_max(
                        pos2(cell.left(), row.top()),
                        pos2((cell.left() + galley.size().x).min(cell.right()), row.bottom()),
                    ));
                }
                text_left(ui.painter(), cell, galley);
            }

            // ---- Modified ----
            if widths[3] > 0.0 {
                scratch.clear();
                fmt::modified(entry.modified, zone, scratch);
                let cell = Rect::from_min_max(
                    pos2(edges[3] + CELL_PAD, text_row.top()),
                    pos2(edges[4] - CELL_PAD, text_row.bottom()),
                );
                let galley = truncated(
                    ui.painter(),
                    scratch,
                    meta_font.clone(),
                    meta_color,
                    cell.width(),
                );
                if inked {
                    ink.push(Rect::from_min_max(
                        pos2(cell.left(), row.top()),
                        pos2((cell.left() + galley.size().x).min(cell.right()), row.bottom()),
                    ));
                }
                text_left(ui.painter(), cell, galley);
            }
        }

        // Every visible row's icon, in one run — see where `deferred` is declared.
        if !deferred.is_empty() {
            let painter = ui.painter();
            for (texture, uv, rect, tint) in &deferred {
                painter.image(*texture, *rect, *uv, *tint);
            }
        }

        // And the git badges over them. Painted glyphs, so they cost the font atlas's own draw call
        // rather than one each, and they go last because a badge belongs on top of the icon it is
        // about — see [`badge_rect`].
        if !badges.is_empty() {
            let painter = ui.painter();
            for (rect, state, under) in &badges {
                icons::git_badge(painter, *rect, *state, t.git(*state), *under);
            }
        }

        // And last of all, over everything including its own row's other columns.
        if let Some((entry_index, left, right, text_row)) = renaming {
            rename_field(ui, t, pane, tab, entry_index, left, right, text_row, out);
        }

        // ---- What the row is, in words -------------------------------------
        //
        // **Nothing here is only in the tooltip**, and that is deliberate: it is the four columns of
        // the row it is over, plus the two things the columns cannot say — a name too long for the
        // Name column, and the exact byte count — plus what git says, which has only a badge on the
        // row. So it is worth reading and never worth waiting for.
        //
        // Not while the pointer is *doing* something. A tooltip over a rubber band is a tooltip in the
        // way of the gesture, and one over a row being renamed covers the field.
        //
        // **At the pointer**, because the response it hangs off is the whole visible block and not the
        // row — see the `ui.interact` above. Anchored to that response the way a button's tooltip is,
        // it would come up below the last row on screen, forty rows from the one it is about.
        let busy = tab.renaming.is_some()
            || tab.band.is_some()
            || ui.input(|i| i.pointer.any_down() || i.pointer.any_released());
        if let Some(position) = hovered_row.filter(|_| !busy) {
            if let Some(about) = row_tooltip(tab, zone, position, scratch) {
                azur_egui_theme::components::tooltip_at_pointer_ui(response.clone(), |ui| {
                    crate::ui::tooltip_table(ui, t, &about);
                });
            }
        }

        // ---- Clicks --------------------------------------------------------
        let modifiers = ui.input(|i| i.modifiers);
        if tab.renaming.is_some() {
            // The field has the keyboard and the pointer; a click that lands outside it
            // is handled by the field losing focus, not by moving the selection.
            return;
        }
        if response.clicked() || response.secondary_clicked() {
            out.push(Action::Focus(pane));
        }
        // ---- A twisty, before anything else a click could mean ---------------
        //
        // Opening or shutting a folder in a tree is not selecting it, so this is tested first and
        // returns: a click that both opened a folder and moved the selection onto it would make
        // the twisty unusable as a way of *looking* at a branch without disturbing what is picked
        // out. Explorer's tree draws the same distinction, and so does every other one.
        //
        // The whole box is the target rather than the chevron's ink — a 6-point arrowhead is not
        // something to ask anybody to hit. Nothing here needs a hovered row: a box was only pushed
        // for a row that drew one.
        if response.clicked() {
            let at = response
                .interact_pointer_pos()
                .or_else(|| ui.ctx().pointer_interact_pos());
            if let Some(&(_, position)) = at
                .and_then(|at| twisties.iter().find(|(hit, _)| hit.contains(at)))
            {
                // The keyboard has already been claimed by the `Focus` above, which every click in
                // the listing pushes — a twisty is still a click in this pane.
                out.push(Action::ToggleCollapsed { pane, position });
                return;
            }
        }

        // Which row the context menu is *for*, decided below. `None` means the folder's own menu
        // — the one with `New folder` and `Paste` in it.
        let mut menu_row = hovered_row;
        if let Some(position) = hovered_row {
            if response.clicked() {
                if modifiers.command {
                    tab.toggle(position);
                } else if modifiers.shift {
                    tab.select_range_to(position);
                } else {
                    tab.select_only(position);
                }
                if tab.is_dir_at(position) {
                    outcome.prefetch = tab.target_at(position);
                }
            }
            if response.double_clicked() {
                if let Some(path) = tab.target_at(position) {
                    if tab.is_dir_at(position) {
                        out.push(Action::Navigate { pane, path });
                    } else {
                        out.push(Action::Open(path));
                    }
                }
            }
            // Middle click opens a folder in its own tab — including a folder *shortcut*, which
            // is a row this cannot tell apart from a file without reading it. So the reading is
            // left to the action, which resolves it the same way opening one does; a shortcut to
            // a file lands there and does nothing, which is what a middle click on any other
            // file does.
            //
            // An archive counts as a folder here, because it is one to this program — see
            // [`crate::archive`]. Asked by extension and so free, unlike the shortcut test beside
            // it, which reads the file.
            if response.middle_clicked() {
                if let Some(path) = tab.target_at(position) {
                    if tab.is_dir_at(position) {
                        out.push(Action::NavigateNewTab { pane, path });
                    } else if crate::archive::browsable(&path) || tab.is_shortcut_at(position) {
                        out.push(Action::OpenNewTab(path));
                    }
                }
            }
            // ---- Right click, on the file or merely in its row -------------
            //
            // A row is mostly space: a 24-point row across a wide pane has ink on perhaps a
            // third of it, and the rest is the listing's background as much as the gap below the
            // last file is. So a right click that lands on the name, the icon or one of the
            // three values is about *that file*, and one that lands in the space around them is
            // about the folder — which is where `New folder` and `Paste` are, and which used to
            // need finding a gap under the last row to reach.
            //
            // Only for a row that is not already in the selection. Right-clicking one that is
            // means the selection, wherever in the row it lands: the files are picked out
            // already, and taking that away because the pointer was between two columns would
            // undo work rather than ask a question. That is also the rule the drag follows.
            if response.secondary_clicked() && !tab.is_selected(position) {
                let at = response
                    .interact_pointer_pos()
                    .or_else(|| ui.ctx().pointer_interact_pos());
                let on_file =
                    at.is_some_and(|at| ink.iter().any(|rect| rect.expand(1.0).contains(at)));
                if on_file {
                    tab.select_only(position);
                } else {
                    tab.clear_selection();
                    menu_row = None;
                }
            }
        } else if response.clicked() {
            // A click on the empty space below the rows clears the selection, which
            // is how every file manager cancels one.
            tab.clear_selection();
        }

        // ---- Dragging: the file, or a band ---------------------------------
        //
        // Which one comes from where the button went *down*: on the icon, the name or one
        // of the three values — a drag of the file — or in the space around them, which
        // bands exactly as it does below the last row. A row is mostly space (a 24-point
        // row across a wide pane has ink on maybe a third of it), and treating all of it
        // as a drag handle is what makes a band a gesture you can only start by finding
        // the bottom of the listing first.
        //
        // Keyed on the *pressed* row and not the hovered one. egui only calls a press a
        // drag once it has travelled, and by then the pointer is a row or two along — so
        // the hovered row is the row the drag arrived at, and using it would both test the
        // wrong ink and pick up the wrong file.
        // Only the two buttons that mean anything here. A middle-button drag scrolls in some
        // applications and does nothing in this one, and the thumb buttons navigate — none of
        // them should pick a file up or draw a selection box, which is what `drag_started()`
        // without a button lets them all do.
        let dragging_files = response.drag_started_by(egui::PointerButton::Primary);
        let dragging_to_ask = response.drag_started_by(egui::PointerButton::Secondary);
        if dragging_files || dragging_to_ask {
            if let Some(grabbed) = pressed_row {
                let on_file = press
                    .is_some_and(|at| ink.iter().any(|rect| rect.expand(1.0).contains(at)));
                if on_file {
                    // A drag that starts on something already selected takes the whole
                    // selection; one that starts anywhere else makes that row the selection
                    // first, which is what makes dragging a single file work without clicking
                    // it beforehand.
                    if !tab.is_selected(grabbed) {
                        tab.select_only(grabbed);
                    }
                    out.push(Action::DragOut {
                        pane,
                        items: tab.selection_paths(),
                    });
                } else {
                    start_band(ui, body, tab, press);
                    out.push(Action::Focus(pane));
                }
            }
        }

        context_menu(ui, &response, pane, tab, menu_row, out);
    });

    // Remembered so the next scroll-into-view can nudge rather than jump, and so a
    // tab keeps its place when the pane it lives in is redrawn elsewhere.
    tab.scroll_y = output.state.offset.y;

    // Everything below the last row: the canvas a short listing leaves above the status line, and
    // the [`TAIL`] a long one leaves once it is scrolled to the end. That space is part of
    // the list — clicking it cancels the selection, as it does in every file manager, and
    // right-clicking it is how the folder's own menu is reached — but the band above covers the rows
    // and nothing else, deliberately, so nothing up there is listening for it.
    //
    // Measured from the rows rather than from `content_size`, which now includes the tail.
    let rows_bottom = body.top() + count as f32 * ROW_HEIGHT - output.state.offset.y;
    let empty = Rect::from_min_max(
        pos2(body.left(), rows_bottom.clamp(body.top(), body.bottom())),
        pos2(output.inner_rect.right(), body.bottom()),
    );
    if empty.height() > 1.0 && empty.width() > 1.0 {
        let response = child.interact(
            empty,
            Id::new(("rows-empty", pane)),
            Sense::click_and_drag(),
        );
        if response.clicked() {
            tab.clear_selection();
            out.push(Action::Focus(pane));
        }
        if response.secondary_clicked() {
            out.push(Action::Focus(pane));
        }
        // The folder's own menu, so right-clicking the space below the files offers New and
        // the rest rather than nothing.
        context_menu(&child, &response, pane, tab, None, out);

        // ---- The rubber band ------------------------------------------------
        //
        // It starts here rather than on a row because a drag *from* a row is how you
        // pick files up and move them — which is the other half of this gesture and
        // the reason the two have to start in different places, exactly as they do in
        // Explorer.
        if response.drag_started_by(egui::PointerButton::Primary)
            || response.drag_started_by(egui::PointerButton::Secondary)
        {
            let origin = child.input(|i| i.pointer.press_origin());
            start_band(&child, body, tab, origin);
            out.push(Action::Focus(pane));
        }
    }

    // **There is more above, and there is more below.** The design system's rule for the edge of a
    // scrolling collection — see `azur_egui_theme::components::scroll_fades` — and a file listing is
    // the collection it was written for.
    //
    // Its own two figures rather than the `ScrollArea`'s, because [`TAIL`] of the extent is
    // deliberately empty: measured off the content, a listing scrolled to its last file would fade at
    // the bottom for the slack under it and say there were more files. Measured off the rows, the
    // fade goes out exactly as the last one arrives.
    let above = output.state.offset.y;
    let below = (count as f32 * ROW_HEIGHT - output.inner_rect.height() - above).max(0.0);
    azur_egui_theme::components::scroll_fades(
        &child.painter_at(output.inner_rect),
        output.inner_rect,
        t.bg.layer,
        above,
        below,
    );

    if tab.band.is_some() && band_move(&mut child, body, tab, count as f32 * ROW_HEIGHT) {
        tab.apply_band();
        band_paint(&child, t, body, tab);
    }
}

/// A row's name, and — dimmed, after a `>` — where it is or what it points at.
///
/// Two sections of one galley rather than two galleys, so the pair share a baseline, a
/// truncation and a single draw. The name comes first and is what survives: everything after
/// the separator is context, and context is the thing to give up when the column is narrow.
///
/// `context` is `None` for the ordinary case — a file in the folder you are looking at, which
/// is not a shortcut — and then this is [`crate::ui::truncated`] and nothing else.
///
/// **The context is elided from its front, not its back.** For a flattened row that means
/// `translations_fr.json > …\Resources\Lang` rather than `> PluginGeosystem\Resour…`: the
/// folder immediately holding the file is what identifies it, and the same is true of a
/// shortcut's target, where the last component is the program it runs. A component at a time,
/// never mid-name, because a path cut mid-component reads as a different path.
///
/// At most one extra layout per level, only for the rows that do not fit, and only for the ~40
/// on screen; egui caches finished galleys, so a row that has not changed costs a hash lookup
/// on every frame after the first.
pub(crate) fn name_galley(
    painter: &egui::Painter,
    name: &str,
    context: Option<&str>,
    font: egui::FontId,
    color: egui::Color32,
    dim: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let Some(context) = context.filter(|text| !text.is_empty()) else {
        return truncated(painter, name, font, color, width);
    };

    let lay = |context: &str| {
        let mut job = egui::text::LayoutJob::default();
        job.append(name, 0.0, egui::TextFormat::simple(font.clone(), color));
        job.append(
            &format!("{CONTEXT}{context}"),
            0.0,
            egui::TextFormat::simple(font.clone(), dim),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
        painter.layout_job(job)
    };

    let whole = lay(context);
    if !whole.elided {
        return whole;
    }
    let mut shortest = None;
    let mut cut = 0;
    while let Some(at) = context[cut..].find(['\\', '/']) {
        cut += at + 1;
        let galley = lay(&format!("…\\{}", &context[cut..]));
        let fits = !galley.elided;
        shortest = Some(galley);
        if fits {
            break;
        }
    }
    // Not even the last component fits. The name is still first, so what is on screen is the
    // name and as much of the context as there was room for, which is the right way round.
    shortest.unwrap_or(whole)
}

/// What a row says about itself when the pointer rests on it, as **key and value**.
///
/// ```text
/// Name       LgsxItemBuilders.cpp
/// In         Inspect\Model             ← only in a flattened listing
/// Target     C:\Windows\System32\cmd.exe   ← only on a shortcut, once it has resolved
/// Arguments  /k build.bat              ← only on a shortcut that runs something
/// Type       C++ source
/// Size       9.38 KB (9,605 bytes)     ← on a file, and on a folder that has been measured
/// Modified   07/08/2026 18:24
/// Git        Changed on disk           ← only where git has something to say
/// ```
///
/// **The name is the reason it exists.** The Name column is whatever the other three leave, and on a
/// narrow pane that is narrower than plenty of names — so the first line is the one thing the row
/// might not have been able to show. Everything after it is either the row's own columns (worth
/// repeating, because those columns can be dragged to nothing) or something the row can only draw as a
/// mark: the exact size, and the badge's meaning in words.
///
/// It reads down the keys rather than across a paragraph, and **the keys are the column headers** —
/// `Name`, `Size`, `Type`, `Modified` — because that is what the four of them already are on screen
/// two inches above. It used to be four unlabelled lines with the type and the size run together by an
/// interpunct, which needed reading rather than glancing at: `C++ source · 9.38 KB (9,605 bytes)` is
/// two facts and a piece of punctuation doing a column's job.
///
/// `None` for a row that is not there. `scratch` is borrowed for the formatters and left holding
/// rubbish, which is what it is for.
pub(crate) fn row_tooltip(
    tab: &Tab,
    zone: &LocalZone,
    position: usize,
    scratch: &mut String,
) -> Option<Vec<(&'static str, String)>> {
    let dir = tab.dir.as_ref()?;
    let entry_index = tab.entry_at(position)?;
    let entry = dir.entries.get(entry_index)?;
    let mut about: Vec<(&'static str, String)> = Vec::with_capacity(8);

    // The row's own name and where it is — which for a merged chain of folders is the whole chain and
    // the folder the chain starts in. `chain_split` is the one place that division is made, so the
    // tooltip cannot disagree with the Name column about which is which.
    let (within, chain) = chain_split(dir.name(entry_index), tab.row_merged(position));
    let mut name = String::new();
    for (i, folder) in chain.split(['\\', '/']).enumerate() {
        if i > 0 {
            name.push_str(CONTEXT);
        }
        name.push_str(folder);
    }
    about.push(("Name", name));

    // Where it is, for a flattened listing — the same fact the Name column shows dimmed after the
    // name, and the first thing that column gives up when it runs out of room.
    if !within.is_empty() {
        about.push(("In", within.to_owned()));
    }

    // **What a shortcut points at, in full.** The Name column shows this too, but it is the half of
    // that cell that gets elided from the front the moment the column is narrow — and a target
    // reading `…\Lang\x.dll` is exactly the case where the whole path is the thing wanted. Here it
    // wraps instead, in a tooltip that is `TIP_VALUE` wide.
    //
    // The command line is its own line rather than run together with the path, because they are two
    // facts: `Arguments` is what the shortcut's own property sheet calls it, and a path with a
    // switch on the end of it reads as a longer path. Only the Name column, with one line to work
    // in, joins them.
    //
    // Whatever has already been resolved for the Name column — nothing is read here, so a tooltip
    // never costs a `.lnk` read, and a row hovered before its answer landed simply has no `Target`
    // line for the frame or two that takes. The grid never asks, so there it has none at all.
    if let Some(target) = tab
        .links
        .get(&(entry_index as u32))
        .and_then(|target| target.as_ref())
    {
        about.push(("Target", target.path.clone()));
        if !target.arguments.is_empty() {
            about.push(("Arguments", target.arguments.clone()));
        }
    }

    scratch.clear();
    fmt::type_label(dir.ext(entry_index), entry.is_dir(), scratch);
    about.push(("Type", scratch.clone()));
    // Whatever the Size cell is showing — a file's own bytes, or a folder's counted ones while the
    // measure button is on. Asked of the same [`crate::pane::Tab::size_shown`] the cell is drawn
    // from, so the tooltip cannot come to disagree with the column about a folder that has not
    // answered yet: there is no line rather than a wrong one.
    if let Some(bytes) = tab.size_shown(entry_index) {
        scratch.clear();
        fmt::size(bytes, scratch);
        // The rounded figure the column shows *and* the exact one, because they answer different
        // questions: `9.38 KB` is for comparing two files at a glance and `9,605 bytes` is for the
        // times only the number will do. Grouped in threes by hand — `fmt` has no separator for it,
        // and one call site does not make a formatter.
        about.push(("Size", format!("{scratch} ({} bytes)", grouped(bytes))));
        // **And deliberately not the share as a figure.** The bar is the whole of that answer: a
        // percentage is a number to compare against other numbers, and comparing is what the column
        // of bars already does at a glance and does better. It would also be the one line here that
        // is about the *listing* rather than about the row.
    }
    scratch.clear();
    fmt::modified(entry.modified, zone, scratch);
    about.push(("Modified", scratch.clone()));

    // And what git says, which the row itself can only say with a badge.
    if let Some(state) = tab
        .git
        .as_ref()
        .and_then(|repo| repo.state(dir.name(entry_index)))
    {
        about.push(("Git", state.describe(entry.is_dir()).to_owned()));
    }
    Some(about)
}

/// A byte count with its thousands grouped: `9605` as `9,605`.
///
/// A comma, and not the locale's separator: reading the user's number format means a call into the
/// platform per row, and this is one figure in a tooltip rather than a column of them. The same trade
/// [`fmt::date`] makes, and for the same reason.
pub(crate) fn grouped(bytes: u64) -> String {
    let digits = bytes.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
