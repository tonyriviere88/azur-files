//! The status line under a listing: the counts, how long the read took, the view switch, and
//! what git says about the folder.
//!
//! The view switch carries a menu of its own — [`tiles_menu`], where "open a folder like this for
//! me" is turned on and given a threshold. It is the only thing on this bar that is a *setting*
//! rather than a control or a fact, and it is here because a right click on the switch is where
//! somebody looks for the settings of switching.

use super::*;

/// The status line at the bottom of a pane.
pub const STATUS_HEIGHT: f32 = 22.0;

/// How close to a column edge counts as grabbing it.
pub(crate) const GRIP: f32 = 4.0;

/// The console's switch at the left end of the status line.
///
/// Smaller than a toolbar button, because it has to sit inside a 22-point bar with air above and
/// below it — [`crate::ui::TOOL_SIZE`] is 24 and would touch both edges. It is the same 18 the
/// preview's in-field toggles take, for the same reason.
pub(crate) const SWITCH: f32 = 18.0;

/// `"s"` unless there is exactly one.
pub(crate) fn plural(count: u32) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// The band the status line actually paints, and the one baseline everything on it sits on.
///
/// Two steps that have to happen in this order, which is why they are one function with a test of
/// their own rather than two lines at the top of [`status_line`].
///
/// **The band goes onto whole device pixels first.** A pane's bottom edge is a fraction of a window
/// divided by splits, so the rect arrives at a fractional `y` about half the time — and then the fill
/// is feathered across two rows at each edge while the text inside it is snapped to the grid by
/// epaint. The band's *visible* middle and the middle everything was centred on are then up to a
/// pixel apart, which is how a status line comes to look a pixel low on one window height and right
/// on the next. Rounded rather than floored, so the band stays [`STATUS_HEIGHT`] tall.
///
/// **Then the baseline comes off the snapped band**, and it is the *ink* baseline: this line holds
/// glyphs as well as words, and a glyph is centred on its own ink while a line of text centred in a
/// box is not — a line box reserves room under the baseline for descenders and above the capitals for
/// accents, and a file name uses neither. `azur::components::ink_baseline` carries the measurements;
/// [`CELL_LIFT`] is the same rule applied to a row of the listing.
///
/// Two rects come back, and the difference between them is [`NUDGE`]: the **band** is what gets
/// painted, and the **line** is what everything on it is placed against.
pub(crate) fn status_geometry(painter: &egui::Painter, t: &Theme, rect: Rect) -> (Rect, Rect, f32) {
    use egui::emath::GuiRounding as _;

    let band = rect.round_to_pixels(painter.pixels_per_point());
    let line = band.translate(vec2(0.0, -NUDGE));
    let baseline = ink_baseline(painter, &t.fonts.caption, line.top(), line.height());
    (band, line, baseline)
}

