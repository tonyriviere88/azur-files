//! The application's palette: Azur's roles, plus the ones only a file manager
//! needs.
//!
//! Built the way the design system documents for an app with domain colours — a
//! struct that owns an [`azur_egui_theme::Theme`] and dereferences to it, so
//! `t.bg.card` and `t.kind(Kind::Image)` read the same at the call site and there
//! is still exactly one place a colour is decided.
//!
//! The file-kind hues extend `tokens::chart`, which is the sanctioned place for
//! categorical colours: the accent and the two status hues carry their usual
//! meaning, and the two additions (teal, lime) are picked at the same lightness so
//! no kind shouts louder than another in a mixed listing.
//!
//! [`Syntax`] is the second domain palette and the larger one, and it is here for the
//! same reason: `azur_egui_theme::contrast`'s own header says an application that adds
//! a role of its own should hold itself to the same floor **in its own tests**, which
//! is what `every_syntax_colour_can_be_read` does below.

use azur::tokens::{chart, palette};
use azur_egui_theme as azur;
use egui::Color32;

use crate::fs::fmt::Kind;
use crate::syntax::Tok;

/// The seven colours source code is set in.
///
/// One per [`Tok`] that gets a colour, and no more than that: a scheme with twenty
/// roles is one nobody can hold in their head, and at the size a preview pane draws
/// text the eye is sorting *kinds* of thing — is this prose, a literal, a name — not
/// grammar. What is deliberately **not** here is punctuation and plain identifiers:
/// they stay `text.primary`, which is both the majority of the characters on screen
/// and the reason the coloured ones read as marked. See [`crate::syntax`] for the
/// other half of that argument, which is about section count rather than taste.
///
/// Every one of them is measured against the surface it lands on, in both themes.
#[derive(Clone, Copy, Debug)]
pub struct Syntax {
    /// Prose inside code. Grey rather than a hue: a comment is the one thing here
    /// that is *not* part of the program, and greying it is what says so.
    pub comment: Color32,
    pub string: Color32,
    pub number: Color32,
    /// `if`, `return`, `fn` — and `true`, `null`, `None`, which are keywords for this
    /// purpose because they are words the language owns.
    pub keyword: Color32,
    /// A name that names a *thing*: a type, an XML tag, an `[ini]` section.
    pub kind: Color32,
    /// A name in the key position: a function being called, a JSON or YAML key, a CSS
    /// property, an XML attribute.
    pub name: Color32,
    /// A name standing for a value — `$PATH`, `%TEMP%`, `${prefix}`.
    pub variable: Color32,
    /// A line a diff adds, and one it takes away.
    ///
    /// **Not `status.success` and `status.danger`**, which is what these obviously want to be,
    /// and the reason is a measurement rather than a preference: those two roles are *marks* —
    /// a dot, a gauge, a glyph, an error line under a field — and neither of them clears 4.5:1
    /// at body size here. `status.danger` is 4.31:1 on the light panel and 4.28:1 on the dark
    /// code fill, and a diff is not a mark, it is a screenful of text somebody is reading. So
    /// the same two hues, one rung further from the surface.
    pub added: Color32,
    pub removed: Color32,
    /// The fill behind a code block and an inline code span in a rendered document.
    ///
    /// **`bg.canvas` on both sides** — the window's own colour, showing through the panel that
    /// is sitting on it, which is what makes a code block read as recessed rather than as a
    /// second panel. It falls out the same way in both themes even though the direction is
    /// opposite: paper's canvas is *darker* than its sheet, and the dark canvas is darker than
    /// its layer, so recessed is down either way.
    ///
    /// **Not `bg.layer_alt`**, which is the obvious choice and 2.2 ΔL\* off `bg.layer` in the
    /// dark theme — the same trap the find bar's own border fell into.
    ///
    /// It is measured twice over: far enough from `bg.layer` to read as a surface, and far
    /// enough from every colour above that each of them still clears AA **on it** rather than
    /// only on the panel. That second measurement is what chose it: on the neutral ramp's
    /// `GRAY_4` the transcribed comment green came to 4.20:1, and on the canvas it is 5.35:1.
    pub fill: Color32,
}

/// The five inks the status line is written in, **measured on the status line**.
///
/// Its own set because its surface is its own: every other status mark in this window sits on
/// `background-layer`, and the bar along the bottom of a pane is `background-layer-alt`. That is one
/// rung, and one rung is the difference between a figure you can read and a figure you cannot —
/// measured on the light bar, Azur's `status.success` comes to **2.23:1** and its `status.warning` to
/// **2.10:1**, which is under the floor for a shape, let alone for `13 changed`.
///
/// **The dark side is Azur's own**, unchanged: `status.*` and `accent.mark` measure 4.67:1 to 7.91:1
/// there, because the bar is nearly black and these hues are bright. **The light side is the same four
/// hues taken down** until each clears 4.5:1 on paper — which is the same thing [`Syntax`] does about
/// the same problem, and it is worth saying why it is not a design-system change: `status.warning` is
/// right for a message bar's icon, and what is wrong is asking a mark's colour to be a word's.
///
/// Every one of them is measured in `every_ink_on_the_status_line_can_be_read`, in both themes.
#[derive(Clone, Copy, Debug)]
pub struct Bar {
    /// The count of what is selected: the accent as a number rather than as a surface.
    pub counted: Color32,
    /// The branch — the one thing on the line that says the rest of it is git.
    pub info: Color32,
    /// Commits this branch has that its remote does not.
    pub success: Color32,
    /// And the ones the remote has that it does not.
    pub danger: Color32,
    /// How much of the working tree is changed, and a head that is on no branch at all.
    pub warning: Color32,
}

