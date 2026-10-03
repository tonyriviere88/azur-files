1//! Turning raw values into the text the details view shows.
2//!
3//! Every function here writes into a caller-owned buffer rather than returning a
4//! `String`. The list re-formats its visible rows every frame — that is what
5//! makes scrolling correct without a cache to invalidate — so the per-frame cost
6//! has to be zero allocations. egui's galley cache is keyed on the finished text,
7//! so a row that has not changed is not laid out twice either.
8
9use std::fmt::Write as _;
10
11use super::time::{DateTime, LocalZone};
12
13/// Write a byte count in the largest unit that leaves a number worth reading.
14///
15/// Explorer rounds everything to whole kilobytes, which turns every small file
16/// into "1 KB" and loses the distinction between a 40-byte stub and a 900-byte
17/// one. This keeps three significant figures instead: `847 B`, `9.34 KB`,
18/// `72.1 MB`, `1.42 GB`.
19pub fn size(bytes: u64, out: &mut String) {
20    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
21
22    if bytes < 1024 {
23        let _ = write!(out, "{bytes} B");
24        return;
25    }
26
27    // Decimal place chosen so the number is always 3-4 characters wide, which is
28    // what keeps the column from jumping as you scroll.
29    let mut value = bytes as f64;
30    let mut unit = 0;
31    while value >= 1024.0 && unit + 1 < UNITS.len() {
32        value /= 1024.0;
33        unit += 1;
34    }
35    let suffix = UNITS[unit];
36    let _ = if value < 10.0 {
37        write!(out, "{value:.2} {suffix}")
38    } else if value < 100.0 {
39        write!(out, "{value:.1} {suffix}")
40    } else {
41        write!(out, "{value:.0} {suffix}")
42    };
43}
44
45/// `dd/MM/yyyy HH:mm` — fixed width, so the Modified column can be measured once
46/// and never re-measured.
47///
48/// Not locale-aware: reading the user's short-date pattern out of the registry
49/// means `GetDateFormatEx` per cell, and a column whose width depends on the
50/// month name is a column that reflows. One unambiguous fixed format is the
51/// better trade for a dense table.
52pub fn date(dt: DateTime, out: &mut String) {
53    let _ = write!(
54        out,
55        "{:02}/{:02}/{:04} {:02}:{:02}",
56        dt.day, dt.month, dt.year, dt.hour, dt.minute
57    );
58}
59
60/// The widest string [`date`] can produce, for measuring the column.
61pub const DATE_TEMPLATE: &str = "00/00/0000 00:00";
62
63/// Write the Modified cell for a raw `FILETIME`, or a dash when there is no date.
64pub fn modified(filetime: u64, zone: &LocalZone, out: &mut String) {
65    match zone.convert(filetime) {
66        Some(dt) => date(dt, out),
67        None => out.push('—'),
68    }
69}
70
71// ---------------------------------------------------------------------------
72// Type names
73// ---------------------------------------------------------------------------
74
75/// Broad kinds, for choosing an icon and a colour without a second lookup.
76#[derive(Clone, Copy, PartialEq, Eq, Debug)]
77pub enum Kind {
78    Folder,
79    Image,
80    Audio,
81    Video,
82    Archive,
83    Code,
84    Document,
85    Executable,
86    Font,
87    Model,
88    Data,
89    Other,
90}
91
92/// The type label and kind for an extension.
93///
94/// Explorer gets this from the registry, via `SHGetFileInfo` with `SHGFI_TYPENAME`
95/// — a `HKEY_CLASSES_ROOT` walk per *file*. [`crate::fs::scan`]'s benchmark measures
96/// that at over a millisecond each, which is 67 seconds for a folder of 60,000 and
97/// the single biggest reason a folder of mixed files can take that long to appear.
98/// This is a sorted table and a binary search: 45 nanoseconds, same answer.
99///
100/// The extension is matched case-insensitively. Anything unknown becomes
101/// `"XYZ file"`, which is what the registry would have said anyway.
102pub fn file_type(ext: &str) -> Option<(&'static str, Kind)> {
103    if ext.is_empty() || ext.len() > MAX_EXT {
104        return None;
105    }
106    // Lowercase into a stack buffer: no allocation, and the table is all ASCII so
107    // a non-ASCII extension simply will not match, which is correct.
108    let mut key = [0u8; MAX_EXT];
109    for (slot, byte) in key.iter_mut().zip(ext.as_bytes()) {
110        *slot = byte.to_ascii_lowercase();
111    }
112    let key = &key[..ext.len()];
113
114    TYPES
115        .binary_search_by(|(candidate, _, _)| candidate.as_bytes().cmp(key))
116        .ok()
117        .map(|i| (TYPES[i].1, TYPES[i].2))
118}
119
120/// The kind of an entry, for its icon.
121pub fn kind_of(ext: &str, is_dir: bool) -> Kind {
122    if is_dir {
123        return Kind::Folder;
124    }
125    file_type(ext).map_or(Kind::Other, |(_, kind)| kind)
126}
127
128/// Write the Type cell.
129pub fn type_label(ext: &str, is_dir: bool, out: &mut String) {
130    if is_dir {
131        out.push_str("File folder");
132        return;
133    }
134    match file_type(ext) {
135        Some((label, _)) => out.push_str(label),
136        None if ext.is_empty() => out.push_str("File"),
137        None => {
138            // `PNG file`, the shell's own fallback shape. Upper-cased because an
139            // extension is a name here, not a word.
140            for c in ext.chars().take(MAX_EXT) {
141                for upper in c.to_uppercase() {
142                    out.push(upper);
143                }
144            }
145            out.push_str(" file");
146        }
147    }
148}
149
150/// Longest extension the table holds, and the cap on what a fallback label will
151/// echo back.
152const MAX_EXT: usize = 12;
153
154/// Extension, label, kind — **sorted by extension**, which [`file_type`]'s binary
155/// search depends on. A `debug_assert` in the tests below keeps it that way.
156///
157/// The labels follow what Windows itself installs, so the column reads the same as
158/// Explorer's for everything common.
159#[rustfmt::skip]
160static TYPES: &[(&str, &str, Kind)] = &[
161    ("7z",    "7-Zip archive",            Kind::Archive),
162    ("aac",   "AAC audio",                Kind::Audio),
163    ("ai",    "Adobe Illustrator file",   Kind::Image),
164    ("aiff",  "AIFF audio",               Kind::Audio),
165    ("apk",   "Android package",          Kind::Archive),
166    ("asm",   "Assembly source",          Kind::Code),
167    ("avi",   "AVI video",                Kind::Video),
168    ("avif",  "AVIF image",               Kind::Image),
169    ("bat",   "Windows batch file",       Kind::Executable),
170    ("bin",   "Binary file",              Kind::Data),
171    ("blend", "Blender scene",            Kind::Model),
172    ("bmp",   "Bitmap image",             Kind::Image),
173    ("bz2",   "bzip2 archive",            Kind::Archive),
174    ("c",     "C source",                 Kind::Code),
175    ("cab",   "Cabinet archive",          Kind::Archive),
176    ("cbr",   "Comic book archive",       Kind::Archive),
177    ("cc",    "C++ source",               Kind::Code),
178    ("cfg",   "Configuration file",       Kind::Code),
179    ("cmake", "CMake script",             Kind::Code),
180    ("cmd",   "Windows command script",   Kind::Executable),
181    ("com",   "MS-DOS application",       Kind::Executable),
182    ("cpp",   "C++ source",               Kind::Code),
183    ("cs",    "C# source",                Kind::Code),
184    ("css",   "Cascading style sheet",    Kind::Code),
185    ("csv",   "Comma separated values",   Kind::Data),
186    ("cur",   "Cursor",                   Kind::Image),
187    ("cxx",   "C++ source",               Kind::Code),
188    ("dae",   "COLLADA model",            Kind::Model),
189    ("dart",  "Dart source",              Kind::Code),
190    ("db",    "Database file",            Kind::Data),
191    ("dll",   "Application extension",    Kind::Executable),
192    ("dmg",   "Apple disk image",         Kind::Archive),
193    ("doc",   "Word 97-2003 document",    Kind::Document),
194    ("docx",  "Word document",            Kind::Document),
195    ("dwg",   "AutoCAD drawing",          Kind::Model),
196    ("dxf",   "AutoCAD exchange file",    Kind::Model),
197    ("e57",   "E57 point cloud",          Kind::Model),
198    ("eot",   "Embedded OpenType font",   Kind::Font),
199    ("eps",   "Encapsulated PostScript",  Kind::Image),
200    ("epub",  "EPUB book",                Kind::Document),
201    ("exe",   "Application",              Kind::Executable),
202    ("fbx",   "FBX model",                Kind::Model),
203    ("flac",  "FLAC audio",               Kind::Audio),
204    ("flv",   "Flash video",              Kind::Video),
205    ("fnt",   "Font file",                Kind::Font),
206    ("gif",   "GIF image",                Kind::Image),
207    ("glb",   "glTF binary model",        Kind::Model),
208    ("gltf",  "glTF model",               Kind::Model),
209    ("go",    "Go source",                Kind::Code),
210    ("gz",    "gzip archive",             Kind::Archive),
211    ("h",     "C/C++ header",             Kind::Code),
212    ("heic",  "HEIF image",               Kind::Image),
213    ("hpp",   "C++ header",               Kind::Code),
214    ("htm",   "HTML document",            Kind::Code),
215    ("html",  "HTML document",            Kind::Code),
216    ("hxx",   "C++ header",               Kind::Code),
217    ("ico",   "Icon",                     Kind::Image),
218    ("ifc",   "IFC building model",       Kind::Model),
219    ("ini",   "Configuration settings",   Kind::Code),
220    ("iso",   "Disc image",               Kind::Archive),
221    ("java",  "Java source",              Kind::Code),
222    ("jfif",  "JPEG image",               Kind::Image),
223    ("jpeg",  "JPEG image",               Kind::Image),
224    ("jpg",   "JPEG image",               Kind::Image),
225    ("js",    "JavaScript file",          Kind::Code),
226    ("json",  "JSON file",                Kind::Code),
227    ("jsx",   "JavaScript JSX file",      Kind::Code),
228    ("kt",    "Kotlin source",            Kind::Code),
229    ("las",   "LAS point cloud",          Kind::Model),
230    ("laz",   "LAZ point cloud",          Kind::Model),
231    ("lib",   "Object library",           Kind::Data),
232    ("licx",  "Licences file",            Kind::Data),
233    ("lnk",   "Shortcut",                 Kind::Other),
234    ("log",   "Log file",                 Kind::Document),
235    ("lua",   "Lua source",               Kind::Code),
236    ("lz4",   "LZ4 archive",              Kind::Archive),
237    ("m4a",   "MPEG-4 audio",             Kind::Audio),
238    ("m4v",   "MPEG-4 video",             Kind::Video),
239    ("md",    "Markdown document",        Kind::Document),
240    ("mid",   "MIDI sequence",            Kind::Audio),
241    ("mkv",   "Matroska video",           Kind::Video),
242    ("mov",   "QuickTime movie",          Kind::Video),
243    ("mp3",   "MP3 audio",                Kind::Audio),
244    ("mp4",   "MPEG-4 video",             Kind::Video),
245    ("mpg",   "MPEG video",               Kind::Video),
246    ("msi",   "Windows installer package", Kind::Executable),
247    ("obj",   "Wavefront model",          Kind::Model),
248    ("odp",   "OpenDocument presentation", Kind::Document),
249    ("ods",   "OpenDocument spreadsheet", Kind::Document),
250    ("odt",   "OpenDocument text",        Kind::Document),
251    ("ogg",   "Ogg audio",                Kind::Audio),
252    ("opus",  "Opus audio",               Kind::Audio),
253    ("otf",   "OpenType font",            Kind::Font),
254    ("parq",  "Parquet file",             Kind::Data),
255    ("pdb",   "Program debug database",   Kind::Data),
256    ("pdf",   "PDF document",             Kind::Document),
257    ("php",   "PHP source",               Kind::Code),
258    ("pl",    "Perl source",              Kind::Code),
259    ("ply",   "PLY model",                Kind::Model),
260    ("png",   "PNG image",                Kind::Image),
261    ("ppt",   "PowerPoint 97-2003",       Kind::Document),
262    ("pptx",  "PowerPoint presentation",  Kind::Document),
263    ("ps1",   "PowerShell script",        Kind::Executable),
264    ("psd",   "Photoshop image",          Kind::Image),
265    ("pts",   "PTS point cloud",          Kind::Model),
266    ("py",    "Python source",            Kind::Code),
267    ("rar",   "RAR archive",              Kind::Archive),
268    ("raw",   "Camera raw image",         Kind::Image),
269    ("rb",    "Ruby source",              Kind::Code),
270    ("rs",    "Rust source",              Kind::Code),
271    ("rtf",   "Rich text document",       Kind::Document),
272    ("scss",  "Sass style sheet",         Kind::Code),
273    ("sh",    "Shell script",             Kind::Executable),
274    ("sql",   "SQL script",               Kind::Code),
275    ("sqlite","SQLite database",          Kind::Data),
276    ("stl",   "STL model",                Kind::Model),
277    ("svg",   "SVG image",                Kind::Image),
278    ("swift", "Swift source",             Kind::Code),
279    ("sys",   "System file",              Kind::Executable),
280    ("tar",   "tar archive",              Kind::Archive),
281    ("tga",   "Targa image",              Kind::Image),
282    ("tif",   "TIFF image",               Kind::Image),
283    ("tiff",  "TIFF image",               Kind::Image),
284    ("toml",  "TOML file",                Kind::Code),
285    ("ts",    "TypeScript file",          Kind::Code),
286    ("tsx",   "TypeScript JSX file",      Kind::Code),
287    ("ttc",   "TrueType font collection", Kind::Font),
288    ("ttf",   "TrueType font",            Kind::Font),
289    ("txt",   "Text document",            Kind::Document),
290    ("url",   "Internet shortcut",        Kind::Other),
291    ("vb",    "Visual Basic source",      Kind::Code),
292    ("wasm",  "WebAssembly module",       Kind::Code),
293    ("wav",   "Wave audio",               Kind::Audio),
294    ("webm",  "WebM video",               Kind::Video),
295    ("webp",  "WebP image",               Kind::Image),
296    ("wma",   "Windows Media audio",      Kind::Audio),
297    ("wmv",   "Windows Media video",      Kind::Video),
298    ("woff",  "Web open font",            Kind::Font),
299    ("woff2", "Web open font 2",          Kind::Font),
300    ("xls",   "Excel 97-2003 worksheet",  Kind::Document),
301    ("xlsx",  "Excel worksheet",          Kind::Document),
302    ("xml",   "XML document",             Kind::Code),
303    ("xz",    "xz archive",               Kind::Archive),
304    ("yaml",  "YAML file",                Kind::Code),
305    ("yml",   "YAML file",                Kind::Code),
306    ("zip",   "Zip archive",              Kind::Archive),
307    ("zst",   "Zstandard archive",        Kind::Archive),
308];
309
310#[cfg(test)]
311mod tests {
312    use super::*;
313
314    #[test]
315    fn table_is_sorted_and_ascii() {
316        for window in TYPES.windows(2) {
317            assert!(
318                window[0].0.as_bytes() < window[1].0.as_bytes(),
319                "`{}` and `{}` are out of order -- the binary search needs byte order",
320                window[0].0,
321                window[1].0
322            );
323        }
324        for (ext, _, _) in TYPES {
325            assert!(
326                ext.is_ascii() && ext.len() <= MAX_EXT,
327                "`{ext}` cannot ever be matched: the lookup lowercases ASCII into a {MAX_EXT}-byte buffer"
328            );
329        }
330    }
331
332    #[test]
333    fn lookup_ignores_case() {
334        assert_eq!(file_type("PNG").unwrap().0, "PNG image");
335        assert_eq!(file_type("Rs").unwrap().0, "Rust source");
336        assert!(file_type("nope").is_none());
337        assert!(file_type("").is_none());
338    }
339
340    #[test]
341    fn labels_fall_back_to_the_extension() {
342        let mut out = String::new();
343        type_label("qqq", false, &mut out);
344        assert_eq!(out, "QQQ file");
345
346        out.clear();
347        type_label("", false, &mut out);
348        assert_eq!(out, "File");
349
350        out.clear();
351        type_label("anything", true, &mut out);
352        assert_eq!(out, "File folder");
353    }
354
355    #[test]
356    fn sizes_keep_three_figures() {
357        let mut out = String::new();
358        for (bytes, expected) in [
359            (0, "0 B"),
360            (847, "847 B"),
361            (1024, "1.00 KB"),
362            (9_560, "9.34 KB"),
363            (75_612_345, "72.1 MB"),
364            (1_524_760_000, "1.42 GB"),
365            (900 * 1024 * 1024, "900 MB"),
366        ] {
367            out.clear();
368            size(bytes, &mut out);
369            assert_eq!(out, expected, "{bytes} bytes");
370        }
371    }
372
373    #[test]
374    fn date_matches_its_template() {
375        let mut out = String::new();
376        date(
377            DateTime {
378                year: 2026,
379                month: 8,
380                day: 2,
381                hour: 9,
382                minute: 5,
383                second: 0,
384            },
385            &mut out,
386        );
387        assert_eq!(out, "02/08/2026 09:05");
388        assert_eq!(out.len(), DATE_TEMPLATE.len());
389    }
390}
