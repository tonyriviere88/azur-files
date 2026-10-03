//! The find bar: the query, the toggles, and the counter.
//!
//! It runs on the UI thread on every keystroke, which is the whole reason for `done` — see
//! [`Find::against`].

use super::*;

/// The find bar's height, and the field inside it with a margin either side.
pub(super) const FIND_H: f32 = TOOL_SIZE + PAD;

/// How wide the find field would like to be, and the least it will accept.
///
/// It is the only part of the bar that gives way when the panel is narrow. Everything after it —
/// the count, the two arrows, the close button — is a control you cannot work the bar without,
/// where a field of half the width is still a field you can type a word into.
pub(super) const FIND_W: f32 = 184.0;

pub(super) const FIND_MIN: f32 = 72.0;

/// One of the three toggles inside the field.
///
/// Smaller than a tool button, because three of them have to sit inside a field that is itself the
/// height of one — and because they are *in* the field rather than beside it: a 24-point box would
/// leave its fill touching the border.
pub(super) const FLAG: f32 = 18.0;

/// The room kept for the count, whatever it currently says.
///
/// Fixed rather than measured, because the bar is anchored to the *right* and a count that grew
/// from `9 of 12` to `10 of 12` would shove the field along under the caret. Wide enough for the
/// longest thing it says, which is `1 of 4096+` — see [`preview::search::HITS`].
pub(super) const COUNTER: f32 = 68.0;

/// The find bar over a text preview: what is being looked for, and what was found.
///
/// **It hangs off the [`Preview`] rather than off the [`Text`]**, so the query outlives the file: you
/// are usually looking for the same thing in the next one, and arrowing down a folder with the bar
/// open is the shape of "where else does this appear". What does not outlive the file is `hits`,
/// which is a set of offsets into one body — `done` is cleared whenever the content changes and the
/// search is run again on the next frame.
#[derive(Default)]
pub(super) struct Find {
    pub(super) open: bool,
    pub(super) search: preview::Search,
    /// Byte ranges into the body on show, in order.
    pub(super) hits: Vec<Range<usize>>,
    /// Which hit is current, as an index into `hits`. Meaningless while it is empty.
    pub(super) at: usize,
    pub(super) capped: bool,
    pub(super) bad: bool,
    /// The search `hits` was computed from, or `None` for "nothing has been run against what is on
    /// the canvas now". The one thing that keeps a megabyte from being scanned on every frame the
    /// pointer moves.
    pub(super) done: Option<preview::Search>,
    /// Bring the current hit into view on the next frame that draws the body.
    pub(super) reveal: bool,
    /// Take the keyboard on the next frame — the one the bar opens on.
    pub(super) grab: bool,
}

impl Find {
    /// Run the search again, if what it would answer has changed.
    ///
    /// Keeps your place across a change of query: the hit you were on has a byte offset, and the
    /// hit chosen from the new set is the first one at or after it. Without that, every keystroke of
    /// `foo` would send you back to the top of the file — which is the difference between typing a
    /// word and typing a word while reading.
    pub(super) fn against(&mut self, body: &str) {
        if self.done.as_ref() == Some(&self.search) {
            return;
        }
        let was = self.hits.get(self.at).map_or(0, |hit| hit.start);
        let found = preview::hits(body, &self.search);
        self.at = found.at.iter().position(|hit| hit.end > was).unwrap_or(0);
        self.hits = found.at;
        self.capped = found.capped;
        self.bad = found.bad;
        self.done = Some(self.search.clone());
        self.reveal = !self.hits.is_empty();
    }

    /// Nothing has been run against what is on the canvas now.
    pub(super) fn forget(&mut self) {
        self.hits.clear();
        self.done = None;
    }

    /// The hits to draw, which are none at all while the bar is shut.
    ///
    /// A bar that has been closed leaves its query in place — that is deliberate, so reopening it
    /// resumes — but the highlights go with the bar, because a file marked up by a search you can no
    /// longer see is a file with something wrong with it.
    pub(super) fn showing(&self) -> &[Range<usize>] {
        if self.open {
            &self.hits
        } else {
            &[]
        }
    }

