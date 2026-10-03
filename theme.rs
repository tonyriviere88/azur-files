1//! The application's palette: Azur's roles, plus the ones only a file manager
2//! needs.
3//!
4//! Built the way the design system documents for an app with domain colours — a
5//! struct that owns an [`azur_egui_theme::Theme`] and dereferences to it, so
6//! `t.bg.card` and `t.kind(Kind::Image)` read the same at the call site and there
7//! is still exactly one place a colour is decided.
8//!
9//! The file-kind hues extend `tokens::chart`, which is the sanctioned place for
10//! categorical colours: the accent and the two status hues carry their usual
11//! meaning, and the two additions (teal, lime) are picked at the same lightness so
12//! no kind shouts louder than another in a mixed listing.
13
14use azur_egui_theme as azur;
15use azur::tokens::{chart, palette};
16use egui::Color32;
17
18use crate::fs::fmt::Kind;
19
20/// Azur, plus file kinds.
21pub struct Theme {
22    az: azur::Theme,
23
24    /// A folder. Warm, so the places stand out from the things.
25    pub folder: Color32,
26    pub image: Color32,
27    pub audio: Color32,
28    pub video: Color32,
29    pub archive: Color32,
30    pub code: Color32,
31    pub document: Color32,
32    pub executable: Color32,
33    pub font: Color32,
34    /// Meshes, point clouds, CAD — the kinds this machine is full of.
35    pub model: Color32,
36    pub data: Color32,
37    pub other: Color32,
38
39    /// The bar behind a drive's used space, and the bar itself.
40    pub gauge_track: Color32,
41    /// A volume with almost nothing left, which is worth noticing.
42    pub gauge_full: Color32,
43}
44
45impl std::ops::Deref for Theme {
46    type Target = azur::Theme;
47    fn deref(&self) -> &Self::Target {
48        &self.az
49    }
50}
51
52impl Theme {
53    /// The dark side, which is what this application opens on.
54    pub fn dark() -> Self {
55        Self::from_azur(azur::Theme::dark())
56    }
57
58    pub fn light() -> Self {
59        Self::from_azur(azur::Theme::light())
60    }
61
62    fn from_azur(az: azur::Theme) -> Self {
63        // Two hues `tokens::chart` does not name, at the same lightness as the
64        // five it does, so a mixed listing has no loud row.
65        const TEAL: Color32 = Color32::from_rgb(0x35, 0xc2, 0xb1);
66        const LIME: Color32 = Color32::from_rgb(0xa8, 0xcc, 0x45);
67
68        let dark = az.dark;
69        let mut az = az;
70        // The design system's application-window preset, which is where the whole of this
71        // program's argument about hovers now lives: `azur::desktop` moves
72        // `background-control-hover` and `background-card-hover` three rungs along the neutral
73        // ramp, because most of what this window hovers is not a control — it is a row, a path
74        // segment, a folder in a dropdown, a *place* — and one rung off a nearly black listing is
75        // not enough to see.
76        //
77        // **One call, not one per call site.** That token is what the context menu, the column
78        // headers, the application mark, the caption buttons, the tab strip, `ui::control_fills`
79        // — and so Back, Forward, Up and Refresh — and the design system's own `MenuItem` all
80        // read. Doing it at the call sites left those four buttons a rung and a half off
81        // everything around them for a week, which is why the rule is now the design system's.
82        //
83        // It also brought the *light* theme back onto the ink's side of the palette: this file
84        // used to hand light the dark theme's `GRAY_9`, which put near-black body text on a
85        // hovered row at 3.26:1. See `azur_egui_theme::desktop`.
86        azur::desktop::apply(&mut az);
87        Self {
88            folder: if dark { palette::AMBER_70 } else { palette::AMBER_60 },
89            image: chart::VIOLET,
90            audio: chart::GREEN,
91            video: chart::MAGENTA,
92            archive: TEAL,
93            code: az.accent.default,
94            // Text-shaped things take the text colour: a folder of documents
95            // should look like a folder of documents, not a fruit bowl.
96            document: az.text.secondary,
97            executable: if dark { palette::AZURE_90 } else { palette::AZURE_50 },
98            font: az.text.tertiary,
99            model: LIME,
100            data: az.text.tertiary,
101            other: az.text.tertiary,
102
103            gauge_track: az.stroke.subtle,
104            gauge_full: az.status.danger,
105            az,
106        }
107    }
108
109    /// The colour for a file kind.
110    pub fn kind(&self, kind: Kind) -> Color32 {
111        match kind {
112            Kind::Folder => self.folder,
113            Kind::Image => self.image,
114            Kind::Audio => self.audio,
115            Kind::Video => self.video,
116            Kind::Archive => self.archive,
117            Kind::Code => self.code,
118            Kind::Document => self.document,
119            Kind::Executable => self.executable,
120            Kind::Font => self.font,
121            Kind::Model => self.model,
122            Kind::Data => self.data,
123            Kind::Other => self.other,
124        }
125    }
126
127    /// The bar colour for a volume that is `used` full, reddening only once the
128    /// number is worth acting on.
129    pub fn gauge(&self, used: f32) -> Color32 {
130        if used >= 0.95 {
131            self.gauge_full
132        } else if used >= 0.85 {
133            self.status.warning
134        } else {
135            self.accent.default
136        }
137    }
138
139    /// The underlying Azur theme, for [`azur::install`].
140    pub fn azur(&self) -> &azur::Theme {
141        &self.az
142    }
143}