/// The pane's two switches and what git says, then — from the other end — how long the folder took,
/// how much is in it, and how much of that is selected.
///
/// # Two groups, and the left one wins
///
/// The left is two *controls* and a fact about the repository; the right is arithmetic about the
/// folder. When the bar is too narrow for both, the right gives way — the scan's figure first, then
/// the size, and the counts last, because the counts are the part of this line a listing cannot be
/// read without. The left is never dropped: a switch nobody can see is a switch nobody can find,
/// and the branch you are on is the one thing here that is said nowhere else in the window.
///
/// # One baseline
///
/// Everything on the line sits on a single [`ink_baseline`] — the branch name, three greys in two
/// alignments, a coloured count, and four glyphs. Not because the fonts differ (they are all
/// `caption`) but because the *glyphs* do not care about line boxes: a line box reserves room under
/// the baseline for descenders and above the capitals for accents, and text centred in it therefore
/// reads off-centre beside a glyph centred on its own ink. It is the same rule [`CELL_LIFT`] applies
/// to a row of the listing, taken from the design system rather than measured again here.
///
/// The timing is not decoration: a file manager that claims to be fast should be willing to be
/// checked, and a folder that suddenly takes 200ms is how you find out something is wrong.
#[allow(clippy::too_many_arguments)]
pub(crate) fn status_line(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    pane: PaneId,
    tab: &Tab,
    console_open: bool,
    // The window's rule for opening a folder as tiles, for the view switch's own menu — which is
    // where it is ticked and dragged. By value: the menu reports what was chosen as an [`Action`]
    // like every other control in this window, so nothing here writes a setting. See [`tiles_menu`].
    auto: crate::pane::AutoTiles,
    // Which file types this machine can draw a picture of, so the menu can say what the rule makes of
    // this folder. `&mut` because asking is what fills the cache — the same shape as the icon and
    // thumbnail services this listing already takes.
    providers: &mut crate::shell::providers::Providers,
    override_text: Option<&str>,
    now: f64,
    scratch: &mut String,
    out: &mut Vec<Action>,
) {
    use std::fmt::Write as _;

    let (band, line, baseline) = status_geometry(ui.painter(), t, rect);
    ui.painter()
        .rect_filled(band, CornerRadius::ZERO, t.bg.layer_alt);
    ui.painter().line_segment(
        [band.left_top(), band.right_top()],
        Stroke::new(1.0, t.stroke.subtle),
    );

    // ---- The two switches, at the left edge -------------------------------
    //
    // Controls rather than statuses, and the only ones on this line that are not a fact about the
    // folder. They are here because the left group is the one that never gives way — a switch nobody
    // can see is a switch nobody can find — and they are **in front of** the figures because that is
    // where a control belongs on a bar that is otherwise read left to right.
    //
    // **The view switch first**, and the console's after it. The order is the order of what they are
    // about: one changes the listing filling the pane above, the other opens a band at the bottom of
    // it, and the further-reaching of the two goes first.
    //
    // **One glyph each, latched**, rather than a pair that swap places. That is the rule every other
    // toggle in this window follows — see [`crate::icons::flatten`] and [`crate::icons::eye`], which
    // say it at length: what a toggle draws is the thing it is *about*, and whether it is on is said
    // by the fill and the ink [`crate::ui::tool_button`] gives a latched button. A button whose art
    // changed under the pointer would have to be read rather than recognised.
    let middle = (line.center().y - SWITCH * 0.5).round();
    let tiles = tab.view_mode.is_icons();
    let view = Rect::from_min_size(pos2(line.left() + space::S2, middle), vec2(SWITCH, SWITCH));
    let switched = crate::ui::tool_button(
        ui,
        t,
        view,
        Id::new(("view-switch", pane)),
        &icons::grid_view,
        if tiles {
            "Show details instead"
        } else {
            "Show large icons, with a thumbnail on anything that has one"
        },
        true,
        tiles,
        t.bg.layer_alt,
    );
    if switched.clicked() {
        out.push(Action::SetView {
            pane,
            mode: tab.view_mode.toggled(),
        });
    }
    // And its own menu, which is where "do this for me" lives. See [`tiles_menu`].
    tiles_menu(ui, t, &switched, tab, auto, providers, out);

    // `space-2` between the two and `space-3` after them: they are one group — the pane's own two
    // switches — and the gap inside a group has to read as smaller than the gap that follows it.
    // Four points is enough that two latched fills read as two buttons rather than one wide one.
    let switch = Rect::from_min_size(pos2(view.right() + space::S2, middle), vec2(SWITCH, SWITCH));
    if crate::ui::tool_button(
        ui,
        t,
        switch,
        Id::new(("console-switch", pane)),
        &icons::terminal,
        if console_open {
            "Hide the console (Ctrl+²)"
        } else {
            "Show the console (Ctrl+²)"
        },
        true,
        console_open,
        t.bg.layer_alt,
    )
    .clicked()
    {
        out.push(Action::ToggleConsole(pane));
    }
    let mut left = switch.right() + space::S3;
    // Where the right-hand groups have to stop: the bar's own edge, since both switches are at the
    // other one.
    let edge = line.right() - space::S3;
    // Cloned, so the rest of this can paint while `ui` is still available for the hit rects the
    // tooltips need.
    let painter = ui.painter().clone();
    let ink = |text: &str, color: Color32| {
        painter.layout_no_wrap(text.to_owned(), t.fonts.caption.clone(), color)
    };

    // A flatten that stopped at its limit has to say so, and it says so here — inside the group
    // that never gives way. **A listing quietly missing rows is the one wrong answer a file manager
    // must not give**: everything else on this line can be checked against the folder, and this
    // cannot. A glyph and a tooltip rather than a sentence, because the sentence was the first
    // thing a narrow pane dropped.
    if tab.dir.as_ref().is_some_and(|dir| dir.truncated) {
        let at = icon_rect(line, left, MARK);
        azur_icons::warning(&painter, at, t.bar.warning);
        let hit = ui.interact(at, Id::new(("status-limit", pane)), Sense::hover());
        azur_egui_theme::components::tooltip(
            hit,
            "This is as much of the tree as was read — not all of it is here",
        );
        left = at.right() + space::S3;
    }

    // A copy in progress or something that went wrong displaces everything but the switch: it is
    // the more urgent fact, and the counts have not changed anyway.
    if let Some(text) = override_text {
        let galley = truncated(
            &painter,
            text,
            t.fonts.caption.clone(),
            t.text.primary,
            (edge - left).max(0.0),
        );
        galley_on_baseline(&painter, left, baseline, galley);
        return;
    }

    // What the listing is, while it is not a listing yet. It takes the git summary's place rather
    // than sitting beside it: a folder that has not been read has no answer from git either, so the
    // two are never both there.
    let word = match &tab.dir {
        // Nothing until the wait is worth mentioning, and then the same word the body uses.
        None if tab.waiting_visibly(now) => Some("Reading…"),
        Some(dir) if dir.error.is_some() => Some("Could not be read"),
        _ => None,
    };
    match word {
        Some(word) => {
            let galley = truncated(
                &painter,
                word,
                t.fonts.caption.clone(),
                t.text.tertiary,
                (edge - left).max(0.0),
            );
            galley_on_baseline(&painter, left, baseline, galley);
        }
        None => left = git_summary(ui, &painter, t, line, baseline, pane, tab, left, out),
    }

    // ---- The right, from the edge inwards, in the order they give way ----
    //
    // Each run is laid out left to right and placed as a block, and every one but the first carries
    // its own separator on its right — so a run that will not fit takes its separator with it and
    // the line never ends in a dangling interpunct.
    let Some(dir) = &tab.dir else { return };
    if dir.error.is_some() {
        return;
    }
    let shown = tab.order.len();
    let hidden = dir.len().saturating_sub(shown);
    let mut runs: Vec<Vec<std::sync::Arc<egui::Galley>>> = Vec::new();

    // **`selected / on show (not shown)`, in three colours rather than three words.** It is the
    // shortest thing that says all of it, and the colour is what keeps it from reading as one
    // number: the selection is the accent's, the total is `text-secondary` because it is the fact
    // the other two are measured against, and what is being held back is quieter still. The
    // tooltip spells it out, and carries the folders-and-files breakdown this line used to show.
    let mut counts = vec![
        ink(&tab.selected_count.to_string(), t.bar.counted),
        ink(&format!(" / {shown}"), t.text.secondary),
    ];
    if hidden > 0 {
        counts.push(ink(&format!(" ({hidden})"), t.text.tertiary));
    }
    runs.push(counts);

    // The folder's size, or the selection's the moment there is one — which is the question
    // somebody selecting files is usually asking. Left out when it is zero rather than shown as
    // `0 B`: a selection of nothing but folders has no size this program knows, since a directory's
    // own byte count is noise, and `0 B` would be an answer rather than a silence.
    let bytes = if tab.selected_count > 0 {
        tab.selected_size
    } else {
        dir.total_size
    };
    if bytes > 0 {
        scratch.clear();
        fmt::size(bytes, scratch);
        runs.push(vec![
            ink(scratch, t.text.secondary),
            ink(SEPARATOR, t.text.disabled),
        ]);
    }

    // How long the folder took. In `text-tertiary` and not `text-disabled`, which is what it wore
    // and which measures **2.20:1** on this surface in the dark theme — under any floor there is. The
    // whole argument for putting a timing on the bar is that a claim about speed nobody can check is
    // not a claim, and a figure nobody can read is not checkable. `text-tertiary` is 3.62:1: still the
    // quietest thing on the line, and legible.
    let millis = dir.scan_micros as f64 / 1000.0;
    let mut figure = String::new();
    let _ = if millis < 10.0 {
        write!(figure, "{millis:.1} ms")
    } else {
        write!(figure, "{millis:.0} ms")
    };
    runs.push(vec![
        ink(&figure, t.text.tertiary),
        ink(SEPARATOR, t.text.tertiary),
    ]);

    let mut right = edge;
    let mut counted: Option<Rect> = None;
    for (which, run) in runs.iter().enumerate() {
        let width: f32 = run.iter().map(|galley| galley.size().x).sum();
        // Whatever is further left than the first thing that will not fit goes too: it is further
        // from the edge, so drawing it would leave a hole where this one would have been.
        if right - width < left + GROUP_GAP {
            break;
        }
        let mut x = right - width;
        if which == 0 {
            counted = Some(Rect::from_min_max(
                pos2(x, band.top()),
                pos2(right, band.bottom()),
            ));
        }
        for galley in run {
            let step = galley.size().x;
            galley_on_baseline(&painter, x, baseline, galley.clone());
            x += step;
        }
        right -= width;
    }

    if let Some(at) = counted {
        let mut tip = String::new();
        let _ = write!(
            tip,
            "{} selected of {shown} on show\n{} folder{} and {} file{} in this folder",
            tab.selected_count,
            dir.dir_count,
            plural(dir.dir_count),
            dir.file_count,
            plural(dir.file_count),
        );
        if hidden > 0 {
            // **A shut folder is the third way a row can be out**, and only in a tree — where it
            // is also much the most likely of the three, since shutting a branch of a large tree
            // takes thousands of rows out of the order at once. Naming only the other two would
            // leave the biggest number on the line explained by neither.
            let why = if tab.is_tree() {
                "inside a folder that is shut, hidden, or filtered out"
            } else {
                "hidden, or filtered out"
            };
            let _ = write!(tip, "\n{hidden} not shown: {why}");
        }
        let hit = ui.interact(at, Id::new(("status-counts", pane)), Sense::hover());
        azur_egui_theme::components::tooltip(hit, &tip);
    }
}