    /// The same three things, split so that a pass over many blocks can hold them at once.
    ///
    /// [`document`] draws one widget per block and each of them both reads the hits and may clear
    /// `reveal`, which is two borrows of one `Find`. Splitting it into disjoint fields here is what
    /// makes that a borrow of two things rather than a fight over one.
    pub(super) fn marks(&mut self) -> Marks<'_> {
        let Self {
            hits,
            at,
            reveal,
            open,
            ..
        } = self;
        Marks {
            hits: if *open { &hits[..] } else { &[] },
            at: *at,
            reveal,
        }
    }

    /// Step to the next hit, or the previous one, wrapping at both ends.
    ///
    /// Wrapping because the arrows are how a file gets swept, and an arrow that stops working at
    /// the last hit is an arrow you have to think about.
    ///
    /// **`by == 0` does nothing at all**, which is not a triviality: the bar calls this once a frame
    /// with whatever its buttons and keys came to, and that is nought on almost all of them. Setting
    /// `reveal` anyway put a `scroll_to_rect` in every frame the panel drew — so the text could not
    /// be scrolled away from the current hit at all. It sprang back under the wheel.
    pub(super) fn step(&mut self, by: isize) {
        if by == 0 || self.hits.is_empty() {
            return;
        }
        let n = self.hits.len() as isize;
        self.at = (self.at as isize + by).rem_euclid(n) as usize;
        self.reveal = true;
    }

    /// The count, in the words a find bar says it in.
    ///
    /// `4096+` rather than a number once the search has stopped counting, because a count that is
    /// really a floor has to look like one — see [`preview::search::HITS`]. Nothing at all until something
    /// has been typed: an empty field has not failed to find anything.
    pub(super) fn counter(&self) -> String {
        if self.search.text.is_empty() {
            String::new()
        } else if self.bad {
            "Bad pattern".to_owned()
        } else if self.hits.is_empty() {
            // **"No results" is a claim, and a search that stopped early cannot make it.** A literal
            // walk gives up once it has spent its comparison budget — see [`preview::search`] — so an
            // empty answer is either "there are none" or "there were none in the part I got through",
            // and saying the first about the second is the quiet lie the `+` below exists to avoid.
            if self.capped {
                "Stopped".to_owned()
            } else {
                "No results".to_owned()
            }
        } else if self.capped {
            format!("{} of {}+", self.at + 1, self.hits.len())
        } else {
            format!("{} of {}", self.at + 1, self.hits.len())
        }
    }
}

