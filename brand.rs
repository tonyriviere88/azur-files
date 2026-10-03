1//! The application's name and its mark.
2//!
3//! The mark is not this program's. `azur-egui-theme/app-icons/` ships four application
4//! marks — a task manager, this, an audio mixer, a program launcher — as one geometry
5//! definition plus a rasterised set per application, and everything below is a reference to
6//! the file-explorer one rather than a copy of it.
7//!
8//! | form | where | comes from |
9//! | --- | --- | --- |
10//! | a painted outline, 16px, one theme colour | the title bar | [`mark`] |
11//! | one RGBA bitmap, decoded at startup | the window's taskbar button and Alt-Tab | [`window_icon`] |
12//! | a nine-size `.ico` in the executable's resources | Explorer, a pinned shortcut, Alt-Tab before launch | `build.rs` |
13//!
14//! Ship only the first two and Explorer shows the generic application icon; ship only the
15//! third and the taskbar button turns into egui's white `e` the moment the window opens.
16//!
17//! # Why by reference
18//!
19//! `app-icons/README.md` says to copy `marks.rs` into the application. Referencing it in
20//! place does the same job without the copy, and the relative path is the one
21//! `Cargo.toml` already depends on to find the theme at all. The point of the design
22//! system holding the geometry is that four applications cannot drift apart; a vendored
23//! copy is exactly how they would.
24//!
25//! What this program keeps is the *name*, and the wiring.
26//!
27//! # The mark
28//!
29//! A folder, in one stroked outline, on a dark plate: `GRAY_2` with the mark in `AZURE_70`,
30//! which is the "dark chrome" row of `docs/icons.md` §15 rather than the white-on-azure
31//! default. The design system's own note on that is worth knowing — against a dark Windows
32//! 11 taskbar the plate is 1.1:1 and effectively invisible, so the icon reads there as a
33//! blue folder floating on the bar. That is a decision recorded in
34//! `app-icons/README.md`, and `PLATE` in the theme's `examples/app_icons.rs` is the one
35//! place to revisit it.
36//!
37//! Inside the window the mark takes whatever colour it is handed, as any icon does.
38
39/// The design system's mark definitions, included where they live rather than copied here.
40///
41/// `#[path]` reaches out of the crate, which is unusual and deliberate: see the note above.
42/// The other three marks come with it and belong to other applications, hence the `allow` —
43/// they cost nothing in the binary, being `const` data nothing refers to.
44#[path = "../../azur-egui-theme/app-icons/marks.rs"]
45#[allow(dead_code)]
46mod marks;
47
48/// What the window is called.
49pub const NAME: &str = "Azur File Explorer";
50
51/// The mark as an icon inside the window: an outline, in one colour, on the design system's
52/// grid and at the stroke weight `Pen` takes from the box it is given.
53///
54/// A thin wrapper on [`marks::file_explorer`] so that the rest of this program refers to
55/// "the brand's mark" rather than to a name in the design system's icon set — the title bar
56/// should not have to know which of the four this application is.
57pub fn mark(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
58    marks::file_explorer(painter, rect, color);
59}
60
61/// The mark as the bitmap the window shows in the taskbar and in Alt-Tab.
62///
63/// 64px of the nine the design system ships. There is only one slot — `IconData` is a single
64/// bitmap, and Windows scales it for the taskbar button (32 at 100% scaling), Alt-Tab, and
65/// the window's own small icon (16). 64 halves exactly into both of those, where the 256 the
66/// design system's README reaches for would be an eighth-scale reduction into the smallest
67/// of them, which is the "grey mush" `docs/icons.md` §13 warns about.
68///
69/// The hand-snapped 16 and 20 in the `.ico` cannot be used here, because nothing in the
70/// window-icon path takes more than one size. They are what Explorer reads.
71pub fn window_icon() -> egui::IconData {
72    // One of the two places this program names a file in the design system's tree; the other
73    // is `build.rs`, which hands `app.ico` to the linker.
74    const PNG: &[u8] = include_bytes!("../../azur-egui-theme/app-icons/file-explorer/app-64.png");
75
76    let image = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
77        .expect("the design system's app-64.png did not decode as a PNG")
78        .to_rgba8();
79    egui::IconData {
80        width: image.width(),
81        height: image.height(),
82        rgba: image.into_raw(),
83    }
84}
85
86#[cfg(test)]
87mod tests {
88    use super::*;
89
90    /// The design system's icon set for this application, as a filesystem path.
91    const ICONS: &str = concat!(
92        env!("CARGO_MANIFEST_DIR"),
93        "/../azur-egui-theme/app-icons/file-explorer"
94    );
95
96    #[test]
97    fn the_window_icon_is_the_design_system_s_bitmap() {
98        let icon = window_icon();
99        assert_eq!((icon.width, icon.height), (64, 64));
100        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
101
102        let pixels: Vec<[u8; 4]> = icon.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
103        // A plate fills the square, so most of it is opaque, and its corners are rounded, so
104        // something is not. Between them they catch a blank or a wrongly decoded file.
105        let opaque = pixels.iter().filter(|p| p[3] == 255).count();
106        let clear = pixels.iter().filter(|p| p[3] == 0).count();
107        assert!(opaque * 100 / pixels.len() > 60, "only {opaque} of 4096 pixels are opaque");
108        assert!(clear > 0, "nothing is transparent, so the plate has square corners");
109
110        // And it is painted in the two palette colours `app-icons/README.md` names, rather
111        // than in something merely near them.
112        let plate = azur_egui_theme::tokens::palette::GRAY_2;
113        let ink = azur_egui_theme::tokens::palette::AZURE_70;
114        for (name, want) in [("plate", plate), ("mark", ink)] {
115            let found = pixels
116                .iter()
117                .filter(|p| p[3] == 255 && [p[0], p[1], p[2]] == [want.r(), want.g(), want.b()])
118                .count();
119            assert!(
120                found > 40,
121                "only {found} pixels are the {name} colour #{:02X}{:02X}{:02X}",
122                want.r(),
123                want.g(),
124                want.b()
125            );
126        }
127    }
128
129    #[test]
130    fn the_executable_s_icon_carries_every_size() {
131        // What `build.rs` hands the linker, checked here because a missing or truncated
132        // `.ico` is otherwise only a `cargo:warning` nobody reads, and the result is an
133        // executable wearing the generic icon.
134        let path = std::path::Path::new(ICONS).join("app.ico");
135        let ico = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
136
137        assert_eq!(&ico[0..4], &[0, 0, 1, 0], "not an ICO header");
138        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
139
140        // Each directory entry is 16 bytes; the first is the width, with 0 meaning 256.
141        let sizes: Vec<u32> = (0..count)
142            .map(|i| match ico[6 + i * 16] {
143                0 => 256,
144                w => u32::from(w),
145            })
146            .collect();
147        assert_eq!(
148            sizes,
149            vec![16, 20, 24, 32, 40, 48, 64, 128, 256],
150            "the sizes Windows asks for are not all in the icon"
151        );
152    }
153}