/// The view switch's own menu: **whether this window picks the view for you, and when.**
///
/// The fourth menu of this shape in the program and it follows the three that came first — see
/// [`crate::ui::breadcrumb::flatten_menu`], which says it at length. **Sticky**, because these are
/// settings rather than commands and a menu that vanished on the tick would have to be reopened to
/// see what the tick did. **Hung off the control the setting is about**, because a right click on the
/// thing that switches the view is where somebody looks for the settings of switching the view — and
/// because there is no room on a 22-point bar for a checkbox and a slider, nor any reason to spend
/// it: this is a question asked once and then left alone.
///
/// # The slider stays usable while the tick is off
///
/// Deliberately, and it is the same rule `Regroup single folders` follows: a setting that greys out
/// in the state you are not in is a setting you cannot find when you go looking for why the last
/// folder opened the way it did. The threshold is remembered either way — see
/// [`crate::config::Config::auto_tiles`] — so somebody can set the number first and then turn the
/// rule on, which is the order most people will do it in anyway.
///
/// # Nothing here changes the folder behind the menu
///
/// Which is worth knowing while reading it, because every other menu in this window does. What this
/// sets is how the *next* folder opens — see [`crate::pane::AutoTiles`] and
/// [`Action::SetAutoTiles`] — so the tick and the drag are both quiet, and the switch above them is
/// still what changes the view you are looking at.
///
/// # And it says what it makes of the folder you are in
///
/// The last line of it is the rule's own arithmetic over the listing behind the menu: what share of
/// its rows count as pictures. **The menu is the instrument**, which is the point — a threshold is a
/// number nobody can pick in the abstract, so open a folder, right-click, and read what the rule makes
/// of it before choosing where to put the slider.
///
/// It is a walk over the display order, so it is paid **only while this menu is open** and never in an
/// ordinary frame. On a flattened tree of two hundred thousand rows that is a few milliseconds a frame
/// for as long as somebody holds the menu up, which is the right place to spend it and the only place
/// the figure can come from.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tiles_menu(
    ui: &Ui,
    t: &Theme,
    trigger: &egui::Response,
    tab: &Tab,
    auto: crate::pane::AutoTiles,
    providers: &mut crate::shell::providers::Providers,
    out: &mut Vec<Action>,
) {
    use azur_egui_theme::components::{menu_divider, ContextMenu, MenuItem, Size, Slider};

    use crate::ui::breadcrumb::ticked;

    ContextMenu::new(trigger)
        .sticky(true)
        .show(ui.ctx(), |ui| {
            if ui
                .add(ticked(
                    MenuItem::new("Automatically switch to thumbnail view"),
                    auto.on,
                ))
                .clicked()
            {
                out.push(Action::SetAutoTiles(!auto.on));
            }
            // The rule, and then the number it is a rule over — behind a rule of its own, because the
            // slider is not a second entry on the same list: it is what the entry above means.
            menu_divider(ui);
            // A menu sets its own row spacing to zero, because entries that touch are entries the
            // pointer cannot cross into nothing. A slider is not an entry: left at zero its label sits
            // on the divider and its rail on the frame's own margin, which reads as a control that has
            // been squeezed in. So it gets `space-1` either side and nothing else does.
            ui.add_space(space::S1);
            // A copy, because the value belongs to `App` and is written by the action below rather
            // than by the widget. Whatever the drag leaves is what the next frame draws, since the
            // action is performed before it.
            let mut threshold = auto.threshold;
            // `Small`, which is the size the rest of this window's chrome is drawn at — a `Medium`
            // rail in a menu of 24-point rows reads as a control that has wandered in from a form.
            //
            // Whole fives. A slider in a menu is around 260 points wide, so a percent is under three
            // points of travel and nobody is choosing 63 rather than 65; what a step buys is a value
            // that comes back the same after the pointer is lifted, and a settings file with `60` in
            // it rather than `59.7`.
            let dragged = ui.add(
                Slider::new(&mut threshold, 0.0..=100.0)
                    .label("Pictures in the folder")
                    .suffix("%")
                    .step(5.0)
                    .decimals(0)
                    .size(Size::Small),
            );
            if dragged.changed() {
                out.push(Action::SetTilesThreshold(threshold));
            }
            ui.add_space(space::S1);

            // ---- What the rule makes of the folder behind the menu -------
            //
            // Computed here rather than carried in, because it is a question about *this* listing —
            // which is the whole reason the line exists: a threshold is a number nobody can pick in
            // the abstract. See this function's own doc for what the walk costs and why here is where
            // to spend it.
            //
            // In the caption role and `text-tertiary`, which is what the bar underneath this menu
            // dresses its own figures in: this is a reading, not a control and not a heading, and it
            // must not compete with the entry and the slider it is a consequence of.
            let (pictures, rows) = tab.picture_rows(providers);
            let reading = if rows == 0 {
                "Nothing here to count".to_owned()
            } else {
                format!(
                    "Here: {:.0}% — {pictures} of {rows} row{}",
                    pictures as f32 / rows as f32 * 100.0,
                    plural(rows as u32)
                )
            };
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(reading)
                        .font(t.fonts.caption.clone())
                        .color(t.text.tertiary),
                );
            });
            ui.add_space(space::S1);
        });
}