/// A palette the window can be set to, by name.
///
/// **Not a `bool` any more**, and that is the whole reason this type exists: the window used to
/// choose its palette with `dark: bool`, which has room for exactly two answers and put the
/// question in every signature it passed through. A third palette is a third *name*, and a fourth
/// is one line in [`Palette::ALL`] — the menu, `config.ini` and `--theme=` all read this list
/// rather than each spelling the answers out again.
///
/// Adding one is: a variant, a row in [`Palette::ALL`], its [`label`](Palette::label) and
/// [`key`](Palette::key), and an arm in [`Theme::of`]. Nothing else, and
/// `every_palette_is_reachable_and_round_trips` fails if any of those is forgotten.
///
/// Two of them at the moment, and the machinery is deliberately more than two needs — see
/// [`ALIASES`](Palette::ALIASES), which is the part that only exists because a palette has
/// already been renamed once.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Palette {
    /// What this application opens on.
    #[default]
    Dark,
    /// The light side: [`azur::Theme::light`]'s ink, accent and status hues on a set of blue
    /// chrome surfaces of its own. See [`Surfaces`], which is what a palette varies, and
    /// [`light`](mod@light) for the surfaces themselves.
    ///
    /// It was called `light-blue` for a while, which still parses — see
    /// [`ALIASES`](Palette::ALIASES).
    Light,
}

impl Palette {
    /// Every palette, in the order the menu lists them: the dark one first, because it is the
    /// default.
    pub const ALL: [Self; 2] = [Self::Dark, Self::Light];

    /// Names [`parse`](Palette::parse) still answers to, and what they mean now.
    ///
    /// **A settings file outlives the name in it.** `theme=light-blue` was a real value that real
    /// `config.ini` files were written with, and when that palette became simply the light one the
    /// word did not stop existing on disk. Dropping it would not have failed loudly either — an
    /// unreadable name falls back to the default — so somebody's window would have quietly opened
    /// dark one morning.
    ///
    /// The rule for adding to this list: an alias is for a name this program *used to write
    /// itself*, not for spellings somebody might guess. It is a compatibility record, and it
    /// should be readable as one.
    pub const ALIASES: [(&'static str, Self); 1] = [("light-blue", Self::Light)];

    /// What the menu under the application mark calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark theme",
            Self::Light => "Light theme",
        }
    }

    /// What `config.ini` and `--theme=` call it.
    ///
    /// `dark` and `light` are the two words the file has always used, so a `config.ini` written
    /// by an older build still reads correctly — which is the reason the key is spelled out here
    /// rather than derived from the variant name.
    pub fn key(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// The palette `text` names, or `None`.
    ///
    /// Case-insensitive, and `_` reads as `-` so that `light_blue` works as well as
    /// `light-blue` — a setting typed by hand should not turn on a hyphen. Retired names are
    /// answered too; see [`ALIASES`](Palette::ALIASES).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().replace('_', "-");
        let named = |key: &str| key.eq_ignore_ascii_case(&text);
        Self::ALL
            .into_iter()
            .find(|p| named(p.key()))
            .or_else(|| {
                Self::ALIASES
                    .into_iter()
                    .find(|(key, _)| named(key))
                    .map(|(_, p)| p)
            })
    }

    /// Whether this palette is a dark one, for the handful of decisions that really are about
    /// which side of the palette we are on rather than about which palette it is.
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

/// The surfaces this window's *regions* are painted with — the part a palette varies.
///
/// # Why these are not Azur roles
///
/// Because a palette wants to tell them apart and Azur's roles cannot. In Azur's light theme the
/// sidebar, the column headers and the status bar are all `background-layer-alt` and the title bar
/// is `background-layer`; [`Palette::Light`] gives those four surfaces three *different* colours,
/// and makes the panel and its headers the same one. No remapping of `layer` / `layer_alt`
/// expresses that, because the question it answers is "which region is this", and Azur's question
/// is "how far from the surface is this".
///
/// So this is one field per region the window paints, and [`Surfaces::from_azur`] is the mapping
/// every region had before regions were named — which is what [`Palette::Dark`] still uses, to
/// the byte.
///
/// **A region, not a widget.** What goes here is a surface some part of the window *is*; a fill a
/// control takes because it is hovered or pressed or selected stays Azur's, because that ladder
/// is about state and states are the same everywhere. `azur::desktop::hover_fill` is still the
/// one hover in this window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Surfaces {
    /// The strip the tabs sit in, and the bands of tabs below it when panes are stacked.
    pub titlebar: Color32,
    /// **One surface, three things**: the focused pane's active tab, the path bar under it, and
    /// the filter field at the right-hand end of that bar.
    ///
    /// They are welded on purpose — the tab is the visible end of the bar, which is why a
    /// hairline is not drawn between them — so they are one value and cannot drift apart. See
    /// [`crate::ui::bar`].
    pub bar: Color32,
    /// A pane's listing, and the surface a preview or a console panel sits on.
    pub panel: Color32,
    /// The column headers along the top of a listing.
    pub header: Color32,
    /// The panel down the left: drives, bookmarks, places.
    pub sidebar: Color32,
    /// The line along the bottom of a pane.
    pub status: Color32,
    /// **The line between two panels**, and nothing else — not the surface the path bar is
    /// painted with, which is [`bar`](Surfaces::bar).
    ///
    /// Those two were one value (`stroke-subtle`) until a palette wanted a pale bar and a visible
    /// seam, which one colour cannot be. In [`Palette::Dark`] they are still the same colour; they
    /// are simply no longer the same *decision*. See [`crate::ui::seam`], and
    /// [`crate::ui::field_outline`] for what that costs the palettes where they do coincide.
    pub separator: Color32,
}

