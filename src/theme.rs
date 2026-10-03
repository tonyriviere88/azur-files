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

/// The palettes themselves are the design system's; see [`azur::palettes`].
pub use azur::palettes::{Palette, Surfaces};

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
    ///
    /// The palette itself — every Azur role, the seven regions, and the desktop preset's hover
    /// under both — is the design system's: see [`azur::palettes`]. What is added here is this
    /// program's own colours.
    pub fn of(palette: Palette) -> Self {
        let (az, surfaces) = palette.build();
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

    /// The ink a Status glyph is drawn in: Explorer's own mapping, onto Azur's status roles.
    ///
    /// Blue for the cloud and for a transfer, which are facts about where the content is; green for
    /// content on this disk, which is Explorer's green tick; amber and red for the two states the
    /// provider is asking somebody to look at.
    pub fn sync(&self, state: crate::fs::dir::Sync) -> Color32 {
        use crate::fs::dir::Sync;
        match state {
            Sync::Online | Sync::Syncing => self.status.info,
            Sync::Local | Sync::Pinned => self.status.success,
            Sync::Warning => self.status.warning,
            Sync::Error => self.status.danger,
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
    use azur::contrast::{apart, ratio, SAME, TEXT};

    /// Every palette builds as itself, rather than as whichever side of Azur it is built on.
    #[test]
    fn every_palette_builds_as_itself() {
        for palette in Palette::ALL {
            assert_eq!(Theme::of(palette).palette, palette, "{palette:?} builds as another");
        }
    }

    /// The two palettes with no hue still tell a selected row from a hovered one.
    ///
    /// `azur::desktop::row_fill` fills a selected row with `accent-active` and a hovered one with
    /// `background-control-hover`, and in the dark palette those two are **2.5 ΔL\* apart** — a
    /// selected row is told from a hovered one almost entirely by *hue*. That is fine for the four
    /// palettes of [the family](azur::palettes) that have one, and it is why the two that do not both
    /// bring their hover down. This is the floor under that, and the second half is the
    /// measurement that makes it necessary: the same pair, in every palette that *has* a hue, is
    /// deliberately under the same floor.
    ///
    /// Six pairs rather than one, because a listing here can show four row states at once and the
    /// window has to be readable with a pane that has not got the keyboard — `ui::row_fill_quiet`
    /// is the fourth, and in a grey palette it is the one at risk of vanishing into the listing.
    #[test]
    fn the_hueless_palettes_tell_a_selected_row_from_a_hovered_one() {
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
            if matches!(palette, Palette::Onyx | Palette::Silver) || !palette.is_dark() {
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