/// The find bar, floating over the top right of the text — and inside the scroll bar rather than on
/// top of it, which is the one place it differs from where an editor puts one. See `gutter` below.
///
/// The order is the one every find bar uses, and it is worth reading as a sentence: what to look for,
/// how to look for it, how many there are, and the two ways to move through them. The three *hows*
/// are inside the field because they belong to the query rather than to the results.
pub(super) fn find_bar(ui: &mut Ui, t: &Theme, canvas: Rect, spot: Spot, find: &mut Find) {
    // Everything but the field is fixed, so the field is what the arithmetic solves for. A panel
    // narrow enough to squeeze it past `FIND_MIN` gets a bar wider than the canvas, clipped at the
    // left — which loses the start of what you typed and keeps every control. The other way round
    // loses the ability to close it.
    // **Clear of the scroll bar, not over it.** The text under the bar still scrolls, and a close
    // button sitting on the thumb is a thumb you have to scroll the panel to reach. Read from the
    // installed style rather than named here: the width is the design system's
    // (`StyleOptions::scroll_bar_width`) and this has to be whatever that is, plus the margin egui
    // keeps inside the scroll area for it. Twenty-two points as the two are set now.
    let gutter = {
        let scroll = ui.style().spacing.scroll;
        scroll.bar_width + scroll.bar_inner_margin + scroll.bar_outer_margin
    };
    let fixed = COUNTER + TOOL_SIZE * 3.0 + PAD * 3.0;
    let field_w = (canvas.width() - PAD * 2.0 - gutter - fixed).clamp(FIND_MIN, FIND_W);
    let size = vec2(fixed + field_w, FIND_H);
    let bar = Rect::from_min_size(
        pos2(
            (canvas.right() - PAD - gutter - size.x).round(),
            (canvas.top() + PAD).round(),
        ),
        size,
    );

    // **Painted into the panel's own layer, after the text, rather than into an `Area` of its own.**
    // An `Area` was the obvious way to float something and it does not work here: with the bar on
    // `Order::Foreground` — above the pane by every rule egui has — the body text still came out over
    // the top of it, faintly, at about a tenth of its own opacity. Measured off the framebuffer with
    // the bar's fill temporarily set to red, so it is not a trick of the eye: the glyphs are there,
    // lighter than the fill, in both themes. Whatever egui is doing with the shapes of a selectable
    // label inside a `ScrollArea`, it survives being put under a higher layer. Painting into the same
    // painter after the text cannot lose that race, because within one layer the order is the order
    // things were added.
    //
    // What the `Area` was buying was input: the body is a selectable label, so a press on the bar
    // that reached the text would start a selection under it. `claim` buys the same thing — a widget
    // over the whole bar, added after the label, and egui gives the pointer to the last widget added
    // over a point.
    let claim = ui.interact(
        bar,
        Id::new(("preview-find-bar", spot)),
        Sense::click_and_drag(),
    );
    if claim.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    {
        {
            // **The edge is what says this is a separate thing, not the fill.** `bg.layer_alt` is the
            // surface every popover in this program floats on and it stays that here — but a popover
            // appears over whatever happens to be underneath, while this one appears over the panel
            // that `layer_alt` is a step from: measured against `bg.layer` it is **2.2 ΔL\* in the
            // dark theme** and 8.5 in the light one, which is invisible in one theme and fine in the
            // other, and that is two rules rather than one. So the fill's job here is only to be
            // opaque, and `stroke.default` draws the outline. `the_find_bar_has_an_edge_you_can_see`
            // measures it.
            let corner = CornerRadius::same(radius::MEDIUM);
            ui.painter()
                .add(azur_egui_theme::tokens::shadow::S16.as_shape(bar, corner));
            ui.painter().rect(
                bar,
                corner,
                t.bg.layer_alt,
                Stroke::new(1.0, t.stroke.default),
                egui::StrokeKind::Inside,
            );

            let inner = bar.shrink(PAD * 0.5);
            let field = Rect::from_min_size(inner.min, vec2(field_w, inner.height()));

            // ---- The field, and the three toggles inside it -------------------
            //
            // Painted after the input, into a slot reserved before it: the border depends on focus,
            // and focus is only known once the input has run. `field_frame` is the design system's,
            // because a composite field still wears the same fill, radius and four-state border as
            // every other field in the window — including the danger border, which is how a pattern
            // that will not compile says so.
            let slot = ui.painter().add(egui::Shape::Noop);
            let glyph = icon_rect(field, field.left() + PAD * 0.5, 12.0);
            azur_icons::search(ui.painter(), glyph, t.text.tertiary);

            let mut x = field.right() - PAD * 0.5;
            for (label, tip, under, on) in [
                (
                    ".*",
                    "Use a regular expression",
                    false,
                    &mut find.search.regex,
                ),
                ("ab", "Whole word only", true, &mut find.search.word),
                ("Aa", "Match case", false, &mut find.search.case),
            ] {
                let at = Rect::from_center_size(
                    pos2(x - FLAG * 0.5, field.center().y),
                    vec2(FLAG, FLAG),
                );
                if flag_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-flag", spot, label)),
                    label,
                    tip,
                    under,
                    *on,
                )
                .clicked()
                {
                    *on = !*on;
                }
                x = at.left();
            }

            let typing = Rect::from_min_max(
                pos2(glyph.right() + PAD * 0.5, field.top()),
                pos2(x - PAD * 0.5, field.bottom()),
            );
            // The mark is taken before the field paints, so the caret can be found again below.
            let first_shape = crate::ui::shape_mark(ui);
            let edit = ui.put(
                typing,
                egui::TextEdit::singleline(&mut find.search.text)
                    // Explicit, not from the auto-id sequence. Two reasons, and the second one bit
                    // hard elsewhere in this panel: focus is a thing other code has to be able to ask
                    // about by name, and an id derived from how many widgets came before it in the
                    // frame moves when anything upstream changes — see `ui::deps`, whose rows stopped
                    // answering clicks for exactly that reason.
                    .id(Id::new(("preview-find-field", spot)))
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .desired_width(typing.width())
                    .font(egui::FontSelection::FontId(t.fonts.body.clone()))
                    .text_color(t.text.primary),
            );
            // The same correction the filter's caret gets, for the same reason — see
            // [`crate::ui::nudge_caret`]. Applied here rather than after `field_frame` below, so the
            // range it searches holds this field's shapes and not the frame's as well.
            crate::ui::nudge_caret(
                ui,
                first_shape,
                crate::ui::CARET_SHORTER,
                crate::ui::CARET_LOWER,
            );
            field_frame(
                ui,
                slot,
                field,
                t,
                FieldLook {
                    enabled: true,
                    focused: edit.has_focus(),
                    hovered: field_hovered(ui, field, true),
                    error: find.bad,
                    // `background-control` and `stroke-control`, which is what a field floating
                    // on a panel wants. The filter box at the end of a path bar is the other case
                    // — see `crate::ui::bar`.
                    fill: None,
                    border: None,
                },
            );
            if std::mem::take(&mut find.grab) {
                edit.request_focus();
            }

            // ---- The count, and the three buttons after it --------------------
            let count = find.counter();
            let after = Rect::from_min_max(pos2(field.right() + PAD, inner.top()), inner.max);
            let galley = truncated(
                ui.painter(),
                &count,
                t.fonts.caption.clone(),
                t.text.secondary,
                COUNTER,
            );
            galley_on_baseline(
                ui.painter(),
                after.left(),
                ink_baseline(ui.painter(), &t.fonts.caption, after.top(), after.height()),
                galley,
            );

            let mut step = 0isize;
            let mut shut = false;
            let mut right = inner.right();
            let any = !find.hits.is_empty();
            for (glyph, tip, id, enabled) in [
                (
                    &azur_icons::close as azur_egui_theme::icons::Icon<'_>,
                    "Close (Esc)",
                    "shut",
                    true,
                ),
                (&azur_icons::chevron_down, "Next match (Enter)", "next", any),
                (
                    &azur_icons::chevron_up,
                    "Previous match (Shift+Enter)",
                    "prev",
                    any,
                ),
            ] {
                let at = Rect::from_center_size(
                    pos2(right - TOOL_SIZE * 0.5, inner.center().y),
                    vec2(TOOL_SIZE, TOOL_SIZE),
                );
                if tool_button(
                    ui,
                    t,
                    at,
                    Id::new(("preview-find", spot, id)),
                    glyph,
                    tip,
                    enabled,
                    false,
                    t.bg.layer_alt,
                )
                .clicked()
                {
                    match id {
                        "shut" => shut = true,
                        "next" => step = 1,
                        _ => step = -1,
                    }
                }
                right = at.left() - PAD * 0.25;
            }

            // ---- The keyboard, while the caret is in the field ----------------
            //
            // `Enter` and `Shift+Enter` are the arrows, which is what makes the bar usable without
            // the pointer ever going near it. egui's single-line field gives up focus on `Enter`, so
            // it is asked back for — otherwise the second `Enter` would go to the window.
            // **`lost_focus` and not only `has_focus`.** egui's single-line field takes both of these
            // keys as "the caret is finished here" and surrenders focus during its own run — so by
            // the time there is a response to ask, the focus this would have keyed off has already
            // gone, on the very keystroke being handled. Asking only `has_focus()` is a find bar
            // whose `Enter` does nothing at all, which is how this was first written.
            if edit.has_focus() || edit.lost_focus() {
                // `Enter` and `F3` both, because both are what people press: `Enter` is the field's
                // own idea of "again", and `F3` is what every Windows program has meant by "find
                // next" for thirty years. `Shift` reverses either.
                //
                // `F3` is the pane's filter shortcut when nothing is being typed into — see
                // `crate::ui::breadcrumb`, which gives it up while another field has the keyboard.
                // This is that other field.
                let (again, back, escape) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::F3),
                        i.modifiers.shift,
                        i.key_pressed(egui::Key::Escape),
                    )
                });
                if again {
                    step = if back { -1 } else { 1 };
                    // And the caret is asked back, or the second `Enter` would go to the window.
                    edit.request_focus();
                }
                shut |= escape;
            }
            find.step(step);
            if shut {
                find.open = false;
            }
        }
    }
}