impl Surfaces {
    /// Every region on the Azur role it already had, which is what [`Palette::Dark`] wears.
    ///
    /// Read it as the record of what the window painted before regions were named, because that
    /// is exactly what it is — `a_palette_with_no_surfaces_of_its_own_paints_what_it_always_did`
    /// holds it.
    pub fn from_azur(az: &azur::Theme) -> Self {
        Self {
            titlebar: az.bg.layer,
            bar: az.stroke.subtle,
            panel: az.bg.layer,
            header: az.bg.layer_alt,
            sidebar: az.bg.layer_alt,
            status: az.bg.layer_alt,
            separator: az.stroke.subtle,
        }
    }

    /// Every region, named, for the measurements over them.
    ///
    /// Exhaustive by construction — it destructures `self`, so adding a field to [`Surfaces`]
    /// stops compiling here until it is listed. A test that iterates six of seven regions is a
    /// test that passes because of the one it forgot.
    #[cfg(test)]
    pub fn each(&self) -> [(&'static str, Color32); 7] {
        let Self {
            titlebar,
            bar,
            panel,
            header,
            sidebar,
            status,
            separator,
        } = *self;
        [
            ("title bar", titlebar),
            ("path bar", bar),
            ("panel", panel),
            ("column headers", header),
            ("sidebar", sidebar),
            ("status bar", status),
            ("seam", separator),
        ]
    }
}

/// Azur, plus file kinds.
pub struct Theme {
    az: azur::Theme,

    /// Which palette this is. See [`Palette`].
    pub palette: Palette,

    /// What this window's regions are painted with. See [`Surfaces`].
    pub surfaces: Surfaces,

    /// A folder. Warm, so the places stand out from the things.
    pub folder: Color32,
    pub image: Color32,
    pub audio: Color32,
    pub video: Color32,
    pub archive: Color32,
    pub code: Color32,
    pub document: Color32,
    pub executable: Color32,
    pub font: Color32,
    /// Meshes, point clouds, CAD — the kinds this machine is full of.
    pub model: Color32,
    pub data: Color32,
    pub other: Color32,

    /// The bar behind a drive's used space, and the bar itself.
    pub gauge_track: Color32,
    /// A volume with almost nothing left, which is worth noticing.
    pub gauge_full: Color32,

    /// What the status line is written in. See [`Bar`].
    pub bar: Bar,

    /// What source code is coloured with.
    pub syntax: Syntax,
}

impl std::ops::Deref for Theme {
    type Target = azur::Theme;
    fn deref(&self) -> &Self::Target {
        &self.az
    }
}

/// The five colours [`Palette::Light`] is specified as, and the Azur roles they are carried
/// back into.
///
/// One module per palette that has surfaces of its own, named after it. [`Palette::Dark`] has none
/// — it takes every region straight from an Azur role, which is [`Surfaces::from_azur`].
///
/// Every value in this module is one of the five. Nothing is interpolated and nothing is
/// derived, which is the property that makes the palette checkable by reading it against the
/// request it came from.
///
/// # Why a bluer near-white is a darker one
///
/// The five regions below [`TITLEBAR`] were asked to be *more blue* without it, and the reason
/// they also moved in lightness is a property of sRGB rather than a liberty taken here: **the
/// chroma available to a colour collapses as it approaches white.** Measured at Lab hue 262, in
/// `C*`:
///
/// | lightness | the most chroma sRGB has there |
/// | --- | --- |
/// | `L*` 98 | 3.4 |
/// | `L*` 97 | 5.1 |
/// | `L*` 95 | 7.9 |
/// | `L*` 93 | 11.0 |
/// | `L*` 90.7 — [`TITLEBAR`]'s | 14.6 |
///
/// The previous values were already spending 61–76% of that ceiling, so there was no room to make
/// them bluer where they stood: the old bar was `L*` 97.69, where the bluest colour that exists at
/// all is `C*` 4.02, and it was at 3.06.
///
/// So the four content surfaces came **down 0.46 `L*` together** — the ladder keeps its shape
/// exactly, every rung in the same order the same distance apart — and each then takes **95% of
/// the chroma its new lightness allows**, which is as blue as sRGB goes there. That buys 1.3× to
/// 1.8× the chroma for about half a point of lightness, and the panel ends at `C*` 8.04 against
/// [`TITLEBAR`]'s 9.91, so the window now reads as one blue family rather than as blue chrome
/// around pale grey panels.
///
/// [`SEPARATOR`] moved for a second reason as well as chroma: the preview's checkerboard is
/// measured between the panel and `background-control-active`, and wants 8 ΔL\* between them. The
/// darker panel no longer left that, so the seam came down to `L*` 85.43 — where it can hold `C*`
/// 16.49, the most saturated thing in the palette, which suits the one surface whose whole job is
/// to be a boundary.
///
/// **[`TITLEBAR`] is untouched**, and so is everything reading `background-layer-alt` from it: the
/// context menus, the popovers, the tab band and the code-block fill. Those were judged right as
/// they were.
mod light {
    use egui::Color32;

    pub const TITLEBAR: Color32 = Color32::from_rgb(0xd8, 0xe6, 0xf7);
    pub const BAR: Color32 = Color32::from_rgb(0xf2, 0xf8, 0xff);
    pub const PANEL: Color32 = Color32::from_rgb(0xe6, 0xf1, 0xff);
    pub const STATUS: Color32 = Color32::from_rgb(0xeb, 0xf4, 0xff);
    pub const SEPARATOR: Color32 = Color32::from_rgb(0xc0, 0xd8, 0xf4);