/// The branch, how far it is from its remote, and how much is changed — laid out to the right from
/// `left`, and returning where it ended.
///
/// **Nothing at all when there is no repository**, which is most folders: the line is about the
/// folder, and inventing a "not a repository" state to display would be furniture that is wrong more
/// often than it is right. Nothing while the answer is still coming either — it arrives within a
/// frame or two of the listing, and a placeholder that flickers past is worse than one row of
/// stillness.
///
/// # `N changed` is a button
///
/// It is the one thing on this line that is also a *question*, and the answer was three gestures
/// away: flatten the folder, find the filter box, know what to ask it for. Pressed, it does every
/// part of it at once — see [`Action::SetLens`], which is the same listing the funnel's
/// `Show git changes` opens. Everything else here stays a fact.
///
/// **Subtle, in the sense the rest of this window's chrome uses the word**: no border and no fill at
/// rest, so the line still reads as a line of figures rather than growing a control in the middle of
/// it, and the same quiet [`crate::ui::control_fills`] step under the pointer that every toolbar
/// button on every other surface wears. Nothing about the text changes — a count that restyled
/// itself for being pressable would be a count you read twice.
#[allow(clippy::too_many_arguments)]
pub(crate) fn git_summary(
    ui: &mut Ui,
    painter: &egui::Painter,
    t: &Theme,
    // The status line's *nudged* rect, not the band it paints — see `status_geometry`.
    line: Rect,
    baseline: f32,
    pane: PaneId,
    tab: &Tab,
    left: f32,
    out: &mut Vec<Action>,
) -> f32 {
    use std::fmt::Write as _;

    let Some(repo) = &tab.git else { return left };
    let caption = t.fonts.caption.clone();
    // The same edge `status_line` measures its own groups inwards from.
    let edge = line.right() - space::S3;

    // Left to right, in the order they matter: the branch — the one thing that says the rest of this
    // is git at all — and then what is between it and its remote, and then the working tree. The
    // flag is whether the segment is the button; only one of them ever is.
    let mut segments: Vec<(
        Option<azur_egui_theme::icons::Icon<'static>>,
        String,
        Color32,
        bool,
    )> = Vec::new();
    let mut text = String::new();

    if !repo.head.is_empty() {
        segments.push((
            Some(&icons::branch),
            repo.head.clone(),
            if repo.detached {
                // A detached head is a state to notice: a commit made here belongs to no branch.
                t.bar.warning
            } else {
                t.bar.info
            },
            false,
        ));
    }
    // Ahead and behind, which only mean anything against an upstream — a branch that has never been
    // pushed is not "0 ahead", it is a branch with nowhere to be ahead of.
    if repo.upstream.is_some() {
        if repo.ahead > 0 {
            text.clear();
            let _ = write!(text, "{}", repo.ahead);
            segments.push((Some(&icons::arrow_up), text.clone(), t.bar.success, false));
        }
        if repo.behind > 0 {
            text.clear();
            let _ = write!(text, "{}", repo.behind);
            segments.push((Some(&icons::arrow_down), text.clone(), t.bar.danger, false));
        }
    }
    if repo.dirty() {
        text.clear();
        let _ = write!(text, "{} changed", repo.changed);
        segments.push((None, text.clone(), t.bar.warning, true));
    } else {
        // A clean tree says so with the same tick a clean file wears, and no number: "0 changed" is
        // a sentence about nothing. Nothing to press either — the listing it would ask for is empty.
        segments.push((
            Some(&azur_egui_theme::icons::check),
            String::new(),
            t.bar.success,
            false,
        ));
    }

    let mut x = left;
    /// The air either side of the button's text, so the fill under the pointer is a shape around
    /// the words rather than a box wrapped tight on them.
    const PRESS_PAD: f32 = space::S1;
    // Where the button ended up, and the shape slot its fill goes in. Reserved *before* the text is
    // painted and filled in long after, because whether there is a fill at all depends on an
    // interaction that has to be registered after the group's own hit rect below — see there.
    let mut button: Option<(Rect, egui::layers::ShapeIdx)> = None;
    for (icon, label, color, pressable) in &segments {
        // The branch name is capped rather than given the line: a repository whose branch names are
        // paragraphs must not push the folder's own figures off the bar.
        let galley = truncated(painter, label, caption.clone(), *color, 160.0);
        let width = galley.size().x
            + if icon.is_some() {
                MARK + if label.is_empty() { 0.0 } else { space::S1 }
            } else {
                0.0
            };
        if x + width > edge {
            break;
        }
        let slot = pressable.then(|| painter.add(egui::Shape::Noop));
        let from = x;
        if let Some(icon) = icon {
            icon(painter, icon_rect(line, x, MARK), *color);
            x += MARK;
            if !label.is_empty() {
                x += space::S1;
            }
        }
        if !label.is_empty() {
            let step = galley.size().x;
            galley_on_baseline(painter, x, baseline, galley);
            x += step;
        }
        if let Some(slot) = slot {
            // The console switch's height, centred on the line: the two are the only controls on
            // this bar, and a target that agrees with the one at the other end of it is one
            // decision instead of two.
            let hit = Rect::from_min_max(
                pos2(from - PRESS_PAD, (line.center().y - SWITCH * 0.5).round()),
                pos2(x + PRESS_PAD, (line.center().y + SWITCH * 0.5).round()),
            );
            button = Some((hit, slot));
        }
        x += space::S3;
    }

    // What `↑2 ↓1` is counted against, which is the one thing the summary cannot show and the first
    // thing anybody asks of it. Over the whole group, since every part of it is about this
    // repository.
    if x > left {
        let mut tip = String::new();
        if repo.detached {
            let _ = write!(tip, "Detached at {}", repo.head);
        } else {
            let _ = write!(tip, "On {}", repo.head);
        }
        match &repo.upstream {
            Some(upstream) => {
                let _ = write!(tip, ", tracking {upstream}");
                if repo.ahead > 0 || repo.behind > 0 {
                    let _ = write!(tip, "\n{} ahead, {} behind", repo.ahead, repo.behind);
                }
            }
            None => tip.push_str(", not tracking a remote"),
        }
        if repo.dirty() {
            let _ = write!(
                tip,
                "\n{} staged, {} changed, {} untracked",
                repo.staged, repo.unstaged, repo.untracked
            );
            if repo.conflicted > 0 {
                let _ = write!(tip, ", {} conflicted", repo.conflicted);
            }
        } else {
            tip.push_str("\nNothing to commit");
        }
        let at = Rect::from_min_max(pos2(left, line.top()), pos2(x, line.bottom()));
        let hit = ui.interact(at, Id::new(("status-git", pane)), Sense::hover());
        azur_egui_theme::components::tooltip(hit, &tip);
    }

    // **The button is registered last, so it wins the pointer inside the group.** The group's hit
    // rect above covers the whole summary including this, and the one registered later is the one on
    // top — the other way round, the count could not be clicked and the tooltip explaining what the
    // click does would never appear. It is the same ordering the filter box's ✕ relies on.
    if let Some((hit, slot)) = button {
        let response = ui.interact(hit, Id::new(("status-changed", pane)), Sense::click());
        let (hover, pressed) = crate::ui::control_fills(t, t.bg.layer_alt);
        let fill = if response.is_pointer_button_down_on() {
            Some(pressed)
        } else if response.hovered() {
            Some(hover)
        } else {
            None
        };
        if let Some(fill) = fill {
            painter.set(
                slot,
                egui::Shape::rect_filled(hit, CornerRadius::same(radius::SMALL), fill),
            );
        }
        if response.has_focus() {
            azur_egui_theme::icons::focus_ring_inset(
                painter,
                hit,
                CornerRadius::same(radius::SMALL),
                t.stroke.focus,
            );
        }
        // The words are the menu entry's, not a copy of them: this button and the funnel's
        // `Show git changes` ask for the same listing, and a tooltip naming it something else would
        // read as a second, subtly different thing.
        azur_egui_theme::components::tooltip(
            response.clone(),
            &format!(
                "{}: this folder's whole tree, filtered to what git says changed",
                crate::pane::Lens::Git.label()
            ),
        );
        if response.clicked() {
            out.push(Action::SetLens {
                pane,
                lens: Some(crate::pane::Lens::Git),
            });
        }
    }
    x
}
