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
/// Eight of them at the moment, six of which are one rule with a different mineral in it — see
/// [`family`](mod@family) — and the machinery is deliberately more than that needs: see
/// [`ALIASES`](Palette::ALIASES), which only exists because a palette has already been renamed
/// once.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Palette {
    /// What this application opens on: Azur's own dark theme, graphite and azure.
    #[default]
    Dark,
    /// Black. [`Dark`](Self::Dark)'s ladder sunk 3.5 ΔL\*, dead neutral, on a canvas of pure
    /// black. See [`family::ONYX`](mod@family).
    Onyx,
    /// Blue, and the palette the family was cut for: [`Dark`](Self::Dark)'s ladder rung for rung
    /// in stone and cobalt blue instead of graphite and azure. See [`family`](mod@family), which
    /// is where the hue and the two chromas are argued.
    Cobalt,
    /// Green. See [`family::MALACHITE`](mod@family).
    Malachite,
    /// Purple. See [`family::AMETHYST`](mod@family).
    Amethyst,
    /// Yellow. See [`family::CITRINE`](mod@family).
    Citrine,
    /// Metal: the family's ladder lifted as far as the measurements allow, with the rest of its
    /// brightness in the pigment. See [`family::SILVER`](mod@family).
    Silver,
    /// The light side: [`azur::Theme::light`]'s ink, accent and status hues on a set of blue
    /// chrome surfaces of its own. See [`Surfaces`], which is what a palette varies, and
    /// [`light`](mod@light) for the surfaces themselves.
    ///
    /// It was called `light-blue` for a while, which still parses — see
    /// [`ALIASES`](Palette::ALIASES).
    Light,
}