    /// The regions, as given. `header` is [`PANEL`] because that is what was asked for — the
    /// headers and the listing under them are one surface here, where Azur's own light theme has
    /// them a rung apart. It stays a field of its own rather than becoming an alias: the next
    /// palette may well separate them again.
    pub fn surfaces() -> super::Surfaces {
        super::Surfaces {
            titlebar: TITLEBAR,
            bar: BAR,
            panel: PANEL,
            header: PANEL,
            // **[`TITLEBAR`], because the side panel is chrome.** It began as a rung of its own,
            // one step *lighter* than the panel, and read as too bright — which is the reading
            // Azur's own light theme agrees with: there the sidebar sits with the popovers and the
            // status bar on `background-layer-alt`, a step down from the listing, not up from it.
            //
            // It is the title bar's surface rather than a new value between the two because there
            // is no room for one: the panel and the title bar are 3.98 ΔL* apart, so anything
            // convincingly darker than the panel is within 2 ΔL* of the title bar anyway. Making
            // it the same surface says what the near-miss only implied — the strip along the top
            // and the panel down the left are one piece of chrome — and both of its edges are
            // drawn by a seam, so nothing depends on the two being told apart.
            sidebar: TITLEBAR,
            status: STATUS,
            separator: SEPARATOR,
        }
    }

    /// Carry the same six colours back into the Azur roles, for everything the window paints
    /// that is *not* one of the named regions.
    ///
    /// The regions above cover the window's structure; these five cover the rest of it — a
    /// context menu, a popover, a code block, a zebra stripe, a field, a dragged row. Without
    /// this the leftovers would keep Azur's own grey-blue ladder, and the one that shows is the
    /// tab band: it exists to stand in for the title bar for the row below it, so a band 3.9 ΔL*
    /// off the title bar is a band that has visibly failed at its only job.
    ///
    /// **Two rungs, not five.** [`PANEL`] is everything content sits on and [`TITLEBAR`] is
    /// everything set one step off it, which is the same two-rung shape Azur's light theme has —
    /// `SHEET` and `SHADE` — read off this palette's own colours instead of `paper`'s.
    pub fn apply(az: &mut azur_egui_theme::Theme) {
        // Content sits on the panel: a card, a field, a button at rest.
        az.bg.layer = PANEL;
        az.bg.card = PANEL;
        az.bg.control = PANEL;

        // One step off it. `layer_alt` is the popover surface — egui's `window_fill`, so every
        // menu and tooltip in the window — and it is also the tab band, an unfocused pane's
        // active tab, a hovered tab and the zebra stripe.
        az.bg.layer_alt = TITLEBAR;
        az.bg.control_disabled = TITLEBAR;
        az.row_alt = TITLEBAR;

        // The window's own colour, showing through the panels that sit on it: the fill behind a
        // code block and behind the console's log, which is what makes monospace read as
        // recessed. One step down from the panel, as in the light theme, and the same step the
        // popovers take.
        az.bg.canvas = TITLEBAR;

        // Lines, and the inert fill. `stroke.subtle` is every border the design system draws at
        // popover elevation plus the find bar's edge, and `control_active` is what a row or a tab
        // takes while it is dragged — both want the palette's darkest rung, which is the seam's.
        az.stroke.subtle = SEPARATOR;
        az.bg.control_active = SEPARATOR;

        // **The hover, in this palette's hue.** `azur::desktop::apply` has already put both hover
        // tokens on its own rung — see `Theme::of`, which runs it first for exactly this reason —
        // and that rung is the *light* theme's: Lab hue 272, which against hue-262 surfaces reads
        // as a lavender-grey band. So the band keeps its lightness to the hundredth and takes this
        // palette's hue instead.
        //
        // **Its lightness is not negotiable**, which is why only the hue and the chroma move:
        // `L*` 74.50 is a measured position — `azur::desktop::place_hover` has the argument, and
        // the short version is that the ink on a row does not change when the pointer arrives, so
        // a hover cannot cross to the other side of it. Holding `L*` exactly means every figure
        // already measured on this band still holds: a name on it is 7.32:1 against the 7.33:1 it
        // was, and the metadata columns 3.60:1.
        //
        // `C*` 18.2 and not the 37.9 that "as blue as sRGB allows" would give here: the ceiling
        // explodes away from white, and 95% of it at this lightness is a saturated sky blue rather
        // than a tinted neutral. A hover is a *surface*, so it is held to the palette's own range
        // — just past [`SEPARATOR`]'s 16.5, the most chromatic thing here.
        //
        // Both tokens, so a hovered row, button, tab, menu entry and card stay one colour.
        az.bg.control_hover = Color32::from_rgb(0x9f, 0xba, 0xd8);
        az.bg.card_hover = az.bg.control_hover;
    }
}

impl Theme {
    /// Every palette, built. The set the measurements below are taken over.
    ///
    /// A palette that is not in here is a palette nothing holds to a contrast floor, which is why
    /// this reads [`Palette::ALL`] rather than listing them: adding a variant enrols it in every
    /// test in this program that iterates palettes, without anybody having to remember to.
    #[cfg(test)]
    pub fn all() -> impl Iterator<Item = Self> {
        Palette::ALL.into_iter().map(Self::of)
    }

    /// One side by name, for a test that is about a side rather than about a palette — a geometry
    /// fixture, or a measurement whose whole point is the dark ramp against the light one.
    ///
    /// `#[cfg(test)]` because nothing else may choose a palette: the window's palette comes from
    /// `config.ini`, the menu or `--theme=`, and all three go through [`Theme::of`]. A second way
    /// in is how a hard-coded `Theme::dark()` ends up somewhere that should have followed the
    /// setting.
    #[cfg(test)]
    pub fn dark() -> Self {
        Self::of(Palette::Dark)
    }