/// One of the three toggles inside the find field.
///
/// `Aa`, `ab` and `.*` are what every find bar draws there, and they are **letterforms rather than
/// glyphs** — so they are drawn as text, on the ink baseline of the box they sit in, like everything
/// else on a row in this program. `ab` carries a rule under it, which is the only thing that makes
/// two letters read as "the whole word and not the start of one".
///
/// Latched the way a tool button latches, from the same `desktop::latched` pair, because that is the
/// one rule in the window for "this control is on".
#[allow(clippy::too_many_arguments)]
pub(super) fn flag_button(
    ui: &mut Ui,
    t: &Theme,
    rect: Rect,
    id: Id,
    label: &str,
    tip: &str,
    under: bool,
    on: bool,
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let corner = CornerRadius::same(radius::SMALL);
    let (hover, pressed) = control_fills(t, t.bg.layer_alt);
    let latched = azur_egui_theme::desktop::latched(t, response.hovered());
    let fill = if on {
        Some(latched.0)
    } else if response.is_pointer_button_down_on() {
        Some(pressed)
    } else if response.hovered() {
        Some(hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, corner, fill);
    }
    let ink = if on {
        latched.1
    } else if response.hovered() {
        t.text.primary
    } else {
        t.text.secondary
    };

    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), t.fonts.caption.clone(), ink);
    let width = galley.size().x;
    let baseline = ink_baseline(ui.painter(), &t.fonts.caption, rect.top(), rect.height());
    let left = (rect.center().x - width * 0.5).round();
    galley_on_baseline(ui.painter(), left, baseline, galley);
    if under {
        // On the baseline itself rather than under the descenders: `ab` has none, and a rule two
        // points lower would read as a border on the button.
        let y = (baseline + 1.0).round() - 0.5;
        ui.painter().line_segment(
            [pos2(left, y), pos2(left + width, y)],
            Stroke::new(1.0, ink),
        );
    }
    if !tip.is_empty() {
        azur_egui_theme::components::tooltip(response.clone(), tip);
    }
    response
}