impl Palette {
    /// Every palette, in the order the menu lists them: Azur's own dark theme first because it is
    /// the default, then [the family](mod@family) up its own ladder — black, the four hues, metal
    /// — and the light one last.
    pub const ALL: [Self; 8] = [
        Self::Dark,
        Self::Onyx,
        Self::Cobalt,
        Self::Malachite,
        Self::Amethyst,
        Self::Citrine,
        Self::Silver,
        Self::Light,
    ];

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
            Self::Onyx => "Onyx theme",
            Self::Cobalt => "Cobalt theme",
            Self::Malachite => "Malachite theme",
            Self::Amethyst => "Amethyst theme",
            Self::Citrine => "Citrine theme",
            Self::Silver => "Silver theme",
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
            Self::Onyx => "onyx",
            Self::Cobalt => "cobalt",
            Self::Malachite => "malachite",
            Self::Amethyst => "amethyst",
            Self::Citrine => "citrine",
            Self::Silver => "silver",
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
        !matches!(self, Self::Light)
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

/// What each part of a row is drawn in. See [`Theme::diff_inks`].
pub struct DiffInks {
    pub name: Color32,
    /// The dimmed half of the Name cell, and any figure with nothing to say.
    pub meta: Color32,
    pub kind: Color32,
    pub size: Color32,
    pub modified: Color32,
}

impl DiffInks {
    /// A row outside a diff: the name in its ink, and every figure in the meta one.
    pub fn plain(name: Color32, meta: Color32) -> Self {
        Self {
            name,
            meta,
            kind: meta,
            size: meta,
            modified: meta,
        }
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

/// The **stone family**: six palettes cut by one rule, and the Azur roles they are carried into.
///
/// # One rotation, and nothing else
///
/// None of these is a new dark theme. Each is [`Palette::Dark`]'s window in a different
/// **material**: every rung below replaces a graphite or an azure one **at that rung's own
/// relative luminance**, so a palette is a hue and a chroma and nothing more.
///
/// That is the load-bearing property, because [`azur::contrast`] measures in the two quantities
/// luminance decides — [`ratio`](azur::contrast::ratio) is a function of it and
/// [`lstar`](azur::contrast::lstar) is a function of it — so **every contrast figure and every
/// ΔL\* in this program carries over.** The seam is 3.93 ΔL\* off a column header in all six as
/// it is in the dark palette, the checkerboard 15.4, the selection bar 3.1:1 on the row it marks,
/// `status.danger` 4.67:1 on the status line. Nothing was retuned, and nothing needed to be.
/// `every_rung_is_the_dark_palettes_own_lightness` is the guard: it holds every rung to 0.25
/// ΔL\* of the ladder position it stands in for, which is half a byte step down here and no
/// more — the worst in the six is 0.21.
///
/// It also means none of them **names a region of its own**: the seven [`Surfaces`] come off Azur
/// roles through [`Surfaces::from_azur`] exactly as [`Palette::Dark`]'s do, and what moved is what
/// those roles *are*. Compare [`light`](mod@light), which had to name regions because it wanted
/// the sidebar and the status bar told apart — a question about structure, where this one is about
/// colour.
///
/// # The mineral picks the hue *and* the chroma
///
/// Each palette is measured off the stone it is named for, and both of its numbers fall out of
/// that one measurement:
///
/// | palette | the stone | `L*` | `C*` | Lab hue | share of sRGB's chroma at that lightness |
/// | --- | --- | --- | --- | --- | --- |
/// | [`COBALT`](family::COBALT) | cobalt blue `#0047AB` | 32.80 | 62.64 | 291.1 | **100.0%** |
/// | [`MALACHITE`](family::MALACHITE) | malachite `#0BDA51` | 76.56 | 89.42 | 143.1 | 99.3% |
/// | [`CITRINE`](family::CITRINE) | citrine `#E4D00A` | 82.72 | 82.69 | 96.8 | 98.9% |
/// | [`AMETHYST`](family::AMETHYST) | amethyst `#9966CC` | 52.55 | 60.72 | 311.6 | **61.3%** |
/// | [`ONYX`](family::ONYX) | onyx `#353839` | 23.27 | 1.50 | — | 0% |
/// | [`SILVER`](family::SILVER) | silver `#C0C0C0` | 77.70 | 0.01 | — | 0% |
///
/// **The last column is the interesting one.** Three of these stones sit *on* the sRGB gamut
/// boundary at their own hue and lightness — cobalt blue exactly on it — because they are
/// pigments, and a pigment is as saturated as its medium allows. Amethyst is a pale violet quartz
/// and takes 61% of what is there, which is why the amethyst palette reads as a gem rather than as
/// neon. Nobody chose that; it was measured.
///
/// So: **the pigment ramp takes the share its mineral takes, and the stone ramp takes half of
/// that.** Half because a rock is not a pigment — the panels have to read as *dark* first and
/// coloured second, or the listing competes with what it is listing.
///
/// **A share of a ceiling and not a fixed chroma**, because the ceiling is not flat: it collapses
/// toward white and opens up toward black. [`light`](mod@light)'s table is the same measurement
/// from the other end — there the bluest colour that exists at `L*` 98 is `C*` 3.4 — and down
/// here, at cobalt's hue, it runs the other way:
///
/// | rung | `L*` | the most chroma sRGB has there | cobalt takes |
/// | --- | --- | --- | --- |
/// | [`Cut::layer`](family::Cut::layer) — the panel | 7.55 | 30.2 | 15.1 |
/// | [`Cut::control_hover`](family::Cut::control_hover) — the hover | 34.77 | 65.0 | 33.1 |
/// | [`Cut::accent_active`](family::Cut::accent_active) — a selected row | 32.26 | 61.8 | 61.6 |
/// | [`Cut::accent_mark`](family::Cut::accent_mark) — the accent as ink | 64.37 | 59.1 | 58.4 |
///
/// Read the last two rows together: the accent's fill is *more* chromatic than the ink above it
/// even though it is half as bright, which is why cobalt reads as a deep colour rather than a
/// bright one. It is also why [`Cut::accent_mark`](family::Cut::accent_mark) stays at azure's own
/// ink rung instead of coming down to where the ceiling is higher. That was tried — `L*` 60 buys
/// `C*` 66 — and it costs the
/// one figure the ink cannot spend: the 2px selection bar is `accent.mark` **on** `accent.active`,
/// and at `L*` 60 that falls to 2.69:1, under the 3:1 a shape wants. At the rung it kept it is
/// 3.13:1, which is the 3.14:1 the dark palette measures. See `filelist::rows::cursor_ink`.
///
/// # The two with no hue
///
/// [`ONYX`](family::ONYX) and [`SILVER`](family::SILVER) cannot be told apart by a rotation,
/// having nothing to rotate. They are told apart by **where their ladder sits**, and each then
/// has to do by hand the one job a hue
/// was doing for the others:
///
/// - **Onyx sinks 3.5 ΔL\***, and its canvas clamps at `#000000` — the window's own colour is
///   black, 4.0 ΔL\* under the panel that sits on it, which still reads as a surface. Its pigment
///   stays on Azur's own lightnesses: a plain mid-grey selection. Matte.
/// - **Silver lifts 1.7 ΔL\***, and its pigment goes the other way — a bright steel selection and
///   near-white marks, `accent.mark` at `L*` 84.9 against Azur's 64.4.
///
/// **1.7 is the whole of the room there is, and it is a measurement rather than a preference.**
/// Two values stop a dark palette lifting its surfaces: `status.danger` drops under 4.5:1 on a
/// status bar lighter than `L*` 11.52, and the code comment's green drops under 4.5:1 on a panel
/// lighter than `L*` 10.13. The dark palette's own rungs are 9.72 and 7.55, so the room is **1.80
/// and 2.58 ΔL\*** — and 1.7 is inside both. A palette that wanted a genuinely pale window would
/// need a [`Bar`] and a [`Syntax`] of its own, which is exactly what [`Palette::Light`] has;
/// Silver's brightness is in its marks instead, where nothing else is measured against it.
///
/// # The hover, which is the other thing a hue was doing
///
/// `azur::desktop::row_fill` fills a selected row with `accent-active` and a hovered one with
/// `background-control-hover`, and in the dark palette those two are **2.5 ΔL\* apart**: a
/// selected row is told from a hovered one almost entirely by *hue*. The four chromatic palettes
/// inherit that and it works. The two hueless ones would have a hovered row and a selected row in
/// the same grey, so both bring the hover **down** — Onyx to `L*` 26.2 and Silver to 28.0, which
/// leaves 6.1 and 11.9 clear of their selections. The floor under that is
/// `the_hueless_palettes_tell_a_selected_row_from_a_hovered_one`, and it is asserted only for
/// those two: for the others the same
/// measurement would fail, correctly.
///
/// It costs a little tidiness. Sixteen rungs on one grey ramp crowd, so Onyx's hover lands within
/// 1.3 ΔL\* of its field outline and of a pressed badge. Neither of those is a row, which is the
/// only place the question gets asked.
///
/// # The one thing these palettes cannot reach
///
/// `azur::desktop::press_fill` is a function of `t.dark` rather than a token, so the fill a
/// control takes **while the button is held** is Azur's `GRAY_9` in every dark palette. Its
/// lightness is right — every [`Cut::stroke_strong`](family::Cut::stroke_strong) here is the same
/// `L*` — but its hue is graphite, so in the four chromatic palettes a pressed toolbar button
/// flashes grey where its
/// hover was coloured. The hover is reachable because `azur::desktop::apply` writes it into
/// `background-control-hover` and `hover_fill` reads the token back; the press has no token behind
/// it. Fixing it belongs in the design system, and until then it is a transient state whose ladder
/// position is correct — and in [`ONYX`](family::ONYX) and [`SILVER`](family::SILVER) it is
/// simply right.
mod family {
    use egui::Color32;

    /// `0xrrggbb`, so a sixteen-rung table fits on sixteen lines and can be read down the column
    /// as a ramp. Every value in this module is one of these and nothing is derived at runtime.
    const fn hex(v: u32) -> Color32 {
        Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// One palette of the family, as the sixteen rungs it is cut from.
    ///
    /// Eight of stone and eight of pigment, named after the Azur role each one *leads* — the fan
    /// out to the roles that share it is [`Cut::apply`], which is the only place the mapping
    /// exists. Read the two halves as ramps: each field is one step further from the surface than
    /// the one above it, in the order [`Palette::Dark`](super::Palette::Dark) climbs its own.
    ///
    /// There is no `hover` field on the pigment side and no `press` anywhere: a press is
    /// `azur::desktop`'s and the accent's hover is [`Cut::accent_hover`].
    #[derive(Clone, Copy)]
    pub struct Cut {
        /// The window itself, and so the fill behind a code block and the console's log.
        pub canvas: Color32,
        /// A pane's listing, the title bar, a card, a control at rest.
        pub layer: Color32,
        /// One step off it: the popover surface, the column headers, the sidebar, the status bar,
        /// the tab band, the zebra stripe.
        pub layer_alt: Color32,
        /// The seam, the path bar and the tab welded to it, and a field's fill.
        pub control: Color32,
        /// The loudest neutral: a panel edge, the find bar's outline, the checkerboard's dark
        /// square, and the inert fill a row takes while it is dragged.
        pub control_active: Color32,
        /// A control's own outline, and the scrollbar.
        pub stroke_control: Color32,
        /// The band under the pointer, on anything you hover in order to *go* somewhere.
        pub control_hover: Color32,
        /// The outline of a control under the pointer, and the cursor ring off a selection.
        pub stroke_strong: Color32,

        /// An accent-tinted fill quiet enough to put text on: every other hit in the find bar, an
        /// accent badge, and a selection in a pane that has not got the keyboard.
        pub accent_subtle: Color32,
        pub accent_subtle_hover: Color32,
        pub accent_subtle_active: Color32,
        /// **A selected row, at rest.** The whole reason the pigment ramp is walked from this end:
        /// a selected row is read *through*, so its fill goes down the ramp rather than up it.
        pub accent_active: Color32,
        /// A selected row under the pointer — and a primary button, the current hit in the find
        /// bar, and a `Kind::Code` glyph.
        pub accent_default: Color32,
        /// A primary button under the pointer.
        pub accent_hover: Color32,
        /// The accent as **ink**: the 2px bar down a selected row, the cursor ring on one, the
        /// focus ring, the selected count on the status line, and the branch beside it.
        pub accent_mark: Color32,
        /// A link in a rendered document, which Azur puts a rung above the ink because it is read
        /// on the canvas rather than through it.
        pub link: Color32,
    }

    impl Cut {
        /// Carry the sixteen rungs into every Azur role that wears one, and answer with the seven
        /// regions that fall out.
        ///
        /// Exhaustive over the neutral *surfaces* and the accent, and deliberately not over the
        /// ink: `text.primary` down to `text.disabled` stay Azur's, for the reason
        /// [`Theme::of`](super::Theme::of) gives — a palette that wanted its own ink would be a
        /// second design system, and those four are a near-white and three greys whose job is to
        /// recede. [`Cut::link`] is the exception because a link is accent-coloured by role.
        ///
        /// The status hues, the file-kind hues and [`Syntax`](super::Syntax) are untouched for
        /// that reason and one more: they are *categorical*, and a category that changes colour
        /// with the palette is a category nobody can learn. Every one of them is measured on these
        /// surfaces by the tests below, and passes on the luminance the surfaces kept.
        pub fn apply(&self, az: &mut azur_egui_theme::Theme) -> super::Surfaces {
            az.bg.canvas = self.canvas;

            az.bg.layer = self.layer;
            // A disabled control sinks back into the surface, as it does in Azur's dark theme.
            az.bg.control_disabled = self.layer;

            az.bg.layer_alt = self.layer_alt;
            az.bg.card = self.layer_alt;
            az.row_alt = self.layer_alt;

            az.bg.control = self.control;
            az.stroke.subtle = self.control;

            az.bg.control_active = self.control_active;
            az.stroke.default = self.control_active;
            // Here rather than on the pigment ramp, because the point of a disabled accent is
            // that it stops reading as live.
            az.accent.disabled = self.control_active;

            az.stroke.control = self.stroke_control;
            az.scrollbar.thumb = self.stroke_control;

            // **The hover, in this palette's material.** `azur::desktop::apply` has already put
            // both hover tokens on its own rung — see `Theme::of`, which runs it first for exactly
            // this reason — and that rung is `GRAY_8`, graphite. Both tokens, so a hovered row,
            // button, tab, menu entry and card stay one colour.
            az.bg.control_hover = self.control_hover;
            az.bg.card_hover = self.control_hover;

            az.stroke.strong = self.stroke_strong;
            az.scrollbar.thumb_hover = self.stroke_strong;

            // The scrim behind a modal: the canvas at 72%, which is what Azur's own is — its
            // `rgba(5,6,7,.72)` is `GRAY_0` less a byte or two.
            let c = self.canvas;
            az.bg.overlay = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 184);

            az.accent.subtle = self.accent_subtle;
            az.status.info_subtle = self.accent_subtle;
            az.accent.subtle_hover = self.accent_subtle_hover;
            az.accent.subtle_active = self.accent_subtle_active;

            az.accent.active = self.accent_active;
            az.accent.default = self.accent_default;
            az.accent.hover = self.accent_hover;

            az.accent.mark = self.accent_mark;
            az.stroke.focus = self.accent_mark;
            az.status.info = self.accent_mark;

            az.text.link = self.link;

            super::Surfaces::from_azur(az)
        }
    }

    /// Black. The family's ladder sunk 3.5 ΔL\*, dead neutral, on a canvas of pure black.
    pub const ONYX: Cut = Cut {
        canvas: hex(0x000000),               // L*  0.00  C*  0.0
        layer: hex(0x0e0e0e),                // L*  3.97
        layer_alt: hex(0x141414),            // L*  6.32
        control: hex(0x1c1c1c),              // L* 10.27
        control_active: hex(0x2f2f2f),       // L* 19.40
        stroke_control: hex(0x3b3b3b),       // L* 24.87
        control_hover: hex(0x3e3e3e),        // L* 26.21 — its own; see "the hover"
        stroke_strong: hex(0x595959),        // L* 37.82
        accent_subtle: hex(0x191919),        // L*  8.76
        accent_subtle_hover: hex(0x292929),  // L* 16.59
        accent_subtle_active: hex(0x3a3a3a), // L* 24.42
        accent_active: hex(0x4c4c4c),        // L* 32.32
        accent_default: hex(0x5d5d5d),       // L* 39.49
        accent_hover: hex(0x757575),         // L* 49.24
        accent_mark: hex(0x9c9c9c),          // L* 64.36
        link: hex(0xc4c4c4),                 // L* 79.16
    };

    /// Blue. Lab hue 291.1 at the whole of sRGB's chroma, which is cobalt blue itself:
    /// [`accent_active`](Cut::accent_active) is `#0046A8` against the pigment's `#0047AB`.
    pub const COBALT: Cut = Cut {
        canvas: hex(0x07051c),               // L*  2.15  C* 13.3
        layer: hex(0x131529),                // L*  7.54  C* 15.1
        layer_alt: hex(0x171930),            // L*  9.73  C* 17.0
        control: hex(0x1c213d),              // L* 13.66  C* 20.0
        control_active: hex(0x2e345b),       // L* 22.97  C* 26.0
        stroke_control: hex(0x38406c),       // L* 28.35  C* 28.4
        control_hover: hex(0x464e83),        // L* 34.78  C* 33.1
        stroke_strong: hex(0x555d9b),        // L* 41.42  C* 37.5
        accent_subtle: hex(0x001642),        // L*  8.71  C* 32.6
        accent_subtle_hover: hex(0x012561),  // L* 16.47  C* 41.6
        accent_subtle_active: hex(0x013586), // L* 24.56  C* 53.1
        accent_active: hex(0x0046a8),        // L* 32.27  C* 61.6 — the pigment
        accent_default: hex(0x0056cd),       // L* 39.69  C* 72.1
        accent_hover: hex(0x006cfc),         // L* 49.20  C* 83.9
        accent_mark: hex(0x8095fe),          // L* 64.35  C* 58.4
        link: hex(0xbcbfff),                 // L* 79.14  C* 34.2
    };

    /// Green. Lab hue 143.1 at 99.3% of the ceiling — malachite is a copper pigment and sits
    /// almost exactly on the gamut boundary, as cobalt does.
    pub const MALACHITE: Cut = Cut {
        canvas: hex(0x050905),               // L*  2.16  C*  2.0
        layer: hex(0x0f190f),                // L*  7.51  C*  8.0
        layer_alt: hex(0x111e11),            // L*  9.71  C* 11.4
        control: hex(0x152716),               // L* 13.64  C* 15.0
        control_active: hex(0x233d24),       // L* 23.07  C* 20.4
        stroke_control: hex(0x2c4a2d),       // L* 28.44  C* 22.9
        control_hover: hex(0x375a39),        // L* 34.84  C* 25.4
        stroke_strong: hex(0x436b44),        // L* 41.42  C* 28.8
        accent_subtle: hex(0x031e01),        // L*  8.57  C* 20.2
        accent_subtle_hover: hex(0x003109),  // L* 16.58  C* 32.4
        accent_subtle_active: hex(0x004513), // L* 24.65  C* 39.8
        accent_active: hex(0x00591b),        // L* 32.31  C* 47.3
        accent_default: hex(0x016d23),       // L* 39.69  C* 54.4
        accent_hover: hex(0x03882e),         // L* 49.28  C* 63.6
        accent_mark: hex(0x08b443),          // L* 64.24  C* 76.9
        link: hex(0x0ce255),                 // L* 79.11  C* 91.6
    };

    /// Purple. Lab hue 311.6 at **61.3%** of the ceiling, which is the share the gemstone itself
    /// takes — amethyst is a pale violet quartz, not a pigment, and that one number is the whole
    /// difference between this palette and a neon one.
    pub const AMETHYST: Cut = Cut {
        canvas: hex(0x0b0610),               // L*  2.16  C*  4.7
        layer: hex(0x1d1226),                // L*  7.53  C* 15.8
        layer_alt: hex(0x22162b),            // L*  9.71  C* 16.4
        control: hex(0x2b1d37),              // L* 13.64  C* 19.5
        control_active: hex(0x412f54),       // L* 22.99  C* 25.7
        stroke_control: hex(0x503966),       // L* 28.36  C* 30.3
        control_hover: hex(0x62467a),        // L* 34.77  C* 34.0
        stroke_strong: hex(0x725591),        // L* 41.40  C* 37.8
        accent_subtle: hex(0x240c3c),        // L*  8.69  C* 35.1
        accent_subtle_hover: hex(0x371957),  // L* 16.48  C* 42.8
        accent_subtle_active: hex(0x4c257a), // L* 24.53  C* 55.3
        accent_active: hex(0x643199),        // L* 32.27  C* 64.9
        accent_default: hex(0x7a3dbb),       // L* 39.69  C* 75.7
        accent_hover: hex(0x905cc8),         // L* 49.20  C* 64.5
        accent_mark: hex(0xb58bd8),          // L* 64.36  C* 45.2
        link: hex(0xd3bbe8),                 // L* 79.17  C* 25.6
    };

    /// Yellow. Lab hue 96.8 at 98.9% of the ceiling. Gold at the ink rungs and olive at the
    /// surface ones, which is not a compromise but what a yellow *is* down here: there is no
    /// bright yellow at `L*` 32, and citrine's own darker tones are exactly this brown-gold.
    pub const CITRINE: Cut = Cut {
        canvas: hex(0x090803),               // L*  2.15  C*  2.0
        layer: hex(0x18170d),                // L*  7.55  C*  6.0
        layer_alt: hex(0x1d1b12),            // L*  9.71  C*  6.4
        control: hex(0x262315),              // L* 13.64  C* 10.0
        control_active: hex(0x3c3720),       // L* 22.99  C* 15.3
        stroke_control: hex(0x4a4325),       // L* 28.39  C* 19.3
        control_hover: hex(0x595231),        // L* 34.76  C* 20.5
        stroke_strong: hex(0x6a623b),        // L* 41.40  C* 23.5
        accent_subtle: hex(0x1e1900),        // L*  8.75  C* 13.2
        accent_subtle_hover: hex(0x2f2902),  // L* 16.48  C* 23.2
        accent_subtle_active: hex(0x413b02), // L* 24.50  C* 32.8
        accent_active: hex(0x544d02),        // L* 32.25  C* 40.1
        accent_default: hex(0x675f01),       // L* 39.70  C* 46.9
        accent_hover: hex(0x827604),         // L* 49.14  C* 54.3
        accent_mark: hex(0xac9e06),          // L* 64.35  C* 67.3
        link: hex(0xd9c60a),                 // L* 79.13  C* 79.6
    };

    /// Metal. The family's ladder lifted 1.7 ΔL\* — all the room there is — with the brightness
    /// that could not go into the surfaces put into the pigment instead: a steel selection and a
    /// near-white mark at `L*` 84.9.
    pub const SILVER: Cut = Cut {
        canvas: hex(0x0e0e0e),               // L*  3.97  C*  0.0
        layer: hex(0x1a1a1a),                // L*  9.26
        layer_alt: hex(0x1e1e1e),            // L* 11.26
        control: hex(0x262626),              // L* 15.16
        control_active: hex(0x3b3b3b),       // L* 24.87
        stroke_control: hex(0x474747),       // L* 30.16
        control_hover: hex(0x424242),        // L* 27.97 — its own; see "the hover"
        stroke_strong: hex(0x666666),        // L* 43.19
        accent_subtle: hex(0x303030),        // L* 19.87 ┐
        accent_subtle_hover: hex(0x3e3e3e),  // L* 26.21 │ the metal: the pigment ramp lifted, and
        accent_subtle_active: hex(0x4b4b4b), // L* 31.89 │ capped at `L*` 45.6, where the module
        accent_active: hex(0x5e5e5e),        // L* 39.90 │ glyph on a selected row would fall under
        accent_default: hex(0x6a6a6a),       // L* 44.82 ┘ the 3:1 a shape wants
        accent_hover: hex(0x727272),         // L* 48.04
        accent_mark: hex(0xd4d4d4),          // L* 84.91 — white-hot, against Azur's 64.4
        link: hex(0xc4c4c4),                 // L* 79.16
    };
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
            // And so does every palette in [`family`](mod@family): each is the same window in
            // another material rather than another window, so it moves what the *roles* are and
            // leaves the mapping alone. One line per mineral, and `Cut::apply` answers with the
            // seven regions that fall out.
            Palette::Onyx => family::ONYX.apply(&mut az),
            Palette::Cobalt => family::COBALT.apply(&mut az),
            Palette::Malachite => family::MALACHITE.apply(&mut az),
            Palette::Amethyst => family::AMETHYST.apply(&mut az),
            Palette::Citrine => family::CITRINE.apply(&mut az),
            Palette::Silver => family::SILVER.apply(&mut az),
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

    /// The inks a folder diff draws a row in. See [`crate::diff`].
    ///
    /// **Only a real difference is in colour.** A name with no counterpart on the other side is
    /// `success`; every name that is on both sides steps back to the secondary ink, whatever else
    /// about the row differs — that is said by the Type, Size or Modified cell that differs, in
    /// `warning`. A folder that is the way down to a difference keeps the ordinary primary ink,
    /// which is not a highlight but is what stops it reading as identical. Figures that match step
    /// back a further step, so a changed one stands out of its column.
    ///
    /// A folder here and a file there differs in the Type column, and a pane too narrow for that
    /// column has given it up — `type_shown` false — so the name says it instead of nothing doing.
    pub fn diff_inks(&self, mark: crate::diff::Mark, type_shown: bool) -> DiffInks {
        use crate::diff::Mark;
        let cells = mark.cells();
        let meta = if mark == Mark::Only {
            self.text.secondary
        } else {
            self.text.tertiary
        };
        let cell = |differs: bool| if differs { self.status.warning } else { meta };
        let name = match mark {
            _ if cells.kind && !type_shown => self.status.warning,
            Mark::Only => self.status.success,
            Mark::Holds => self.text.primary,
            Mark::Same | Mark::Differs(_) => self.text.secondary,
        };
        DiffInks {
            name,
            meta,
            kind: cell(cells.kind),
            size: cell(cells.size),
            modified: cell(cells.modified),
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
        // At least one palette per side, said as a sweep rather than a list, so a palette added
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

        // And every palette in the family is back on the map, because each varies the roles
        // rather than the regions — the same seven, off the same roles, in a different material.
        // See `every_rung_is_the_dark_palettes_own_lightness`, which is the other half of this.
        for palette in Palette::ALL {
            if matches!(palette, Palette::Dark | Palette::Light) {
                continue;
            }
            let t = Theme::of(palette);
            assert_eq!(
                t.surfaces,
                Surfaces::from_azur(t.azur()),
                "{}: a family palette has named a region of its own, so the map above no longer \
                 describes it",
                palette.key()
            );
        }
    }

    /// Every palette in [`family`](mod@family) is [`Palette::Dark`] in another material, stated as
    /// a measurement.
    ///
    /// The whole of the family's claim on the figures this file already took: every rung holds
    /// the *lightness* of the rung it stands in for, and [`ratio`](azur::contrast::ratio) and
    /// [`lstar`](azur::contrast::lstar) are both functions of nothing else. So the seam is still
    /// 3.93 ΔL\* off a column header, the selection bar still 3.1:1 on the row it marks,
    /// `status.danger` still 4.67:1 on the status line — not because those were re-derived six
    /// times, but because *this* holds. The tests below then measure them anyway, on every
    /// palette, which is what keeps this one honest.
    ///
    /// Two palettes declare a shift, and it is the shift that tells them apart: [`Palette::Onyx`]
    /// sinks 3.5 ΔL\* and [`Palette::Silver`] lifts 1.7, which is all the room the status line's
    /// red and the code comment's green leave. Their ladders keep their *shape* to the same
    /// tolerance, so every ΔL\* between two of their own surfaces is the dark palette's as well.
    /// [`Palette::Silver`] also declares its pigment its own — the brightness that could not go
    /// into its surfaces went there instead.
    ///
    /// It is asserted the other way as well — that every rung really did move — because a role
    /// forgotten in `Cut::apply` is a graphite rung in a coloured window, and that is a defect no
    /// ratio in this file would see. And each rung is checked to be in the right corner of the
    /// colour wheel, which is the cheapest true thing that can be said about a hue with a ruler
    /// that only measures lightness: it catches a table row pasted into the wrong palette.
    #[test]
    fn every_rung_is_the_dark_palettes_own_lightness() {
        use azur::contrast::lstar;

        /// **Half a byte step, down where this ladder lives.**
        ///
        /// The cuts are specified in CIELAB and a colour has to land on a byte. A chromatic rung
        /// has three channels to spend and gets within 0.05; a *grey* has one, and near black one
        /// byte is a third of a ΔL\* wide — so [`Palette::Onyx`] and [`Palette::Silver`] drift up
        /// to 0.21. Nothing a real retune could hide inside: the smallest thing this file
        /// measures anywhere is a 3.93 ΔL\* seam.
        const ROUNDING: f32 = 0.25;

        /// One row per palette: how far its stone is shifted off the dark ladder, whether it
        /// names its own hover, whether its pigment is its own, and which channel is the largest
        /// in every rung it has (`=` for the two that have no hue).
        const CUTS: [(Palette, f32, bool, bool, char); 6] = [
            (Palette::Onyx, -3.5, true, false, '='),
            (Palette::Cobalt, 0.0, false, false, 'b'),
            (Palette::Malachite, 0.0, false, false, 'g'),
            (Palette::Amethyst, 0.0, false, false, 'b'),
            (Palette::Citrine, 0.0, false, false, 'r'),
            (Palette::Silver, 1.7, true, true, '='),
        ];

        /// The stone, in a fixed order so two palettes zip. `Cut::apply` is the other half of
        /// this list and the two are each other's proof: a role in one and not the other fails
        /// here.
        fn stone(t: &Theme) -> Vec<(&'static str, Color32)> {
            let mut out = t.surfaces.each().to_vec();
            out.extend([
                ("bg.canvas", t.bg.canvas),
                ("bg.layer", t.bg.layer),
                ("bg.layer_alt", t.bg.layer_alt),
                ("bg.card", t.bg.card),
                ("bg.control", t.bg.control),
                ("bg.control_active", t.bg.control_active),
                ("bg.control_disabled", t.bg.control_disabled),
                ("row_alt", t.row_alt),
                ("stroke.default", t.stroke.default),
                ("stroke.subtle", t.stroke.subtle),
                ("stroke.strong", t.stroke.strong),
                ("stroke.control", t.stroke.control),
                ("scrollbar.thumb", t.scrollbar.thumb),
                ("scrollbar.thumb_hover", t.scrollbar.thumb_hover),
                ("accent.disabled", t.accent.disabled),
                // The two domain palettes that read a stone role rather than restating one, so a
                // rung which drifts takes them with it and this notices.
                ("the code fill", t.syntax.fill),
                ("a drive's gauge track", t.gauge_track),
            ]);
            out
        }

        /// The pigment, the same way.
        fn pigment(t: &Theme) -> Vec<(&'static str, Color32)> {
            vec![
                ("accent.subtle", t.accent.subtle),
                ("accent.subtle_hover", t.accent.subtle_hover),
                ("accent.subtle_active", t.accent.subtle_active),
                ("accent.active", t.accent.active),
                ("accent.default", t.accent.default),
                ("accent.hover", t.accent.hover),
                ("accent.mark", t.accent.mark),
                ("stroke.focus", t.stroke.focus),
                ("status.info", t.status.info),
                ("status.info_subtle", t.status.info_subtle),
                ("text.link", t.text.link),
                ("a Kind::Code glyph", t.code),
                ("the selected count", t.bar.counted),
                ("the branch", t.bar.info),
            ]
        }

        /// The band under the pointer, which the two hueless palettes move and the other four do
        /// not. Both tokens, because a hovered row and a hovered button are one colour.
        fn hover(t: &Theme) -> Vec<(&'static str, Color32)> {
            vec![
                ("bg.control_hover", t.bg.control_hover),
                ("bg.card_hover", t.bg.card_hover),
            ]
        }

        let dark = Theme::dark();
        for (palette, shift, own_hover, own_pigment, biggest) in CUTS {
            let t = Theme::of(palette);
            let name = palette.key();

            // The stone, shifted by the constant this palette declares. `max(0.0)` is the one
            // clamp in the family: Onyx's canvas would be below black, so it is black.
            for ((what, mine), (_, theirs)) in stone(&t).into_iter().zip(stone(&dark)) {
                let want = (lstar(theirs) + shift).max(0.0);
                let got = lstar(mine);
                assert!(
                    (got - want).abs() <= ROUNDING,
                    "{name}: {what} is L* {got:.2} against the L* {want:.2} its ladder position \
                     asks for — every figure this file measures on it has moved with it"
                );
            }
            // The pigment, on the dark palette's own lightnesses unless the palette says
            // otherwise.
            if !own_pigment {
                for ((what, mine), (_, theirs)) in pigment(&t).into_iter().zip(pigment(&dark)) {
                    let (got, want) = (lstar(mine), lstar(theirs));
                    assert!(
                        (got - want).abs() <= ROUNDING,
                        "{name}: {what} is L* {got:.2} against the dark palette's L* {want:.2}"
                    );
                }
            }
            if !own_hover {
                for ((what, mine), (_, theirs)) in hover(&t).into_iter().zip(hover(&dark)) {
                    let (got, want) = (lstar(mine), lstar(theirs));
                    assert!(
                        (got - want).abs() <= ROUNDING,
                        "{name}: {what} is L* {got:.2} against the preset's L* {want:.2}"
                    );
                }
            }

            // Every rung moved, and moved into the right corner of the wheel.
            let mine_all = stone(&t).into_iter().chain(pigment(&t)).chain(hover(&t));
            let dark_all = stone(&dark)
                .into_iter()
                .chain(pigment(&dark))
                .chain(hover(&dark));
            for ((what, mine), (_, theirs)) in mine_all.zip(dark_all) {
                assert_ne!(
                    mine, theirs,
                    "{name}: {what} is still the dark palette's own colour, so `Cut::apply` does \
                     not set it — a graphite rung in a coloured window"
                );
                let (r, g, b) = (mine.r(), mine.g(), mine.b());
                let ok = match biggest {
                    '=' => r == g && g == b,
                    'r' => r >= g && r > b,
                    'g' => g > r && g > b,
                    _ => b > r && b > g,
                };
                assert!(
                    ok,
                    "{name}: {what} is {mine:?}, whose largest channel is not {biggest:?} — a row \
                     of the table has landed in the wrong palette"
                );
            }
        }

        // No two cuts are the same cut, said on the three rungs a reader would recognise: the
        // panel, the selected row and the mark. Six tables of sixteen literals is exactly the
        // shape of thing a copy-and-paste gets wrong.
        for (i, (a, ..)) in CUTS.iter().enumerate() {
            for (b, ..) in &CUTS[i + 1..] {
                let (x, y) = (Theme::of(*a), Theme::of(*b));
                for (what, p, q) in [
                    ("panel", x.surfaces.panel, y.surfaces.panel),
                    ("selected row", x.accent.active, y.accent.active),
                    ("mark", x.accent.mark, y.accent.mark),
                ] {
                    assert_ne!(p, q, "{:?} and {:?} share a {what}", a, b);
                }
            }
        }

        // **The claim [`Palette::Cobalt`] is named for**, and the reason the pigment ramp takes
        // the whole of sRGB's chroma rather than a comfortable fraction of it: at the rung a
        // selected row is filled with, the gamut boundary at hue 291 *is* the pigment — cobalt
        // blue, `#0047AB`, to within three bytes on one channel.
        let pigment = Color32::from_rgb(0x00, 0x47, 0xab);
        let got = Theme::of(Palette::Cobalt).accent.active;
        for (channel, mine, theirs) in [
            ("red", got.r(), pigment.r()),
            ("green", got.g(), pigment.g()),
            ("blue", got.b(), pigment.b()),
        ] {
            assert!(
                mine.abs_diff(theirs) <= 3,
                "a selected row is {got:?}, whose {channel} is {mine} against cobalt blue's \
                 {theirs} — this is not cobalt"
            );
        }
    }

    /// The two palettes with no hue still tell a selected row from a hovered one.
    ///
    /// `azur::desktop::row_fill` fills a selected row with `accent-active` and a hovered one with
    /// `background-control-hover`, and in the dark palette those two are **2.5 ΔL\* apart** — a
    /// selected row is told from a hovered one almost entirely by *hue*. That is fine for the four
    /// palettes of [the family](mod@family) that have one, and it is why the two that do not both
    /// bring their hover down. This is the floor under that, and the second half is the
    /// measurement that makes it necessary: the same pair, in every palette that *has* a hue, is
    /// deliberately under the same floor.
    ///
    /// Six pairs rather than one, because a listing here can show four row states at once and the
    /// window has to be readable with a pane that has not got the keyboard — `ui::row_fill_quiet`
    /// is the fourth, and in a grey palette it is the one at risk of vanishing into the listing.
    #[test]
    fn the_hueless_palettes_tell_a_selected_row_from_a_hovered_one() {
        use azur::contrast::{apart, SAME};

        for palette in [Palette::Onyx, Palette::Silver] {
            let t = Theme::of(palette);
            let name = palette.key();
            let hover = crate::ui::hover_fill(&t);
            let picked = crate::ui::row_fill(&t, true, false).expect("a picked row has a fill");
            let both = crate::ui::row_fill(&t, true, true).expect("so does a hovered picked row");
            let quiet = crate::ui::row_fill_quiet(&t);
            for (a, b, what) in [
                (hover, t.surfaces.panel, "a hovered row and the listing"),
                (picked, hover, "a picked row and a hovered one"),
                (both, hover, "a hovered picked row and a merely hovered one"),
                (both, picked, "a hovered picked row and a resting one"),
                (quiet, t.surfaces.panel, "a selection in an unfocused pane and the listing"),
                (quiet, hover, "a selection in an unfocused pane and a hovered row"),
            ] {
                let got = apart(a, b);
                assert!(
                    got >= SAME,
                    "{name}: {what} are {got:.1} ΔL* apart, and there is no hue here to tell them \
                     apart with"
                );
            }
        }

        // And the pair that only hue separates, everywhere it does. If this starts passing, the
        // design system has moved its two row fills apart and the paragraph above is out of date.
        //
        // The dark side only, and that is `azur::desktop::row_fill`'s own split rather than an
        // exemption taken here: a light palette selects with `accent-subtle` — a *tint*, because
        // the ink on a row does not change when the row is selected — and its picked row is 14.0
        // ΔL\* off its hover as a result. It is the dark rule, filling a row from the accent's
        // surface ramp, that lands two rungs from the hover.
        for palette in Palette::ALL {
            let t = Theme::of(palette);
            if matches!(palette, Palette::Onyx | Palette::Silver | Palette::Light) {
                continue;
            }
            let picked = crate::ui::row_fill(&t, true, false).expect("a picked row has a fill");
            let got = apart(picked, crate::ui::hover_fill(&t));
            assert!(
                got < SAME,
                "{}: a picked row and a hovered one are {got:.1} ΔL* apart, so lightness tells \
                 them apart after all",
                palette.key()
            );
        }
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