    #[cfg(test)]
    pub fn light() -> Self {
        Self::of(Palette::Light)
    }

    /// The palette by name, which is the constructor everything else goes through.
    pub fn of(palette: Palette) -> Self {
        // Which side of Azur it is built on. **A palette is a variant of one of Azur's two sides
        // rather than a side of its own**, and that is the load-bearing part of this design: the
        // light palette takes Azur's light ink, accent, status and syntax colours unchanged and
        // varies only its surfaces, which is what keeps its contrast floors the ones already
        // measured. A palette that wanted its own ink would be a second design system.
        //
        // `is_dark` and not a match, so a palette added tomorrow says which side it belongs to
        // once, in one place, rather than in every arm that grows here.
        let mut az = if palette.is_dark() {
            azur::Theme::dark()
        } else {
            azur::Theme::light()
        };
        // The design system's application-window preset, which is where the whole of this
        // program's argument about hovers now lives: `azur::desktop` moves
        // `background-control-hover` and `background-card-hover` three rungs along the neutral
        // ramp, because most of what this window hovers is not a control — it is a row, a path
        // segment, a folder in a dropdown, a *place* — and one rung off a nearly black listing is
        // not enough to see.
        //
        // **One call, not one per call site.** That token is what the context menu, the column
        // headers, the application mark, the caption buttons, the tab strip, `ui::control_fills`
        // — and so Back, Forward, Up and Refresh — and the design system's own `MenuItem` all
        // read. Doing it at the call sites left those four buttons a rung and a half off
        // everything around them for a week, which is why the rule is now the design system's.
        //
        // It also brought the *light* theme back onto the ink's side of the palette: this file
        // used to hand light the dark theme's `GRAY_9`, which put near-black body text on a
        // hovered row at 3.26:1. See `azur_egui_theme::desktop`.
        //
        // **Before the palette below, so the palette has the last word.** It used to run after,
        // in `from_azur`, where a hover the palette had just set was silently put back.
        azur::desktop::apply(&mut az);
        let surfaces = match palette {
            // The dark palette wears the Azur roles it always did.
            Palette::Dark => Surfaces::from_azur(&az),
            Palette::Light => {
                light::apply(&mut az);
                light::surfaces()
            }
        };
        Self::from_azur(az, palette, surfaces)
    }

    fn from_azur(az: azur::Theme, palette: Palette, surfaces: Surfaces) -> Self {
        // Two hues `tokens::chart` does not name, at the same lightness as the
        // five it does, so a mixed listing has no loud row.
        const TEAL: Color32 = Color32::from_rgb(0x35, 0xc2, 0xb1);
        const LIME: Color32 = Color32::from_rgb(0xa8, 0xcc, 0x45);

        let dark = az.dark;
        Self {
            folder: if dark { palette::AMBER_70 } else { palette::AMBER_60 },
            image: chart::VIOLET,
            audio: chart::GREEN,
            video: chart::MAGENTA,
            archive: TEAL,
            code: az.accent.default,
            // Text-shaped things take the text colour: a folder of documents
            // should look like a folder of documents, not a fruit bowl.
            document: az.text.secondary,
            executable: if dark { palette::AZURE_90 } else { palette::AZURE_50 },
            font: az.text.tertiary,
            model: LIME,
            data: az.text.tertiary,
            other: az.text.tertiary,

            gauge_track: az.stroke.subtle,
            gauge_full: az.status.danger,

            // Each light value is the palette hue walked down in lightness until it cleared 4.5:1 on
            // `paper::SHADE`, and the figure it stopped at is in the comment. `AZURE_50` is the
            // palette's own rung and needed no new value; the other three are between rungs, because
            // this ramp has nothing below 60.
            //
            // **Walked twice.** `paper::SHADE` moved 2.3 ΔL* down when Azur's light theme took
            // its own cool ramp, which put three of the four under the floor — 4.39:1, 4.34:1 and
            // 4.35:1 — so the same walk again, from the same hues. It lands somewhere worth
            // writing down: all four inks are within **0.25** ΔL* of each other (L* 39.7 to 39.9),
            // which is not a rule anybody imposed. Four hues each stopping at the first rung that
            // clears 4.5:1 on one surface *is* a constant-lightness set, and that is the same
            // thing as saying no colour on this line shouts louder than another.
            //
            // **The figures beside them are not the ones that chose them.** They are measured on
            // the bar the light palette actually paints — `surfaces.status`, nine ΔL* lighter than
            // the `paper::SHADE` the walk used — so they read a ratio higher than it stopped at.
            // The values stay where it left them: it cleared the floor on the *darkest* bar this
            // program has put them on, and re-walking them lighter would tune them to one palette
            // and break them for the next. `every_ink_on_the_status_line_can_be_read` measures
            // whatever bar a palette has.
            bar: if dark {
                Bar {
                    counted: az.accent.mark,
                    info: az.status.info,
                    success: az.status.success,
                    danger: az.status.danger,
                    warning: az.status.warning,
                }
            } else {
                // The branch and the count are the same blue here, as they are in the dark theme:
                // `status.info` and `accent.mark` are one value there too, and the two are never
                // side by side.
                Bar {
                    counted: palette::AZURE_50,                   // 5.89:1
                    info: palette::AZURE_50,                      // 5.89:1
                    success: Color32::from_rgb(0x15, 0x6c, 0x2b), // 5.89:1
                    danger: Color32::from_rgb(0xb6, 0x21, 0x2a),  // 5.84:1
                    warning: Color32::from_rgb(0x7f, 0x56, 0x00), // 5.85:1
                }
            },

            // ---- The code palette ------------------------------------------
            //
            // **The dark side is a transcription**, of the seven
            // `editor.tokenColorCustomizations` this program's author reads code in every
            // day, and there is no better argument for a scheme than that. It is not a
            // design-system ramp and it is not trying to be: what it has to be is legible
            // on this window's two surfaces, and `every_syntax_colour_can_be_read`
            // measures every one of them.
            //
            // The light side is the same seven hues taken down far enough to read on
            // paper, and it is a **compromise rather than a translation**. The scheme
            // separates by *lightness* on black — five of the seven are in the warm
            // yellow-green family and are told apart by how bright they are — and a light
            // theme has no lightness to spend: every ink has to be dark, which compresses
            // five hues into one narrow band. Green stays green and blue stays blue, but
            // the gold, the khaki and the orange all land in the same third of the range.
            // See the note on the light `kind` below.
            syntax: if dark {
                Syntax {
                    comment: Color32::from_rgb(0x64, 0x8f, 0x51),
                    string: Color32::from_rgb(0xce, 0x91, 0x78),
                    number: Color32::from_rgb(0xac, 0xc4, 0xa0),
                    keyword: Color32::from_rgb(0x56, 0x9c, 0xd6),
                    kind: Color32::from_rgb(0xff, 0xd7, 0x00),
                    name: Color32::from_rgb(0xff, 0x80, 0x00),
                    variable: Color32::from_rgb(0xbd, 0xb7, 0x6b),
                    // A diff is coloured by the line and never carries a string or a comment
                    // token, so these two are free of the scheme above and are the plainest
                    // green and red that clear the floor on both surfaces.
                    added: chart::GREEN,
                    removed: Color32::from_rgb(0xff, 0x6b, 0x70),
                    fill: az.bg.canvas,
                }
            } else {
                Syntax {
                    comment: Color32::from_rgb(0x4d, 0x6e, 0x3e),
                    string: Color32::from_rgb(0xa0, 0x3a, 0x28),
                    number: Color32::from_rgb(0x3f, 0x5c, 0x2a),
                    keyword: Color32::from_rgb(0x1f, 0x5f, 0x96),
                    // The gold is the one that cannot survive the trip. `#ffd700` on paper
                    // is 1.4:1 — invisible — and a gold dark enough to read is an amber,
                    // which is a different colour rather than a darker one. This is that
                    // amber, and it is the reason the paragraph above calls the light side a
                    // compromise.
                    kind: Color32::from_rgb(0x8a, 0x5a, 0x00),
                    name: Color32::from_rgb(0x96, 0x4a, 0x00),
                    variable: Color32::from_rgb(0x6e, 0x60, 0x18),
                    added: Color32::from_rgb(0x17, 0x72, 0x2f),
                    removed: Color32::from_rgb(0xb3, 0x26, 0x1e),
                    fill: az.bg.canvas,
                }
            },
            az,
            palette,
            surfaces,
        }
    }

    /// The colour a token is set in.
    ///
    /// Total, because a [`Tok`] is only produced for something that gets a colour —
    /// punctuation and plain identifiers are the *absence* of a token, not a variant
    /// of one, and they keep `text.primary`.
    pub fn tok(&self, tok: Tok) -> Color32 {
        let s = &self.syntax;
        match tok {
            Tok::Comment => s.comment,
            Tok::Str => s.string,
            Tok::Number => s.number,
            Tok::Keyword => s.keyword,
            Tok::Kind => s.kind,
            Tok::Name => s.name,
            Tok::Variable => s.variable,
            Tok::Added => s.added,
            Tok::Removed => s.removed,
        }
    }

    /// The colour for a file kind.
    pub fn kind(&self, kind: Kind) -> Color32 {
        match kind {
            Kind::Folder => self.folder,
            Kind::Image => self.image,
            Kind::Audio => self.audio,
            Kind::Video => self.video,
            Kind::Archive => self.archive,
            Kind::Code => self.code,
            Kind::Document => self.document,
            Kind::Executable => self.executable,
            Kind::Font => self.font,
            Kind::Model => self.model,
            Kind::Data => self.data,
            Kind::Other => self.other,
        }
    }

    /// What a git state is coloured with.
    ///
    /// **The convention is posh-git's**, because that is the one most people already read every day
    /// in a prompt: green for what is in the index, red for what is not, cyan for the branch. Two
    /// deliberate departures, both because a status line is not a prompt:
    ///
    /// - **Changed is amber, not red.** posh-git's working-tree colour is `DarkRed`, and red in a
    ///   status line is where this window says *something has gone wrong*. Having uncommitted work is
    ///   the normal state of working, so it gets the role that means "worth noticing" rather than the
    ///   one that means "worth fixing". Red is kept for the two states that really are losses: a file
    ///   gone from disk, and a conflict.
    /// - **Untracked is grey, not red.** It is the state of every build artefact and editor backup on
    ///   the disk; colouring those alarms nobody twice.
    /// - **Staged is amber too, not green.** Green is the colour of *nothing left to do*, and a staged
    ///   file still has a commit owed on it — so it belongs with the other uncommitted work rather
    ///   than with the clean rows. The plus inside the disc is what tells it from a plain change,
    ///   which is the job the shape is there for.
    ///
    /// No new colours: these are Azur's status roles, which is what keeps them legible on both sides
    /// of the theme and moving with it if the palette does. Which role means which state is this
    /// program's decision, and it is here.
    pub fn git(&self, state: crate::git::State) -> Color32 {
        use crate::git::State;
        match state {
            State::Clean => self.status.success,
            State::Staged | State::Modified => self.status.warning,
            State::Deleted | State::Conflicted => self.status.danger,
            // A move is neither a gain nor a loss, and `info` is the role for a fact.
            State::Renamed => self.status.info,
            State::Untracked => self.text.tertiary,
        }
    }

    /// The bar colour for a volume that is `used` full, reddening only once the
    /// number is worth acting on.
    pub fn gauge(&self, used: f32) -> Color32 {
        if used >= 0.95 {
            self.gauge_full
        } else if used >= 0.85 {
            self.status.warning
        } else {
            self.accent.default
        }
    }

    /// The underlying Azur theme, for [`azur::install`].
    pub fn azur(&self) -> &azur::Theme {
        &self.az
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use azur::contrast::{apart, ratio, SAME, SHAPE, TEXT};

    /// Every palette can be named, saved, read back and built — which is the whole of what
    /// [`Palette::ALL`] promises.
    ///
    /// The list is what the menu, `config.ini` and `--theme=` all read, so a variant that is
    /// missing from it is a palette with no way in and no way out. Each of the four things a new
    /// variant has to bring is checked here rather than trusted: a key, a label, a round trip
    /// through [`Palette::parse`], and an arm in [`Theme::of`] that actually answers with it.
    #[test]
    fn every_palette_is_reachable_and_round_trips() {
        for p in Palette::ALL {
            assert_eq!(Palette::parse(p.key()), Some(p), "{:?} does not parse back", p);
            // The two spellings a setting typed by hand can take, and the case it can arrive in.
            assert_eq!(Palette::parse(&p.key().to_uppercase()), Some(p), "{:?}", p);
            assert_eq!(Palette::parse(&p.key().replace('-', "_")), Some(p), "{:?}", p);
            assert_eq!(Palette::parse(&format!("  {}  ", p.key())), Some(p), "{:?}", p);
            // And `Theme::of` answers with the palette it was asked for, rather than with
            // whichever side of Azur it happens to be built on.
            assert_eq!(Theme::of(p).palette, p, "{:?} builds as another palette", p);
        }
        // Distinct names, both kinds. Two palettes sharing a key would make `config.ini`
        // ambiguous, and two sharing a label would make the menu unreadable.
        for (i, a) in Palette::ALL.iter().enumerate() {
            for b in &Palette::ALL[i + 1..] {
                assert_ne!(a.key(), b.key(), "{a:?} and {b:?} share a key");
                assert_ne!(a.label(), b.label(), "{a:?} and {b:?} share a label");
            }
        }
        assert_eq!(Palette::parse("chartreuse"), None);
        assert_eq!(Palette::parse(""), None);
        // **Every retired name still answers**, which is the half of this that has already gone
        // wrong once: `light-blue` was written into real settings files, and a name that stops
        // parsing does not fail loudly — it falls back to the default and opens the window dark.
        for (retired, means) in Palette::ALIASES {
            assert_eq!(Palette::parse(retired), Some(means), "the alias {retired} stopped working");
            assert_eq!(Palette::parse(&retired.to_uppercase()), Some(means), "{retired}");
            assert_eq!(Palette::parse(&retired.replace('-', "_")), Some(means), "{retired}");
            // And an alias is a *retired* name: one that is still a palette's own key is a
            // duplicate entry rather than a compatibility record.
            assert!(
                !Palette::ALL.iter().any(|p| p.key() == retired),
                "{retired} is both a live key and an alias"
            );
        }
        // The default is what the window opens on, and `Config::default` leans on it.
        assert_eq!(Palette::default(), Palette::Dark);
        // Exactly one palette per side, said as a sweep rather than a list, so a palette added
        // later is covered without this line being edited.
        assert!(Palette::Dark.is_dark());
        assert!(Palette::ALL.iter().any(|p| p.is_dark()), "no palette is a dark one");
        assert!(Palette::ALL.iter().any(|p| !p.is_dark()), "no palette is a light one");
    }

    /// A palette that names no region of its own paints what the window always painted.
    ///
    /// [`Surfaces`] was extracted so that a palette could tell the sidebar from the status bar
    /// from the column headers. [`Palette::Dark`] must not have moved a pixel while that happened,
    /// and this is what says so: every region back on the Azur role the call site used to name
    /// directly.
    ///
    /// It is also the map. Reading it against [`Surfaces::from_azur`] is how you find out what a
    /// region *was*, which is the first question to ask when a palette looks wrong.
    #[test]
    fn a_palette_with_no_surfaces_of_its_own_paints_what_it_always_did() {
        // The dark palette names no region: it takes all seven off an Azur role, exactly as every
        // call site did before regions had names. This is the record of which role each one was.
        let t = Theme::dark();
        let s = t.surfaces;
        assert_eq!(s.titlebar, t.bg.layer, "the title bar");
        assert_eq!(s.panel, t.bg.layer, "a pane");
        assert_eq!(s.header, t.bg.layer_alt, "the column headers");
        assert_eq!(s.sidebar, t.bg.layer_alt, "the sidebar");
        assert_eq!(s.status, t.bg.layer_alt, "the status bar");
        assert_eq!(s.bar, t.stroke.subtle, "the path bar");
        assert_eq!(s.separator, t.stroke.subtle, "the seam");
        // And the two that were one value: still one value here, which is the point — the split is
        // a split of the *decision*, not of this palette's colours.
        assert_eq!(s.bar, s.separator);
        assert_eq!(s, Surfaces::from_azur(t.azur()), "the dark palette has drifted off the map");

        // Whereas a palette with surfaces of its own is the case they had to come apart for.
        let light = Theme::light();
        assert_ne!(
            light.surfaces.bar, light.surfaces.separator,
            "the light palette wants a pale bar and a seam you can see; if these are equal it has \
             lost the distinction this struct was extracted for"
        );
        assert_ne!(
            light.surfaces,
            Surfaces::from_azur(light.azur()),
            "the light palette is painting Azur's roles, so `light::surfaces` never ran"
        );
    }

    /// Every region of the window carries the window's text, in every palette.
    ///
    /// The floor a new palette is actually at risk of missing. A palette arrives as a list of
    /// surfaces somebody picked by eye, and the thing nobody checks by eye is whether the ink —
    /// which the palette does *not* restate — still reads on all of them. Seven regions times
    /// every palette, and the tightest is the light palette's seam at 10.03:1.
    ///
    /// `text.secondary` is held to [`SHAPE`] rather than [`TEXT`] for the reason the design
    /// system holds it there: it is the metadata columns, read at a glance beside a name.
    #[test]
    fn every_region_carries_the_windows_text() {
        for t in Theme::all() {
            let name = t.palette.key();
            for (what, fill) in t.surfaces.each() {
                let got = ratio(t.text.primary, fill);
                assert!(
                    got >= TEXT,
                    "{name}: a name on the {what} is {got:.2}:1, under the {TEXT}:1 floor"
                );
                let got = ratio(t.text.secondary, fill);
                assert!(
                    got >= SHAPE,
                    "{name}: metadata on the {what} is {got:.2}:1, under the {SHAPE}:1 floor"
                );
            }
        }
    }

    /// The line between two panels reads against both of the surfaces it divides.
    ///
    /// Measured in ΔL\*, which is the ruler for two surfaces meeting rather than for ink on a
    /// fill — see `azur::contrast`, whose header is about exactly this. A seam is the one thing
    /// in this window that has no job except to be a boundary, so a palette whose seam has
    /// drifted onto one of its neighbours has lost the window's structure and nothing else here
    /// would notice.
    ///
    /// Every region, not only the panel: the seam runs along the sidebar's edge, under the title
    /// bar's tabs and above a preview panel, so it meets nearly all of them somewhere.
    #[test]
    fn the_separator_reads_against_everything_it_divides() {
        for t in Theme::all() {
            let name = t.palette.key();
            for (what, fill) in t.surfaces.each() {
                if fill == t.surfaces.separator {
                    continue;
                }
                let got = apart(t.surfaces.separator, fill);
                assert!(
                    got >= SAME,
                    "{name}: the seam is only {got:.1} ΔL* from the {what}"
                );
            }
        }
    }

    /// Every colour source code is set in, on every surface it is set on, in both themes.
    ///
    /// The measurement the design system's `contrast` module asks an application to make about
    /// roles of its own. **Two surfaces, not one**, and the second is the one that decides
    /// things: a fenced code block puts the whole palette on [`Syntax::fill`] rather than on the
    /// panel, and it is what chose the fill — on the neutral ramp's `GRAY_4` the transcribed
    /// comment green came to 4.20:1, and on the canvas it is 5.34:1.
    ///
    /// Every value clears 4.5:1 twice over. The two tightest are the comments, which is the
    /// right place for the margin to be thin: 4.72:1 for the light green on a code block and
    /// 4.79:1 for the dark one on the panel. `status.danger` measured 4.28:1 on the dark fill,
    /// which is why a diff has [`Syntax::removed`] of its own instead.
    #[test]
    fn every_syntax_colour_can_be_read() {
        for t in Theme::all() {
            let side = t.palette.key();
            let s = t.syntax;
            let inks = [
                ("comment", s.comment),
                ("string", s.string),
                ("number", s.number),
                ("keyword", s.keyword),
                ("kind", s.kind),
                ("name", s.name),
                ("variable", s.variable),
                ("added", s.added),
                ("removed", s.removed),
                // The body's own ink is in the list because most of a source file is set in
                // it: an unlisted identifier and every piece of punctuation.
                ("plain", t.text.primary),
            ];
            for (surface, on) in [("panel", t.surfaces.panel), ("code fill", s.fill)] {
                for (role, ink) in inks {
                    let got = ratio(ink, on);
                    assert!(
                        got >= TEXT,
                        "{side}: {role} on the {surface} is {got:.2}:1, under {TEXT}"
                    );
                }
            }
            // And the fill has to read as a surface in its own right, or a fenced block is a
            // box nobody can see. Measured in ΔL*, which is the ruler for two surfaces meeting
            // — `bg.layer_alt` is the obvious candidate and fails this at 2.2 in the dark.
            let got = apart(s.fill, t.surfaces.panel);
            assert!(
                got >= SAME,
                "{side}: the code fill is {got:.1} ΔL* off the panel, under {SAME}"
            );
        }
    }

    /// No two of them are the same colour.
    ///
    /// Not a contrast floor but a distinctness one, and it is the thing a palette actually gets
    /// wrong: a scheme where two roles have drifted onto the same swatch has stopped
    /// distinguishing the two things it exists to distinguish, and nothing else here would
    /// notice — every ratio would still pass.
    #[test]
    fn no_two_syntax_roles_share_a_colour() {
        for t in Theme::all() {
            let side = t.palette.key();
            let s = t.syntax;
            let roles = [
                ("comment", s.comment),
                ("string", s.string),
                ("number", s.number),
                ("keyword", s.keyword),
                ("kind", s.kind),
                ("name", s.name),
                ("variable", s.variable),
                ("removed", s.removed),
            ];
            for (i, (one, a)) in roles.iter().enumerate() {
                for (other, b) in &roles[i + 1..] {
                    assert_ne!(a, b, "{side}: {one} and {other} are the same colour");
                }
            }
            // A diff's two colours are not part of the scheme and are allowed to sit near
            // something in it — `removed` is close to the light theme's string red. What they
            // may not do is collide with each other, which is the only pair in a diff.
            assert_ne!(s.added, s.removed, "{side}");
        }
    }
}
